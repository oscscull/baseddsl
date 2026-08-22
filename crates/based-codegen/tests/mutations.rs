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

fn gen_mysql(src: &str) -> String {
    gen_for(src, Dialect::MySql)
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
fn hard_delete_all_wipes_the_table_truncate_on_pg_delete_elsewhere() {
    let src = r#"
        Widget { id: Id, name: text }
        mutation wipe() -> ok {
          hard delete all Widget;
        }
        "#;
    // Postgres: TRUNCATE (transaction-safe there).
    let pg = gen_pg(src);
    assert!(pg.contains("TRUNCATE \"widget\";"), "\n{pg}");
    assert!(!pg.contains("DELETE FROM"), "PG wipe uses TRUNCATE:\n{pg}");
    // MySQL/MariaDB: plain DELETE FROM (TRUNCATE auto-commits — would break the tx).
    let maria = gen(src);
    assert!(maria.contains("DELETE FROM `widget`;"), "\n{maria}");
    assert!(!maria.contains("TRUNCATE"), "\n{maria}");
    let mysql = gen_mysql(src);
    assert!(mysql.contains("DELETE FROM `widget`;"), "\n{mysql}");
    // No WHERE on any dialect (whole-table).
    assert!(!maria.contains("WHERE"), "wipe has no WHERE:\n{maria}");
}

#[test]
fn scoped_hard_delete_all_keeps_the_scope_predicate_not_truncate() {
    let src = r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Order { id: Id, org: Org, @index(org) }
        mutation wipe_mine() -> ok scoped Tenant {
          hard delete all Order;
        }
        "#;
    // A scoped `all` is all rows *in scope* — a filtered DELETE, never a TRUNCATE.
    let pg = gen_pg(src);
    assert!(
        !pg.contains("TRUNCATE"),
        "scoped wipe is not a TRUNCATE:\n{pg}"
    );
    assert!(
        pg.contains("DELETE FROM \"order\"") && pg.contains("\"org_id\" = :ctx_org"),
        "\n{pg}"
    );
    let maria = gen(src);
    assert!(
        maria.contains("DELETE FROM `order`\nWHERE `order`.`org_id` = :ctx_org;"),
        "\n{maria}"
    );
}

#[test]
fn soft_delete_all_tombstones_every_row_never_a_real_delete() {
    let src = r#"
        @soft_delete(deleted_at)
        Order { id: Id, deleted_at: timestamp? }
        mutation archive_all() -> ok {
          delete all Order;
        }
        "#;
    let out = gen(src);
    assert!(
        out.contains("-- delete all (soft): tombstone every row in scope"),
        "\n{out}"
    );
    // UPDATE … SET deleted_at = now(), filtered only by the live predicate (no user WHERE).
    assert!(out.contains("UPDATE `order`"), "\n{out}");
    assert!(
        out.contains("SET `order`.`deleted_at` = CURRENT_TIMESTAMP"),
        "\n{out}"
    );
    assert!(
        out.contains("WHERE `order`.`deleted_at` IS NULL;"),
        "soft wipe keeps only the live predicate:\n{out}"
    );
    assert!(
        !out.contains("DELETE FROM") && !out.contains("TRUNCATE"),
        "soft delete all must not physically delete:\n{out}"
    );
}

#[test]
fn hard_delete_all_on_soft_model_still_physically_wipes() {
    let src = r#"
        @soft_delete(deleted_at)
        Order { id: Id, deleted_at: timestamp? }
        mutation nuke() -> ok {
          hard delete all Order;
        }
        "#;
    let pg = gen_pg(src);
    assert!(pg.contains("TRUNCATE \"order\";"), "\n{pg}");
    let maria = gen(src);
    assert!(maria.contains("DELETE FROM `order`;"), "\n{maria}");
    assert!(
        !maria.contains("deleted_at"),
        "hard wipe ignores the tombstone:\n{maria}"
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
fn tx_step_ref_binds_bound_create_id() {
    let out = gen(r#"
        User { id: Id, email: text }
        Address { id: Id, user: User, city: text }
        shape UserCard from User { email }
        mutation signup(email: text, city: text) -> UserCard {
          tx {
            create User { email = $email } as user;
            create Address { user = $user.id, city = $city };
          }
        }
        "#);
    // The bound User re-selects its written row (`RETURNING`), and `$user.id` reads the
    // committed id from the capture bind `:bref_user__id`; Address's own id is `:id_1`.
    assert!(
        out.contains("INSERT INTO `user` (`id`, `email`)\nVALUES (:id_0, :email) RETURNING `id`"),
        "\n{out}"
    );
    assert!(
        out.contains("INSERT INTO `address` (`id`, `user_id`, `city`)"),
        "\n{out}"
    );
    assert!(
        out.contains("VALUES (:id_1, :bref_user__id, :city)"),
        "\n{out}"
    );
}

#[test]
fn unbound_create_emits_no_row_read_back() {
    // An UNBOUND create is unchanged by D124: no `RETURNING`, no follow-up SELECT — just
    // the plain INSERT (its only re-select is the mutation's own declared-shape return).
    let out = gen(r#"
        User { id: Id, email: text }
        Address { id: Id, user: User, city: text }
        shape UserCard from User { email }
        mutation signup(email: text, city: text) -> UserCard {
          tx {
            create User { email = $email } as user;
            create Address { user = $user.id, city = $city };
          }
        }
        "#);
    // The Address create binds no step, so its INSERT carries no `RETURNING`.
    let addr = out
        .split("INSERT INTO `address`")
        .nth(1)
        .expect("address insert");
    let addr_stmt = addr.split(";\n").next().unwrap_or(addr);
    assert!(
        !addr_stmt.contains("RETURNING"),
        "unbound create must emit no read-back:\n{addr_stmt}"
    );
}

#[test]
fn mysql_bound_create_reads_back_via_follow_up_select() {
    // MySQL has no `INSERT … RETURNING`, so a bound create's row read-back is a follow-up
    // keyed `SELECT` (here `id = :id_0`, the app-minted surrogate), whose `id` column feeds
    // the sibling's `$user.id` (`:bref_user__id`).
    let out = gen_mysql(
        r#"
        User { id: Id, email: text }
        Address { id: Id, user: User, city: text }
        shape UserCard from User { email }
        mutation signup(email: text, city: text) -> UserCard {
          tx {
            create User { email = $email } as user;
            create Address { user = $user.id, city = $city };
          }
        }
        "#,
    );
    // The bound User INSERT carries no RETURNING (MySQL); a follow-up SELECT reads `id`.
    assert!(
        !out.contains("RETURNING"),
        "MySQL has no INSERT … RETURNING:\n{out}"
    );
    assert!(
        out.contains("SELECT `id` FROM `user` WHERE `id` = :id_0"),
        "MySQL bound create reads back via a follow-up keyed SELECT:\n{out}"
    );
    assert!(
        out.contains("VALUES (:id_1, :bref_user__id, :city)"),
        "\n{out}"
    );
}

#[test]
fn mysql_serial_read_back_uses_last_insert_id_follow_up_select() {
    // A `serial` return create on MySQL reads its DB-generated id back via a follow-up
    // `SELECT … WHERE id = LAST_INSERT_ID()` (no RETURNING), keying the declared re-select.
    let out = gen_mysql(
        r#"
        Org { id: serial, name: text }
        shape OrgCard from Org { id, name }
        mutation make(name: text) -> OrgCard { create Org { name = $name }; }
        "#,
    );
    assert!(!out.contains("RETURNING"), "MySQL has no RETURNING:\n{out}");
    assert!(
        out.contains("SELECT `id` FROM `org` WHERE `id` = LAST_INSERT_ID()"),
        "MySQL serial read-back keys on LAST_INSERT_ID():\n{out}"
    );
}

#[test]
fn tx_step_ref_reaches_any_prior_step() {
    // A 3-step tx where step 3 references step 1 — the case `^` (immediately-preceding
    // only) could not express. `$org.id` at step 3 must bind step 1's `:id_0`.
    let out = gen(r#"
        Org { id: Id, name: text }
        User { id: Id, org: Org, email: text }
        Log { id: Id, org: Org, actor: User }
        shape OrgCard from Org { name }
        mutation onboard(name: text, email: text) -> OrgCard {
          tx {
            create Org { name = $name } as org;
            create User { org = $org.id, email = $email } as user;
            create Log { org = $org.id, actor = $user.id };
          }
        }
        "#);
    // Each bound create re-selects its written row; a sibling reads a prior step's
    // committed value from that step's capture bind (`:bref_<name>__<column>`).
    // Step 2 (User, `:id_1`) references step 1's org (`:bref_org__id`).
    assert!(
        out.contains("VALUES (:id_1, :bref_org__id, :email)"),
        "\n{out}"
    );
    // Step 3 (Log, `:id_2`) references step 1's org AND step 2's user.
    assert!(
        out.contains("VALUES (:id_2, :bref_org__id, :bref_user__id)"),
        "\n{out}"
    );
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

#[test]
fn keyless_create_reselects_by_the_unique_column_not_a_generated_id() {
    // A `@no_id` model has no generated id — the INSERT sets no `id`, and the
    // declared-shape re-select keys on the `(unique)` column the create set.
    let out = gen(r#"
        @no_id("legacy audit log, no surrogate key")
        Event { source: text (unique), action: text }
        shape EventRow from Event { source, action }
        mutation record(s: text, a: text) -> EventRow { create Event { source = $s, action = $a }; }
        "#);
    // INSERT names only the assigned columns — no `id`.
    assert!(
        out.contains("INSERT INTO `event` (`source`, `action`)"),
        "\n{out}"
    );
    assert!(!out.contains("`id`"), "\n{out}");
    // The re-select keys on the unique `source` = the create's bound value.
    assert!(out.contains("`event`.`source` = :s"), "\n{out}");
    assert!(!out.contains(":result_id"), "\n{out}");
}

const COMPOSITE_SERIAL: &str = r#"
    Device { id: Id  name: text }
    @key(device, seq)
    Reading { device: Device  seq: serial  value: int }
    shape ReadingRow from Reading { seq, value }
    mutation record(device: Device, value: int) -> ReadingRow {
      create Reading { device = $device, value = $value };
    }
    "#;

#[test]
fn composite_serial_create_omits_the_serial_part_and_reads_back_the_full_tuple_mariadb() {
    // A composite `@key(device, seq)` with a DB-generated `seq`: the INSERT omits `seq`
    // (auto-increment), reads it back via `RETURNING` (captured as `:result_id`), and the
    // declared-shape re-select keys on that serial value AND the app-supplied `device` part.
    let out = gen(COMPOSITE_SERIAL);
    // The INSERT column list is exactly (device_id, value) — `seq` is DB-generated, omitted.
    assert!(
        out.contains("INSERT INTO `reading` (`device_id`, `value`)"),
        "\n{out}"
    );
    // MariaDB (11.4) reads the auto-increment `seq` back via `INSERT … RETURNING` (D124),
    // captured under `:result_id`; the re-select keys on it AND the app-supplied `device`.
    assert!(
        out.contains("VALUES (:device, :value) RETURNING `seq`"),
        "\n{out}"
    );
    assert!(
        out.contains("`reading`.`seq` = :result_id AND `reading`.`device_id` = :device"),
        "\n{out}"
    );
}

#[test]
fn composite_serial_create_uses_returning_on_postgres() {
    // Postgres recovers the DB-generated `seq` via `RETURNING "seq"`, then re-selects the
    // full composite tuple.
    let out = gen_pg(COMPOSITE_SERIAL);
    assert!(
        out.contains(r#"VALUES (:device, :value) RETURNING "seq""#),
        "\n{out}"
    );
    assert!(
        out.contains(r#""reading"."seq" = :result_id AND "reading"."device_id" = :device"#),
        "\n{out}"
    );
}

#[test]
fn tx_binding_reuses_bound_creates_scope_column() {
    // A `$name.<scope-column>` tx reference resolves to the `:ctx_<field>` the bound create
    // engine-set the column from (never a silent NULL): the whole mutation runs under one
    // `$ctx`, so the value is knowable. Regression for the H6 write-path sweep.
    let out = gen(r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        Project { id: Id, org: Org, name: text }
        Task { id: Id, project: Project, owner_org: Org?, title: text }
        shape ProjectCard from Project { name }
        mutation new_project(pn: text, tt: text) -> ProjectCard scoped Tenant {
          tx {
            create Project { name = $pn } as p;
            create Task { project = $p.id, owner_org = $p.org, title = $tt };
          }
        }
        "#);
    // The bound Project re-selects its written row (`RETURNING` its `id` + engine-set
    // `org_id`); `$p.id` and `$p.org` read those committed values from the capture binds.
    assert!(
        out.contains("RETURNING `id`, `org_id`"),
        "Project must re-select id + org_id:\n{out}"
    );
    assert!(
        out.contains("VALUES (:id_1, :bref_p__id, :bref_p__org_id, :tt)"),
        "$p.org must bind the captured org_id, not NULL:\n{out}"
    );
    assert!(
        !out.contains("not set by bound create"),
        "a knowable scope column must not degrade to the NULL marker:\n{out}"
    );
}

// ---------- BW1: bulk / structured shape-input create ----------------------

const BULK_SCHEMA: &str = r#"
    Org { id: Id, name: text }
    scope Tenant (org: Org = $ctx.org)
    Category { id: Id, name: text }
    @created(created_at)
    @updated(updated_at)
    @scope Tenant
    Product {
      id: Id
      org: Org
      category: Category
      sku: text
      name: text
      price: int
      created_at: timestamp
      updated_at: timestamp
      @index(org)
      @index(category)
    }
    shape ProductIn from Product { sku, name, price, category { id } }
    mutation bulk_add(rows: ProductIn[]) -> ok scoped Tenant { create Product[] from $rows; }
    mutation add_one(row: ProductIn) -> ok scoped Tenant { create Product from $row; }
"#;

fn lower(src: &str, dialect: Dialect) -> Vec<based_codegen::sql::mutations::LoweredMutation> {
    let sf = parse_file(src, FileId(0)).unwrap();
    let (schema, _) = check(&sf.decls);
    based_codegen::sql::mutations::lower_mutations(&schema, &sf.decls, dialect)
}

#[test]
fn bulk_create_builds_a_presence_driven_column_plan() {
    use based_codegen::sql::mutations::BulkSource;
    let lm = lower(BULK_SCHEMA, Dialect::Sqlite);
    let bulk = lm
        .iter()
        .find(|m| m.name == "bulk_add")
        .and_then(|m| m.stmts[0].bulk.as_ref())
        .expect("bulk_add lowers to a BulkInsert");
    assert!(bulk.bulk, "`Product[] from` is the bulk form");
    // Column → source, presence-driven: shape scalars verbatim, the FK-link `{ id }`, the
    // `@scope` org from `$ctx`, a minted uuid id, and engine `@created`/`@updated` stamps.
    let by_col = |name: &str| {
        bulk.columns
            .iter()
            .find(|c| c.column == name)
            .map(|c| &c.source)
    };
    assert!(matches!(by_col("sku"), Some(BulkSource::Field { json_key, .. }) if json_key == "sku"));
    assert!(matches!(by_col("price"), Some(BulkSource::Field { .. })));
    assert!(
        matches!(by_col("category_id"), Some(BulkSource::FkPart { relation, key_field }) if relation == "category" && key_field == "id")
    );
    assert!(matches!(by_col("org_id"), Some(BulkSource::Ctx { ctx_field }) if ctx_field == "org"));
    assert!(matches!(by_col("id"), Some(BulkSource::MintUuid)));
    assert!(matches!(by_col("created_at"), Some(BulkSource::Now)));
    assert!(matches!(by_col("updated_at"), Some(BulkSource::Now)));
    // A scope column is NEVER taken from the payload, even if the shape were to name it.
    assert!(!bulk
        .columns
        .iter()
        .any(|c| c.column == "org_id" && matches!(c.source, BulkSource::Field { .. })));
}

#[test]
fn single_structured_create_is_a_one_row_bulk_plan() {
    let lm = lower(BULK_SCHEMA, Dialect::Sqlite);
    let bulk = lm
        .iter()
        .find(|m| m.name == "add_one")
        .and_then(|m| m.stmts[0].bulk.as_ref())
        .expect("add_one lowers to a BulkInsert");
    assert!(!bulk.bulk, "`Product from` is the single form");
}

#[test]
fn serial_bulk_create_omits_the_db_generated_id() {
    use based_codegen::sql::mutations::BulkSource;
    let src = r#"
        Widget { id: serial, label: text }
        shape WidgetIn from Widget { label }
        mutation add_widgets(rows: WidgetIn[]) -> ok { create Widget[] from $rows; }
    "#;
    let lm = lower(src, Dialect::Postgres);
    let bulk = lm[0].stmts[0].bulk.as_ref().unwrap();
    assert!(
        !bulk.columns.iter().any(|c| c.column == "id"),
        "a serial id is DB-generated — omitted from the INSERT"
    );
    assert!(!bulk
        .columns
        .iter()
        .any(|c| matches!(c.source, BulkSource::MintUuid | BulkSource::MintUlid)));
}

// ---------- bulk upsert (BW2) + bulk read-back (BW1b, D127) -----------------

const BULK_UPSERT_SCHEMA: &str = r#"
    Org { id: Id, name: text }
    scope Tenant (org: Org = $ctx.org)
    @scope Tenant
    Inventory {
      id: Id
      org: Org
      sku: text
      qty: int
      price: int
      @index(org, sku) unique
      @index(org)
    }
    shape InvIn from Inventory { sku, qty, price }
    mutation restock(rows: InvIn[]) -> InvIn[] scoped Tenant {
      create Inventory[] from $rows
        on conflict (org, sku) update { qty = qty + incoming.qty, price = incoming.price };
    }
"#;

#[test]
fn bulk_upsert_postgres_uses_excluded_for_incoming() {
    let lm = lower(BULK_UPSERT_SCHEMA, Dialect::Postgres);
    let bulk = lm[0].stmts[0].bulk.as_ref().expect("bulk insert");
    let tail = bulk.conflict_tail.as_deref().expect("conflict tail");
    // Bare stored column + `excluded.<col>` for the incoming proposed row.
    assert_eq!(
        tail,
        "\nON CONFLICT (\"org_id\", \"sku\") DO UPDATE SET \"qty\" = (\"qty\" + excluded.\"qty\"), \"price\" = excluded.\"price\""
    );
    // The read-back keys on the conflict target (not a generated id), app-known.
    assert_eq!(
        bulk.readback_key,
        vec!["org_id".to_string(), "sku".to_string()]
    );
    assert!(!bulk.readback_serial);
}

#[test]
fn bulk_upsert_mysql_uses_values_for_incoming() {
    let lm = lower(BULK_UPSERT_SCHEMA, Dialect::MySql);
    let bulk = lm[0].stmts[0].bulk.as_ref().expect("bulk insert");
    let tail = bulk.conflict_tail.as_deref().expect("conflict tail");
    // MariaDB/MySQL: no explicit target list; `incoming.<col>` → `VALUES(<col>)`.
    assert_eq!(
        tail,
        "\nON DUPLICATE KEY UPDATE `qty` = (`qty` + VALUES(`qty`)), `price` = VALUES(`price`)"
    );
}

#[test]
fn bulk_read_back_is_an_in_keyed_reselect_of_the_shape() {
    let lm = lower(BULK_UPSERT_SCHEMA, Dialect::Sqlite);
    let br = lm[0]
        .bulk_readback
        .as_ref()
        .expect("a shape-returning from-create reads back");
    assert!(br.bulk, "`-> InvIn[]` is an array read-back");
    assert!(!br.serial);
    assert_eq!(br.key_cols, vec!["org_id".to_string(), "sku".to_string()]);
    // The re-select projects the shape, carries hidden key columns, and leaves a sentinel
    // for the key-tuple IN-list the runtime splices per written row.
    assert!(br.sql.contains("/*BULK_KEYS*/"), "sql: {}", br.sql);
    assert!(
        br.sql.contains("AS `__bkk_0`") && br.sql.contains("AS `__bkk_1`"),
        "sql: {}",
        br.sql
    );
    assert!(br.sql.contains("IN (/*BULK_KEYS*/)"), "sql: {}", br.sql);
}

#[test]
fn serial_bulk_read_back_learns_keys_from_the_insert() {
    let src = r#"
        Ticket { id: serial, subject: text }
        shape TicketIn from Ticket { subject }
        shape TicketOut from Ticket { id, subject }
        mutation file(rows: TicketIn[]) -> TicketOut[] { create Ticket[] from $rows; }
    "#;
    let lm = lower(src, Dialect::Postgres);
    let bulk = lm[0].stmts[0].bulk.as_ref().expect("bulk insert");
    assert_eq!(bulk.readback_key, vec!["id".to_string()]);
    assert!(
        bulk.readback_serial,
        "a serial id read-back is DB-generated"
    );
    let br = lm[0].bulk_readback.as_ref().expect("read-back");
    assert!(br.serial);
}
