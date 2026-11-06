use super::*;

// ---------- operand typing --------------------------------------------------

#[test]
fn like_on_non_text_rejected() {
    // `~` needs a text column; `qty` is an int.
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, qty: int }
        shape P from Product { name }
        query q(x) -> P[] { list Product where (qty ~ "%a%") order (name); }
        "#,
    );
    assert!(errors(&d).contains(&"E0150"), "{:?}", codes(&d));
}

#[test]
fn like_on_text_ok() {
    assert_clean(
        r#"
        Product { id: Id, name: text, @index name }
        shape P from Product { name }
        query q() -> P[] { list Product where (name ~ "%a%") order (name); }
        "#,
    );
}

#[test]
fn ordering_on_relation_rejected() {
    // `<` on a relation edge is nonsense.
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, maker: Maker }
        Maker { id: Id, name: text }
        shape P from Product { name }
        query q() -> P[] { list Product where (maker < "x") order (name); }
        "#,
    );
    assert!(errors(&d).contains(&"E0150"), "{:?}", codes(&d));
}

#[test]
fn literal_type_mismatch_rejected() {
    // comparing an int column to a string literal.
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, qty: int }
        shape P from Product { name }
        query q() -> P[] { list Product where (qty = "lots") order (name); }
        "#,
    );
    assert!(errors(&d).contains(&"E0151"), "{:?}", codes(&d));
}

#[test]
fn bool_column_vs_number_rejected() {
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, active: bool }
        shape P from Product { name }
        query q() -> P[] { list Product where (active > 3) order (name); }
        "#,
    );
    // ordering on a bool column is not orderable.
    assert!(errors(&d).contains(&"E0150"), "{:?}", codes(&d));
}

#[test]
fn relation_compared_to_uuid_literal_ok() {
    // a relation edge is comparable to its key (a uuid string).
    assert_clean(
        r#"
        Product { id: Id, name: text, maker: Maker, @index maker }
        Maker { id: Id, name: text }
        shape P from Product { name }
        query q() -> P[] { list Product where (maker = "0f-uuid") order (name); }
        "#,
    );
}

#[test]
fn column_vs_column_family_mismatch_rejected() {
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, qty: int }
        shape P from Product { name }
        query q() -> P[] { list Product where (qty = name) order (name); }
        "#,
    );
    assert!(errors(&d).contains(&"E0151"), "{:?}", codes(&d));
}

#[test]
fn param_type_disagrees_with_column_rejected() {
    // `id` param annotated `int` but maps to a text column.
    let (_, d) = analyze(
        r#"
        Product { id: Id, sku: text }
        shape P from Product { sku }
        query q(sku: int) -> P[];
        "#,
    );
    assert!(errors(&d).contains(&"E0152"), "{:?}", codes(&d));
}

#[test]
fn relation_param_typed_as_model_ok() {
    assert_clean(
        r#"
        @sort(name asc)
        Product { id: Id, name: text, maker: Maker, @index maker }
        Maker { id: Id, name: text }
        shape P from Product { name }
        query by_maker(maker: Maker) -> P[];
        "#,
    );
}

#[test]
fn relation_param_typed_wrong_model_rejected() {
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, maker: Maker }
        Maker { id: Id, name: text }
        Other { id: Id, name: text }
        shape P from Product { name }
        query by_maker(maker: Other) -> P[];
        "#,
    );
    assert!(errors(&d).contains(&"E0152"), "{:?}", codes(&d));
}

#[test]
fn own_key_param_typed_as_model_ok() {
    // A model's own `id` is that model's key, so a param mapped onto it may be typed as
    // the model.
    let (_, d) = analyze(
        r#"
        Maker { id: Id, name: text }
        @key(code)
        Brand { code: text, name: text }
        shape M from Maker { name }
        shape B from Brand { name }
        query maker(id: Maker) -> M;
        query brand(code: Brand) -> B;
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn own_key_param_typed_as_other_model_rejected() {
    let (_, d) = analyze(
        r#"
        Maker { id: Id, name: text }
        Other { id: Id, name: text }
        shape M from Maker { name }
        query maker(id: Other) -> M;
        "#,
    );
    assert!(errors(&d).contains(&"E0152"), "{:?}", codes(&d));
}

#[test]
fn composite_key_part_param_typed_as_model_rejected() {
    let (_, d) = analyze(
        r#"
        Tenant { id: Id, name: text }
        @key(tenant, code)
        Member { tenant: Tenant  code: uuid  name: text }
        shape M from Member { name }
        query members(code: Member) -> M[];
        "#,
    );
    assert!(errors(&d).contains(&"E0152"), "{:?}", codes(&d));
}

#[test]
fn relation_param_typed_as_id_ok() {
    // a relation param may be typed as its key (`Id`) instead of the model.
    assert_clean(
        r#"
        @sort(name asc)
        Product { id: Id, name: text, maker: Maker, @index maker }
        Maker { id: Id, name: text }
        shape P from Product { name }
        query by_maker(maker: Id) -> P[];
        "#,
    );
}
