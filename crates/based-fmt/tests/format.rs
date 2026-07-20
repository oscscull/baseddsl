//! Formatter conformance: the canonical worked examples are stable under `based fmt`
//! (a no-op), formatting is idempotent + reparses, and each construct renders in its
//! canonical shape.

use based_ast::FileId;
use based_fmt::format_source;
use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// The committed worked examples define the canonical style: formatting them must
/// change nothing, or the formatter and the style have diverged.
#[test]
fn canonical_examples_are_a_noop() {
    let files = [
        "spec/examples/commerce/order/model.bsl",
        "spec/examples/commerce/order/queries.bsl",
        "spec/examples/commerce/user/model.bsl",
        "spec/examples/commerce/org/model.bsl",
        "spec/examples/commerce/product/model.bsl",
        "spec/examples/commerce/product/queries.bsl",
        "spec/examples/commerce/order_item/model.bsl",
        "spec/examples/commerce/membership/model.bsl",
        "examples/sqlite-quickstart/schema/order.bsl",
        "examples/sqlite-quickstart/schema/org.bsl",
        "examples/sqlite-quickstart/schema/queries.bsl",
        "examples/sqlite-quickstart/schema/user.bsl",
        "examples/mariadb-quickstart/schema/order.bsl",
        "examples/mariadb-quickstart/schema/org.bsl",
        "examples/mariadb-quickstart/schema/queries.bsl",
        "examples/mariadb-quickstart/schema/user.bsl",
        "examples/postgres-quickstart/schema/order.bsl",
        "examples/postgres-quickstart/schema/org.bsl",
        "examples/postgres-quickstart/schema/queries.bsl",
        "examples/postgres-quickstart/schema/user.bsl",
    ];
    for f in files {
        let src = read(f);
        let out = format_source(&src).unwrap();
        assert_eq!(out, src, "formatting churned {f}");
    }
}

/// Formatting is idempotent and its output always reparses (structure-preserving).
#[test]
fn idempotent_and_reparses() {
    let mut inputs: Vec<String> = Vec::new();
    for dir in ["tests/conformance-sema", "tests/conformance"] {
        for entry in fs::read_dir(repo_root().join(dir)).unwrap() {
            let path = entry.unwrap().path().join("input.bsl");
            if path.exists() {
                inputs.push(fs::read_to_string(path).unwrap());
            }
        }
    }
    for src in inputs {
        // Skip the deliberately-unparseable fixtures — an unparseable file is not formattable.
        let Ok(out) = format_source(&src) else {
            continue;
        };
        assert!(
            based_parser::parse_file(&out, FileId(0)).is_ok(),
            "formatted output did not reparse:\n{out}"
        );
        let twice = format_source(&out).unwrap();
        assert_eq!(out, twice, "formatting was not idempotent");
    }
}

fn fmt(src: &str) -> String {
    format_source(src).unwrap()
}

#[test]
fn field_type_column_aligns() {
    let got = fmt("Order {\n deleted_at: timestamp?\n fulfilled_by: User?\n total: int\n}");
    assert_eq!(
        got,
        "Order {\n  deleted_at:   timestamp?\n  fulfilled_by: User?\n  total:        int\n}\n"
    );
}

#[test]
fn inverse_ref_column_aligns() {
    let got = fmt("User {\n invited_users: User[] (User.invited_by)\n placed_orders: Order[] (Order.placed_by)\n}");
    assert_eq!(
        got,
        "User {\n  invited_users: User[]  (User.invited_by)\n  placed_orders: Order[] (Order.placed_by)\n}\n"
    );
}

#[test]
fn index_forms() {
    let got = fmt("W {\n a: int\n @index a\n @index(a, b)\n @index(a) unique\n}");
    assert_eq!(
        got,
        "W {\n  a: int\n  @index a\n  @index(a, b)\n  @index a unique\n}\n"
    );
}

#[test]
fn shape_inline_below_threshold_else_multiline() {
    // Short shapes stay on one line.
    assert_eq!(
        fmt("shape OrgRow from Org { id, name, slug }"),
        "shape OrgRow from Org { id, name, slug }\n"
    );
    // A wider shape breaks a field per line, aligning rename `=`.
    assert_eq!(
        fmt("shape OrderCard from Order { status, total, buyer = placed_by.name, org = org.name }"),
        "shape OrderCard from Order {\n  status\n  total\n  buyer = placed_by.name\n  org   = org.name\n}\n"
    );
}

#[test]
fn shape_nest_ref_formats_canonically() {
    // A named-shape nest prints `field -> Shape`, inline or one per line by width.
    assert_eq!(
        fmt("shape D from Order { total, buyer   ->   UserRef }"),
        "shape D from Order { total, buyer -> UserRef }\n"
    );
    assert_eq!(
        fmt("shape OrderDetail from Order { status, total, placed_by -> UserRef }"),
        "shape OrderDetail from Order {\n  status\n  total\n  placed_by -> UserRef\n}\n"
    );
}

#[test]
fn query_block_inline_and_expanded_by_clause_count() {
    // 0 clauses: inline block.
    assert_eq!(
        fmt("query my() -> Card[] scoped Tenant { list Order; }"),
        "query my() -> Card[] scoped Tenant { list Order; }\n"
    );
    // 1 clause: still inline.
    assert_eq!(
        fmt("query my() -> Card[] { list Order where (org = $ctx.org); }"),
        "query my() -> Card[] { list Order where (org = $ctx.org); }\n"
    );
    // 2 clauses: expanded block, statement on one line.
    assert_eq!(
        fmt("query my() -> Card[] { list Order order (placed_at desc) page (2); }"),
        "query my() -> Card[] {\n  list Order order (placed_at desc) page (2);\n}\n"
    );
    // 3 clauses: a clause per line.
    assert_eq!(
        fmt("query p(org: Id) -> Card[] { list Product where (org = $org and active) order (created_at desc) page (20); }"),
        "query p(org: Id) -> Card[] {\n  list Product\n    where (org = $org and active)\n    order (created_at desc)\n    page (20);\n}\n"
    );
}

#[test]
fn stream_return_form_round_trips() {
    assert_eq!(
        fmt("query export() ->   stream   OrderCard scoped Tenant;"),
        "query export() -> stream OrderCard scoped Tenant;\n"
    );
    assert_eq!(
        fmt("query export() -> stream OrderCard { list Order; }"),
        "query export() -> stream OrderCard { list Order; }\n"
    );
}

#[test]
fn mutation_body_and_tx() {
    let got = fmt(
        "mutation m(e: text, c: text) -> R { tx { create User { email = $e } create Addr { user = ^.id, city = $c } } }",
    );
    assert_eq!(
        got,
        "mutation m(e: text, c: text) -> R {\n  tx {\n    create User { email = $e };\n    create Addr { user = ^.id, city = $c };\n  }\n}\n"
    );
}

#[test]
fn predicate_precedence_parenthesizes_minimally() {
    // `or` under `and` needs parens; the redundant set around an `and` under `or` is dropped.
    let got = fmt("filter f = (a or b) and c;");
    assert_eq!(got, "filter f = (a or b) and c;\n");
    let got = fmt("filter g = a or (b and c);");
    assert_eq!(got, "filter g = a or b and c;\n");
}

#[test]
fn in_value_list_formats_canonically() {
    // Elements one-space separated; `not` needs no parens around the atom; the
    // single-bind form stays bare.
    let got = fmt("filter f = status in (open,waiting,  $extra) and id in $ids;");
    assert_eq!(
        got,
        "filter f = status in (open, waiting, $extra) and id in $ids;\n"
    );
    let got = fmt("filter g = not (status in (resolved, closed));");
    assert_eq!(got, "filter g = not status in (resolved, closed);\n");
}

#[test]
fn comments_preserved_including_between_decorators() {
    let src = "# header\n@soft_delete(deleted_at)\n# between decorators\n@scope Tenant\nOrder {\n  deleted_at: timestamp?\n}\n";
    assert_eq!(fmt(src), src);
}

#[test]
fn raw_query_body_reprints_byte_exactly() {
    // A single-line raw body inlines like a one-clause block; the SQL text between
    // the backticks is opaque and never re-wrapped. Idempotent.
    let got = fmt("query all()   ->  UserRow[] { raw`SELECT name FROM user` }");
    assert_eq!(
        got,
        "query all() -> UserRow[] { raw`SELECT name FROM user`; }\n"
    );
    assert_eq!(fmt(&got), got);

    // A multi-line raw body expands to a block, its interior untouched.
    let src = "query heavy(min: int) -> UserRow[] {\n  raw`SELECT u.name AS name\n      FROM user u\n      WHERE u.total >= ${min}`;\n}\n";
    let got = fmt(src);
    assert_eq!(got, src);
    assert_eq!(fmt(&got), got);
}

#[test]
fn atomic_update_expr_reprints_with_minimal_parens() {
    // An arithmetic assignment RHS reprints canonically: one space around each
    // operator, parentheses only where precedence/associativity require them.
    let got = fmt(
        "mutation adjust(id: Id, base: int, n: int) -> P {\n  update Product where (id = $id) { qty=(qty+$base)*$n-1 };\n}\n",
    );
    assert_eq!(
        got,
        "mutation adjust(id: Id, base: int, n: int) -> P {\n  update Product where (id = $id) { qty = (qty + $base) * $n - 1 };\n}\n"
    );
    assert!(based_parser::parse_file(&got, FileId(0)).is_ok());
    assert_eq!(fmt(&got), got);
}

#[test]
fn ack_return_prints_bare_ok() {
    let got = fmt("mutation purge(id: Id) -> ok   { hard delete Comment where (id = $id) }");
    assert_eq!(
        got,
        "mutation purge(id: Id) -> ok {\n  hard delete Comment where (id = $id);\n}\n"
    );
}
