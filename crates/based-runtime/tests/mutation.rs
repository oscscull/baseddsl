//! Runtime write-path tests: a mutation request (JSON args + `$ctx`) → engine
//! id-gen + bound positional writes → executed under one transaction → write
//! response. Each test compiles a whole-schema snippet into a `Compiled`, then
//! drives `plan_mutation` / `run_mutation` with a deterministic `SeqIdGen`.
//!
//! Headline assertions: (1) a `create`'s engine `id` is generated and bound as the
//! leading `?`; (2) args + `$ctx` bind exactly as on the read side; (3) a `tx`
//! numbers sibling creates and a `^.id` back-reference reuses the prior create's
//! generated id; (4) the whole body runs between one `begin`/`commit`.

use based_ast::FileId;
use based_parser::parse_file;
use based_sema::check;
use serde_json::json;

use based_runtime::plan::PlanError;
use based_runtime::value::SqlValue;
use based_runtime::{plan_mutation, run_mutation, Compiled, MockDb, Request, SeqIdGen};

/// Compile a whole-schema snippet into a served `Compiled`, asserting it is clean.
fn compile(src: &str) -> Compiled {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    Compiled::from_checked(schema, sf.decls)
}

fn req(name: &str, args: serde_json::Value) -> Request {
    Request::new(name, args, json!({}))
}

/// A canned result row for a `MockDb` re-select response (the D12 declared-shape read-back).
fn row(pairs: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    pairs.as_object().cloned().unwrap()
}

const CREATE_SCHEMA: &str = r#"
    Org { name: text }
    User { name: text }
    @created(created_at)
    Order {
        created_at: timestamp,
        org: Org,
        buyer: User,
        total: int,
    }
    shape OrderCard from Order { total }
    mutation place_order(org: Id, buyer: Id, total: int) -> OrderCard {
        create Order { org = $org, buyer = $buyer, total = $total };
    }
"#;

#[test]
fn create_generates_id_and_binds_params_positionally() {
    let c = compile(CREATE_SCHEMA);
    let mut ids = SeqIdGen::default();
    let plan = plan_mutation(
        &c,
        &req(
            "place_order",
            json!({ "org": "o-1", "buyer": "u-1", "total": 42 }),
        ),
        &mut ids,
    )
    .unwrap();

    assert_eq!(plan.stmts.len(), 1);
    let s = &plan.stmts[0];
    assert!(s.sql.contains("INSERT INTO `order`"), "{}", s.sql);
    assert!(
        s.sql.contains("VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP)"),
        "{}",
        s.sql
    );
    assert!(!s.sql.contains(':'), "no named binds left: {}", s.sql);
    // engine `id` leads, then the params in column order.
    assert_eq!(
        s.params,
        vec![
            SqlValue::Text("id-0".into()),
            SqlValue::Text("o-1".into()),
            SqlValue::Text("u-1".into()),
            SqlValue::Int(42),
        ]
    );
    // the response identifies the created row by its generated id (return model = Order).
    assert_eq!(plan.result_id.as_deref(), Some("id-0"));
}

#[test]
fn run_create_reselects_the_declared_shape_inside_the_tx() {
    let c = compile(CREATE_SCHEMA);
    let mut ids = SeqIdGen::default();
    // The mutation returns `OrderCard { total }`, so after the INSERT the engine
    // re-selects the created row in that shape (D12). The mock replies to that fetch
    // with the shaped row — which becomes the response, not a bare `{ id }`.
    let mut db = MockDb::new(vec![vec![row(json!({ "total": 42 }))]]);
    let out = run_mutation(
        &c,
        &mut db,
        &mut ids,
        &req(
            "place_order",
            json!({ "org": "o-1", "buyer": "u-1", "total": 42 }),
        ),
    )
    .unwrap();

    // The response is the declared shape (matches the client's decoded output type),
    // not `{ id }`.
    assert_eq!(out, json!({ "total": 42 }));
    // The re-select runs inside the transaction: INSERT then the shaped SELECT, all
    // between one begin/commit (principle 7).
    assert_eq!(db.tx, vec!["begin", "commit"]);
    assert_eq!(db.calls.len(), 2);
    let (write_sql, _) = &db.calls[0];
    assert!(write_sql.contains("INSERT INTO `order`"), "{write_sql}");
    let (sel_sql, sel_params) = &db.calls[1];
    assert!(sel_sql.starts_with("SELECT"), "{sel_sql}");
    assert!(sel_sql.contains("FROM `order`"), "{sel_sql}");
    // keyed on the created row's engine id, bound positionally.
    assert!(sel_sql.contains("`order`.`id` = ?"), "{sel_sql}");
    assert_eq!(sel_params, &vec![SqlValue::Text("id-0".into())]);
}

#[test]
fn missing_required_arg_is_rejected_before_id_gen() {
    let c = compile(CREATE_SCHEMA);
    let mut ids = SeqIdGen::default();
    let err =
        plan_mutation(&c, &req("place_order", json!({ "org": "o-1" })), &mut ids).unwrap_err();
    assert_eq!(err, PlanError::MissingArg("buyer".into()));
}

#[test]
fn unknown_mutation_is_rejected() {
    let c = compile(CREATE_SCHEMA);
    let mut ids = SeqIdGen::default();
    let err = plan_mutation(&c, &req("nope", json!({})), &mut ids).unwrap_err();
    assert_eq!(err, PlanError::UnknownMutation("nope".into()));
}

const UPDATE_SCHEMA: &str = r#"
    Org { name: text }
    @soft_delete(deleted_at)
    @scope(org = $ctx.org)
    @updated(updated_at)
    Order {
        deleted_at: timestamp?,
        updated_at: timestamp,
        org: Org,
        status: text,
    }
    shape OrderCard from Order { status }
    mutation set_status(id: Id, status: text) -> OrderCard {
        update Order where (id = $id) { status = $status };
    }
"#;

#[test]
fn update_binds_arg_then_ctx_scope_and_returns_no_created_row() {
    let c = compile(UPDATE_SCHEMA);
    let mut ids = SeqIdGen::default();
    let r = Request::new(
        "set_status",
        json!({ "id": "ord-9", "status": "shipped" }),
        json!({ "org": "org-7" }),
    );
    let plan = plan_mutation(&c, &r, &mut ids).unwrap();

    let s = &plan.stmts[0];
    assert!(s.sql.starts_with("UPDATE `order`"), "{}", s.sql);
    assert!(!s.sql.contains(':'), "no named binds left: {}", s.sql);
    // placeholder order: SET `status` first, then the WHERE `id`, then injected `:ctx_org`.
    assert_eq!(
        s.params,
        vec![
            SqlValue::Text("shipped".into()),
            SqlValue::Text("ord-9".into()),
            SqlValue::Text("org-7".into()),
        ]
    );
    // no create -> the response identifies no row (re-select deferred).
    assert!(plan.result_id.is_none());
}

#[test]
fn update_missing_ctx_is_rejected() {
    let c = compile(UPDATE_SCHEMA);
    let mut ids = SeqIdGen::default();
    let err = plan_mutation(
        &c,
        &req("set_status", json!({ "id": "ord-9", "status": "x" })),
        &mut ids,
    )
    .unwrap_err();
    assert_eq!(err, PlanError::MissingCtx("org".into()));
}

#[test]
fn soft_delete_executes_a_tombstone_update_never_a_real_delete() {
    let c = compile(
        r#"
        @soft_delete(deleted_at)
        @updated(updated_at)
        Order { deleted_at: timestamp?, updated_at: timestamp, status: text }
        shape OrderCard from Order { status }
        mutation remove(id: Id) -> OrderCard {
            delete Order where (id = $id);
        }
        "#,
    );
    let mut ids = SeqIdGen::default();
    let mut db = MockDb::default();
    let out = run_mutation(
        &c,
        &mut db,
        &mut ids,
        &req("remove", json!({ "id": "ord-1" })),
    )
    .unwrap();

    // the executed statement is the tombstone UPDATE, not a DELETE.
    let (sql, params) = &db.calls[0];
    assert!(sql.starts_with("UPDATE `order`"), "{sql}");
    assert!(!sql.contains("DELETE"), "must not be a real DELETE: {sql}");
    assert_eq!(params, &vec![SqlValue::Text("ord-1".into())]);
    // nothing created -> empty write response.
    assert_eq!(out, json!({}));
}

#[test]
fn tx_numbers_sibling_creates_and_backref_reuses_prior_id() {
    let c = compile(
        r#"
        User { email: text }
        Address { user: User, city: text }
        shape UserCard from User { email }
        mutation signup(email: text, city: text) -> UserCard {
            tx {
                create User { email = $email };
                create Address { user = ^.id, city = $city };
            }
        }
        "#,
    );
    let mut ids = SeqIdGen::default();
    let plan = plan_mutation(
        &c,
        &req("signup", json!({ "email": "a@b.c", "city": "NYC" })),
        &mut ids,
    )
    .unwrap();

    assert_eq!(plan.stmts.len(), 2);
    // User: its own generated id (`id-0`), then the email.
    assert!(
        plan.stmts[0].sql.contains("INSERT INTO `user`"),
        "{}",
        plan.stmts[0].sql
    );
    assert_eq!(
        plan.stmts[0].params,
        vec![
            SqlValue::Text("id-0".into()),
            SqlValue::Text("a@b.c".into())
        ]
    );
    // Address: its own generated id (`id-1`), the `^.id` back-reference reusing the
    // User's `id-0`, then the city — the back-ref binds the *same* value the User got.
    assert!(
        plan.stmts[1].sql.contains("INSERT INTO `address`"),
        "{}",
        plan.stmts[1].sql
    );
    assert_eq!(
        plan.stmts[1].params,
        vec![
            SqlValue::Text("id-1".into()),
            SqlValue::Text("id-0".into()),
            SqlValue::Text("NYC".into()),
        ]
    );
    // the response identifies the User row (the return model).
    assert_eq!(plan.result_id.as_deref(), Some("id-0"));

    // both writes plus the declared-shape re-select run under one transaction, in
    // order; the re-select reads the created User (the return model) back as UserCard.
    let mut db = MockDb::new(vec![vec![row(json!({ "email": "a@b.c" }))]]);
    let out = run_mutation(
        &c,
        &mut db,
        &mut SeqIdGen::default(),
        &req("signup", json!({ "email": "a@b.c", "city": "NYC" })),
    )
    .unwrap();
    assert_eq!(out, json!({ "email": "a@b.c" }));
    assert_eq!(db.tx, vec!["begin", "commit"]);
    // two INSERTs, then the shaped re-select.
    assert_eq!(db.calls.len(), 3);
    assert!(db.calls[2].0.starts_with("SELECT"), "{}", db.calls[2].0);
}
