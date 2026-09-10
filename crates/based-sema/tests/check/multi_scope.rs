use super::*;

// ---------- multi-scope DNF: alternatives, E0185, E0186  --------------

#[test]
fn or_model_query_injects_only_its_named_alternative() {
    // A model with two stacked `@scope` decorators is an OR of alternatives; each
    // query names one and injects only that axis.
    let (schema, diags) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page
        @scope Author
        @sort(created desc)
        Post {
          id: Id
          page:    Page
          author:  User
          body:    text
          created: timestamp
          @index page
          @index author
        }
        shape PostCard from Post { body }
        query posts_on_page() -> PostCard[] scoped Page   { list Post order (created desc); }
        query my_posts()      -> PostCard[] scoped Author { list Post order (created desc); }
        "#,
    );
    assert!(errors(&diags).is_empty(), "{:?}", codes(&diags));
    let by_page = schema
        .queries
        .iter()
        .find(|q| q.name == "posts_on_page")
        .unwrap();
    let by_author = schema
        .queries
        .iter()
        .find(|q| q.name == "my_posts")
        .unwrap();
    // Each query resolved a *different* alternative for the same model.
    assert_eq!(by_page.scope_inject.len(), 1);
    assert_eq!(
        by_page.scope_inject[0].terms,
        vec![("page".into(), "page".into())]
    );
    assert_eq!(by_author.scope_inject.len(), 1);
    assert_eq!(
        by_author.scope_inject[0].terms,
        vec![("author".into(), "user".into())]
    );
}

#[test]
fn and_model_naming_one_axis_is_e0185() {
    // `@scope Page, Author` is a single two-axis alternative; a callable naming just
    // `Page` doesn't ⊇ it → E0185.
    let (_, d) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page, Author
        @sort(created desc)
        Comment {
          id: Id
          page:    Page
          author:  User
          body:    text
          created: timestamp
          @index page
        }
        shape CommentCard from Comment { body }
        query my_comments() -> CommentCard[] scoped Page {
          list Comment order (created desc);
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0185"), "{:?}", codes(&d));
}

#[test]
fn and_model_naming_both_axes_is_clean() {
    let (schema, d) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page, Author
        @sort(created desc)
        Comment {
          id: Id
          page:    Page
          author:  User
          body:    text
          created: timestamp
          @index page
        }
        shape CommentCard from Comment { body }
        query my_comments() -> CommentCard[] scoped Page, Author {
          list Comment order (created desc);
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let q = schema
        .queries
        .iter()
        .find(|q| q.name == "my_comments")
        .unwrap();
    // Both axes injected (the single alternative), in decl order.
    assert_eq!(
        q.scope_inject[0].terms,
        vec![
            ("page".into(), "page".into()),
            ("author".into(), "user".into())
        ]
    );
}

#[test]
fn create_not_satisfying_any_alternative_is_e0186() {
    // A `create` on an AND model whose mutation names only one axis can satisfy no
    // alternative — the other scope column would be left unset → E0186.
    let (_, d) = analyze(
        r#"
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
        mutation add_comment(body: text) -> CommentCard scoped Page {
          create Comment { body = $body };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0186"), "{:?}", codes(&d));
}

#[test]
fn create_satisfying_an_alternative_has_no_e0186() {
    // Naming the full alternative (`scoped Page, Author`) lets the engine auto-set both
    // scope columns from $ctx — no E0186.
    let (_, d) = analyze(
        r#"
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
        "#,
    );
    assert!(!codes(&d).contains(&"E0186"), "{:?}", codes(&d));
}

#[test]
fn or_model_ctx_follows_the_chosen_alternative() {
    // The `$ctx` requirement derives from the alternative the callable *chose*,
    // not the model's first `@scope` line — sema's ctx bag must carry exactly the
    // `:ctx_<field>` binds codegen injects.
    let (schema, d) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page
        @scope Author
        @sort(created desc)
        Post {
          id: Id
          page:    Page
          author:  User
          body:    text
          created: timestamp
          @index page
          @index author
        }
        shape PostCard from Post { body }
        query my_posts() -> PostCard[] scoped Author { list Post order (created desc); }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let q = schema
        .queries
        .iter()
        .find(|q| q.name == "my_posts")
        .unwrap();
    assert_eq!(q.ctx_requires.len(), 1, "{:?}", q.ctx_requires);
    assert_eq!(q.ctx_requires[0].field, "user");
}

#[test]
fn or_model_create_naming_both_alternatives_is_clean() {
    // A create naming several alternatives auto-sets every named axis's column;
    // none of them is "missing" (E0146), and the ctx bag carries both fields.
    let (schema, d) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page
        @scope Author
        @sort(created desc)
        Post {
          id: Id
          page:    Page
          author:  User
          body:    text
          created: timestamp (default now())
          @index page
          @index author
        }
        shape PostCard from Post { body }
        query on_page() -> PostCard[] scoped Page   { list Post; }
        query mine()    -> PostCard[] scoped Author { list Post; }
        mutation write_post(body: text) -> PostCard scoped Page, Author {
          create Post { body = $body };
        }
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
    let m = schema
        .mutations
        .iter()
        .find(|m| m.name == "write_post")
        .unwrap();
    let mut fields: Vec<&str> = m.ctx_requires.iter().map(|c| c.field.as_str()).collect();
    fields.sort_unstable();
    assert_eq!(fields, ["page", "user"]);
}

#[test]
fn scoped_create_assigning_an_unchosen_alternative_column_is_e0181() {
    // Every alternative's column is engine-domain on a scoped create — assigning
    // one the callable didn't choose is still planting the row into a scope.
    let (_, d) = analyze(
        r#"
        scope Page   (page:   Page = $ctx.page)
        scope Author (author: User = $ctx.user)
        Page { id: Id, title: text }
        User { id: Id, name: text }
        @scope Page
        @scope Author
        @sort(body asc)
        Post {
          id: Id
          page:   Page
          author: User?
          body:   text
          @index page
        }
        shape PostCard from Post { body }
        query on_page() -> PostCard[] scoped Page { list Post; }
        mutation write_post(body: text, author: Id) -> PostCard scoped Page {
          create Post { body = $body, author = $author };
        }
        "#,
    );
    assert!(errors(&d).contains(&"E0181"), "{:?}", codes(&d));
}

#[test]
fn unindexed_annotation_on_an_unscoped_query_is_not_stale() {
    // An `unscoped` query injects no scope, so a scope column's index cannot make
    // its annotation "stale" — the pattern is the query's own filter only.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        @sort(created desc)
        Doc {
          id: Id
          org:     Org
          title:   text
          created: timestamp
          @index(org, title)
        }
        shape D from Doc { title }
        query docs() -> D[] scoped Tenant { list Doc; }
        query export(since: timestamp > created) -> D[]
          unscoped("audit: whole-corpus export")
          unindexed(unsafe, "deliberate full scan");
        "#,
    );
    assert!(
        !codes(&d).contains(&"W0105"),
        "annotation wrongly stale: {:?}",
        codes(&d)
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}
