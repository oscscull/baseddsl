//! Client (query/mutation -> typed Rust surface) codegen tests. Parse + check a
//! whole-schema snippet, then assert on the generated module text. The headline
//! assertions are the input/output typing, the pagination envelope, and the wire
//! routes (calling.md's closed RPC surface).

use based_ast::FileId;
use based_codegen::client::{client, ClientTarget};
use based_parser::parse_file;
use based_sema::check;

fn gen(src: &str) -> String {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    client(&schema, &sf.decls, ClientTarget::Rust)
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
        out.contains("pub fn order_by_id(&self, input: OrderByIdInput) -> Result<Option<OrderCard>, ClientError>"),
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
        out.contains("pub fn orders_in_org(&self, input: OrdersInOrgInput) -> Result<Vec<OrderCard>, ClientError>"),
        "\n{out}"
    );
    // A relation same-name param is the FK id on the wire.
    assert!(out.contains("pub struct OrdersInOrgInput {"), "\n{out}");
    assert!(out.contains("pub org: Uuid,"), "\n{out}");
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
            "pub fn active(&self, input: ActiveInput) -> Result<Page<ProductCard>, ClientError>"
        ),
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
    // `-> placed_by` edge param is the FK id; `since` keeps its explicit type.
    assert!(out.contains("pub user: Uuid,"), "\n{out}");
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
            "pub fn place_order(&self, input: PlaceOrderInput) -> Result<OrderCard, ClientError>"
        ),
        "\n{out}"
    );
    assert!(out.contains("pub org: Uuid,"), "\n{out}");
    assert!(out.contains("pub buyer: Uuid,"), "\n{out}");
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
    // Scalars by type, the forward FK as a Uuid under the relation field name.
    assert!(out.contains("pub struct Order {"), "\n{out}");
    assert!(out.contains("pub status: String,"), "\n{out}");
    assert!(out.contains("pub total: i64,"), "\n{out}");
    assert!(out.contains("pub org: Uuid,"), "\n{out}");
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
