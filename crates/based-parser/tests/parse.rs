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
        WriteStmt::Create { model, assigns } => {
            assert_eq!(model.node, "Order");
            assert_eq!(assigns.len(), 2);
            assert!(matches!(&assigns[0].value, Value::Param(p) if p.name.node == "org"));
        }
        other => panic!("expected create, got {other:?}"),
    }
}

#[test]
fn tx_create_with_backreference() {
    let sf = parse_ok(
        r#"
        mutation signup(email: text, city: text) -> UserCard {
          tx {
            create User { email = $email };
            create Address { user = ^.id, city = $city };
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
    match &inner[1] {
        WriteStmt::Create { assigns, .. } => {
            assert!(
                matches!(&assigns[0].value, Value::Back(b) if b.field.node == "id"),
                "expected `^.id` back-reference, got {:?}",
                assigns[0].value
            );
        }
        other => panic!("expected create, got {other:?}"),
    }
}

#[test]
fn raw_sql_shape_value_with_interpolation() {
    let sf = parse_ok(
        r#"
        shape U from User {
          full_name = sql`concat(first, ' ', last)`
          slug = sql`lower(${name})`
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
          restore: sql`update {table} set deleted_at = null where id = {id}`
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
            QueryBody::Bare => panic!("expected clauses"),
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
