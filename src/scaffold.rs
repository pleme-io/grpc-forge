//! The tonic crate scaffold: `Cargo.toml` + `build.rs` + `src/lib.rs`. The lib
//! includes the generated proto types, re-exports the service trait, and ships a
//! ready-to-run `Unimplemented` impl (every rpc returns `Status::unimplemented`)
//! — so the generated crate COMPILES into a working tonic server skeleton. The
//! author swaps in their own type implementing the trait over their data layer.
//!
//! ## serde mode (the JSON↔typed bridge)
//!
//! With `serde = true` (the default) the scaffold wires **pbjson** so every
//! generated message is `serde::Serialize`/`Deserialize` per the proto3 JSON
//! mapping. This is the high-leverage mode for the pleme-io fleet: a service
//! whose data layer returns `serde_json::Value` (the universal facade shape used
//! across REST/GraphQL/MCP) bridges to typed gRPC for free —
//! `serde_json::from_value::<pb::Band>(v)` instead of a hand-written mapping.
//! Well-known types (`google.protobuf.Struct`/`Empty`) come from `pbjson-types`
//! (prost + serde), so open CRD-JSON sub-objects (`metadata`, free-form `spec`)
//! round-trip cleanly. `serde = false` emits a minimal prost-only crate.

use heck::ToSnakeCase;
use sekkei::OpenApiSpec;

use crate::proto::{rpc_signatures, service_name};

/// A generated file: relative path + contents.
pub struct File {
    pub path: String,
    pub contents: String,
}

/// Emit the full scaffold (the `.proto` is written separately by the caller).
///
/// `serde` wires pbjson so the generated messages are serde-(de)serializable
/// (the JSON↔typed bridge); `false` emits a minimal prost-only crate.
#[must_use]
pub fn scaffold(spec: &OpenApiSpec, package: &str, crate_name: &str, proto_filename: &str, serde: bool) -> Vec<File> {
    vec![
        File { path: "Cargo.toml".into(), contents: cargo_toml(crate_name, serde) },
        File { path: "build.rs".into(), contents: build_rs(proto_filename, package, serde) },
        File { path: "src/lib.rs".into(), contents: lib_rs(spec, package, serde) },
    ]
}

fn cargo_toml(name: &str, serde: bool) -> String {
    // serde mode swaps prost-types → pbjson-types (serde-capable well-known
    // types) and adds pbjson + serde + the pbjson-build build-dep.
    let (runtime_extra, build_extra) = if serde {
        (
            "prost-types = \"0.13\"\n\
             pbjson = \"0.7\"\n\
             pbjson-types = \"0.7\"\n\
             serde = { version = \"1\", features = [\"derive\"] }\n",
            "prost-build = \"0.13\"\npbjson-build = \"0.7\"\n",
        )
    } else {
        ("prost-types = \"0.13\"\n", "")
    };
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

[dependencies]
tonic = "0.12"
prost = "0.13"
{runtime_extra}tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}

[build-dependencies]
tonic-build = "0.12"
{build_extra}# vendored protoc for local builds; CI/nix can provide system protobuf instead.
protoc-bin-vendored = "3"
which = "6"
"#
    )
}

fn build_rs(proto_filename: &str, package: &str, serde: bool) -> String {
    // Both variants prefer a system `protoc` (CI/nix nativeBuildInput =
    // protobuf) and fall back to the vendored binary for local dev.
    let protoc_fallback = r#"    if std::env::var_os("PROTOC").is_none() && which::which("protoc").is_err() {
        if let Ok(p) = protoc_bin_vendored::protoc_bin_path() {
            // SAFETY: build scripts are single-threaded.
            unsafe { std::env::set_var("PROTOC", p); }
        }
    }"#;

    if serde {
        // Emit a file-descriptor set, map the well-known types to pbjson-types,
        // then generate the serde impls into `<package>.serde.rs`.
        // `compile_well_known_types()` removes prost's default
        // `.google.protobuf → ::prost_types` extern so our pbjson-types extern
        // is the only one (otherwise: "duplicate extern Protobuf path").
        format!(
            r#"//! Compile the gRPC proto with serde-capable messages (pbjson).
use std::path::PathBuf;

fn main() {{
{protoc_fallback}
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let descriptor_path = out_dir.join("proto_descriptor.bin");

    let mut config = prost_build::Config::new();
    config.compile_well_known_types();
    config.extern_path(".google.protobuf", "::pbjson_types");
    config.file_descriptor_set_path(&descriptor_path);

    tonic_build::configure()
        .compile_protos_with_config(config, &["proto/{proto_filename}"], &["proto"])
        .expect("compile proto/{proto_filename}");

    let descriptor_set = std::fs::read(&descriptor_path).expect("read descriptor set");
    pbjson_build::Builder::new()
        .register_descriptors(&descriptor_set)
        .expect("register descriptors")
        .build(&[".{package}"])
        .expect("build pbjson serde impls");

    println!("cargo:rerun-if-changed=proto/{proto_filename}");
}}
"#
        )
    } else {
        format!(
            r#"//! Compile the gRPC proto (prost-only).
fn main() {{
{protoc_fallback}
    tonic_build::compile_protos("proto/{proto_filename}").expect("compile proto/{proto_filename}");
    println!("cargo:rerun-if-changed=proto/{proto_filename}");
}}
"#
        )
    }
}

/// Map a proto response type to its tonic/prost Rust type. In serde mode the
/// well-known types resolve to `pbjson-types` (so a no-content rpc returns
/// `::pbjson_types::Empty`); otherwise prost's defaults apply (`Empty` → `()`).
fn rust_type(proto_ty: &str, serde: bool) -> String {
    match (proto_ty, serde) {
        ("google.protobuf.Empty", true) => "::pbjson_types::Empty".to_string(),
        ("google.protobuf.Empty", false) => "()".to_string(),
        ("google.protobuf.Struct", true) => "::pbjson_types::Struct".to_string(),
        ("google.protobuf.Struct", false) => "::prost_types::Struct".to_string(),
        (other, _) => format!("pb::{other}"),
    }
}

fn lib_rs(spec: &OpenApiSpec, package: &str, serde: bool) -> String {
    let service = service_name(package);
    let svc_mod = format!("{}_server", service.to_snake_case());
    let sigs = rpc_signatures(spec);

    let mut methods = String::new();
    for s in &sigs {
        methods.push_str(&format!(
            "    async fn {method}(&self, _request: tonic::Request<pb::{req}>) -> Result<tonic::Response<{resp}>, tonic::Status> {{\n        Err(tonic::Status::unimplemented(\"{rpc}\"))\n    }}\n",
            method = s.method,
            req = s.req_type,
            resp = rust_type(&s.resp_type, serde),
            rpc = s.rpc,
        ));
    }

    // In serde mode pbjson writes its impls into `<package>.serde.rs`; include
    // it inside the same module the messages live in.
    let serde_include = if serde {
        format!("\n    include!(concat!(env!(\"OUT_DIR\"), \"/{package}.serde.rs\"));")
    } else {
        String::new()
    };
    let serde_note = if serde {
        "//! Messages are serde-(de)serializable (pbjson): a `serde_json::Value`\n//! data layer bridges to typed gRPC via `serde_json::from_value::<pb::T>(v)`.\n//!\n"
    } else {
        ""
    };

    format!(
        r#"//! Generated tonic gRPC scaffold for `{package}`. The proto types live in
//! [`pb`]; implement [`{service}`] over your data layer (the shipped
//! [`Unimplemented`] is a ready-to-run starting point).
{serde_note}//!
//! Serve it:
//! ```ignore
//! let svc = MyService::new(/* state */);
//! tonic::transport::Server::builder()
//!     .add_service({service}Server::new(svc))
//!     .serve(addr).await?;
//! ```

#![allow(clippy::needless_lifetimes, clippy::wildcard_imports)]

/// The generated proto messages, enums, and the service definition.
pub mod pb {{
    tonic::include_proto!("{package}");{serde_include}
}}

pub use pb::{svc_mod}::{{{service}, {service}Server}};

/// A ready-to-run stub: every rpc returns `Status::unimplemented`. Swap in your
/// own type implementing [`{service}`] over your data layer.
#[derive(Default, Clone)]
pub struct Unimplemented;

#[tonic::async_trait]
impl {service} for Unimplemented {{
{methods}}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = r##"
openapi: 3.0.3
info: { title: breathe control API, version: 0.1.0 }
paths:
  /api/v1/bands/{kind}/{namespace}/{name}:
    get:
      operationId: bandGet
      parameters:
        - { name: kind, in: path, schema: { type: string } }
        - { name: namespace, in: path, schema: { type: string } }
        - { name: name, in: path, schema: { type: string } }
      responses:
        "200": { content: { application/json: { schema: { $ref: "#/components/schemas/Band" } } } }
  /healthz:
    get:
      operationId: healthz
      responses:
        "200": { description: ok }
components:
  schemas:
    Band: { type: object, properties: { kind: { type: string } } }
"##;

    fn files(serde: bool) -> Vec<File> {
        let spec: OpenApiSpec = serde_yaml_ng::from_str(SPEC).unwrap();
        scaffold(&spec, "breathe.v1", "breathe-grpc", "breathe.proto", serde)
    }

    fn contents_of(path: &str, serde: bool) -> String {
        files(serde).into_iter().find(|f| f.path == path).unwrap().contents
    }

    #[test]
    fn emits_the_three_scaffold_files() {
        let f = files(true);
        let paths: Vec<&str> = f.iter().map(|x| x.path.as_str()).collect();
        assert!(paths.contains(&"Cargo.toml"));
        assert!(paths.contains(&"build.rs"));
        assert!(paths.contains(&"src/lib.rs"));
    }

    #[test]
    fn lib_includes_proto_and_reexports_the_trait() {
        let lib = contents_of("src/lib.rs", true);
        assert!(lib.contains("tonic::include_proto!(\"breathe.v1\");"));
        assert!(lib.contains("pub use pb::breathe_server::{Breathe, BreatheServer};"));
        assert!(lib.contains("impl Breathe for Unimplemented {"));
    }

    #[test]
    fn handler_methods_map_response_types_correctly() {
        // serde mode: no-content response → pbjson_types::Empty.
        let lib = contents_of("src/lib.rs", true);
        assert!(lib.contains("async fn band_get(&self, _request: tonic::Request<pb::BandGetRequest>) -> Result<tonic::Response<pb::Band>, tonic::Status>"));
        assert!(lib.contains("Result<tonic::Response<::pbjson_types::Empty>, tonic::Status>"));
    }

    #[test]
    fn non_serde_mode_maps_empty_to_unit() {
        // prost-only mode: no-content response → () (prost's well-known default).
        let lib = contents_of("src/lib.rs", false);
        assert!(lib.contains("Result<tonic::Response<()>, tonic::Status>"));
        assert!(!lib.contains("pbjson"));
    }

    #[test]
    fn serde_mode_wires_pbjson_in_build_and_cargo() {
        let cargo = contents_of("Cargo.toml", true);
        assert!(cargo.contains("pbjson = \"0.7\""));
        assert!(cargo.contains("pbjson-types = \"0.7\""));
        assert!(cargo.contains("pbjson-build = \"0.7\""));
        assert!(cargo.contains("serde = { version = \"1\""));

        let build = contents_of("build.rs", true);
        assert!(build.contains(".extern_path(\".google.protobuf\", \"::pbjson_types\")"));
        assert!(build.contains("file_descriptor_set_path"));
        assert!(build.contains("pbjson_build::Builder::new()"));
        assert!(build.contains(".build(&[\".breathe.v1\"])"));

        let lib = contents_of("src/lib.rs", true);
        assert!(lib.contains("include!(concat!(env!(\"OUT_DIR\"), \"/breathe.v1.serde.rs\"));"));
    }

    #[test]
    fn non_serde_mode_is_prost_only() {
        let cargo = contents_of("Cargo.toml", false);
        assert!(!cargo.contains("pbjson"));
        assert!(!cargo.contains("serde"));

        let build = contents_of("build.rs", false);
        assert!(build.contains("tonic_build::compile_protos(\"proto/breathe.proto\")"));
        assert!(!build.contains("pbjson"));

        let lib = contents_of("src/lib.rs", false);
        assert!(!lib.contains("serde.rs"));
    }

    #[test]
    fn build_rs_has_protoc_fallback_in_both_modes() {
        for serde in [true, false] {
            let b = contents_of("build.rs", serde);
            assert!(b.contains("protoc_bin_vendored::protoc_bin_path()"), "serde={serde}");
        }
    }
}
