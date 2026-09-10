use super::*;

// ---------- multi-scope DNF: per-callable alternative injection  -------

/// An OR model (`@scope Page` + `@scope Author`, two stacked decorators = two
/// alternatives): a query naming `Page` injects `page = $ctx.page`; a query naming
/// `Author` injects `author = $ctx.user`. The *same model* is filtered by a *different*
/// predicate per callable.
#[test]
fn or_scope_injects_the_callable_chosen_alternative() {
    let ddl = gen(r#"
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
        }
        shape PostCard from Post { body }
        query posts_on_page() -> PostCard[] scoped Page   { list Post order (created desc); }
        query my_posts()      -> PostCard[] scoped Author { list Post order (created desc); }
        "#);
    let by_page = query_section(&ddl, "posts_on_page");
    let by_author = query_section(&ddl, "my_posts");
    // Each query injects only its chosen alternative's axis — never the other's.
    assert!(
        by_page.contains("WHERE `post`.`page_id` = :ctx_page"),
        "\n{by_page}"
    );
    assert!(!by_page.contains("author_id"), "\n{by_page}");
    assert!(
        by_author.contains("WHERE `post`.`author_id` = :ctx_user"),
        "\n{by_author}"
    );
    assert!(!by_author.contains("page_id"), "\n{by_author}");
}

/// An AND model (`@scope Page, Author`, one decorator = one two-axis alternative): a
/// callable naming both axes injects *both* equalities, ANDed, into the read `WHERE`.
#[test]
fn and_scope_injects_both_axes() {
    let ddl = gen(r#"
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
        }
        shape CommentCard from Comment { body }
        query my_comments() -> CommentCard[] scoped Page, Author {
          list Comment order (created desc);
        }
        "#);
    let sec = query_section(&ddl, "my_comments");
    assert!(
        sec.contains("WHERE `comment`.`page_id` = :ctx_page AND `comment`.`author_id` = :ctx_user"),
        "\n{sec}"
    );
}
