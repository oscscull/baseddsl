//! OpenAPI (query/mutation -> OpenAPI 3.1 spec) codegen tests. Parse + check a whole-
//! schema snippet, emit the spec, and assert on the resulting JSON tree (parsed, so
//! the assertions are structural, not text-position dependent). The headline
//! assertions are the per-callable path + operation, the input/response schema
//! mapping, and the shared error/`$ctx`-header surface .

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
    // The pagination envelope: rows + an opaque cursor.
    assert_eq!(sch["type"], "object");
    assert_eq!(sch["properties"]["rows"]["type"], "array");
    assert_eq!(
        sch["properties"]["rows"]["items"]["$ref"],
        "#/components/schemas/ProductCard"
    );
    // cursor is nullable string.
    assert_eq!(sch["properties"]["cursor"]["type"][0], "string");
    assert_eq!(sch["properties"]["cursor"]["type"][1], "null");
    // A keyset page's request body carries the opaque cursor back for the next page.
    let input = &doc["components"]["schemas"]["ActiveInput"];
    assert_eq!(input["properties"]["cursor"]["type"][0], "string");
    assert_eq!(input["properties"]["cursor"]["type"][1], "null");
    // The cursor is optional (absent = first page), so never in `required`.
    assert!(
        !input["required"]
            .as_array()
            .map(|r| r.iter().any(|v| v == "cursor"))
            .unwrap_or(false),
        "\n{input}"
    );
}

#[test]
fn offset_page_input_carries_offset() {
    let doc = gen(r#"
        Post { id: Id, title: text }
        shape PostCard from Post { title }
        query posts() -> PostCard[] {
          list Post order (id asc) page (50) offset;
        }
        "#);
    // An offset page's request body carries an integer offset, not a cursor.
    let input = &doc["components"]["schemas"]["PostsInput"];
    assert_eq!(input["properties"]["offset"]["type"], "integer");
    assert!(input["properties"]["cursor"].is_null(), "\n{input}");
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
    // A mutation posts to `/m/<name>` and (create-returning) advertises the shape.
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
    // The `$ctx.org` the query silently requires , typed by inference, surfaces
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

#[test]
fn nest_ref_emits_schema_ref_to_the_named_shape() {
    // A named-shape nest (`placed_by -> UserRef`) `$ref`s the shape's own component
    // schema instead of inlining an object; the referenced schema is registered once
    // even when no callable returns it directly. To-many refs are arrays of the `$ref`.
    let doc = gen(r#"
        User { name: text, email: text }
        @sort(id asc)
        Order { placed_by: User, fulfilled_by: User?, total: int, items: OrderItem[] }
        @sort(id asc)
        OrderItem { order: Order, sku: text }
        shape UserRef from User { name, email }
        shape ItemRow from OrderItem { sku }
        shape OrderDetail from Order {
          total
          placed_by -> UserRef
          fulfilled_by -> UserRef
          items -> ItemRow
        }
        query order_detail(id) -> OrderDetail;
        "#);
    let props = &doc["components"]["schemas"]["OrderDetail"]["properties"];
    assert_eq!(props["placed_by"]["$ref"], "#/components/schemas/UserRef");
    assert_eq!(
        props["fulfilled_by"]["$ref"],
        "#/components/schemas/UserRef"
    );
    assert_eq!(props["items"]["type"], "array");
    assert_eq!(
        props["items"]["items"]["$ref"],
        "#/components/schemas/ItemRow"
    );
    // the referenced schemas exist as components with their projected properties.
    let user_ref = &doc["components"]["schemas"]["UserRef"];
    assert_eq!(user_ref["properties"]["name"]["type"], "string");
    let item_row = &doc["components"]["schemas"]["ItemRow"];
    assert_eq!(item_row["properties"]["sku"]["type"], "string");
    // required: the non-optional relation is, the optional one is not, the array is.
    let required = doc["components"]["schemas"]["OrderDetail"]["required"]
        .as_array()
        .unwrap();
    assert!(required.iter().any(|v| v == "placed_by"));
    assert!(!required.iter().any(|v| v == "fulfilled_by"));
    assert!(required.iter().any(|v| v == "items"));
}

#[test]
fn nested_to_many_shape_emits_array_of_object_schema() {
    // A to-many `items { … }` nest becomes an `array` property whose items are the
    // element object schema; always present (empty array when childless) → required.
    let doc = gen(r#"
        @sort(id asc)
        Order { total: int, items: OrderItem[] }
        @sort(id asc)
        OrderItem { order: Order, sku: text, qty: int }
        shape OrderCard from Order { total, items { sku, qty } }
        query order_by_id(id) -> OrderCard;
        "#);
    let props = &doc["components"]["schemas"]["OrderCard"]["properties"];
    assert_eq!(props["items"]["type"], "array");
    assert_eq!(props["items"]["items"]["type"], "object");
    assert_eq!(
        props["items"]["items"]["properties"]["sku"]["type"],
        "string"
    );
    assert_eq!(
        props["items"]["items"]["properties"]["qty"]["type"],
        "integer"
    );
    let required = doc["components"]["schemas"]["OrderCard"]["required"]
        .as_array()
        .unwrap();
    assert!(required.iter().any(|v| v == "items"));
}

#[test]
fn enum_field_is_a_string_schema_with_enum_list() {
    let doc = gen(r#"
        enum Status { pending, paid, shipped }
        Order { status: Status, total: int }
        shape OrderRow from Order { status, total }
        query orders() -> OrderRow[];
    "#);
    let status = &doc["components"]["schemas"]["OrderRow"]["properties"]["status"];
    assert_eq!(status["type"], "string", "\n{doc:#}");
    assert_eq!(
        status["enum"],
        serde_json::json!(["pending", "paid", "shipped"]),
        "\n{doc:#}"
    );
}

#[test]
fn enum_annotated_param_is_the_enum_schema() {
    // `status: Status` on a signature documents as the constrained enum schema,
    // never as a uuid FK string.
    let doc = gen(r#"
        enum Status { pending, paid, shipped }
        @sort(total desc)
        Order { status: Status, total: int }
        shape OrderRow from Order { status, total }
        query by_status(status: Status) -> OrderRow[] { list Order where (status = $status); }
    "#);
    let param = &doc["components"]["schemas"]["ByStatusInput"]["properties"]["status"];
    assert_eq!(
        param["type"], "string",
        "
{doc:#}"
    );
    assert_eq!(
        param["enum"],
        serde_json::json!(["pending", "paid", "shipped"]),
        "
{doc:#}"
    );
}

#[test]
fn string_enum_with_explicit_value_lists_the_wire_values() {
    let doc = gen(r#"
        enum Status { pending, paid = "PAID" }
        Order { status: Status, total: int }
        shape OrderRow from Order { status, total }
        query orders() -> OrderRow[];
    "#);
    let status = &doc["components"]["schemas"]["OrderRow"]["properties"]["status"];
    assert_eq!(status["type"], "string", "\n{doc:#}");
    assert_eq!(
        status["enum"],
        serde_json::json!(["pending", "PAID"]),
        "\n{doc:#}"
    );
}

#[test]
fn int_enum_field_is_an_integer_schema_with_int_enum_list() {
    let doc = gen(r#"
        enum Priority { low = 0, medium = 1, high = 2 }
        Ticket { priority: Priority, title: text }
        shape TicketRow from Ticket { priority, title }
        query tickets() -> TicketRow[];
    "#);
    let priority = &doc["components"]["schemas"]["TicketRow"]["properties"]["priority"];
    assert_eq!(priority["type"], "integer", "\n{doc:#}");
    assert_eq!(priority["enum"], serde_json::json!([0, 1, 2]), "\n{doc:#}");
}

#[test]
fn decimal_is_a_string_and_float_a_number() {
    let doc = gen(r#"
        Ledger { price: decimal(12, 2), score: float }
        shape LedgerRow from Ledger { price, score }
        query ledger() -> LedgerRow[];
        "#);
    let row = &doc["components"]["schemas"]["LedgerRow"];
    // A decimal is a lossless string on the wire, never a JSON float.
    assert_eq!(row["properties"]["price"]["type"], "string");
    assert_eq!(row["properties"]["price"]["format"], "decimal");
    assert_eq!(row["properties"]["score"]["type"], "number");
    assert_eq!(row["properties"]["score"]["format"], "double");
}

#[test]
fn stream_query_response_is_ndjson_with_the_envelope_line_schema() {
    let doc = gen(r#"
        @sort(total desc)
        Order { status: text, total: int }
        shape OrderCard from Order { status, total }
        query export_orders(status) -> stream OrderCard;
        "#);
    let ok = &doc["paths"]["/q/export_orders"]["post"]["responses"]["200"];
    // The 200 body is NDJSON, never a JSON document.
    assert!(
        ok["content"]["application/json"].is_null(),
        "a stream response must not advertise application/json\n{doc:#}"
    );
    let line = &ok["content"]["application/x-ndjson"]["schema"];
    // Each line is exactly one of the three envelopes: row, terminal done, or the
    // shared error envelope.
    let one_of = line["oneOf"].as_array().expect("oneOf envelope");
    assert_eq!(one_of.len(), 3, "\n{doc:#}");
    assert_eq!(
        one_of[0]["properties"]["row"]["$ref"],
        "#/components/schemas/OrderCard"
    );
    assert_eq!(
        one_of[1]["properties"]["done"]["properties"]["rows"]["type"],
        "integer"
    );
    assert_eq!(one_of[2]["$ref"], "#/components/schemas/Error");
    // Pre-body failures keep the ordinary JSON error responses.
    assert_eq!(
        doc["paths"]["/q/export_orders"]["post"]["responses"]["400"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/Error"
    );
}

// ---------- idempotency key + guard wire surface -----------------------------

#[test]
fn mutation_documents_the_idempotency_key_header_and_outcomes() {
    let doc = gen(r#"
        Order { status: text, total: int }
        shape OrderCard from Order { status, total }
        query order_by_id(id) -> OrderCard;
        mutation place_order(status, total: int) -> OrderCard {
            create Order { status = $status, total = $total };
        }
        "#);
    // The reusable header parameter exists and the mutation references it.
    assert_eq!(
        doc["components"]["parameters"]["IdempotencyKey"]["name"],
        "Idempotency-Key"
    );
    let op = &doc["paths"]["/m/place_order"]["post"];
    assert_eq!(
        op["parameters"][1]["$ref"],
        "#/components/parameters/IdempotencyKey"
    );
    // The keyed-write outcomes are documented on the mutation only.
    assert!(op["responses"]["409"].is_object());
    assert!(op["responses"]["422"].is_object());
    let q = &doc["paths"]["/q/order_by_id"]["post"];
    assert_eq!(q["parameters"].as_array().map(Vec::len), Some(1));
    assert!(q["responses"]["409"].is_null());
}

#[test]
fn query_only_schema_carries_no_idempotency_parameter() {
    let doc = gen(r#"
        Order { status: text }
        shape OrderCard from Order { status }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(doc["components"]["parameters"]["IdempotencyKey"].is_null());
}

#[test]
fn guarded_mutation_documents_the_403_denial() {
    let doc = gen(r#"
        Order { status: text, total: int }
        shape OrderCard from Order { status, total }
        mutation close_order(id) -> OrderCard guard caller_can_close {
            update Order where (id = $id) { status = "closed" };
        }
        mutation open_order(id) -> OrderCard {
            update Order where (id = $id) { status = "open" };
        }
        "#);
    let guarded = &doc["paths"]["/m/close_order"]["post"];
    assert!(guarded["responses"]["403"].is_object());
    assert!(guarded["responses"]["403"]["description"]
        .as_str()
        .unwrap()
        .contains("caller_can_close"));
    // An unguarded mutation documents no 403.
    assert!(doc["paths"]["/m/open_order"]["post"]["responses"]["403"].is_null());
}
