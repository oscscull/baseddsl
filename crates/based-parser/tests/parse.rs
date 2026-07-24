//! Parser tests: structural assertions over the tricky productions, plus a
//! guard that every commerce example file parses clean.

use based_ast::*;
use based_parser::parse_file;

fn parse_ok(src: &str) -> SchemaFile {
    match parse_file(src, FileId(0)) {
        Ok(sf) => sf,
        Err(diags) => panic!("expected clean parse, got: {diags:#?}"),
    }
}

fn parse_err(src: &str) -> Vec<based_diagnostics::Diagnostic> {
    match parse_file(src, FileId(0)) {
        Ok(_) => panic!("expected a parse error, got a clean parse"),
        Err(diags) => diags,
    }
}

fn only_model(src: &str) -> Model {
    match parse_ok(src).decls.into_iter().next() {
        Some(Decl::Model(m)) => m,
        other => panic!("expected a model, got {other:?}"),
    }
}

#[test]
fn model_decorators_fields_index_inverse() {
    let m = only_model(
        r#"
        @soft_delete(deleted_at)
        @sort(placed_at desc)
        Order {
          deleted_at:   timestamp?
          org:          Org
          fulfilled_by: User?
          status:       text (default "pending")
          items:        OrderItem[]
          @index(org, status)
          @index placed_at
        }
        "#,
    );
    assert_eq!(m.name.node, "Order");

    // decorators: soft_delete(path/ident) + sort(sort_term)
    assert_eq!(m.decorators.len(), 2);
    assert_eq!(m.decorators[0].name.node, "soft_delete");
    assert!(matches!(m.decorators[0].args[0], DecoArg::Ident(_)));
    assert_eq!(m.decorators[1].name.node, "sort");
    match &m.decorators[1].args[0] {
        DecoArg::Sort(SortTerm { path, dir }) => {
            assert_eq!(path.segments[0].node, "placed_at");
            assert_eq!(*dir, SortDir::Desc);
        }
        other => panic!("expected sort term, got {other:?}"),
    }

    let fields: Vec<&Field> = m
        .members
        .iter()
        .filter_map(|mem| match mem {
            Member::Field(f) => Some(f),
            _ => None,
        })
        .collect();
    assert_eq!(fields.len(), 5);

    // nullable primitive
    let deleted = &fields[0];
    assert_eq!(deleted.name.node, "deleted_at");
    assert!(matches!(
        deleted.ty.base,
        BaseType::Primitive(Primitive::Timestamp)
    ));
    assert!(deleted.ty.optional);

    // relation, nullable
    let fulfilled = &fields[2];
    assert!(fulfilled.ty.optional);
    assert!(matches!(&fulfilled.ty.base, BaseType::Model(m) if m.node == "User"));

    // default modifier
    let status = &fields[3];
    assert!(
        matches!(&status.modifiers[0], Modifier::Default(DefaultVal::Lit(Literal::Str(s))) if s == "pending")
    );

    // to-many relation
    let items = &fields[4];
    assert!(items.ty.many);

    // two index members: composite (2 cols) + single
    let indexes: Vec<&IndexDecl> = m
        .members
        .iter()
        .filter_map(|mem| match mem {
            Member::Index(i) => Some(i),
            _ => None,
        })
        .collect();
    assert_eq!(indexes.len(), 2);
    assert_eq!(indexes[0].columns.len(), 2);
    assert_eq!(indexes[1].columns.len(), 1);
}

#[test]
fn field_inverse_ref_and_unique() {
    let m = only_model(
        r#"
        User {
          email:         text (unique)
          invited_by:    User?
          invited_users: User[]  (User.invited_by)
        }
        "#,
    );
    let fields: Vec<&Field> = m
        .members
        .iter()
        .filter_map(|mem| match mem {
            Member::Field(f) => Some(f),
            _ => None,
        })
        .collect();
    assert!(matches!(fields[0].modifiers[0], Modifier::Unique));
    let inv = fields[2].inverse.as_ref().expect("inverse ref");
    assert_eq!(inv.model.node, "User");
    assert_eq!(inv.field.node, "invited_by");
}

#[test]
fn legacy_reserved_word_field_name() {
    // `order` is a reserved word; it must still parse as a field name (the
    // canonical OrderItem model uses it).
    let m = only_model(
        r#"
        OrderItem {
          order:   Order
          product: Product
        }
        "#,
    );
    let f = match &m.members[0] {
        Member::Field(f) => f,
        other => panic!("expected field, got {other:?}"),
    };
    assert_eq!(f.name.node, "order");
    assert!(matches!(&f.ty.base, BaseType::Model(m) if m.node == "Order"));
}

#[test]
fn query_tiers_and_bindings() {
    let sf = parse_ok(
        r#"
        query order_by_id(id) -> OrderCard;
        query orders_by_buyer(user -> placed_by) -> OrderCard[];
        query recent(since: timestamp > created_at) -> OrderCard[];
        "#,
    );
    let qs: Vec<&Query> = sf
        .decls
        .iter()
        .filter_map(|d| match d {
            Decl::Query(q) => Some(q),
            _ => None,
        })
        .collect();
    assert_eq!(qs.len(), 3);

    // bare form: params are the filter, scalar return
    assert!(matches!(qs[0].body, QueryBody::Bare));
    assert!(!qs[0].ret.many);
    assert_eq!(qs[0].ret.ty.node, "OrderCard");

    // edge binding + list return
    assert!(qs[1].ret.many);
    match &qs[1].params[0].binding {
        Some(ParamBinding::Edge(e)) => assert_eq!(e.node, "placed_by"),
        other => panic!("expected edge binding, got {other:?}"),
    }

    // explicit column + operator binding
    match &qs[2].params[0].binding {
        Some(ParamBinding::ColOp { op: Op::Gt, col }) => assert_eq!(col.node, "created_at"),
        other => panic!("expected col+op binding, got {other:?}"),
    }
}

#[test]
fn unscoped_clause_on_query_and_mutation() {
    // The `unscoped("reason")` opt-out  parses after the return type on a query,
    // after any `guard` on a mutation. The reason string is mandatory.
    let sf = parse_ok(
        r#"
        query all_orders(org) -> OrderCard[] unscoped("admin: cross-org");
        mutation import_order(org: Id) -> OrderCard unscoped("data import") {
          create Order { org = $org };
        }
        "#,
    );
    let q = match &sf.decls[0] {
        Decl::Query(q) => q,
        other => panic!("expected query, got {other:?}"),
    };
    assert!(matches!(q.body, QueryBody::Bare));
    assert_eq!(q.unscoped.as_ref().unwrap().reason, "admin: cross-org");

    let m = match &sf.decls[1] {
        Decl::Mutation(m) => m,
        other => panic!("expected mutation, got {other:?}"),
    };
    assert_eq!(m.unscoped.as_ref().unwrap().reason, "data import");
}

#[test]
fn full_query_body_predicate_precedence() {
    let sf = parse_ok(
        r#"
        query active_products(org: Id) -> ProductCard[] {
          list Product
            where (org = $org and active)
            order (created_at desc)
            page (20);
        }
        "#,
    );
    let q = match &sf.decls[0] {
        Decl::Query(q) => q,
        other => panic!("expected query, got {other:?}"),
    };
    let stmt = match &q.body {
        QueryBody::Block(s) => s,
        other => panic!("expected block body, got {other:?}"),
    };
    assert_eq!(stmt.verb, Verb::List);
    assert_eq!(stmt.model.node, "Product");
    assert_eq!(stmt.clauses.len(), 3);

    // where predicate is `And(Cmp(org=$org), Bare(active))`
    match &stmt.clauses[0] {
        Clause::Where(Predicate::And(l, r)) => {
            assert!(matches!(**l, Predicate::Cmp { op: Op::Eq, .. }));
            assert!(matches!(**r, Predicate::Bare(_)));
        }
        other => panic!("expected where(and), got {other:?}"),
    }
    match &stmt.clauses[2] {
        Clause::Page(p) => {
            assert_eq!(p.size, 20);
            assert!(!p.offset);
        }
        other => panic!("expected page, got {other:?}"),
    }
}

#[test]
fn or_binds_looser_than_and() {
    // `a and b or c` must parse as `(a and b) or c`.
    let sf = parse_ok("filter f = a and b or c;");
    let nf = match &sf.decls[0] {
        Decl::Filter(f) => f,
        other => panic!("expected filter, got {other:?}"),
    };
    match &nf.pred {
        Predicate::Or(l, r) => {
            assert!(matches!(**l, Predicate::And(_, _)));
            assert!(matches!(**r, Predicate::Bare(_)));
        }
        other => panic!("expected or at the root, got {other:?}"),
    }
}

#[test]
fn in_value_list_and_single_bind_forms() {
    // `in (…)` is a value list; `in $param` stays a single-bind comparison.
    let sf = parse_ok(
        r#"
        filter f = status in (open, "closed", 3, $extra) and status in $one;
        "#,
    );
    let nf = match &sf.decls[0] {
        Decl::Filter(f) => f,
        other => panic!("expected filter, got {other:?}"),
    };
    let Predicate::And(l, r) = &nf.pred else {
        panic!("expected and, got {:?}", nf.pred);
    };
    match &**l {
        Predicate::InList { path, values } => {
            assert_eq!(path.segments[0].node, "status");
            assert_eq!(values.len(), 4);
            assert!(matches!(&values[0], Value::Path(p) if p.segments[0].node == "open"));
            assert!(matches!(&values[1], Value::Lit(Literal::Str(s)) if s == "closed"));
            assert!(matches!(&values[2], Value::Lit(Literal::Int(3))));
            assert!(matches!(&values[3], Value::Param(pr) if pr.name.node == "extra"));
        }
        other => panic!("expected in-list, got {other:?}"),
    }
    assert!(matches!(
        &**r,
        Predicate::Cmp {
            op: Op::In,
            value: Value::Param(_),
            ..
        }
    ));
}

#[test]
fn in_empty_list_is_a_parse_error() {
    assert!(parse_file("filter f = status in ();", FileId(0)).is_err());
}

#[test]
fn shape_bare_rename_and_nest() {
    let sf = parse_ok(
        r#"
        shape OrderCard from Order {
          status
          buyer = placed_by.name
          org { name slug }
        }
        "#,
    );
    let shape = match &sf.decls[0] {
        Decl::Shape(s) => s,
        other => panic!("expected shape, got {other:?}"),
    };
    assert_eq!(shape.from.node, "Order");
    assert!(matches!(&shape.body[0], ShapeField::Bare(b) if b.node == "status"));
    match &shape.body[1] {
        ShapeField::Rename {
            out,
            value: ShapeValue::Path(p),
        } => {
            assert_eq!(out.node, "buyer");
            assert_eq!(p.segments.len(), 2);
        }
        other => panic!("expected rename, got {other:?}"),
    }
    match &shape.body[2] {
        ShapeField::Nest { field, body } => {
            assert_eq!(field.node, "org");
            assert_eq!(body.len(), 2);
        }
        other => panic!("expected nest, got {other:?}"),
    }
}

#[test]
fn shape_nest_by_named_reference() {
    let sf = parse_ok(
        r#"
        shape OrderDetail from Order {
          status
          placed_by -> UserRef
        }
        "#,
    );
    let shape = match &sf.decls[0] {
        Decl::Shape(s) => s,
        other => panic!("expected shape, got {other:?}"),
    };
    match &shape.body[1] {
        ShapeField::NestRef { field, shape } => {
            assert_eq!(field.node, "placed_by");
            assert_eq!(shape.node, "UserRef");
        }
        other => panic!("expected nest ref, got {other:?}"),
    }
}

#[test]
fn shape_nest_reference_requires_uppercamel_name() {
    // `-> full` (or any lower ident) is not a shape-reference target.
    assert!(parse_file("shape D from Order { placed_by -> full }", FileId(0)).is_err());
}

#[test]
fn shape_far_side_flattening_projection() {
    // `courses = enrollments.course { title }` — a derived field (`=`) naming a relation
    // path with a projection body parses to `Flatten`, distinct from a plain reach.
    let sf = parse_ok(
        r#"
        shape StudentCourses from Student {
          name
          courses = enrollments.course { title }
        }
        "#,
    );
    let shape = match &sf.decls[0] {
        Decl::Shape(s) => s,
        other => panic!("expected shape, got {other:?}"),
    };
    match &shape.body[1] {
        ShapeField::Flatten { out, path, body } => {
            assert_eq!(out.node, "courses");
            assert_eq!(path.segments.len(), 2);
            assert_eq!(path.segments[0].node, "enrollments");
            assert_eq!(path.segments[1].node, "course");
            assert_eq!(body.len(), 1);
            assert!(matches!(&body[0], ShapeField::Bare(b) if b.node == "title"));
        }
        other => panic!("expected flatten, got {other:?}"),
    }
    // A `= path` with no body stays a plain reach (not a flatten).
    let sf = parse_ok("shape C from Student { city = school.city }");
    let shape = match &sf.decls[0] {
        Decl::Shape(s) => s,
        other => panic!("expected shape, got {other:?}"),
    };
    assert!(matches!(&shape.body[0], ShapeField::Rename { .. }));
}

#[test]
fn mutation_with_create_and_param_refs() {
    let sf = parse_ok(
        r#"
        mutation place_order(org: Id, buyer: Id) -> OrderCard {
          create Order { org = $org, placed_by = $buyer };
        }
        "#,
    );
    let mu = match &sf.decls[0] {
        Decl::Mutation(m) => m,
        other => panic!("expected mutation, got {other:?}"),
    };
    assert_eq!(mu.params.len(), 2);
    match &mu.body[0] {
        WriteStmt::Create { model, assigns, .. } => {
            assert_eq!(model.node, "Order");
            assert_eq!(assigns.len(), 2);
            assert!(
                matches!(assigns[0].value.as_value(), Some(Value::Param(p)) if p.name.node == "org")
            );
        }
        other => panic!("expected create, got {other:?}"),
    }
}

#[test]
fn tx_create_binds_and_references_step() {
    let sf = parse_ok(
        r#"
        mutation signup(email: text, city: text) -> UserCard {
          tx {
            create User { email = $email } as user;
            create Address { user = $user.id, city = $city };
          }
        }
        "#,
    );
    let mu = match &sf.decls[0] {
        Decl::Mutation(m) => m,
        other => panic!("expected mutation, got {other:?}"),
    };
    let inner = match &mu.body[0] {
        WriteStmt::Tx(inner) => inner,
        other => panic!("expected tx, got {other:?}"),
    };
    // Step 1 binds the produced row as `user`.
    match &inner[0] {
        WriteStmt::Create { binding, .. } => {
            assert_eq!(binding.as_ref().map(|b| b.node.as_str()), Some("user"));
        }
        other => panic!("expected create, got {other:?}"),
    }
    // Step 2 references it as `$user.id` (an ordinary `$name.field` param ref).
    match &inner[1] {
        WriteStmt::Create { assigns, .. } => {
            assert!(
                matches!(
                    assigns[0].value.as_value(),
                    Some(Value::Param(pr)) if pr.name.node == "user"
                        && pr.path.len() == 1 && pr.path[0].node == "id"
                ),
                "expected `$user.id` step reference, got {:?}",
                assigns[0].value
            );
        }
        other => panic!("expected create, got {other:?}"),
    }
}

#[test]
fn bare_caret_is_a_parse_error_pointing_to_as() {
    let diags = parse_err(
        r#"
        mutation signup(email: text, city: text) -> UserCard {
          tx {
            create User { email = $email };
            create Address { user = ^.id, city = $city };
          }
        }
        "#,
    );
    assert!(
        diags.iter().any(|d| d.message.contains("as")),
        "a bare `^` should point the user at `create … as <name>;`, got {diags:?}"
    );
}

#[test]
fn raw_sql_shape_value_with_interpolation() {
    let sf = parse_ok(
        r#"
        shape U from User {
          full_name = raw`concat(first, ' ', last)`
          slug = raw`lower(${name})`
        }
        "#,
    );
    let shape = match &sf.decls[0] {
        Decl::Shape(s) => s,
        other => panic!("expected shape, got {other:?}"),
    };
    match &shape.body[1] {
        ShapeField::Rename {
            value: ShapeValue::Raw(raw),
            ..
        } => {
            // one text part + one bound param
            let has_param = raw
                .parts
                .iter()
                .any(|p| matches!(p, RawPart::Param(pr) if pr.name.node == "name"));
            assert!(
                has_param,
                "expected ${{name}} bound param, got {:?}",
                raw.parts
            );
        }
        other => panic!("expected raw shape value, got {other:?}"),
    }
}

#[test]
fn soft_delete_override_member() {
    let m = only_model(
        r#"
        Doc {
          deleted_at: timestamp?
          restore: raw`update {table} set deleted_at = null where id = {id}`
        }
        "#,
    );
    let ov = m.members.iter().find_map(|mem| match mem {
        Member::SoftOverride(o) => Some(o),
        _ => None,
    });
    let ov = ov.expect("soft override");
    assert_eq!(ov.op, SoftOp::Restore);
    assert!(ov
        .raw
        .parts
        .iter()
        .any(|p| matches!(p, RawPart::Engine(e) if e.node == "table")));
}

#[test]
fn errors_recover_across_declarations() {
    // First decl is malformed (missing type); the second must still be reported
    // cleanly, proving recovery at the declaration boundary.
    let res = parse_file("Bad { x: }\nGood { y: int }", FileId(0));
    assert!(res.is_err(), "malformed input should error");
    let diags = res.unwrap_err();
    assert!(!diags.is_empty());
}

#[test]
fn separators_are_insignificant() {
    // Same model with commas and with newlines must parse identically.
    let with_commas = only_model("M { a: int, b: text, @index a }");
    let with_newlines = only_model("M {\n a: int\n b: text\n @index a\n }");
    assert_eq!(with_commas.members.len(), with_newlines.members.len());
    assert_eq!(with_commas.members.len(), 3);
}

#[test]
fn unindexed_clause_forms() {
    // Both annotation forms, inline and in a block statement.
    let sf = parse_ok(
        r#"
        query a(status) -> P[] unindexed(max_rows: 500);
        query b(status) -> P[] unindexed(unsafe);
        query c(s) -> P[] { list Product where (status = $s) unindexed(unsafe, "ops table, tiny"); }
        "#,
    );
    let clause_of = |d: &Decl| -> Unindexed {
        let Decl::Query(q) = d else {
            panic!("expected query")
        };
        let clauses = match &q.body {
            QueryBody::Inline(cs) => cs.as_slice(),
            QueryBody::Block(s) => s.clauses.as_slice(),
            QueryBody::Bare | QueryBody::Raw(_) => panic!("expected clauses"),
        };
        match clauses.iter().find_map(|c| match c {
            Clause::Unindexed(u) => Some(u.clone()),
            _ => None,
        }) {
            Some(u) => u,
            None => panic!("no unindexed clause on `{}`", q.name.node),
        }
    };
    assert!(matches!(
        clause_of(&sf.decls[0]).kind,
        UnindexedKind::MaxRows(500)
    ));
    assert!(matches!(
        clause_of(&sf.decls[1]).kind,
        UnindexedKind::Unsafe(None)
    ));
    assert!(
        matches!(clause_of(&sf.decls[2]).kind, UnindexedKind::Unsafe(Some(r)) if r == "ops table, tiny")
    );
}

#[test]
fn scope_decl_and_refs() {
    // The named-scope surface: a `scope` decl, the `@scope Name` model decorator (an
    // alternative; comma-separated names are one conjunction), and the `scoped Name`
    // callable acknowledgement.
    let sf = parse_ok(
        r#"
        scope Tenant (org: Org = $ctx.org, region: Region = $ctx.region)
        @soft_delete(deleted_at)
        @scope Tenant
        Order { deleted_at: timestamp?, org: Org, region: Region, total: int }
        shape OrderCard from Order { total }
        query order_by_id(id) -> OrderCard scoped Tenant;
        mutation place(total: int) -> OrderCard scoped Tenant { create Order { total = $total }; }
        "#,
    );
    // The scope decl carries two `col: Type = $ctx.field` terms.
    let Decl::Scope(s) = &sf.decls[0] else {
        panic!("expected a scope decl, got {:?}", sf.decls[0]);
    };
    assert_eq!(s.name.node, "Tenant");
    assert_eq!(s.terms.len(), 2);
    assert_eq!(s.terms[0].col.node, "org");
    assert_eq!(s.terms[0].ctx.name.node, "ctx");
    assert_eq!(s.terms[0].ctx.path[0].node, "org");

    // The model carries one `@scope` alternative naming `Tenant`; the generic
    // decorator stack keeps only `@soft_delete`.
    let Decl::Model(m) = &sf.decls[1] else {
        panic!("expected a model, got {:?}", sf.decls[1]);
    };
    assert_eq!(m.scopes.len(), 1);
    assert_eq!(m.scopes[0].names.len(), 1);
    assert_eq!(m.scopes[0].names[0].node, "Tenant");
    assert!(m.decorators.iter().all(|d| d.name.node != "scope"));

    // The callables carry `scoped Tenant`, not `unscoped`.
    let Decl::Query(q) = &sf.decls[3] else {
        panic!("expected a query, got {:?}", sf.decls[3]);
    };
    let scoped = q.scoped.as_ref().expect("query has `scoped`");
    assert_eq!(scoped.names[0].node, "Tenant");
    assert!(q.unscoped.is_none());
}

#[test]
fn repeated_scope_decorator_is_two_alternatives() {
    // Stacked `@scope` decorators are OR-alternatives; a comma inside one is an AND.
    let m = only_model(
        r#"
        @scope Page, Author
        @scope Admin
        Post { page: Page, author: User, body: text }
        "#,
    );
    assert_eq!(m.scopes.len(), 2);
    assert_eq!(
        m.scopes[0]
            .names
            .iter()
            .map(|n| n.node.as_str())
            .collect::<Vec<_>>(),
        vec!["Page", "Author"]
    );
    assert_eq!(m.scopes[1].names[0].node, "Admin");
}

#[test]
fn callable_scoped_and_unscoped_are_exclusive_forms() {
    // A callable takes at most one of `scoped Name` / `unscoped("reason")`.
    let sf = parse_ok(
        r#"
        query a() -> P[] scoped Tenant order (id);
        query b() -> P[] unscoped("admin") order (id);
        "#,
    );
    let Decl::Query(a) = &sf.decls[0] else {
        panic!("expected query");
    };
    assert!(a.scoped.is_some() && a.unscoped.is_none());
    let Decl::Query(b) = &sf.decls[1] else {
        panic!("expected query");
    };
    assert!(b.unscoped.is_some() && b.scoped.is_none());
}

#[test]
fn raw_query_body_parses_with_interpolations() {
    // The whole body is one `raw` backtick block; `${param}` interpolations are
    // split out as bound-parameter parts, the SQL text kept verbatim.
    let sf = parse_ok(
        r#"
        query heavy_users(min: int) -> UserRow[] {
          raw`SELECT u.name AS name FROM user u WHERE u.total >= ${min}`;
        }
        "#,
    );
    let Decl::Query(q) = &sf.decls[0] else {
        panic!("expected query");
    };
    let QueryBody::Raw(raw) = &q.body else {
        panic!("expected raw body, got {:?}", q.body);
    };
    assert!(matches!(
        &raw.parts[..],
        [RawPart::Text(_), RawPart::Param(pr)] if pr.name.node == "min"
    ));

    // The terminating `;` is optional inside the block (same leniency as a statement).
    let sf = parse_ok("query all() -> UserRow[] { raw`SELECT 1` }");
    let Decl::Query(q) = &sf.decls[0] else {
        panic!("expected query");
    };
    assert!(matches!(q.body, QueryBody::Raw(_)));
}

#[test]
fn mutation_ret_ok_parses_as_ack() {
    let sf = parse_ok(
        r#"
        mutation purge(id: Id) -> ok {
          hard delete Comment where (id = $id);
        }
        "#,
    );
    let Some(Decl::Mutation(m)) = sf.decls.into_iter().next() else {
        panic!("expected a mutation");
    };
    assert!(m.ret.ack);
    assert!(!m.ret.many);
    assert!(!m.ret.stream);
    assert_eq!(m.ret.ty.node, "ok");
}

#[test]
fn ret_ok_with_brackets_is_an_error() {
    let diags = parse_file(
        "mutation purge(id) -> ok[] { delete Tag where (id = $id); }",
        FileId(0),
    )
    .expect_err("`ok[]` must not parse");
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("bare acknowledgement")),
        "{diags:#?}"
    );
}

#[test]
fn a_shape_field_or_model_named_ok_still_parses() {
    // `ok` is contextual: a keyword only in return-type position.
    let sf = parse_ok(
        r#"
        Check { ok: bool }
        query checks() -> Check[];
        "#,
    );
    assert_eq!(sf.decls.len(), 2);
}

#[test]
fn upsert_parses_conflict_target_and_update_branch() {
    let sf = parse_ok(
        r#"
        mutation record_hit(path: text) -> PageRow {
          create Page { path = $path, hits = 1 } on conflict (path) update { hits = hits + 1 };
        }
        "#,
    );
    let mu = match &sf.decls[0] {
        Decl::Mutation(m) => m,
        other => panic!("expected mutation, got {other:?}"),
    };
    match &mu.body[0] {
        WriteStmt::Create {
            model,
            assigns,
            conflict,
            ..
        } => {
            assert_eq!(model.node, "Page");
            assert_eq!(assigns.len(), 2);
            let oc = conflict.as_ref().expect("on conflict clause");
            assert_eq!(oc.target.len(), 1);
            assert_eq!(oc.target[0].node, "path");
            assert_eq!(oc.update.len(), 1);
            assert_eq!(oc.update[0].col.node, "hits");
        }
        other => panic!("expected create, got {other:?}"),
    }
}

#[test]
fn plain_create_has_no_conflict() {
    let sf = parse_ok("mutation m() -> R { create Page { hits = 1 } }");
    let Decl::Mutation(mu) = &sf.decls[0] else {
        panic!("expected mutation")
    };
    match &mu.body[0] {
        WriteStmt::Create { conflict, .. } => assert!(conflict.is_none()),
        other => panic!("expected create, got {other:?}"),
    }
}

#[test]
fn opaque_raw_types_and_exotic_indexes() {
    let m = only_model(
        r#"
        Place {
          id:       Id
          location: raw("geometry(Point,4326)")?
          search:   raw({ postgres: "tsvector", mariadb: "text" })
          @index location using gist
          @index raw("(lower(name))")
          @index(a, b) unique
        }
        "#,
    );
    let fields: Vec<&Field> = m
        .members
        .iter()
        .filter_map(|mem| match mem {
            Member::Field(f) => Some(f),
            _ => None,
        })
        .collect();

    let BaseType::Raw(spec) = &fields[1].ty.base else {
        panic!("expected an opaque type, got {:?}", fields[1].ty.base)
    };
    assert!(fields[1].ty.optional);
    assert_eq!(spec.for_dialect("sqlite"), Some("geometry(Point,4326)"));
    assert_eq!(spec.render(), r#"raw("geometry(Point,4326)")"#);

    let BaseType::Raw(map) = &fields[2].ty.base else {
        panic!("expected an opaque type")
    };
    assert_eq!(map.for_dialect("postgres"), Some("tsvector"));
    assert_eq!(map.for_dialect("sqlite"), None);

    let indexes: Vec<&IndexDecl> = m
        .members
        .iter()
        .filter_map(|mem| match mem {
            Member::Index(i) => Some(i),
            _ => None,
        })
        .collect();
    assert_eq!(indexes[0].method.as_ref().unwrap().node, "gist");
    assert_eq!(indexes[0].columns[0].node, "location");
    assert!(indexes[1].columns.is_empty());
    assert_eq!(
        indexes[1].raw.as_ref().unwrap().for_dialect("postgres"),
        Some("(lower(name))")
    );
    assert!(indexes[2].unique && indexes[2].method.is_none());
}

#[test]
fn opaque_raw_type_outside_a_field_is_a_parse_error() {
    // A param annotation and a scope term are not field positions.
    assert!(parse_file(r#"query q(p: raw("inet")) -> R;"#, FileId(0)).is_err());
    assert!(parse_file(r#"scope S (net: raw("inet") = $ctx.net)"#, FileId(0)).is_err());
    // An opaque type has no array form.
    assert!(parse_file(r#"M { id: Id, a: raw("inet")[] }"#, FileId(0)).is_err());
}

#[test]
fn fk_and_no_fk_annotations_parse() {
    let m = only_model(
        r#"
        Order {
          org:        Org @fk(on_delete: cascade, on_update: restrict)
          reason_fk:  Org @fk("orders die with their org", on_delete: cascade)
          bare:       Org @fk
          skip:       Org @no_fk("legacy")
          skip2:      Org @no_fk
        }
        "#,
    );
    let field = |name: &str| {
        m.members
            .iter()
            .find_map(|mem| match mem {
                Member::Field(f) if f.name.node == name => Some(f),
                _ => None,
            })
            .unwrap()
    };
    let org = field("org").fk.as_ref().unwrap();
    assert_eq!(org.on_delete.as_ref().unwrap().node, "cascade");
    assert_eq!(org.on_update.as_ref().unwrap().node, "restrict");
    assert!(org.reason.is_none());

    let reason_fk = field("reason_fk").fk.as_ref().unwrap();
    assert_eq!(
        reason_fk.reason.as_ref().unwrap().node,
        "orders die with their org"
    );
    assert_eq!(reason_fk.on_delete.as_ref().unwrap().node, "cascade");

    let bare = field("bare").fk.as_ref().unwrap();
    assert!(bare.reason.is_none() && bare.on_delete.is_none() && bare.on_update.is_none());

    assert_eq!(
        field("skip")
            .no_fk
            .as_ref()
            .unwrap()
            .reason
            .as_ref()
            .unwrap()
            .node,
        "legacy"
    );
    assert!(field("skip2").no_fk.as_ref().unwrap().reason.is_none());
}

#[test]
fn model_level_no_fk_parses_as_a_decorator() {
    let m = only_model(r#"@no_fk("legacy table") Order { org: Org }"#);
    assert!(m.decorators.iter().any(|d| d.name.node == "no_fk"));
}
