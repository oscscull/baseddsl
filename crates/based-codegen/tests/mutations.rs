//! Mutation (write) -> INSERT/UPDATE/DELETE codegen tests. Parse + check a whole
//! schema, then assert on the generated statements. The headline assertions are the
//! soft-delete *rewrite* (a `delete` becomes a tombstone UPDATE, never a real
//! DELETE) and the injected guards (live predicate + `@scope`) on every write.

use based_ast::FileId;
use based_codegen::{sql, Dialect};
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
    sql::mutations::mutations(&schema, &sf.decls, Dialect::MariaDb)
}

#[test]
fn create_binds_id_relation_fk_and_engine_timestamps() {
    let out = gen(r#"
        Org { name: text }
        @created(created_at)
        @updated(updated_at)
        User {
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
    // app-generated `id` (D1) leads; relation param maps to its FK; created/updated
    // are engine-set on insert (D2). Column and value lists line up positionally.
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
        Org { name: text }
        @soft_delete(deleted_at)
        @scope(org = $ctx.org)
        @updated(updated_at)
        Order {
          deleted_at: timestamp?
          updated_at: timestamp
          org: Org
          status: text
        }
        shape OrderCard from Order { status }
        mutation set_status(id: Id, status: text) -> OrderCard {
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
fn update_where_inlines_named_filter() {
    // A named filter used in a mutation `where` is inlined the same way as on the
    // read side — the write chain threads `decls` so the filter body is available.
    let out = gen(r#"
        @updated(updated_at)
        Product { updated_at: timestamp, active: bool, stock: int, name: text }
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
        Order { deleted_at: timestamp?, updated_at: timestamp, status: text }
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
        Org { name: text }
        @soft_delete(deleted_at)
        @scope(org = $ctx.org)
        Order { deleted_at: timestamp?, org: Org, status: text }
        shape OrderCard from Order { status }
        mutation purge(id: Id) -> OrderCard {
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
        Doc { archived: bool, updated_at: timestamp, title: text }
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
        Tag { label: text }
        shape TagCard from Tag { label }
        mutation drop_tag(id: Id) -> TagCard {
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
        User { email: text }
        Address { user: User, city: text }
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
        User { email: text }
        Address { user: User, city: text }
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
fn update_where_across_relation_uses_multi_table_form() {
    let out = gen(r#"
        Org { name: text }
        @updated(updated_at)
        Order { updated_at: timestamp, org: Org, status: text }
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
