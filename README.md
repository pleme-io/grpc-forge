# grpc-forge

Generate **typed** Rust [tonic](https://github.com/hyperium/tonic) gRPC servers
from OpenAPI specs. The gRPC sibling of [`mcp-forge`](https://github.com/pleme-io/mcp-forge)
in the [`forge-gen`](https://github.com/pleme-io/forge-gen) ecosystem.

One OpenAPI spec is the single source of truth; forge-gen emits REST, gRPC,
GraphQL, MCP, SDKs, and docs from it. grpc-forge is the gRPC half: it maps the
spec to a typed `.proto` (component schemas → messages/enums; operations → a
service with one rpc each, over synthesized request/response messages) plus a
compilable tonic crate scaffold (`Cargo.toml` + `build.rs` + `tonic::include_proto!`
+ the service trait + a ready-to-run handler stub the author fills over their
data layer).

## Usage

```sh
grpc-forge proto    --spec api.yaml --package myapi.v1     # print the typed .proto
grpc-forge generate --spec api.yaml --output ./gen --package myapi.v1
grpc-forge generate --spec api.yaml --output ./gen --no-serde   # minimal prost-only crate
```

## The JSON↔typed bridge (serde, default-on)

By default the generated messages are **serde-(de)serializable** via
[`pbjson`](https://github.com/influxdata/pbjson) (the proto3 JSON mapping). This
is the high-leverage mode for the pleme-io fleet: a service whose data layer
returns `serde_json::Value` (the universal facade shape shared across
REST/GraphQL/MCP) bridges to typed gRPC for free —

```rust
let v: serde_json::Value = facade.get_band(kind, ns, name).await?;   // CRD JSON
let band: pb::Band = serde_json::from_value(v)?;                      // typed, no hand-mapping
Ok(tonic::Response::new(band))
```

Well-known types (`google.protobuf.Struct`/`Empty`) come from `pbjson-types`
(prost + serde), so open CRD-JSON sub-objects (`metadata`, free-form `spec`)
round-trip cleanly.

**Empirically verified** against breathe's real spec + a full Kubernetes CRD
JSON (apiVersion/kind/rich metadata/spec/status):

- ✅ faithful CRD JSON deserializes into the typed message; rich `metadata`
  (uid/resourceVersion/managedFields) is absorbed by the `Struct` field.
- ✅ **both** camelCase (`growAbove`) and snake_case (`grow_above`) field names
  are accepted.
- ⚠️ **pbjson is strict**: unknown fields are *rejected*. The typed bridge is a
  strict contract — keep the spec faithful to the data shape (the spec-first
  standard already mandates this); drift surfaces as a typed error, never a
  silent wrong answer.

`--no-serde` emits a minimal prost-only crate (`Empty` → `()`, no pbjson deps).

## Mapping

| OpenAPI | proto3 |
|---|---|
| `components.schemas.X` (object) | `message X { … }` |
| string `enum` | `enum X { X_UNSPECIFIED = 0; … }` |
| `$ref` | the referenced message |
| `array` | `repeated <item>` |
| `integer`/`number`/`string`/`boolean` | `int64`/`double`/`string`/`bool` (format-aware) |
| `additionalProperties` | `map<string, V>` |
| inline object / `oneOf` / `anyOf` / freeform | `google.protobuf.Struct` (documented fallback) |
| operation (`operationId`) | `rpc OpId(OpIdRequest) returns (Resp)` |
| path/query params + body | fields of `OpIdRequest` |
| 200 response | the `$ref` message, or a synthesized `OpIdResponse` |

Built on [`sekkei`](https://github.com/pleme-io/sekkei) (the canonical pleme-io
OpenAPI 3.0 model). MIT.
