//! DDL codegen tests: parse + check a whole-schema snippet, then assert on the
//! generated `CREATE TABLE` text. Snippets are multi-decl so relation FKs, indexes,
//! and cross-model FK types are exercised the way the CLI drives them.

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_sema::check;

fn gen_for(src: &str, dialect: Dialect) -> String {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    sql::ddl(&schema, dialect)
}

fn gen(src: &str) -> String {
    gen_for(src, Dialect::MariaDb)
}

fn gen_sqlite(src: &str) -> String {
    gen_for(src, Dialect::Sqlite)
}

fn gen_pg(src: &str) -> String {
    gen_for(src, Dialect::Postgres)
}

#[test]
fn implicit_id_is_uuid_primary_key() {
    let ddl = gen("Org { id: Id, name: text }");
    assert!(ddl.contains("CREATE TABLE `org` ("), "\n{ddl}");
    assert!(ddl.contains("`id` UUID NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("PRIMARY KEY (`id`)"), "\n{ddl}");
    // implicit id carries no SQL default (app-generated)
    assert!(!ddl.contains("`id` UUID NOT NULL DEFAULT"), "\n{ddl}");
}

#[test]
fn type_mapping_and_nullability() {
    let ddl = gen(r#"
        Widget {
          id: Id
          name:   text
          note:   text?
          count:  int
          active: bool
          at:     timestamp
          day:    date
          blob:   json
          tags:   text[]
        }
        "#);
    assert!(ddl.contains("`name` VARCHAR(255) NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`note` VARCHAR(255) NULL"), "\n{ddl}");
    assert!(ddl.contains("`count` BIGINT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`active` BOOLEAN NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`at` DATETIME NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`day` DATE NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`blob` JSON NOT NULL"), "\n{ddl}");
    // to-many scalar has no columnar form -> JSON array
    assert!(ddl.contains("`tags` JSON NOT NULL"), "\n{ddl}");
}

#[test]
fn decimal_and_float_map_per_dialect() {
    let src = r#"
        Ledger {
          id: Id
          price: decimal(12, 2)
          bare:  decimal
          score: float
        }
        "#;
    let maria = gen(src);
    assert!(
        maria.contains("`price` DECIMAL(12, 2) NOT NULL"),
        "\n{maria}"
    );
    assert!(
        maria.contains("`bare` DECIMAL(38, 9) NOT NULL"),
        "\n{maria}"
    );
    assert!(maria.contains("`score` DOUBLE NOT NULL"), "\n{maria}");

    let sqlite = gen_sqlite(src);
    // SQLite stores a decimal as TEXT (exact string round-trip, not lossy NUMERIC affinity).
    assert!(sqlite.contains("`price` TEXT NOT NULL"), "\n{sqlite}");
    assert!(sqlite.contains("`score` REAL NOT NULL"), "\n{sqlite}");

    let pg = gen_pg(src);
    assert!(pg.contains("\"price\" NUMERIC(12, 2) NOT NULL"), "\n{pg}");
    assert!(pg.contains("\"score\" DOUBLE PRECISION NOT NULL"), "\n{pg}");
}

#[test]
fn decimal_default_is_byte_exact() {
    // The trailing zero survives (a float round-trip would drop it).
    let ddl = gen("Order { id: Id, total: decimal(12, 2) (default 0.10) }");
    assert!(
        ddl.contains("`total` DECIMAL(12, 2) NOT NULL DEFAULT 0.10"),
        "\n{ddl}"
    );
}

#[test]
fn defaults_render_as_sql() {
    let ddl = gen(r#"
        Order {
          id: Id
          status: text (default "pending")
          total:  int (default 0)
          live:   bool (default true)
          at:     timestamp (default now())
        }
        "#);
    assert!(
        ddl.contains("`status` VARCHAR(255) NOT NULL DEFAULT 'pending'"),
        "\n{ddl}"
    );
    assert!(ddl.contains("`total` BIGINT NOT NULL DEFAULT 0"), "\n{ddl}");
    assert!(
        ddl.contains("`live` BOOLEAN NOT NULL DEFAULT TRUE"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("`at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP"),
        "\n{ddl}"
    );
}

#[test]
fn unique_modifier_becomes_constraint() {
    let ddl = gen("Org { id: Id, slug: text (unique) }");
    assert!(
        ddl.contains("CONSTRAINT `uq_org_slug` UNIQUE (`slug`)"),
        "\n{ddl}"
    );
}

#[test]
fn forward_relation_emits_fk_column_no_constraint() {
    let ddl = gen(r#"
        Org { id: Id, name: text }
        Order { id: Id, org: Org, buyer: Org? }
        "#);
    // FK column named `<field>_id`, typed as the target's PK (uuid); optional -> NULL
    assert!(ddl.contains("`org_id` UUID NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`buyer_id` UUID NULL"), "\n{ddl}");
    // no FK constraints by default
    assert!(!ddl.contains("FOREIGN KEY"), "\n{ddl}");
    assert!(!ddl.contains("REFERENCES"), "\n{ddl}");
}

#[test]
fn inverse_edge_stores_no_column() {
    let ddl = gen(r#"
        Org { id: Id, name: text, orders: Order[] }
        Order { id: Id, org: Org }
        "#);
    // the to-many `orders` edge lives on `order.org_id`, never a column on `org`
    let org = ddl.split("CREATE TABLE `order`").next().unwrap();
    assert!(!org.contains("orders"), "inverse leaked a column:\n{org}");
}

#[test]
fn index_columns_resolve_relations_to_fk() {
    let ddl = gen(r#"
        Org { id: Id, name: text }
        User { id: Id, name: text }
        Membership {
          id: Id
          org:  Org
          user: User
          role: text
          @index(org, user) unique
          @index role
        }
        "#);
    // composite unique index over the two FK columns (not the field names)
    assert!(
        ddl.contains("UNIQUE KEY `uq_membership_org_user` (`org_id`, `user_id`)"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("KEY `idx_membership_role` (`role`)"),
        "\n{ddl}"
    );
}

#[test]
fn declared_index_on_soft_delete_model_is_predicate_leading() {
    // A written `@index` on a `@soft_delete` model is rendered soft-delete-leading:
    // the always-filtered tombstone column is prepended (predicate-leading — MariaDB
    // has no partial indexes), so the declared index still leads with what selects.
    let ddl = gen(r#"
        @sort(placed_at desc)
        Order { id: Id, placed_at: timestamp, items: OrderItem[], @index placed_at }
        @soft_delete(deleted_at)
        OrderItem { id: Id, deleted_at: timestamp?, order: Order, qty: int, @index order }
        shape O from Order { first_qty = items.qty }
        query orders() -> O[];
        "#);
    assert!(
        ddl.contains("KEY `idx_order_item_deleted_at_order` (`deleted_at`, `order_id`)"),
        "\n{ddl}"
    );
}

#[test]
fn declared_join_key_index_names_the_fk_column() {
    // The join-key index is written, not inferred; a non-soft-delete model renders it
    // as-declared over the FK column, with no engine-owned `inf_` key.
    let ddl = gen(r#"
        @sort(placed_at desc)
        Order { id: Id, placed_at: timestamp, items: OrderItem[], @index placed_at }
        OrderItem { id: Id, order: Order, qty: int, @index order }
        shape O from Order { first_qty = items.qty }
        query orders() -> O[];
        "#);
    assert!(
        ddl.contains("KEY `idx_order_item_order` (`order_id`)"),
        "\n{ddl}"
    );
    assert!(!ddl.contains("`inf_"), "\n{ddl}");
}

// ---------------------------------------------------------------------------
// SQLite dialect . Same resolved schema, SQLite-shaped physical output:
// a small type set, no inline index clauses (indexes trail as `CREATE INDEX`),
// bool defaults as `0`/`1`. The DML/mutation SQL is unchanged (dialect-portable).
// ---------------------------------------------------------------------------

#[test]
fn sqlite_header_names_the_dialect() {
    let ddl = gen_sqlite("Org { id: Id, name: text }");
    assert!(ddl.contains("(dialect: sqlite)"), "\n{ddl}");
}

#[test]
fn sqlite_type_mapping_and_nullability() {
    let ddl = gen_sqlite(
        r#"
        Widget {
          id: Id
          name:   text
          note:   text?
          count:  int
          active: bool
          at:     timestamp
          day:    date
          blob:   json
          tags:   text[]
        }
        "#,
    );
    // text/uuid/timestamp/date/json all ride as TEXT; int/bool as INTEGER
    assert!(ddl.contains("`name` TEXT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`note` TEXT NULL"), "\n{ddl}");
    assert!(ddl.contains("`count` INTEGER NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`active` INTEGER NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`at` TEXT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`day` TEXT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`blob` TEXT NOT NULL"), "\n{ddl}");
    // to-many scalar -> TEXT (SQLite has no JSON type; stored as a JSON string)
    assert!(ddl.contains("`tags` TEXT NOT NULL"), "\n{ddl}");
    // no MariaDB types leak through
    assert!(!ddl.contains("VARCHAR"), "\n{ddl}");
    assert!(!ddl.contains("BIGINT"), "\n{ddl}");
    assert!(!ddl.contains("BOOLEAN"), "\n{ddl}");
    assert!(!ddl.contains("DATETIME"), "\n{ddl}");
}

#[test]
fn sqlite_id_is_text_primary_key() {
    let ddl = gen_sqlite("Org { id: Id, name: text }");
    assert!(ddl.contains("`id` TEXT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("PRIMARY KEY (`id`)"), "\n{ddl}");
    // app-generated id carries no SQL default
    assert!(!ddl.contains("`id` TEXT NOT NULL DEFAULT"), "\n{ddl}");
}

#[test]
fn sqlite_fk_column_is_text() {
    let ddl = gen_sqlite(
        r#"
        Org { id: Id, name: text }
        Order { id: Id, org: Org, buyer: Org? }
        "#,
    );
    assert!(ddl.contains("`org_id` TEXT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("`buyer_id` TEXT NULL"), "\n{ddl}");
    assert!(!ddl.contains("FOREIGN KEY"), "\n{ddl}");
}

#[test]
fn sqlite_bool_default_renders_as_integer() {
    let ddl = gen_sqlite(
        r#"
        Order {
          id: Id
          status: text (default "pending")
          total:  int (default 0)
          live:   bool (default true)
          off:    bool (default false)
          at:     timestamp (default now())
        }
        "#,
    );
    assert!(
        ddl.contains("`status` TEXT NOT NULL DEFAULT 'pending'"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("`total` INTEGER NOT NULL DEFAULT 0"),
        "\n{ddl}"
    );
    // bool default is an integer literal on SQLite (no TRUE/FALSE keyword reliance)
    assert!(ddl.contains("`live` INTEGER NOT NULL DEFAULT 1"), "\n{ddl}");
    assert!(ddl.contains("`off` INTEGER NOT NULL DEFAULT 0"), "\n{ddl}");
    assert!(!ddl.contains("DEFAULT TRUE"), "\n{ddl}");
    // now() still lowers to CURRENT_TIMESTAMP (SQLite supports it)
    assert!(
        ddl.contains("`at` TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP"),
        "\n{ddl}"
    );
}

#[test]
fn sqlite_unique_constraint_stays_inline() {
    // A column-level `(unique)` is an inline table constraint on both dialects.
    let ddl = gen_sqlite("Org { id: Id, slug: text (unique) }");
    assert!(
        ddl.contains("CONSTRAINT `uq_org_slug` UNIQUE (`slug`)"),
        "\n{ddl}"
    );
}

#[test]
fn sqlite_indexes_are_separate_create_index_statements() {
    let ddl = gen_sqlite(
        r#"
        Org { id: Id, name: text }
        User { id: Id, name: text }
        Membership {
          id: Id
          org:  Org
          user: User
          role: text
          @index(org, user) unique
          @index role
        }
        "#,
    );
    // No inline KEY / UNIQUE KEY index clause on SQLite (it has no such table syntax).
    // (`PRIMARY KEY` is the column constraint, not an index clause — allowed.)
    assert!(!ddl.contains("UNIQUE KEY"), "\n{ddl}");
    assert!(!ddl.contains("  KEY `"), "\n{ddl}");
    // Instead each index trails as its own statement, columns resolved to FK cols.
    assert!(
        ddl.contains(
            "CREATE UNIQUE INDEX `uq_membership_org_user` ON `membership` (`org_id`, `user_id`);"
        ),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("CREATE INDEX `idx_membership_role` ON `membership` (`role`);"),
        "\n{ddl}"
    );
    // The CREATE INDEX statements come *after* the CREATE TABLE they index.
    let table_end = ddl.find("CREATE TABLE `membership`").unwrap();
    let idx_at = ddl.find("CREATE INDEX `idx_membership_role`").unwrap();
    assert!(idx_at > table_end, "index emitted before its table:\n{ddl}");
}

#[test]
fn sqlite_declared_soft_delete_index_is_a_predicate_leading_create_index() {
    // A written `@index` on a soft-delete model becomes a trailing CREATE INDEX on
    // SQLite, rendered predicate-leading (soft-delete column first).
    let ddl = gen_sqlite(
        r#"
        @sort(placed_at desc)
        Order { id: Id, placed_at: timestamp, items: OrderItem[], @index placed_at }
        @soft_delete(deleted_at)
        OrderItem { id: Id, deleted_at: timestamp?, order: Order, qty: int, @index order }
        shape O from Order { first_qty = items.qty }
        query orders() -> O[];
        "#,
    );
    assert!(
        ddl.contains(
            "CREATE INDEX `idx_order_item_deleted_at_order` ON `order_item` (`deleted_at`, `order_id`);"
        ),
        "\n{ddl}"
    );
}

// ---------- Postgres  -------------------------------------------------

#[test]
fn pg_uses_double_quoted_identifiers_and_native_types() {
    // Postgres double-quotes identifiers (`"order"`, a reserved word — the reason
    // quoting matters) and has native BIGINT / BOOLEAN / TIMESTAMPTZ / UUID types.
    let ddl = gen_pg(
        r#"
        @created(created_at)
        Order {
          id: Id
          created_at: timestamp
          status:     text
          total:      int
          live:       bool
          tags:       text[]
          meta:       json
        }
        "#,
    );
    assert!(ddl.contains("CREATE TABLE \"order\" ("), "\n{ddl}");
    assert!(ddl.contains("\"id\" UUID NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("PRIMARY KEY (\"id\")"), "\n{ddl}");
    assert!(ddl.contains("\"status\" TEXT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("\"total\" BIGINT NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("\"live\" BOOLEAN NOT NULL"), "\n{ddl}");
    assert!(
        ddl.contains("\"created_at\" TIMESTAMPTZ NOT NULL"),
        "\n{ddl}"
    );
    // json -> JSONB (the indexable/`@>`-queryable form); a to-many scalar -> JSONB too.
    assert!(ddl.contains("\"meta\" JSONB NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("\"tags\" JSONB NOT NULL"), "\n{ddl}");
    // no backtick-quoted identifiers in a Postgres artifact (the `based gen sql`
    // header comment has backticks in its prose, so check the statement body only).
    let body = &ddl[ddl.find("CREATE TABLE").unwrap()..];
    assert!(!body.contains('`'), "\n{ddl}");
}

#[test]
fn pg_fk_column_is_uuid() {
    let ddl = gen_pg(
        r#"
        Org { id: Id, name: text }
        Order { id: Id, org: Org, buyer: Org? }
        "#,
    );
    assert!(ddl.contains("\"org_id\" UUID NOT NULL"), "\n{ddl}");
    assert!(ddl.contains("\"buyer_id\" UUID NULL"), "\n{ddl}");
    assert!(!ddl.contains("FOREIGN KEY"), "\n{ddl}");
}

#[test]
fn pg_bool_default_uses_keyword() {
    // Unlike SQLite, Postgres has TRUE/FALSE keywords (like MariaDB).
    let ddl = gen_pg(
        r#"
        Order {
          id: Id
          status: text (default "pending")
          live:   bool (default true)
          off:    bool (default false)
          at:     timestamp (default now())
        }
        "#,
    );
    assert!(
        ddl.contains("\"status\" TEXT NOT NULL DEFAULT 'pending'"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("\"live\" BOOLEAN NOT NULL DEFAULT TRUE"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("\"off\" BOOLEAN NOT NULL DEFAULT FALSE"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("\"at\" TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP"),
        "\n{ddl}"
    );
}

#[test]
fn pg_indexes_are_separate_create_index_statements() {
    // Like SQLite, Postgres has no inline KEY / UNIQUE KEY clause: indexes trail the
    // table as CREATE [UNIQUE] INDEX statements. `(unique)` stays an inline constraint.
    let ddl = gen_pg(
        r#"
        Org { id: Id, slug: text (unique) }
        User { id: Id, name: text }
        Membership {
          id: Id
          org:  Org
          user: User
          role: text
          @index(org, user) unique
          @index role
        }
        "#,
    );
    assert!(!ddl.contains("UNIQUE KEY"), "\n{ddl}");
    assert!(!ddl.contains("  KEY \""), "\n{ddl}");
    assert!(
        ddl.contains("CONSTRAINT \"uq_org_slug\" UNIQUE (\"slug\")"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains(
            "CREATE UNIQUE INDEX \"uq_membership_org_user\" ON \"membership\" (\"org_id\", \"user_id\");"
        ),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("CREATE INDEX \"idx_membership_role\" ON \"membership\" (\"role\");"),
        "\n{ddl}"
    );
}

// ---------- enums ----------------------------------------------------------

const ENUM_SCHEMA: &str = r#"
enum Status { pending, paid, shipped, cancelled }
Order { id: Id, status: Status (default pending), total: int }
"#;

#[test]
fn enum_column_mariadb_is_text_with_check_and_default() {
    let ddl = gen(ENUM_SCHEMA);
    assert!(
        ddl.contains("`status` VARCHAR(255) NOT NULL DEFAULT 'pending'"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("CONSTRAINT `ck_order_status` CHECK (`status` IN ('pending', 'paid', 'shipped', 'cancelled'))"),
        "\n{ddl}"
    );
}

#[test]
fn enum_column_sqlite_is_text_with_check() {
    let ddl = gen_sqlite(ENUM_SCHEMA);
    assert!(
        ddl.contains("`status` TEXT NOT NULL DEFAULT 'pending'"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("CONSTRAINT `ck_order_status` CHECK (`status` IN ('pending', 'paid', 'shipped', 'cancelled'))"),
        "\n{ddl}"
    );
}

#[test]
fn enum_column_postgres_is_text_with_check() {
    let ddl = gen_pg(ENUM_SCHEMA);
    assert!(
        ddl.contains("\"status\" TEXT NOT NULL DEFAULT 'pending'"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("CONSTRAINT \"ck_order_status\" CHECK (\"status\" IN ('pending', 'paid', 'shipped', 'cancelled'))"),
        "\n{ddl}"
    );
}

const STRING_ENUM_NAME_NE_VALUE: &str = r#"
enum Status { pending, paid = "PAID" }
Order { id: Id, status: Status (default paid), total: int }
"#;

#[test]
fn string_enum_check_and_default_use_the_wire_value_not_the_name() {
    let ddl = gen_sqlite(STRING_ENUM_NAME_NE_VALUE);
    // The CHECK and DEFAULT carry the wire value `PAID`, not the variant name `paid`.
    assert!(
        ddl.contains("`status` TEXT NOT NULL DEFAULT 'PAID'"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("CHECK (`status` IN ('pending', 'PAID'))"),
        "\n{ddl}"
    );
}

const INT_ENUM_SCHEMA: &str = r#"
enum Priority { low = 0, medium = 1, high = 2 }
Ticket { id: Id, priority: Priority (default low), title: text }
"#;

#[test]
fn int_enum_column_mariadb_is_integer_with_int_check() {
    let ddl = gen(INT_ENUM_SCHEMA);
    assert!(
        ddl.contains("`priority` BIGINT NOT NULL DEFAULT 0"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("CONSTRAINT `ck_ticket_priority` CHECK (`priority` IN (0, 1, 2))"),
        "\n{ddl}"
    );
}

#[test]
fn int_enum_column_sqlite_is_integer_with_int_check() {
    let ddl = gen_sqlite(INT_ENUM_SCHEMA);
    assert!(
        ddl.contains("`priority` INTEGER NOT NULL DEFAULT 0"),
        "\n{ddl}"
    );
    assert!(ddl.contains("CHECK (`priority` IN (0, 1, 2))"), "\n{ddl}");
}

#[test]
fn int_enum_column_postgres_is_integer_with_int_check() {
    let ddl = gen_pg(INT_ENUM_SCHEMA);
    assert!(
        ddl.contains("\"priority\" BIGINT NOT NULL DEFAULT 0"),
        "\n{ddl}"
    );
    assert!(ddl.contains("CHECK (\"priority\" IN (0, 1, 2))"), "\n{ddl}");
}

// ---------- opaque columns + exotic indexes --------------------------------

const OPAQUE: &str = r#"
    Place {
      id:       Id
      name:     text
      location: raw("geometry(Point,4326)")?
      search:   raw({ postgres: "tsvector", mariadb: "text", sqlite: "text" })?
      @index raw("(lower(name))")
    }
    "#;

#[test]
fn opaque_column_type_is_the_literal_string_per_dialect() {
    let pg = gen_pg(OPAQUE);
    assert!(
        pg.contains(r#""location" geometry(Point,4326) NULL"#),
        "\n{pg}"
    );
    assert!(pg.contains(r#""search" tsvector NULL"#), "\n{pg}");

    let maria = gen(OPAQUE);
    assert!(
        maria.contains("`location` geometry(Point,4326) NULL"),
        "\n{maria}"
    );
    assert!(maria.contains("`search` text NULL"), "\n{maria}");

    let lite = gen_sqlite(OPAQUE);
    assert!(lite.contains("`search` text NULL"), "\n{lite}");
}

#[test]
fn opaque_index_body_rides_verbatim_on_every_dialect() {
    // The body replaces the column list; the name is content-derived, so it is
    // identical across dialects and stable under reordering.
    for ddl in [gen(OPAQUE), gen_sqlite(OPAQUE), gen_pg(OPAQUE)] {
        let line = ddl
            .lines()
            .find(|l| l.contains("_raw_"))
            .unwrap_or_else(|| panic!("no opaque index emitted:\n{ddl}"));
        assert!(line.starts_with("CREATE INDEX "), "{line}");
        assert!(line.ends_with("(lower(name));"), "{line}");
    }
    // Even on MariaDB, where ordinary indexes are inline `KEY` clauses.
    assert!(
        !gen(OPAQUE).contains("KEY `idx_place_raw"),
        "\n{}",
        gen(OPAQUE)
    );
}

#[test]
fn index_access_method_renders_per_dialect() {
    let pg = gen_pg(r#"Place { id: Id, tags: raw("tsvector")?, @index tags using gin }"#);
    assert!(
        pg.contains(r#"CREATE INDEX "idx_place_tags" ON "place" USING gin ("tags");"#),
        "\n{pg}"
    );
    // MariaDB spells its two exotic methods as index kinds, and btree/hash as a
    // trailing `USING`.
    let maria = gen(r#"Doc { id: Id, body: text, @index body using fulltext }"#);
    assert!(
        maria.contains("FULLTEXT KEY `idx_doc_body` (`body`)"),
        "\n{maria}"
    );
    let hashed = gen(r#"Doc { id: Id, body: text, @index body using hash }"#);
    assert!(
        hashed.contains("KEY `idx_doc_body` (`body`) USING HASH"),
        "\n{hashed}"
    );
}

// ---------- foreign-key constraints (opt-in) -------------------------------

const FK_SCHEMA: &str = "Org { id: Id  name: text }\n\
     Order { id: Id  org: Org @fk(on_delete: cascade, on_update: restrict) }";

#[test]
fn fk_constraint_renders_per_dialect() {
    let maria = gen(FK_SCHEMA);
    assert!(
        maria.contains(
            "CONSTRAINT `fk_order_org_id` FOREIGN KEY (`org_id`) REFERENCES `org` (`id`) ON DELETE CASCADE ON UPDATE RESTRICT"
        ),
        "\n{maria}"
    );
    let sqlite = gen_sqlite(FK_SCHEMA);
    assert!(
        sqlite.contains(
            "CONSTRAINT `fk_order_org_id` FOREIGN KEY (`org_id`) REFERENCES `order` (`id`)"
        ) || sqlite.contains("REFERENCES `org` (`id`) ON DELETE CASCADE ON UPDATE RESTRICT"),
        "\n{sqlite}"
    );
    let pg = gen_pg(FK_SCHEMA);
    assert!(
        pg.contains(
            r#"CONSTRAINT "fk_order_org_id" FOREIGN KEY ("org_id") REFERENCES "order" ("id") ON DELETE CASCADE ON UPDATE RESTRICT"#
        ) || pg.contains(r#"REFERENCES "org" ("id") ON DELETE CASCADE ON UPDATE RESTRICT"#),
        "\n{pg}"
    );
}

#[test]
fn bare_fk_has_no_action_clause() {
    let maria = gen("Org { id: Id  name: text }\nOrder { id: Id  org: Org @fk }");
    assert!(
        maria.contains("FOREIGN KEY (`org_id`) REFERENCES `org` (`id`)"),
        "\n{maria}"
    );
    assert!(!maria.contains("ON DELETE"), "\n{maria}");
}

#[test]
fn no_fk_relation_emits_no_constraint_even_under_all() {
    let src = "Org { id: Id  name: text }\n\
         Order { id: Id  org: Org @no_fk(\"legacy\") }";
    let sf = parse_file(src, FileId(0)).unwrap();
    let (schema, _) = check(&sf.decls);
    let ddl = sql::ddl_with(&schema, Dialect::Postgres, based_sema::ForeignKeys::All);
    assert!(!ddl.contains("FOREIGN KEY"), "\n{ddl}");
    // The FK column still exists — only the constraint is opted out.
    assert!(ddl.contains(r#""org_id" UUID"#), "\n{ddl}");
}

#[test]
fn foreign_keys_all_constrains_every_relation() {
    let src = "Org { id: Id  name: text }\nOrder { id: Id  org: Org }";
    let sf = parse_file(src, FileId(0)).unwrap();
    let (schema, _) = check(&sf.decls);
    let ddl = sql::ddl_with(&schema, Dialect::Postgres, based_sema::ForeignKeys::All);
    assert!(
        ddl.contains(r#"FOREIGN KEY ("org_id") REFERENCES "org" ("id")"#),
        "\n{ddl}"
    );
}

#[test]
fn foreign_keys_none_default_emits_no_constraint() {
    let ddl = gen("Org { id: Id  name: text }\nOrder { id: Id  org: Org }");
    assert!(!ddl.contains("FOREIGN KEY"), "\n{ddl}");
}
