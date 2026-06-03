//! The tonic crate scaffold: `Cargo.toml` + `build.rs` + `src/lib.rs`. The lib
//! includes the generated proto types, re-exports the service trait, and ships a
//! ready-to-run `Unimplemented` impl (every rpc returns `Status::unimplemented`)
//! — so the generated crate COMPILES into a working tonic server skeleton. The
//! author swaps in their own type implementing the trait over their data layer.

use heck::ToSnakeCase;
use sekkei::OpenApiSpec;

use crate::proto::{rpc_signatures, service_name};

/// A generated file: relative path + contents.
pub struct File {
    pub path: String,
    pub contents: String,
}

/// Emit the full scaffold (the `.proto` is written separately by the caller).
#[must_use]
pub fn scaffold(spec: &OpenApiSpec, package: &str, crate_name: &str, proto_filename: &str) -> Vec<File> {
    vec![
        File { path: "Cargo.toml".into(), contents: cargo_toml(crate_name) },
        File { path: "build.rs".into(), contents: build_rs(proto_filename) },
        File { path: "src/lib.rs".into(), contents: lib_rs(spec, package) },
    ]
}

fn cargo_toml(name: &str) -> String {
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

[dependencies]
tonic = "0.12"
prost = "0.13"
prost-types = "0.13"
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}

[build-dependencies]
tonic-build = "0.12"
# vendored protoc for local builds; CI/nix can provide system protobuf instead.
protoc-bin-vendored = "3"
which = "6"
"#
    )
}

fn build_rs(proto_filename: &str) -> String {
    format!(
        r#"//! Compile the gRPC proto. Prefer a system `protoc` (CI/nix nativeBuildInput
//! = protobuf); fall back to the vendored binary for local dev builds.
fn main() {{
    if std::env::var_os("PROTOC").is_none() && which::which("protoc").is_err() {{
        if let Ok(p) = protoc_bin_vendored::protoc_bin_path() {{
            // SAFETY: build scripts are single-threaded.
            unsafe {{ std::env::set_var("PROTOC", p); }}
        }}
    }}
    tonic_build::compile_protos("proto/{proto_filename}").expect("compile proto/{proto_filename}");
    println!("cargo:rerun-if-changed=proto/{proto_filename}");
}}
"#
    )
}

/// Map a proto response type to its tonic/prost Rust type.
fn rust_type(proto_ty: &str) -> String {
    match proto_ty {
        "google.protobuf.Empty" => "()".to_string(),
        "google.protobuf.Struct" => "::prost_types::Struct".to_string(),
        other => format!("pb::{other}"),
    }
}

fn lib_rs(spec: &OpenApiSpec, package: &str) -> String {
    let service = service_name(package);
    let svc_mod = format!("{}_server", service.to_snake_case());
    let sigs = rpc_signatures(spec);

    let mut methods = String::new();
    for s in &sigs {
        methods.push_str(&format!(
            "    async fn {method}(&self, _request: tonic::Request<pb::{req}>) -> Result<tonic::Response<{resp}>, tonic::Status> {{\n        Err(tonic::Status::unimplemented(\"{rpc}\"))\n    }}\n",
            method = s.method,
            req = s.req_type,
            resp = rust_type(&s.resp_type),
            rpc = s.rpc,
        ));
    }

    format!(
        r#"//! Generated tonic gRPC scaffold for `{package}`. The proto types live in
//! [`pb`]; implement [`{service}`] over your data layer (the shipped
//! [`Unimplemented`] is a ready-to-run starting point).
//!
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
    tonic::include_proto!("{package}");
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

    fn files() -> Vec<File> {
        let spec: OpenApiSpec = serde_yaml_ng::from_str(SPEC).unwrap();
        scaffold(&spec, "breathe.v1", "breathe-grpc", "breathe.proto")
    }

    #[test]
    fn emits_the_three_scaffold_files() {
        let f = files();
        let paths: Vec<&str> = f.iter().map(|x| x.path.as_str()).collect();
        assert!(paths.contains(&"Cargo.toml"));
        assert!(paths.contains(&"build.rs"));
        assert!(paths.contains(&"src/lib.rs"));
    }

    fn contents_of(path: &str) -> String {
        files().into_iter().find(|f| f.path == path).unwrap().contents
    }

    #[test]
    fn lib_includes_proto_and_reexports_the_trait() {
        let lib = contents_of("src/lib.rs");
        assert!(lib.contains("tonic::include_proto!(\"breathe.v1\");"));
        assert!(lib.contains("pub use pb::breathe_server::{Breathe, BreatheServer};"));
        assert!(lib.contains("impl Breathe for Unimplemented {"));
    }

    #[test]
    fn handler_methods_map_response_types_correctly() {
        let lib = contents_of("src/lib.rs");
        // $ref response → pb::Band
        assert!(lib.contains("async fn band_get(&self, _request: tonic::Request<pb::BandGetRequest>) -> Result<tonic::Response<pb::Band>, tonic::Status>"));
        // no-content response → google.protobuf.Empty → ()
        assert!(lib.contains("Result<tonic::Response<()>, tonic::Status>"));
    }

    #[test]
    fn build_rs_has_protoc_fallback() {
        let b = contents_of("build.rs");
        assert!(b.contains("protoc_bin_vendored::protoc_bin_path()"));
        assert!(b.contains("compile_protos(\"proto/breathe.proto\")"));
    }
}
