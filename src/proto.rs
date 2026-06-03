//! OpenAPI → proto3 emitter. The faithful mapping that makes gRPC TYPED:
//! component schemas → proto messages/enums; operations → a service with one rpc
//! each, over synthesized request messages (params + body) and response messages
//! (the 200 schema). Exotic shapes (oneOf/anyOf, inline freeform objects) map to
//! `google.protobuf.Struct` — a documented fallback, not a silent wrong answer.

use std::collections::BTreeSet;

use heck::{ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use sekkei::{ref_name, OpenApiSpec, Schema};

/// Emit a complete `.proto` for `spec` under `package` (e.g. `breathe.v1`).
#[must_use]
pub fn emit(spec: &OpenApiSpec, package: &str) -> String {
    let mut imports: BTreeSet<String> = BTreeSet::new();
    let mut body = String::new();

    // 1. messages + enums from the named component schemas.
    if let Some(c) = &spec.components {
        for (name, schema) in &c.schemas {
            emit_named(name, schema, &mut body, &mut imports);
        }
    }

    // 2. the service: one rpc per operation, with synthesized request/response.
    emit_service(spec, package, &mut body, &mut imports);

    // 3. assemble: header + imports + body.
    let mut out = String::from("syntax = \"proto3\";\n\n");
    out.push_str(&format!("package {package};\n\n"));
    for imp in &imports {
        out.push_str(&format!("import \"{imp}\";\n"));
    }
    if !imports.is_empty() {
        out.push('\n');
    }
    out.push_str(&body);
    out
}

const STRUCT: &str = "google.protobuf.Struct";
const EMPTY: &str = "google.protobuf.Empty";

/// The proto field type for a schema: `(label, type)` where label is `""` or
/// `"repeated "`. Records any needed well-known imports.
fn field_type(schema: &Schema, imports: &mut BTreeSet<String>) -> (String, String) {
    if schema.is_ref() {
        return (String::new(), ref_name(schema.ref_path.as_deref().unwrap_or("")).to_upper_camel_case());
    }
    if schema.is_array() {
        let inner = schema.items.as_deref().cloned().unwrap_or_default();
        let (_, ity) = field_type(&inner, imports);
        return (String::from("repeated "), ity);
    }
    let ty = match schema.schema_type.as_deref() {
        Some("integer") => if schema.format.as_deref() == Some("int32") { "int32" } else { "int64" }.to_string(),
        Some("number") => if schema.format.as_deref() == Some("float") { "float" } else { "double" }.to_string(),
        Some("string") => match schema.format.as_deref() {
            Some("byte" | "binary") => "bytes".to_string(),
            _ => "string".to_string(),
        },
        Some("boolean") => "bool".to_string(),
        Some("object") => {
            if let Some(ap) = &schema.additional_properties {
                let (_, vty) = field_type(ap, imports);
                format!("map<string, {vty}>")
            } else {
                // inline object / freeform → Struct (documented fallback).
                imports.insert("google/protobuf/struct.proto".into());
                STRUCT.to_string()
            }
        }
        // composed (oneOf/anyOf) or untyped → Struct.
        _ => {
            imports.insert("google/protobuf/struct.proto".into());
            STRUCT.to_string()
        }
    };
    (String::new(), ty)
}

/// Emit a named component schema as a proto message or enum.
fn emit_named(name: &str, schema: &Schema, out: &mut String, imports: &mut BTreeSet<String>) {
    let msg = name.to_upper_camel_case();
    if schema.is_enum() {
        emit_enum(&msg, schema, out);
    } else if schema.is_object() || !schema.properties.is_empty() {
        emit_message(&msg, schema, out, imports);
    } else if schema.is_array() {
        let (label, ity) = field_type(schema, imports);
        out.push_str(&format!("message {msg} {{\n  {label}{ity} items = 1;\n}}\n\n"));
    } else if schema.is_primitive() {
        let (_, ity) = field_type(schema, imports);
        out.push_str(&format!("message {msg} {{\n  {ity} value = 1;\n}}\n\n"));
    } else {
        // composed / freeform → a Struct-valued wrapper.
        imports.insert("google/protobuf/struct.proto".into());
        out.push_str(&format!("message {msg} {{\n  {STRUCT} value = 1;\n}}\n\n"));
    }
}

fn emit_enum(name: &str, schema: &Schema, out: &mut String) {
    out.push_str(&format!("enum {name} {{\n"));
    let prefix = name.to_shouty_snake_case();
    out.push_str(&format!("  {prefix}_UNSPECIFIED = 0;\n"));
    if let Some(values) = &schema.enum_values {
        for (i, v) in values.iter().enumerate() {
            if let Some(s) = v.as_str() {
                out.push_str(&format!("  {prefix}_{} = {};\n", s.to_shouty_snake_case(), i + 1));
            }
        }
    }
    out.push_str("}\n\n");
}

fn emit_message(name: &str, schema: &Schema, out: &mut String, imports: &mut BTreeSet<String>) {
    out.push_str(&format!("message {name} {{\n"));
    for (i, (prop, pschema)) in schema.properties.iter().enumerate() {
        let (label, ty) = field_type(pschema, imports);
        out.push_str(&format!("  {label}{ty} {} = {};\n", prop.to_snake_case(), i + 1));
    }
    out.push_str("}\n\n");
}

/// The proto service name for a package (`breathe.v1` → `Breathe`). Shared by
/// the proto emitter and the scaffold so the generated `service` and the handler
/// trait can never disagree.
#[must_use]
pub fn service_name(package: &str) -> String {
    package.split('.').next().unwrap_or("Api").to_upper_camel_case()
}

/// One rpc's typed signature — the model the proto service AND the tonic handler
/// stub both render from (solve the operation→rpc mapping once).
pub struct RpcSig {
    /// The rpc name (`BandGet`).
    pub rpc: String,
    /// The tonic method name (`band_get`).
    pub method: String,
    /// The request message type (`BandGetRequest`).
    pub req_type: String,
    /// The response type (`Band`, `BandListResponse`, or `google.protobuf.Empty`).
    pub resp_type: String,
}

/// Compute every rpc's typed signature from the spec's operations.
#[must_use]
pub fn rpc_signatures(spec: &OpenApiSpec) -> Vec<RpcSig> {
    let mut sink = String::new();
    let mut imps = BTreeSet::new();
    spec.all_operations()
        .filter_map(|(_m, _p, op)| {
            let op_id = op.operation_id.as_ref()?;
            let rpc = op_id.to_upper_camel_case();
            let resp_type = response_type(&rpc, op.success_response_schema(), &mut sink, &mut imps);
            Some(RpcSig {
                method: rpc.to_snake_case(),
                req_type: format!("{rpc}Request"),
                resp_type,
                rpc,
            })
        })
        .collect()
}

/// Emit the service + the synthesized request/response messages for each operation.
fn emit_service(spec: &OpenApiSpec, package: &str, out: &mut String, imports: &mut BTreeSet<String>) {
    let service = service_name(package);
    let mut rpcs = String::new();
    let mut messages = String::new();

    for (_method, _path, op) in spec.all_operations() {
        let Some(op_id) = &op.operation_id else { continue };
        let rpc = op_id.to_upper_camel_case();

        // request message: path/query params + body.
        let req_name = format!("{rpc}Request");
        let mut field_no = 0usize;
        let mut req_fields = String::new();
        for p in &op.parameters {
            if matches!(p.location.as_str(), "path" | "query") {
                let sch = p.schema.clone().unwrap_or_default();
                let (label, ty) = field_type(&sch, imports);
                field_no += 1;
                req_fields.push_str(&format!("  {label}{ty} {} = {};\n", p.name.to_snake_case(), field_no));
            }
        }
        if let Some(body) = op.json_body_schema() {
            if body.is_ref() {
                field_no += 1;
                let ty = ref_name(body.ref_path.as_deref().unwrap_or("")).to_upper_camel_case();
                req_fields.push_str(&format!("  {ty} body = {field_no};\n"));
            } else if !body.properties.is_empty() {
                // inline body object → embed its properties as request fields.
                for (prop, pschema) in &body.properties {
                    let (label, ty) = field_type(pschema, imports);
                    field_no += 1;
                    req_fields.push_str(&format!("  {label}{ty} {} = {};\n", prop.to_snake_case(), field_no));
                }
            } else {
                imports.insert("google/protobuf/struct.proto".into());
                field_no += 1;
                req_fields.push_str(&format!("  {STRUCT} body = {field_no};\n"));
            }
        }
        messages.push_str(&format!("message {req_name} {{\n{req_fields}}}\n\n"));

        // response type: the 200 schema.
        let resp_ty = response_type(&rpc, op.success_response_schema(), &mut messages, imports);

        rpcs.push_str(&format!("  rpc {rpc}({req_name}) returns ({resp_ty});\n"));
    }

    out.push_str(&messages);
    out.push_str(&format!("service {service} {{\n{rpcs}}}\n"));
}

/// Resolve the rpc return type, synthesizing a `<Rpc>Response` message when the
/// 200 schema is an array or inline object.
fn response_type(
    rpc: &str,
    schema: Option<&Schema>,
    messages: &mut String,
    imports: &mut BTreeSet<String>,
) -> String {
    let Some(schema) = schema else {
        imports.insert("google/protobuf/empty.proto".into());
        return EMPTY.to_string();
    };
    if schema.is_ref() {
        return ref_name(schema.ref_path.as_deref().unwrap_or("")).to_upper_camel_case();
    }
    let resp = format!("{rpc}Response");
    if schema.is_array() {
        let (label, ity) = field_type(schema, imports);
        messages.push_str(&format!("message {resp} {{\n  {label}{ity} items = 1;\n}}\n\n"));
    } else if !schema.properties.is_empty() {
        emit_message(&resp, schema, messages, imports);
    } else {
        imports.insert("google/protobuf/struct.proto".into());
        messages.push_str(&format!("message {resp} {{\n  {STRUCT} value = 1;\n}}\n\n"));
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = r##"
openapi: 3.0.3
info: { title: breathe control API, version: 0.1.0 }
paths:
  /api/v1/catalog:
    get:
      operationId: catalogList
      responses:
        "200": { content: { application/json: { schema: { $ref: "#/components/schemas/Catalog" } } } }
  /api/v1/bands/{kind}:
    get:
      operationId: bandList
      parameters:
        - { name: kind, in: path, required: true, schema: { type: string } }
        - { name: namespace, in: query, schema: { type: string } }
      responses:
        "200": { content: { application/json: { schema: { type: array, items: { $ref: "#/components/schemas/Band" } } } } }
  /api/v1/bands/{kind}/{namespace}/{name}/dry-run:
    patch:
      operationId: bandSetDryRun
      parameters:
        - { name: kind, in: path, schema: { type: string } }
        - { name: namespace, in: path, schema: { type: string } }
        - { name: name, in: path, schema: { type: string } }
      requestBody:
        content: { application/json: { schema: { type: object, properties: { dryRun: { type: boolean } } } } }
      responses:
        "200": { content: { application/json: { schema: { $ref: "#/components/schemas/Band" } } } }
components:
  schemas:
    BandKind: { type: string, enum: [memory, cpu, storage, arc, cgroup] }
    BandStatus:
      type: object
      properties:
        phase: { type: string }
        lastChangeEpoch: { type: integer }
    Band:
      type: object
      properties:
        spec: { type: object }
        status: { $ref: "#/components/schemas/BandStatus" }
    Catalog:
      type: object
      properties:
        dimensions: { type: array, items: { type: object } }
"##;

    fn proto() -> String {
        let spec: OpenApiSpec = serde_yaml_ng::from_str(SPEC).unwrap();
        emit(&spec, "breathe.v1")
    }

    #[test]
    fn header_package_and_syntax() {
        let p = proto();
        assert!(p.starts_with("syntax = \"proto3\";"));
        assert!(p.contains("package breathe.v1;"));
    }

    #[test]
    fn enum_maps_with_unspecified_zero() {
        let p = proto();
        assert!(p.contains("enum BandKind {"));
        assert!(p.contains("BAND_KIND_UNSPECIFIED = 0;"));
        assert!(p.contains("BAND_KIND_MEMORY = 1;"));
        assert!(p.contains("BAND_KIND_CGROUP = 5;"));
    }

    #[test]
    fn object_schema_becomes_typed_message_with_ref_and_scalar() {
        let p = proto();
        assert!(p.contains("message BandStatus {"));
        // fields number alphabetically (BTreeMap, deterministic): lastChangeEpoch < phase
        assert!(p.contains("int64 last_change_epoch = 1;"));
        assert!(p.contains("string phase = 2;"));
        // a $ref property keeps the typed message name
        assert!(p.contains("BandStatus status ="));
    }

    #[test]
    fn service_rpcs_with_synthesized_request_and_typed_response() {
        let p = proto();
        assert!(p.contains("service Breathe {"));
        // array response → synthesized <Rpc>Response { repeated Band items }
        assert!(p.contains("rpc BandList(BandListRequest) returns (BandListResponse);"));
        assert!(p.contains("repeated Band items = 1;"));
        // $ref response → the message directly
        assert!(p.contains("rpc CatalogList(CatalogListRequest) returns (Catalog);"));
        // inline body props embedded into the request + path params typed
        assert!(p.contains("rpc BandSetDryRun(BandSetDryRunRequest) returns (Band);"));
        assert!(p.contains("bool dry_run ="));
        assert!(p.contains("string kind ="));
    }

    #[test]
    fn struct_import_only_when_used() {
        let p = proto();
        // Band.spec is an inline object → Struct → the import is present
        assert!(p.contains("import \"google/protobuf/struct.proto\";"));
        assert!(p.contains("google.protobuf.Struct spec ="));
    }
}
