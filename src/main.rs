//! grpc-forge — generate typed Rust tonic gRPC servers from OpenAPI specs.
//!
//! Sibling of mcp-forge in the forge-gen ecosystem: forge-gen orchestrates it for
//! the `grpc` target. It emits the typed `.proto` (the faithful OpenAPI→proto3
//! mapping in [`proto`]) plus a compilable tonic crate scaffold (Cargo + build.rs
//! + lib with the service trait + a ready-to-run handler stub, in [`scaffold`]).
//! By default the messages are serde-capable (pbjson) so a `serde_json::Value`
//! data layer bridges to typed gRPC for free; `--no-serde` emits a prost-only crate.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use heck::ToSnakeCase;

pub mod proto;
pub mod scaffold;

#[derive(Parser)]
#[command(name = "grpc-forge", version, about = "Generate typed Rust tonic gRPC servers from OpenAPI specs")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate the gRPC server project from an OpenAPI spec.
    Generate {
        /// Path to the OpenAPI 3.0.3 YAML or JSON spec.
        #[arg(long, short)]
        spec: PathBuf,
        /// Output directory.
        #[arg(long, short, default_value = ".")]
        output: PathBuf,
        /// Proto package (default: `<name>.v1`, name from spec title).
        #[arg(long)]
        package: Option<String>,
        /// Project name override (defaults to spec `info.title`, snake-cased).
        #[arg(long)]
        name: Option<String>,
        /// Emit a minimal prost-only crate (no pbjson serde impls). By default
        /// the generated messages are serde-(de)serializable so a
        /// `serde_json::Value` data layer bridges to typed gRPC for free.
        #[arg(long)]
        no_serde: bool,
    },
    /// Print the generated `.proto` to stdout (for debugging).
    Proto {
        #[arg(long, short)]
        spec: PathBuf,
        #[arg(long)]
        package: Option<String>,
    },
}

fn load(path: &Path) -> Result<sekkei::OpenApiSpec> {
    let content = std::fs::read_to_string(path).with_context(|| format!("reading spec {}", path.display()))?;
    if path.extension().is_some_and(|e| e == "json") {
        Ok(serde_json::from_str(&content)?)
    } else {
        Ok(serde_yaml_ng::from_str(&content)?)
    }
}

fn pkg_of(spec: &sekkei::OpenApiSpec, name: &Option<String>, package: &Option<String>) -> String {
    package.clone().unwrap_or_else(|| {
        let n = name.clone().unwrap_or_else(|| spec.info.title.to_snake_case());
        format!("{}.v1", n.to_snake_case().replace('_', ""))
    })
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")))
        .init();

    match Cli::parse().command {
        Command::Proto { spec, package } => {
            let api = load(&spec)?;
            let pkg = pkg_of(&api, &None, &package);
            print!("{}", proto::emit(&api, &pkg));
        }
        Command::Generate { spec, output, package, name, no_serde } => {
            let api = load(&spec)?;
            let pkg = pkg_of(&api, &name, &package);
            let serde = !no_serde;
            let stem = pkg.split('.').next().unwrap_or("api");
            let crate_name = name.clone().unwrap_or_else(|| format!("{stem}-grpc"));
            let proto_filename = format!("{stem}.proto");

            // 1. the typed .proto.
            let proto_dir = output.join("proto");
            std::fs::create_dir_all(&proto_dir).with_context(|| format!("mkdir {}", proto_dir.display()))?;
            std::fs::write(proto_dir.join(&proto_filename), proto::emit(&api, &pkg))
                .with_context(|| format!("writing proto/{proto_filename}"))?;

            // 2. the tonic crate scaffold (Cargo + build.rs + src/lib.rs).
            for file in scaffold::scaffold(&api, &pkg, &crate_name, &proto_filename, serde) {
                let path = output.join(&file.path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
                }
                std::fs::write(&path, file.contents).with_context(|| format!("writing {}", path.display()))?;
            }

            tracing::info!(
                "grpc-forge: generated tonic gRPC crate '{crate_name}' ({} operations) → {}",
                api.operation_count(),
                output.display()
            );
        }
    }
    Ok(())
}
