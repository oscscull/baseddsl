//! OpenAPI (query/mutation -> OpenAPI 3.1 spec) codegen tests. Parse + check a whole-
//! schema snippet, emit the spec, and assert on the resulting JSON tree (parsed, so
//! the assertions are structural, not text-position dependent). The headline
//! assertions are the per-callable path + operation, the input/response schema
//! mapping, and the shared error/`$ctx`-header surface (D23).

use based_ast::FileId;
use based_codegen::openapi::openapi;
use based_parser::parse_file;
use based_sema::check;
use serde_json::Value;

/// Parse + check `src`, emit the spec, and return it parsed into a JSON tree.
fn gen(src: &str) -> Value {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    let text = openapi(&schema, &sf.decls);
    serde_json::from_str(&text).expect("emitted spec is valid JSON")
}

/// The `200` response schema of an operation, as a JSON value.
fn ok_schema<'a>(doc: &'a Value, path: &str) -> &'a Value {
    &doc["paths"][path]["post"]["responses"]["200"]["content"]["application/json"]["schema"]
}

#[test]
fn document_carries_openapi_version_and_shared_surface() {
    let doc = gen(r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, status: text }
        shape OrderCard from Order { status }
        query order_by_id(id) -> OrderCard;
        "#);
    assert_eq!(doc["openapi"], "3.1.0");
    // The two fixed shared schemas + the reusable ctx header parameter.
    assert!(doc["components"]["schemas"]["Error"].is_object());
    assert!(doc["components"]["schemas"]["MutationResult"].is_object());
    assert_eq!(
        doc["components"]["parameters"]["BasedContext"]["name"],
        "X-Based-Context"
    );
    assert_eq!(
        doc["components"]["parameters"]["BasedContext"]["in"],
        "header"
    );
}

#[test]
fn query_is_a_post_with_input_body_and_ctx_header() {
    let doc = gen(r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, status: text }
        shape OrderCard from Order { status }
        query order_by_id(id) -> OrderCard;
        "#);
    let op = &doc["paths"]["/q/order_by_id"]["post"];
    assert_eq!(op["operationId"], "order_by_id");
    // Body references the generated input schema.
    assert_eq!(
        op["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/OrderByIdInput"
    );
    // Every operation references the shared `$ctx` header parameter, never a body field.
    assert_eq!(
        op["parameters"][0]["$ref"],
        "#/components/parameters/BasedContext"
    );
    // The shared error responses are present.
    assert!(op["responses"]["400"].is_object());
    assert!(op["responses"]["404"].is_object());
    assert!(op["responses"]["503"].is_object());
}

#[test]
fn get_query_response_is_nullable_shape() {
    let doc = gen(r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, status: text, total: int }
        shape OrderCard from Order { status, total }
        query order_by_id(id) -> OrderCard;
        "#);
    // A `get` (single, keyed) may miss -> the shape or null.
    let one_of = &ok_schema(&doc, "/q/order_by_id")["oneOf"];
    assert_eq!(one_of[0]["$ref"], "#/components/schemas/OrderCard");
    assert_eq!(one_of[1]["type"], "null");
    // The shape schema projects the body, typed per column.
    let card = &doc["components"]["schemas"]["OrderCard"];
    assert_eq!(card["properties"]["status"]["type"], "string");
    assert_eq!(card["properties"]["total"]["type"], "integer");
    assert_eq!(card["properties"]["total"]["format"], "int64");
}

#[test]
fn list_query_response_is_array() {
    let doc = gen(r#"
        @soft_delete(deleted_at)
        Org { deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, org: Org, total: int }
        shape OrderCard from Order { total }
        query orders_in_org(org) -> OrderCard[];
        "#);
    let sch = ok_schema(&doc, "/q/orders_in_org");
    assert_eq!(sch["type"], "array");
    assert_eq!(sch["items"]["$ref"], "#/components/schemas/OrderCard");
    // A relation same-name param is the FK uuid on the wire.
    let input = &doc["components"]["schemas"]["OrdersInOrgInput"];
    assert_eq!(input["properties"]["org"]["type"], "string");
    assert_eq!(input["properties"]["org"]["format"], "uuid");
    assert_eq!(input["required"][0], "org");
}

#[test]
fn paginated_query_response_is_page_envelope() {
    let doc = gen(r#"
        @soft_delete(deleted_at)
        Product { deleted_at: timestamp?, name: text, active: bool }
        shape ProductCard from Product { name }
        query active(org: Id) -> ProductCard[] {
          list Product where (active) page (20);
        }
        "#);
    let sch = ok_schema(&doc, "/q/active");
    // The pagination envelope: rows + an opaque cursor, never a bare array.
    assert_eq!(sch["type"], "object");
    assert_eq!(sch["properties"]["rows"]["type"], "array");
    assert_eq!(
        sch["properties"]["rows"]["items"]["$ref"],
        "#/components/schemas/ProductCard"
    );
    // cursor is nullable string.
    assert_eq!(sch["properties"]["cursor"]["type"][0], "string");
    assert_eq!(sch["properties"]["cursor"]["type"][1], "null");
}

#[test]
fn create_mutation_advertises_its_declared_shape() {
    let doc = gen(r#"
        @soft_delete(deleted_at)
        Org { deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, org: Org, placed_by: Org, total: int }
        shape OrderCard from Order { total }
        mutation place_order(org: Id, buyer: Id) -> OrderCard {
          create Order { org = $org, placed_by = $buyer, total = 0 };
        }
        "#);
    // A mutation posts to `/m/<name>` and (create-returning, D12) advertises the shape.
    let op = &doc["paths"]["/m/place_order"]["post"];
    assert_eq!(op["summary"], "Mutation `place_order`");
    assert_eq!(
        ok_schema(&doc, "/m/place_order")["$ref"],
        "#/components/schemas/OrderCard"
    );
    // Both id params ride as uuid strings.
    let input = &doc["components"]["schemas"]["PlaceOrderInput"];
    assert_eq!(input["properties"]["org"]["format"], "uuid");
    assert_eq!(input["properties"]["buyer"]["format"], "uuid");
}

#[test]
fn mutation_with_model_return_advertises_the_model() {
    // A mutation declaring a bare-model return advertises that model's schema (the
    // return type is resolvable, so `MutationResult` is not the fallback here). The
    // `{ id }` fallback covers only a callable whose return model can't be resolved.
    let doc = gen(r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, status: text }
        mutation set_status(id, status: text) -> Order {
          update Order where (id = $id) { status = $status };
        }
        "#);
    assert!(doc["components"]["schemas"]["Order"].is_object());
    assert_eq!(
        ok_schema(&doc, "/m/set_status")["$ref"],
        "#/components/schemas/Order"
    );
}

#[test]
fn bare_model_return_projects_every_stored_column() {
    let doc = gen(r#"
        @soft_delete(deleted_at)
        Org { deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, org: Org, status: text, total: int }
        query order_by_id(id) -> Order;
        "#);
    let model = &doc["components"]["schemas"]["Order"];
    // Scalars by type, the forward FK as a uuid under the relation field name.
    assert_eq!(model["properties"]["status"]["type"], "string");
    assert_eq!(model["properties"]["total"]["type"], "integer");
    assert_eq!(model["properties"]["org"]["format"], "uuid");
}

#[test]
fn optional_and_defaulted_params_are_not_required() {
    let doc = gen(r#"
        @soft_delete(deleted_at)
        Product { deleted_at: timestamp?, name: text, active: bool }
        shape ProductCard from Product { name }
        query search(name: text?, limit: int = 20) -> ProductCard[] {
          list Product where (name = $name) page (20);
        }
        "#);
    let input = &doc["components"]["schemas"]["SearchInput"];
    // Both properties present, neither in `required` (optional / defaulted).
    assert!(input["properties"]["name"].is_object());
    assert!(input["properties"]["limit"].is_object());
    let required = input["required"].as_array();
    assert!(
        required.is_none_or(|r| r.is_empty()),
        "expected no required params, got {:?}",
        input["required"]
    );
}

#[test]
fn ctx_requirements_surface_as_vendor_extension() {
    let doc = gen(r#"
        @soft_delete(deleted_at)
        Org { deleted_at: timestamp?, name: text }
        scope Tenant (org: Org = $ctx.org)
        @soft_delete(deleted_at)
        @scope Tenant
        Order { deleted_at: timestamp?, org: Org, total: int }
        shape OrderCard from Order { total }
        query my_org_orders() -> OrderCard[] scoped Tenant {
          list Order where (org = $ctx.org);
        }
        "#);
    // The `$ctx.org` the query silently requires (D4/D5), typed by inference, surfaces
    // on the operation as an `x-ctx-requires` listing (descriptive, not a body param).
    let ctx = &doc["paths"]["/q/my_org_orders"]["post"]["x-ctx-requires"];
    assert_eq!(ctx[0]["field"], "org");
    assert_eq!(ctx[0]["type"], "-> Org");
}

#[test]
fn shared_shape_emits_one_schema() {
    let doc = gen(r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, status: text }
        shape OrderCard from Order { status }
        query a(id) -> OrderCard;
        query b(status) -> OrderCard[];
        "#);
    // Two callables returning OrderCard -> exactly one schema definition (deduped).
    assert!(doc["components"]["schemas"]["OrderCard"].is_object());
    // Both operations reference it.
    assert_eq!(
        ok_schema(&doc, "/q/a")["oneOf"][0]["$ref"],
        "#/components/schemas/OrderCard"
    );
    assert_eq!(
        ok_schema(&doc, "/q/b")["items"]["$ref"],
        "#/components/schemas/OrderCard"
    );
}

#[test]
fn nested_to_one_shape_emits_inline_object_schema() {
    // A to-one `placed_by { … }` nest becomes an inline nested object property in the
    // output schema, required unless the relation is optional.
    let doc = gen(r#"
        User { name: text, email: text }
        @sort(id asc)
        Order { placed_by: User, fulfilled_by: User?, total: int }
        shape OrderCard from Order {
          total
          placed_by { name, email }
          fulfilled_by { name }
        }
        query order_by_id(id) -> OrderCard;
        "#);
    let props = &doc["components"]["schemas"]["OrderCard"]["properties"];
    // the nested to-one is an object schema with the projected properties.
    assert_eq!(props["placed_by"]["type"], "object");
    assert_eq!(props["placed_by"]["properties"]["name"]["type"], "string");
    assert_eq!(props["placed_by"]["properties"]["email"]["type"], "string");
    assert_eq!(props["fulfilled_by"]["type"], "object");
    // required: the non-optional relation is required, the optional one is not.
    let required = doc["components"]["schemas"]["OrderCard"]["required"]
        .as_array()
        .unwrap();
    assert!(required.iter().any(|v| v == "placed_by"));
    assert!(!required.iter().any(|v| v == "fulfilled_by"));
}
