//! Client (query/mutation -> typed Rust surface) codegen tests. Parse + check a
//! whole-schema snippet, then assert on the generated module text. The headline
//! assertions are the input/output typing, the pagination envelope, and the wire
//! routes (calling.md's closed RPC surface).

use based_ast::FileId;
use based_codegen::client::{client, client_with, ClientOptions, ClientTarget};
use based_parser::parse_file;
use based_sema::check;

fn gen(src: &str) -> String {
    gen_opts(src, ClientOptions::default())
}

fn gen_opts(src: &str, opts: ClientOptions) -> String {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    client_with(&schema, &sf.decls, ClientTarget::Rust, opts)
}

#[test]
fn preamble_carries_envelope_and_transport() {
    let out = gen(r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, status: text }
        shape OrderCard from Order { status }
        query order_by_id(id) -> OrderCard;
        "#);
    // The fixed surface every module needs.
    assert!(out.contains("pub struct Page<T>"), "\n{out}");
    assert!(out.contains("pub trait Transport"), "\n{out}");
    assert!(out.contains("pub struct Client<T>"), "\n{out}");
    assert!(out.contains("pub type Uuid = String;"), "\n{out}");
}

#[test]
fn get_query_returns_option_of_shape() {
    let out = gen(r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, status: text, total: int }
        shape OrderCard from Order { status, total }
        query order_by_id(id) -> OrderCard;
        "#);
    // Output struct from the shape body, typed per column.
    assert!(out.contains("pub struct OrderCard {"), "\n{out}");
    assert!(out.contains("pub status: String,"), "\n{out}");
    assert!(out.contains("pub total: i64,"), "\n{out}");
    // A `get` (single, unique key) -> Option<T>.
    assert!(
        out.contains("pub fn order_by_id(&self, input: OrderByIdInput, ctx: ()) -> Result<Option<OrderCard>, ClientError>"),
        "\n{out}"
    );
    assert!(
        out.contains("pub const ORDER_BY_ID_ROUTE: &str = \"/q/order_by_id\";"),
        "\n{out}"
    );
}

#[test]
fn list_query_returns_vec() {
    let out = gen(r#"
        @soft_delete(deleted_at)
        Org { deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, org: Org, total: int }
        shape OrderCard from Order { total }
        query orders_in_org(org) -> OrderCard[];
        "#);
    assert!(
        out.contains("pub fn orders_in_org(&self, input: OrdersInOrgInput, ctx: ()) -> Result<Vec<OrderCard>, ClientError>"),
        "\n{out}"
    );
    // A relation same-name param is the target's typed id (the FK on the wire).
    assert!(out.contains("pub struct OrdersInOrgInput {"), "\n{out}");
    assert!(out.contains("pub org: Id<entity::Org>,"), "\n{out}");
}

#[test]
fn paginated_query_returns_page_envelope() {
    let out = gen(r#"
        @soft_delete(deleted_at)
        Product { deleted_at: timestamp?, name: text, active: bool }
        shape ProductCard from Product { name }
        query active(org: Id) -> ProductCard[] {
          list Product where (active) page (20);
        }
        "#);
    assert!(
        out.contains(
            "pub fn active(&self, input: ActiveInput, ctx: ()) -> Result<Page<ProductCard>, ClientError>"
        ),
        "\n{out}"
    );
    // A keyset page's input carries the opaque cursor to fetch the next page.
    assert!(out.contains("pub cursor: Option<String>"), "\n{out}");
}

#[test]
fn offset_page_input_carries_offset() {
    let out = gen(r#"
        Post { id: Id, title: text }
        shape PostCard from Post { title }
        query posts() -> PostCard[] {
          list Post order (id asc) page (50) offset;
        }
        "#);
    // An offset page's input carries an explicit offset, and no cursor (the `pub cursor`
    // in the shared `Page<T>` envelope is the only one in the module).
    assert!(
        out.contains("pub struct PostsInput {\n    pub offset: Option<i64>,\n}"),
        "\n{out}"
    );
}

#[test]
fn explicit_param_type_and_relation_reach() {
    // `buyer` reaches `placed_by.name` (a joined text column); the `since`/`user`
    // params carry explicit and inferred types respectively.
    let out = gen(r#"
        @soft_delete(deleted_at)
        User { deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, placed_by: User, created_at: timestamp }
        shape OrderCard from Order { buyer = placed_by.name }
        query recent(user -> placed_by, since: timestamp > created_at) -> OrderCard[];
        "#);
    // Reached relation column keeps its scalar type.
    assert!(out.contains("pub buyer: String,"), "\n{out}");
    // `-> placed_by` edge param is the target's typed id; `since` keeps its explicit type.
    assert!(out.contains("pub user: Id<entity::User>,"), "\n{out}");
    assert!(out.contains("pub since: Timestamp,"), "\n{out}");
}

#[test]
fn mutation_returns_single_shape_and_maps_to_m_route() {
    let out = gen(r#"
        @soft_delete(deleted_at)
        Org { deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, org: Org, placed_by: Org, total: int }
        shape OrderCard from Order { total }
        mutation place_order(org: Id, buyer: Id) -> OrderCard {
          create Order { org = $org, placed_by = $buyer, total = 0 };
        }
        "#);
    // A mutation posts to `/m/<name>` and returns the (single) shape.
    assert!(
        out.contains("pub const PLACE_ORDER_ROUTE: &str = \"/m/place_order\";"),
        "\n{out}"
    );
    assert!(
        out.contains(
            "pub fn place_order(&self, input: PlaceOrderInput, ctx: ()) -> Result<OrderCard, ClientError>"
        ),
        "\n{out}"
    );
    // Both params are bare `Id` annotations, but the body resolves them to their write
    // targets (`org` → Order.org, `buyer` → placed_by), so each carries the typed id.
    assert!(out.contains("pub org: Id<entity::Org>,"), "\n{out}");
    assert!(out.contains("pub buyer: Id<entity::Org>,"), "\n{out}");
}

#[test]
fn bare_model_return_projects_every_stored_column() {
    let out = gen(r#"
        @soft_delete(deleted_at)
        Org { deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, org: Org, status: text, total: int }
        query order_by_id(id) -> Order;
        "#);
    // Scalars by type, the own `id` and the forward FK as typed ids under the field name.
    assert!(out.contains("pub struct Order {"), "\n{out}");
    assert!(out.contains("pub id: Id<entity::Order>,"), "\n{out}");
    assert!(out.contains("pub status: String,"), "\n{out}");
    assert!(out.contains("pub total: i64,"), "\n{out}");
    assert!(out.contains("pub org: Id<entity::Org>,"), "\n{out}");
}

#[test]
fn optional_and_defaulted_params_are_option() {
    let out = gen(r#"
        @soft_delete(deleted_at)
        Product { deleted_at: timestamp?, name: text, active: bool }
        shape ProductCard from Product { name }
        query search(name: text?, limit: int = 20) -> ProductCard[] {
          list Product where (name = $name) page (20);
        }
        "#);
    // Optional annotation and a defaulted param both become Option<T>.
    assert!(out.contains("pub name: Option<String>,"), "\n{out}");
    assert!(out.contains("pub limit: Option<i64>,"), "\n{out}");
}

#[test]
fn shared_shape_emits_one_struct() {
    let out = gen(r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, status: text }
        shape OrderCard from Order { status }
        query a(id) -> OrderCard;
        query b(status) -> OrderCard[];
        "#);
    // Two callables returning OrderCard -> exactly one struct definition.
    assert_eq!(out.matches("pub struct OrderCard {").count(), 1, "\n{out}");
}

#[test]
fn keyword_field_is_raw_escaped() {
    let out = gen(r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, type: text }
        shape OrderCard from Order { type }
        query order_by_id(id) -> OrderCard;
        "#);
    // A DSL field named `type` collides with a Rust keyword -> `r#type`.
    assert!(out.contains("pub r#type: String,"), "\n{out}");
}

#[test]
fn ctx_transport_carries_typed_context() {
    // The abstract transport threads a typed context alongside the input (D30).
    let out = gen(r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, status: text }
        shape OrderCard from Order { status }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(
        out.contains(
            "fn call<I, C, O>(&self, route: &str, input: &I, ctx: &C) -> Result<O, ClientError>"
        ),
        "\n{out}"
    );
}

#[test]
fn callable_reading_ctx_gets_typed_ctx_struct() {
    // A `$ctx.<field>` requirement (D4/D5) surfaces as a per-callable `<Name>Ctx`
    // struct the method takes; a relation field carries the model's typed id.
    let out = gen(r#"
        @soft_delete(deleted_at)
        Org { deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, org: Org, status: text }
        shape OrderCard from Order { status }
        query my_org_orders() -> OrderCard[] { list Order where (org = $ctx.org); }
        "#);
    // The typed context struct exists, `org` typed as the relation's typed id.
    assert!(out.contains("pub struct MyOrgOrdersCtx {"), "\n{out}");
    assert!(out.contains("pub org: Id<entity::Org>,"), "\n{out}");
    // The method takes it (not `()`) and forwards it to the transport.
    assert!(
        out.contains("pub fn my_org_orders(&self, input: MyOrgOrdersInput, ctx: MyOrgOrdersCtx) -> Result<Vec<OrderCard>, ClientError>"),
        "\n{out}"
    );
    assert!(
        out.contains("self.transport.call(MY_ORG_ORDERS_ROUTE, &input, &ctx)"),
        "\n{out}"
    );
}

#[test]
fn public_callable_takes_unit_ctx_and_emits_no_ctx_struct() {
    // A callable that reads no `$ctx` stays clean: `ctx: ()`, no `<Name>Ctx` struct.
    let out = gen(r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, status: text }
        shape OrderCard from Order { status }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(!out.contains("OrderByIdCtx"), "\n{out}");
    assert!(
        out.contains(
            "pub fn order_by_id(&self, input: OrderByIdInput, ctx: ()) -> Result<Option<OrderCard>, ClientError>"
        ),
        "\n{out}"
    );
}

#[test]
fn ctx_scalar_field_typed_by_inference() {
    // A `$ctx` field compared against a scalar column infers that column's type
    // (here `int`), not a Uuid — the same inference untyped params use.
    let out = gen(r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, status: text, tier: int }
        shape OrderCard from Order { status }
        query my_tier() -> OrderCard[] { list Order where (tier = $ctx.tier); }
        "#);
    assert!(out.contains("pub struct MyTierCtx {"), "\n{out}");
    assert!(out.contains("pub tier: i64,"), "\n{out}");
}

#[test]
fn nested_to_one_shape_emits_nested_struct() {
    // A to-one `placed_by { … }` nest emits a nested struct `<Parent><Field>` and the
    // parent field takes that type; an optional relation nests as `Option<…>`.
    let out = gen(r#"
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
    // parent references the nested struct type (required + optional relations).
    assert!(out.contains("pub struct OrderCard {"), "\n{out}");
    assert!(out.contains("pub total: i64,"), "\n{out}");
    assert!(out.contains("pub placed_by: OrderCardPlacedBy,"), "\n{out}");
    assert!(
        out.contains("pub fulfilled_by: Option<OrderCardFulfilledBy>,"),
        "\n{out}"
    );
    // the nested structs are emitted with their projected fields.
    assert!(out.contains("pub struct OrderCardPlacedBy {"), "\n{out}");
    assert!(out.contains("pub name: String,"), "\n{out}");
    assert!(out.contains("pub email: String,"), "\n{out}");
    assert!(out.contains("pub struct OrderCardFulfilledBy {"), "\n{out}");
}

#[test]
fn nested_to_many_shape_emits_vec_of_nested_struct() {
    // A to-many `items { … }` nest emits an element struct `<Parent><Field>` and the
    // parent field takes `Vec<…>` (the runtime decodes the SQL JSON array into it).
    let out = gen(r#"
        @sort(id asc)
        Order { total: int, items: OrderItem[] }
        @sort(id asc)
        OrderItem { order: Order, sku: text, qty: int }
        shape OrderCard from Order { total, items { sku, qty } }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(out.contains("pub struct OrderCard {"), "\n{out}");
    assert!(out.contains("pub items: Vec<OrderCardItems>,"), "\n{out}");
    assert!(out.contains("pub struct OrderCardItems {"), "\n{out}");
    assert!(out.contains("pub sku: String,"), "\n{out}");
    assert!(out.contains("pub qty: i64,"), "\n{out}");
}

#[test]
fn embedded_bridge_is_gated_on_the_option() {
    let src = r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, status: text }
        shape OrderCard from Order { status }
        query order_by_id(id) -> OrderCard;
        "#;
    // The wire client (default `client()`) must NOT reference based-runtime — a pure-wire
    // consumer need not depend on it, so forcing the reference would break its build.
    let sf = parse_file(src, FileId(0)).unwrap();
    let (schema, _) = check(&sf.decls);
    let wire = client(&schema, &sf.decls, ClientTarget::Rust);
    assert!(!wire.contains("based_runtime"), "\n{wire}");
    assert!(!wire.contains("pub fn embedded("), "\n{wire}");

    // With the option on, the embedded bridge is appended: the `Embedded` transport over
    // `based_runtime::Engine` and the one-call `embedded(&engine)` constructor.
    let embed = gen_opts(src, ClientOptions { embedded: true });
    assert!(
        embed.contains("pub fn embedded(engine: &based_runtime::Engine) -> Client<Embedded<'_>>"),
        "\n{embed}"
    );
    assert!(
        embed.contains("impl Transport for Embedded<'_>"),
        "\n{embed}"
    );
    assert!(
        embed.contains("self.engine.call(route, args, ctx)"),
        "\n{embed}"
    );
}

#[test]
fn typed_ids_are_phantom_newtypes_per_entity() {
    let out = gen(r#"
        @soft_delete(deleted_at)
        Org { deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        User { deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, org: Org, placed_by: User, total: int }
        shape OrderCard from Order { id, total, owner = org.id }
        query order_by_id(id) -> OrderCard;
        "#);
    // The transparent phantom newtype + its explicit raw constructor.
    assert!(out.contains("pub struct Id<E> {"), "\n{out}");
    assert!(
        out.contains("#[serde(transparent, bound = \"\")]"),
        "\n{out}"
    );
    assert!(out.contains("pub fn from_raw("), "\n{out}");
    // A marker per model, so `Id<entity::Org>` and `Id<entity::User>` differ.
    assert!(out.contains("pub mod entity {"), "\n{out}");
    assert!(out.contains("pub enum Org {}"), "\n{out}");
    assert!(out.contains("pub enum User {}"), "\n{out}");
    assert!(out.contains("pub enum Order {}"), "\n{out}");
    // The shape's `id` is the row's own typed id; a reached FK's id is the target's.
    assert!(out.contains("pub id: Id<entity::Order>,"), "\n{out}");
    assert!(out.contains("pub owner: Id<entity::Org>,"), "\n{out}");
    // The blanket `From<String>` hole stays closed — no such impl is emitted.
    assert!(!out.contains("impl<E> From<String> for Id<E>"), "\n{out}");
}

#[test]
fn no_inner_allow_attribute_so_include_accepts_it() {
    // The module must not carry an inner `#![allow(dead_code)]` — `include!` rejects inner
    // attributes, so consumers apply an outer `#[allow(dead_code)] mod client { … }` instead
    // (no string surgery, D62).
    let out = gen(r#"
        @soft_delete(deleted_at)
        Order { deleted_at: timestamp?, status: text }
        shape OrderCard from Order { status }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(
        !out.lines().any(|l| l.trim_start().starts_with("#![")),
        "\n{out}"
    );
}
