# grpc-forge

Generate **typed** Rust [tonic](https://github.com/hyperium/tonic) gRPC servers
from OpenAPI specs. The gRPC sibling of [`mcp-forge`](https://github.com/pleme-io/mcp-forge)
in the [`forge-gen`](https://github.com/pleme-io/forge-gen) ecosystem.

One OpenAPI spec is the single source of truth; forge-gen emits REST, gRPC,
GraphQL, MCP, SDKs, and docs from it. grpc-forge is the gRPC half: it maps the
spec to a typed `.proto` (component schemas → messages/enums; operations → a
service with one rpc each, over synthesized request/response messages) and (M2)
the tonic crate scaffold (`Cargo.toml` + `build.rs` + `tonic::include_proto!` +
the service trait + a handler stub the author fills over their data layer).

## Usage

```sh
grpc-forge proto    --spec api.yaml --package myapi.v1     # print the typed .proto
grpc-forge generate --spec api.yaml --output ./gen --package myapi.v1
```

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
