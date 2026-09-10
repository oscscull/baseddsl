use super::*;

// ---------- optional `$ctx.<field>?` reads (auth.md Handle 1) ----------------

#[test]
fn optional_ctx_read_in_query_filter_is_allowed_and_recorded() {
    // The public-or-mine pattern: an anonymous caller (no `user` context) sees public rows; a
    // signed-in one also sees their own. The `?` marks the field optional and is recorded so.
    let (schema, diags) = analyze(
        r#"
        User { id: Id, name: text }
        Post { id: Id, author: User, visibility: text, body: text }
        shape PostCard from Post { id, body }
        query feed() -> PostCard[] {
          list Post where (author = $ctx.user? or visibility = "public");
        }
        "#,
    );
    let errs = errors(&diags);
    assert!(
        !errs.contains(&"E0339") && !errs.contains(&"E0349"),
        "optional ctx in a query filter must be allowed: {errs:?}"
    );
    let q = schema
        .queries
        .iter()
        .find(|q| q.name == "feed")
        .expect("feed query");
    let user = q
        .ctx_requires
        .iter()
        .find(|c| c.field == "user")
        .expect("ctx.user requirement");
    assert!(user.optional, "`$ctx.user?` must be recorded as optional");
}

#[test]
fn optional_ctx_in_scope_term_is_rejected() {
    // Absent-means-widen would silently unfilter a scope — E0339.
    let (_, diags) = analyze(
        r#"
        scope Tenant (org: Org = $ctx.org?)
        Org { id: Id, name: text }
        @scope Tenant
        Doc { id: Id, org: Org, body: text }
        shape DocCard from Doc { id, body }
        query docs() -> DocCard[] scoped Tenant { list Doc; }
        "#,
    );
    assert!(errors(&diags).contains(&"E0339"), "{:?}", codes(&diags));
}

#[test]
fn optional_ctx_in_mutation_filter_is_rejected() {
    // A write filter can't widen on absence — E0339.
    let (_, diags) = analyze(
        r#"
        User { id: Id, name: text }
        Post { id: Id, author: User, body: text }
        shape PostCard from Post { id, body }
        mutation retitle() -> PostCard {
          update Post where (author = $ctx.user?) { body = "x" };
        }
        "#,
    );
    assert!(errors(&diags).contains(&"E0339"), "{:?}", codes(&diags));
}

#[test]
fn mixed_optional_and_required_ctx_in_one_query_is_rejected() {
    // `$ctx.user` read once optional, once required — E0349.
    let (_, diags) = analyze(
        r#"
        User { id: Id, name: text }
        Post { id: Id, author: User, editor: User, body: text }
        shape PostCard from Post { id, body }
        query feed() -> PostCard[] {
          list Post where (author = $ctx.user? or editor = $ctx.user);
        }
        "#,
    );
    assert!(errors(&diags).contains(&"E0349"), "{:?}", codes(&diags));
}

#[test]
fn optional_marker_on_a_non_ctx_reference_is_rejected() {
    // `?` at a use site is only for `$ctx.<field>`, never a plain param — E0339.
    let (_, diags) = analyze(
        r#"
        User { id: Id, name: text }
        Post { id: Id, author: User, body: text }
        shape PostCard from Post { id, body }
        query by_author(u: User) -> PostCard[] { list Post where (author = $u?); }
        "#,
    );
    assert!(errors(&diags).contains(&"E0339"), "{:?}", codes(&diags));
}
