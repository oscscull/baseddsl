//! Mutation (write) -> INSERT/UPDATE/DELETE codegen tests. Parse + check a whole
//! schema, then assert on the generated statements. The headline assertions are the
//! soft-delete *rewrite* (a `delete` becomes a tombstone UPDATE, never a real
//! DELETE) and the injected guards (live predicate + `@scope`) on every write.

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_sema::check;

fn gen(src: &str) -> String {
    gen_for(src, Dialect::MariaDb)
}

fn gen_pg(src: &str) -> String {
    gen_for(src, Dialect::Postgres)
}

fn gen_for(src: &str, dialect: Dialect) -> String {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    // These snippets exercise write lowering, not index completeness — a write whose
    // `where` scans an unindexed column (`E0260`) still lowers correctly, and the index
    // requirement is covered authoritatively in based-sema's tests + conformance.
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260")
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    sql::mutations::mutations(&schema, &sf.decls, dialect)
}

#[test]
fn create_binds_id_relation_fk_and_engine_timestamps() {
    let out = gen(r#"
        Org { id: Id, name: text }
        @created(created_at)
        @updated(updated_at)
        User {
          id: Id
          created_at: timestamp
          updated_at: timestamp
          org: Org
          email: text
        }
        shape UserCard from User { email }
        mutation make_user(org: Id, email: text) -> UserCard {
          create User { org = $org, email = $email };
        }
        "#);
    // app-generated `id`  leads; relation param maps to its FK; created/updated
    // are engine-set on insert . Column and value lists line up positionally.
    assert!(
        out.contains(
            "INSERT INTO `user` (`id`, `org_id`, `email`, `created_at`, `updated_at`)\nVALUES (:id, :org, :email, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);"
        ),
        "\n{out}"
    );
}

#[test]
fn update_injects_soft_delete_scope_and_bumps_updated() {
    let out = gen(r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @soft_delete(deleted_at)
        @scope Tenant
        @updated(updated_at)
        Order {
          id: Id
          deleted_at: timestamp?
          updated_at: timestamp
          org: Org
          status: text
        }
        shape OrderCard from Order { status }
        mutation set_status(id: Id, status: text) -> OrderCard scoped Tenant {
          update Order where (id = $id) { status = $status };
        }
        "#);
    assert!(out.contains("UPDATE `order`"), "\n{out}");
    // user SET + engine @updated bump.
    assert!(
        out.contains("SET `order`.`status` = :status, `order`.`updated_at` = CURRENT_TIMESTAMP"),
        "\n{out}"
    );
    // user predicate, then injected live guard, then injected @scope.
    assert!(
        out.contains(
            "WHERE `order`.`id` = :id AND `order`.`deleted_at` IS NULL AND `order`.`org_id` = :ctx_org;"
        ),
        "\n{out}"
    );
}

#[test]
fn atomic_update_lowers_to_real_sql_expression() {
    // `qty = qty + $delta` becomes a computed SET — a real SQL expression over the
    // row's own column, not a read-modify-write. MariaDB qualifies the SET target.
    let src = r#"
        @updated(updated_at)
        Product { id: Id, updated_at: timestamp, qty: int, name: text }
        shape P from Product { qty }
        mutation adjust(id: Id, delta: int) -> P {
          update Product where (id = $id) { qty = qty + $delta };
        }
        "#;
    let maria = gen(src);
    assert!(
        maria.contains("SET `product`.`qty` = (`product`.`qty` + :delta)"),
        "\n{maria}"
    );
    // Postgres/SQLite take a bare SET target but qualify the RHS column read.
    let pg = gen_pg(src);
    assert!(
        pg.contains(r#"SET "qty" = ("product"."qty" + :delta)"#),
        "\n{pg}"
    );
    let lite = gen_for(src, Dialect::Sqlite);
    assert!(
        lite.contains("SET `qty` = (`product`.`qty` + :delta)"),
        "\n{lite}"
    );
}

#[test]
fn atomic_update_respects_precedence_with_parens() {
    // `(qty + $base) * $n` — the AST tree already encodes precedence; codegen wraps
    // each binary node so the SQL evaluates in the same order.
    let out = gen(r#"
        @updated(updated_at)
        Product { id: Id, updated_at: timestamp, qty: int }
        shape P from Product { qty }
        mutation recompute(id: Id, base: int, n: int) -> P {
          update Product where (id = $id) { qty = (qty + $base) * $n };
        }
        "#);
    assert!(
        out.contains("SET `product`.`qty` = ((`product`.`qty` + :base) * :n)"),
        "\n{out}"
    );
}

#[test]
fn update_where_inlines_named_filter() {
    // A named filter used in a mutation `where` is inlined the same way as on the
    // read side — the write chain threads `decls` so the filter body is available.
    let out = gen(r#"
        @updated(updated_at)
        Product { id: Id, updated_at: timestamp, active: bool, stock: int, name: text }
        shape P from Product { name }
        filter sellable = active and stock > 0;
        mutation retire(name: text) -> P {
          update Product where (sellable) { active = false };
        }
        "#);
    assert!(
        out.contains("`product`.`active` = TRUE AND `product`.`stock` > 0"),
        "\n{out}"
    );
}

#[test]
fn delete_on_soft_model_rewrites_to_tombstone_update() {
    let out = gen(r#"
        @soft_delete(deleted_at)
        @updated(updated_at)
        Order { id: Id, deleted_at: timestamp?, updated_at: timestamp, status: text }
        shape OrderCard from Order { status }
        mutation remove(id: Id) -> OrderCard {
          delete Order where (id = $id);
        }
        "#);
    assert!(
        out.contains("-- delete (soft): tombstone, never a real DELETE"),
        "\n{out}"
    );
    // the tombstone write + updated bump; never a real DELETE.
    assert!(
        out.contains("UPDATE `order`\nSET `order`.`deleted_at` = CURRENT_TIMESTAMP, `order`.`updated_at` = CURRENT_TIMESTAMP"),
        "\n{out}"
    );
    // only live rows are tombstoned (idempotent re-delete is a no-op).
    assert!(
        out.contains("WHERE `order`.`id` = :id AND `order`.`deleted_at` IS NULL;"),
        "\n{out}"
    );
    assert!(
        !out.contains("DELETE FROM") && !out.contains("DELETE `"),
        "must not emit a real DELETE:\n{out}"
    );
}

#[test]
fn hard_delete_emits_real_delete_and_keeps_scope() {
    let out = gen(r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @soft_delete(deleted_at)
        @scope Tenant
        Order { id: Id, deleted_at: timestamp?, org: Org, status: text }
        mutation purge(id: Id) -> ok scoped Tenant {
          hard delete Order where (id = $id);
        }
        "#);
    assert!(
        out.contains("-- hard delete: real DELETE (explicit soft-delete opt-out)"),
        "\n{out}"
    );
    // real DELETE, soft-delete NOT injected, but @scope still guards it.
    assert!(
        out.contains(
            "DELETE FROM `order`\nWHERE `order`.`id` = :id AND `order`.`org_id` = :ctx_org;"
        ),
        "\n{out}"
    );
    assert!(
        !out.contains("deleted_at"),
        "hard delete must not inject the tombstone predicate:\n{out}"
    );
}

#[test]
fn restore_clears_tombstone_without_live_predicate() {
    let out = gen(r#"
        @soft_delete(archived)
        @updated(updated_at)
        Doc { id: Id, archived: bool, updated_at: timestamp, title: text }
        shape DocCard from Doc { title }
        mutation unarchive(id: Id) -> DocCard {
          restore Doc where (id = $id);
        }
        "#);
    assert!(out.contains("-- restore: clear the tombstone"), "\n{out}");
    // bool soft-delete: live = FALSE, so restore clears to FALSE.
    assert!(
        out.contains("SET `doc`.`archived` = FALSE, `doc`.`updated_at` = CURRENT_TIMESTAMP"),
        "\n{out}"
    );
    // restore targets deleted rows -> no live predicate injected.
    assert!(out.contains("WHERE `doc`.`id` = :id;"), "\n{out}");
    assert!(
        !out.contains("`archived` = TRUE"),
        "restore must not inject the live predicate:\n{out}"
    );
}

#[test]
fn delete_on_plain_model_is_a_real_delete() {
    let out = gen(r#"
        Tag { id: Id, label: text }
        mutation drop_tag(id: Id) -> ok {
          delete Tag where (id = $id);
        }
        "#);
    assert!(
        out.contains("DELETE FROM `tag`\nWHERE `tag`.`id` = :id;"),
        "\n{out}"
    );
}

#[test]
fn tx_renders_each_write_in_order() {
    let out = gen(r#"
        User { id: Id, email: text }
        Address { id: Id, user: User?, city: text }
        shape UserCard from User { email }
        mutation signup(email: text, city: text) -> UserCard {
          tx {
            create User { email = $email };
            create Address { city = $city };
          }
        }
        "#);
    assert!(
        out.contains("-- tx: one engine-owned transaction"),
        "\n{out}"
    );
    assert!(
        out.contains("INSERT INTO `user` (`id`, `email`)"),
        "\n{out}"
    );
    assert!(
        out.contains("INSERT INTO `address` (`id`, `city`)"),
        "\n{out}"
    );
    // ordering preserved: user insert precedes address insert.
    let u = out.find("INSERT INTO `user`").unwrap();
    let a = out.find("INSERT INTO `address`").unwrap();
    assert!(u < a, "tx statement order not preserved:\n{out}");
    // sibling creates in a tx get distinct id binds so they don't collide.
    assert!(out.contains("VALUES (:id_0, :email)"), "\n{out}");
    assert!(out.contains("VALUES (:id_1, :city)"), "\n{out}");
}

#[test]
fn tx_backref_binds_prior_create_id() {
    let out = gen(r#"
        User { id: Id, email: text }
        Address { id: Id, user: User, city: text }
        shape UserCard from User { email }
        mutation signup(email: text, city: text) -> UserCard {
          tx {
            create User { email = $email };
            create Address { user = ^.id, city = $city };
          }
        }
        "#);
    // `^.id` binds the preceding create's generated id (`:id_0`); Address's own id is `:id_1`.
    assert!(
        out.contains("INSERT INTO `address` (`id`, `user_id`, `city`)"),
        "\n{out}"
    );
    assert!(out.contains("VALUES (:id_1, :id_0, :city)"), "\n{out}");
}

#[test]
fn create_returning_mutation_reselects_the_declared_shape() {
    let out = gen(r#"
        Org { id: Id, name: text }
        User { id: Id, name: text }
        @soft_delete(deleted_at)
        Order {
          id: Id
          deleted_at: timestamp?,
          org: Org,
          placed_by: User,
          total: int,
        }
        shape OrderCard from Order { total, buyer = placed_by.name }
        mutation place_order(org: Id, buyer: Id, total: int) -> OrderCard {
          create Order { org = $org, placed_by = $buyer, total = $total };
        }
        "#);
    // After the INSERT the created row is read back in its declared shape .
    assert!(
        out.contains("-- return: re-select the written row's declared shape"),
        "\n{out}"
    );
    // Projects the shape exactly as a `get` would: local `total` + the relation reach
    // `buyer = placed_by.name`, which joins the target (soft-delete guarded in the ON).
    assert!(out.contains("`order`.`total` AS `total`"), "\n{out}");
    assert!(out.contains("`j_placed_by`.`name` AS `buyer`"), "\n{out}");
    assert!(out.contains("JOIN `user` AS `j_placed_by`"), "\n{out}");
    // Keyed on the created row's id (bound to `:result_id` by the runtime), and the
    // root soft-delete live predicate rides along — a re-select is just a `get`.
    assert!(
        out.contains("WHERE `order`.`id` = :result_id AND `order`.`deleted_at` IS NULL"),
        "\n{out}"
    );
}

#[test]
fn update_mutation_reselects_by_the_write_where() {
    let out = gen(r#"
        @updated(updated_at)
        Order { id: Id, updated_at: timestamp, status: text }
        shape OrderCard from Order { status }
        mutation set_status(id: Id, status: text) -> OrderCard {
          update Order where (id = $id) { status = $status };
        }
        "#);
    // An update's row survives, so the declared shape is re-selected keyed off the write's
    // own `where`  — no engine `id`, so no `:result_id`.
    assert!(out.contains("-- return:"), "\n{out}");
    assert!(
        !out.contains(":result_id"),
        "where-keyed re-select must not use :result_id:\n{out}"
    );
    assert!(out.contains("`order`.`status` AS `status`"), "\n{out}");
    // keyed on the update's own predicate (`id = :id`), reusing its bound param.
    assert!(out.contains("WHERE `order`.`id` = :id;"), "\n{out}");
}

#[test]
fn soft_delete_mutation_reselects_without_the_live_predicate() {
    // A soft `delete` tombstones the row (it survives); the declared shape is re-selected
    // keyed off the write `where`, but *without* the live predicate — so the just-tombstoned
    // row is still read back .
    let out = gen(r#"
        @soft_delete(deleted_at)
        @updated(updated_at)
        Order { id: Id, deleted_at: timestamp?, updated_at: timestamp, status: text }
        shape OrderCard from Order { status }
        mutation remove(id: Id) -> OrderCard {
          delete Order where (id = $id);
        }
        "#);
    assert!(out.contains("-- return:"), "\n{out}");
    // no live predicate on the re-select (the row is tombstoned now).
    assert!(
        out.contains(
            "SELECT\n  `order`.`status` AS `status`\nFROM `order`\nWHERE `order`.`id` = :id;"
        ),
        "\n{out}"
    );
}

#[test]
fn hard_delete_mutation_emits_no_reselect() {
    // A real DELETE removes the row — no surviving row to read back, so an `-> ok`
    // mutation emits no re-select (the response is `{}` at runtime).
    let out = gen(r#"
        Tag { id: Id, label: text }
        mutation drop_tag(id: Id) -> ok {
          delete Tag where (id = $id);
        }
        "#);
    assert!(
        !out.contains("-- return:"),
        "a real delete must not re-select:\n{out}"
    );
}

#[test]
fn update_where_across_relation_uses_multi_table_form() {
    let out = gen(r#"
        Org { id: Id, name: text }
        @updated(updated_at)
        Order { id: Id, updated_at: timestamp, org: Org, status: text }
        shape OrderCard from Order { status }
        mutation flag_org_orders(name: text, status: text) -> OrderCard {
          update Order where (org.name = $name) { status = $status };
        }
        "#);
    // relation-reaching predicate -> MariaDB multi-table UPDATE with a JOIN.
    assert!(
        out.contains("UPDATE `order`\nJOIN `org` AS `j_org` ON `j_org`.`id` = `order`.`org_id`"),
        "\n{out}"
    );
    assert!(out.contains("`j_org`.`name` = :name"), "\n{out}");
}

#[test]
fn create_auto_sets_the_scope_column_from_ctx() {
    // On a scoped model the scope column is engine-managed on create : auto-set
    // from `:ctx_<field>`, never a caller param. Cross-scope create is inexpressible.
    let out = gen(r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Order { id: Id, org: Org, total: int }
        shape OrderCard from Order { total }
        mutation place(total: int) -> OrderCard scoped Tenant { create Order { total = $total }; }
        "#);
    // `org_id` is injected into the INSERT bound to `:ctx_org`, alongside the engine id.
    assert!(
        out.contains("INSERT INTO `order` (`total`, `org_id`, `id`)")
            || out.contains("INSERT INTO `order` (`id`, `total`, `org_id`)"),
        "\n{out}"
    );
    assert!(
        out.contains(":ctx_org"),
        "scope column not auto-set:\n{out}"
    );
    // the re-select still applies scope (a create that lands out of scope reads absent).
    assert!(
        out.contains("WHERE `order`.`id` = :result_id")
            && out.contains("`order`.`org_id` = :ctx_org"),
        "\n{out}"
    );
}

#[test]
fn unscoped_mutation_omits_scope_injection_and_auto_set() {
    // `unscoped(...)`  drops the write guard *and* the create auto-set: the caller
    // supplies the scope column and the write carries no injected scope predicate.
    let out = gen(r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @soft_delete(deleted_at)
        @scope Tenant
        Order { id: Id, deleted_at: timestamp?, org: Org, total: int }
        shape OrderCard from Order { total }
        mutation import_order(org: Id, total: int) -> OrderCard
          unscoped("data import: rows land in the supplied org") {
          create Order { org = $org, total = $total };
        }
        "#);
    // caller-supplied org (`:org`), no auto-set from ctx.
    assert!(out.contains(":org") && !out.contains(":ctx_org"), "\n{out}");
    // the re-select is keyed on the created row but carries no scope predicate.
    assert!(out.contains(":result_id"), "\n{out}");
    assert!(
        !out.contains("`order`.`org_id` = :ctx_org"),
        "unscoped must not inject scope:\n{out}"
    );
}

#[test]
fn unscoped_update_omits_the_scope_guard() {
    let out = gen(r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        @updated(updated_at)
        Order { id: Id, updated_at: timestamp, org: Org, status: text }
        shape OrderCard from Order { status }
        mutation admin_set_status(id: Id, status: text) -> OrderCard
          unscoped("admin: correct any org's order") {
          update Order where (id = $id) { status = $status };
        }
        "#);
    assert!(
        !out.contains(":ctx_org"),
        "unscoped update must not inject scope:\n{out}"
    );
    assert!(out.contains("WHERE `order`.`id` = :id;"), "\n{out}");
}

// ---------- Postgres  -------------------------------------------------

#[test]
fn pg_create_double_quotes_and_keeps_named_placeholders() {
    let out = gen_pg(
        r#"
        Org { id: Id, name: text }
        Order { id: Id, org: Org, status: text, total: int }
        shape OrderCard from Order { status, total }
        mutation place(org: Id, status: text, total: int) -> OrderCard {
          create Order { org = $org, status = $status, total = $total };
        }
        "#,
    );
    assert!(out.contains("INSERT INTO \"order\" ("), "\n{out}");
    // engine `id` bound first, then the assigns — identifiers double-quoted.
    assert!(
        out.contains("(\"id\", \"org_id\", \"status\", \"total\")"),
        "\n{out}"
    );
    assert!(
        out.contains("VALUES (:id, :org, :status, :total)"),
        "\n{out}"
    );
    // the create re-select comes back double-quoted, keyed on :result_id.
    assert!(
        out.contains("WHERE \"order\".\"id\" = :result_id"),
        "\n{out}"
    );
    // no backtick-quoted identifiers in the statement body (the header has backticks).
    let body = &out[out.find("INSERT").unwrap()..];
    assert!(!body.contains('`'), "\n{out}");
}

#[test]
fn pg_soft_delete_tombstone_uses_bare_set_column() {
    // Postgres forbids the target alias in a SET clause, so the tombstone SET column
    // is bare `"deleted_at" = …` (not `"order"."deleted_at" = …`).
    let out = gen_pg(
        r#"
        @soft_delete(deleted_at)
        @updated(updated_at)
        Order { id: Id, deleted_at: timestamp?, updated_at: timestamp, status: text }
        shape OrderCard from Order { status }
        mutation remove(id: Id) -> OrderCard { delete Order where (id = $id); }
        "#,
    );
    assert!(
        out.contains("UPDATE \"order\"\nSET \"deleted_at\" = CURRENT_TIMESTAMP, \"updated_at\" = CURRENT_TIMESTAMP"),
        "\n{out}"
    );
    // the WHERE still qualifies the target row + injects the live predicate.
    assert!(
        out.contains("WHERE \"order\".\"id\" = :id AND \"order\".\"deleted_at\" IS NULL"),
        "\n{out}"
    );
    assert!(
        !out.contains("DELETE FROM") && !out.contains("DELETE \""),
        "must not emit a real DELETE:\n{out}"
    );
}

#[test]
fn pg_update_across_relation_uses_from_clause() {
    // Postgres has no inline join in UPDATE: the joined table goes in `FROM` and the
    // join `ON` folds into the WHERE (ahead of the user predicate).
    let out = gen_pg(
        r#"
        Org { id: Id, name: text }
        @updated(updated_at)
        Order { id: Id, updated_at: timestamp, org: Org, status: text }
        shape OrderCard from Order { status }
        mutation flag_org_orders(name: text, status: text) -> OrderCard {
          update Order where (org.name = $name) { status = $status };
        }
        "#,
    );
    assert!(
        out.contains("UPDATE \"order\"\nSET \"status\" = :status"),
        "\n{out}"
    );
    assert!(out.contains("\nFROM \"org\" AS \"j_org\""), "\n{out}");
    // join ON folded into WHERE ahead of the user predicate; no inline JOIN keyword.
    assert!(
        out.contains(
            "WHERE \"j_org\".\"id\" = \"order\".\"org_id\" AND \"j_org\".\"name\" = :name"
        ),
        "\n{out}"
    );
    // The UPDATE *statement* uses FROM, not an inline JOIN (the trailing re-select is a
    // plain SELECT and may carry a JOIN — scope the assertion to the update).
    let update_stmt = &out[out.find("UPDATE").unwrap()..out.find("-- return:").unwrap()];
    assert!(
        !update_stmt.contains("\nJOIN "),
        "no inline join in a PG update:\n{update_stmt}"
    );
}

#[test]
fn pg_hard_delete_across_relation_uses_using_clause() {
    let out = gen_pg(
        r#"
        Org { id: Id, name: text }
        Order { id: Id, org: Org, status: text }
        mutation purge(name: text) -> ok {
          hard delete Order where (org.name = $name);
        }
        "#,
    );
    assert!(out.contains("DELETE FROM \"order\""), "\n{out}");
    assert!(out.contains("\nUSING \"org\" AS \"j_org\""), "\n{out}");
    assert!(
        out.contains(
            "WHERE \"j_org\".\"id\" = \"order\".\"org_id\" AND \"j_org\".\"name\" = :name"
        ),
        "\n{out}"
    );
}

// ---------- multi-scope DNF: create auto-set of the named alternative  ---

/// A `create` on an AND model (`@scope Page, Author`) auto-sets *both* scope columns
/// from `$ctx` — every axis of the alternative the mutation's `scoped …` named — so a
/// row can never be created half-owned (E0186 guards the missing-axis case).
#[test]
fn and_scope_create_auto_sets_every_named_axis() {
    let out = gen(r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page, Author
        Comment {
          id: Id
          page:   Page
          author: User
          body:   text
        }
        shape CommentCard from Comment { body }
        mutation add_comment(body: text) -> CommentCard scoped Page, Author {
          create Comment { body = $body };
        }
        "#);
    assert!(out.contains("INSERT INTO `comment`"), "\n{out}");
    // both engine-managed scope columns are set from $ctx, alongside the caller's body.
    assert!(out.contains("`page_id`"), "\n{out}");
    assert!(out.contains("`author_id`"), "\n{out}");
    assert!(out.contains(":ctx_page"), "\n{out}");
    assert!(out.contains(":ctx_user"), "\n{out}");
}

/// An OR model create names *one* alternative; only that axis's column is auto-set
/// (the other alternative's column is left to whatever the model allows). Proves the
/// create auto-set follows the callable's chosen alternative, not the whole model.
#[test]
fn or_scope_create_auto_sets_only_the_named_alternative() {
    let out = gen(r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page
        @scope Author
        Post {
          id: Id
          page:    Page?
          author:  User?
          body:    text
        }
        shape PostCard from Post { body }
        mutation post_to_page(body: text) -> PostCard scoped Page {
          create Post { body = $body };
        }
        "#);
    assert!(out.contains("`page_id`"), "\n{out}");
    assert!(out.contains(":ctx_page"), "\n{out}");
    // the un-named alternative's column is NOT auto-set by this create.
    assert!(!out.contains("`author_id`"), "\n{out}");
    assert!(!out.contains(":ctx_user"), "\n{out}");
}

#[test]
fn enum_create_assign_lowers_to_a_string_literal() {
    let src = r#"
        enum Status { pending, paid }
        Order { id: Id, status: Status, total: int }
        shape OrderRow from Order { status, total }
        mutation place() -> OrderRow { create Order { status = paid, total = 1 } }
    "#;
    let sql = gen(src);
    assert!(sql.contains("'paid'"), "\n{sql}");
}

// ---------- upsert (`create … on conflict update`) -------------------------

const UPSERT: &str = r#"
    Page {
      id: Id
      path: text (unique)
      hits: int
    }
    shape PageRow from Page { path, hits }
    mutation record_hit(path: text) -> PageRow {
      create Page { path = $path, hits = 1 } on conflict (path) update { hits = hits + 1 };
    }
"#;

#[test]
fn upsert_mariadb_on_duplicate_key_update() {
    let out = gen(UPSERT);
    // MariaDB carries no explicit conflict target; bare columns name the existing row.
    assert!(
        out.contains(
            "INSERT INTO `page` (`id`, `path`, `hits`)\nVALUES (:id, :path, 1)\nON DUPLICATE KEY UPDATE `hits` = (`hits` + 1);"
        ),
        "\n{out}"
    );
    // The declared-shape re-select keys on the conflict target, not the generated id.
    assert!(out.contains("WHERE `page`.`path` = :path"), "\n{out}");
    assert!(!out.contains(":result_id"), "\n{out}");
}

#[test]
fn upsert_postgres_on_conflict_do_update() {
    let out = gen_pg(UPSERT);
    assert!(
        out.contains(
            "INSERT INTO \"page\" (\"id\", \"path\", \"hits\")\nVALUES (:id, :path, 1)\nON CONFLICT (\"path\") DO UPDATE SET \"hits\" = (\"hits\" + 1);"
        ),
        "\n{out}"
    );
    assert!(out.contains("WHERE \"page\".\"path\" = :path"), "\n{out}");
}

#[test]
fn upsert_sqlite_on_conflict_do_update() {
    let out = gen_for(UPSERT, Dialect::Sqlite);
    assert!(
        out.contains(
            "INSERT INTO `page` (`id`, `path`, `hits`)\nVALUES (:id, :path, 1)\nON CONFLICT (`path`) DO UPDATE SET `hits` = (`hits` + 1);"
        ),
        "\n{out}"
    );
    assert!(out.contains("WHERE `page`.`path` = :path"), "\n{out}");
}

#[test]
fn upsert_composite_unique_index_target_and_scope() {
    // Per-tenant uniqueness: a composite `@index (org, slug) unique`, org scope-managed.
    let src = r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Doc {
          id: Id
          org: Org
          slug: text
          views: int
          @index (org, slug) unique
        }
        shape DocRow from Doc { slug, views }
        mutation touch_doc(slug: text) -> DocRow scoped Tenant {
          create Doc { slug = $slug, views = 1 } on conflict (org, slug) update { views = views + 1 };
        }
    "#;
    let out = gen_pg(src);
    // `org` is auto-set from $ctx and is part of the conflict target + re-select key.
    assert!(
        out.contains(
            "ON CONFLICT (\"org_id\", \"slug\") DO UPDATE SET \"views\" = (\"views\" + 1)"
        ),
        "\n{out}"
    );
    assert!(
        out.contains("\"doc\".\"org_id\" = :ctx_org") && out.contains("\"doc\".\"slug\" = :slug"),
        "\n{out}"
    );
}
