{
  description = "grpc-forge — generate typed Rust tonic gRPC servers from OpenAPI specs";

  # Canonical pleme-io Rust-tool consumer flake. substrate.rust.tool pre-binds
  # nixpkgs / crate2nix / flake-utils / fenix / devenv / gen so a substrate bump
  # propagates fleet-wide without touching this file.
  inputs.substrate.url = "github:pleme-io/substrate";

  outputs = { substrate, ... }: substrate.rust.tool {
    src = ./.;
    module = {
      description = "grpc-forge — generate typed Rust tonic gRPC servers from OpenAPI specs";
    };
  };
}
