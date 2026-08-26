//! Dialect-gated construct checks (`ordered_nest_diagnostics`): an ordered to-many
//! nest / m2m flatten cannot be generated for MySQL (its `JSON_ARRAYAGG` rejects
//! `ORDER BY`), so `check` must error for that target and stay quiet for the others.

use based_ast::FileId;
use based_codegen::{ordered_nest_diagnostics, Dialect};
use based_parser::parse_file;
use based_sema::{check, expand_spreads};

fn codes_for(src: &str, dialect: Dialect) -> Vec<&'static str> {
    let mut sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let _ = expand_spreads(&mut sf.decls);
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    ordered_nest_diagnostics(&schema, &sf.decls, dialect)
        .iter()
        .map(|d| d.code)
        .collect()
}

// A post whose comments (ordered by the child model's `@sort`) are projected as a
// to-many nest. The `@index(post)` covers the nest's correlation join (else `E0260`).
const ORDERED_NEST: &str = r#"
    @sort(created_at asc)
    Comment { id: Id, body: text, created_at: timestamp, post: Post, @index(post) }
    Post { id: Id, comments: Comment[] (Comment.post) }
    shape PostDetail from Post { id, comments { id, body } }
    query get_post(id) -> PostDetail;
"#;

#[test]
fn ordered_to_many_nest_errors_on_mysql() {
    assert_eq!(codes_for(ORDERED_NEST, Dialect::MySql), vec!["E0350"]);
}

#[test]
fn ordered_to_many_nest_clean_on_other_dialects() {
    for d in [Dialect::MariaDb, Dialect::Postgres, Dialect::Sqlite] {
        assert!(
            codes_for(ORDERED_NEST, d).is_empty(),
            "{d:?} should permit an ordered nest"
        );
    }
}

#[test]
fn unordered_to_many_nest_clean_on_mysql() {
    // No `@sort` at either tier — the nest lowers to an unordered aggregate MySQL can emit.
    let src = r#"
        Comment { id: Id, body: text, post: Post, @index(post) }
        Post { id: Id, comments: Comment[] (Comment.post) }
        shape PostDetail from Post { id, comments { id, body } }
        query get_post(id) -> PostDetail;
    "#;
    assert!(codes_for(src, Dialect::MySql).is_empty());
}

#[test]
fn to_one_nest_never_errors_on_mysql() {
    // A to-one relation nests a single object (no aggregate), even reaching a model
    // that declares a `@sort`.
    let src = r#"
        @sort(name asc)
        User { id: Id, name: text }
        Post { id: Id, author: User }
        shape PostDetail from Post { id, author { id, name } }
        query get_post(id) -> PostDetail;
    "#;
    assert!(codes_for(src, Dialect::MySql).is_empty());
}

#[test]
fn ordered_nest_ref_errors_on_mysql() {
    // The `field -> Shape` named-nest form must be caught too.
    let src = r#"
        @sort(created_at asc)
        Comment { id: Id, body: text, created_at: timestamp, post: Post, @index(post) }
        Post { id: Id, comments: Comment[] (Comment.post) }
        shape CommentRow from Comment { id, body }
        shape PostDetail from Post { id, comments -> CommentRow }
        query get_post(id) -> PostDetail;
    "#;
    assert_eq!(codes_for(src, Dialect::MySql), vec!["E0350"]);
}

#[test]
fn ordered_m2m_flatten_errors_on_mysql() {
    // A far-side flatten aggregates the far rows ordered by the far model's `@sort`.
    let src = r#"
        @sort(name asc)
        Tag { id: Id, name: text }
        PostTag { id: Id, post: Post, tag: Tag, @index(post), @index(tag) }
        Post { id: Id, links: PostTag[] (PostTag.post) }
        shape PostDetail from Post { id, tags = links.tag { id, name } }
        query get_post(id) -> PostDetail;
    "#;
    assert_eq!(codes_for(src, Dialect::MySql), vec!["E0350"]);
}
