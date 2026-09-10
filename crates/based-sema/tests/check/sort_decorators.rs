use super::*;

// ---------- misplaced field-level `@sort` (E0348) ---------------------------

#[test]
fn sort_on_scalar_field_is_e0348() {
    // A field `@sort` after a scalar column is silently consumed by the parser as that
    // field's relation sort — then dropped, since a scalar has no collection to order.
    // It must error, not vanish.
    let (_, d) =
        analyze("Post { id: Id, title: text, created_at: timestamp @sort(created_at desc) }");
    assert!(errors(&d).contains(&"E0348"), "{:?}", codes(&d));
}

#[test]
fn sort_on_to_one_relation_is_e0348() {
    // A to-one relation nests a single sub-object — nothing to order.
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        Post { id: Id, author: User @sort(name asc) }
        "#,
    );
    assert!(errors(&d).contains(&"E0348"), "{:?}", codes(&d));
}

#[test]
fn sort_on_to_many_relation_is_clean() {
    // The one valid field-`@sort` position: a to-many edge orders its nested array.
    let (_, d) = analyze(
        r#"
        Post { id: Id, author: User }
        User { id: Id, name: text, posts: Post[] (Post.author) @sort(id desc) }
        "#,
    );
    assert!(!errors(&d).contains(&"E0348"), "{:?}", codes(&d));
}

#[test]
fn model_before_sort_is_clean() {
    // The model-wide default: `@sort` before the model, not inside its body.
    assert_clean("@sort(created_at desc)\nPost { id: Id, created_at: timestamp }");
}
