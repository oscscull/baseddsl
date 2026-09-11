use super::*;

// ---------- queries / mutations -------------------------------------------

#[test]
fn get_must_be_keyed_on_unique() {
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, @index name }
        shape P from Product { name }
        query by_name(name) -> P;
        "#,
    );
    assert_eq!(errors(&d), ["E0144"]);
}

#[test]
fn get_on_unique_column_ok() {
    assert_clean(
        r#"
        Product { id: Id, sku: text (unique) }
        shape P from Product { sku }
        query by_sku(sku) -> P;
        "#,
    );
}

#[test]
fn explicit_query_verb_must_match_return_cardinality() {
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text }
        shape P from Product { name }
        query scalar(id) -> P { list Product where (id = $id); }
        query collection(id) -> P[] { get Product where (id = $id); }
        "#,
    );
    assert_eq!(errors(&d), ["E0203", "E0203"]);
}

#[test]
fn unknown_return_type() {
    let (_, d) = analyze("query q(id) -> Nope;");
    assert_eq!(errors(&d), ["E0140"]);
}

#[test]
fn edge_binding_must_be_relation() {
    let (_, d) = analyze(
        r#"
        Product { id: Id, name: text, @index name }
        shape P from Product { name }
        query q(x -> name) -> P[];
        "#,
    );
    assert_eq!(errors(&d), ["E0143"]);
}

#[test]
fn restore_requires_soft_delete() {
    let (_, d) = analyze(
        r#"
        Doc { id: Id, name: text }
        shape D from Doc { name }
        mutation undo(id: Id) -> D { restore Doc where (id = $id); }
        "#,
    );
    assert_eq!(errors(&d), ["E0145"]);
}

#[test]
fn mutation_create_unknown_column() {
    let (_, d) = analyze(
        r#"
        Doc { id: Id, name: text? }
        shape D from Doc { name }
        mutation make(t: text) -> D { create Doc { nope = $t }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0111"]);
}

#[test]
fn mutation_create_missing_required_field() {
    // `title` is a non-optional, non-defaulted column, so a create that omits it
    // is `E0146`. `id`/soft-delete/`@created`/`@updated` are engine-set, exempt.
    let (_, d) = analyze(
        r#"
        @soft_delete(deleted_at)
        @created(created_at)
        Doc { id: Id, deleted_at: timestamp?, created_at: timestamp, title: text, note: text? }
        shape D from Doc { title }
        mutation make(n: text) -> D { create Doc { note = $n }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0146"]);
}

#[test]
fn mutation_create_missing_required_relation_fk() {
    // A non-optional forward relation must have its FK set on create.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, title: text }
        shape D from Doc { title }
        mutation make(t: text) -> D { create Doc { title = $t }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0146"]);
}

#[test]
fn mutation_create_defaulted_and_optional_not_required() {
    // Defaulted, optional, and engine-managed columns need no assignment.
    assert_clean(
        r#"
        @soft_delete(deleted_at)
        @updated(updated_at)
        Doc {
          id: Id
          deleted_at: timestamp?
          updated_at: timestamp
          title: text
          status: text (default "draft")
          note: text?
        }
        shape D from Doc { title }
        mutation make(t: text) -> D { create Doc { title = $t }; }
        "#,
    );
}

#[test]
fn mutation_create_wrong_literal_type_rejected() {
    // `count` is an int column; assigning a text literal is `E0153` — the write-side
    // twin of the `=` operand-typing on the read side.
    let (_, d) = analyze(
        r#"
        Doc { id: Id, count: int, title: text }
        shape D from Doc { title }
        mutation make(t: text) -> D { create Doc { count = "lots", title = $t }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0153"]);
}

#[test]
fn mutation_update_wrong_column_type_rejected() {
    // Assigning one column to another of an incompatible family (text ← int).
    let (_, d) = analyze(
        r#"
        Doc { id: Id, count: int, title: text }
        shape D from Doc { title }
        mutation rename(id: Id) -> D { update Doc where (id = $id) { title = count }; }
        "#,
    );
    assert_eq!(errors(&d), ["E0153"]);
}

#[test]
fn mutation_assign_relation_key_is_clean() {
    // A forward FK accepts its key as a uuid string or an int ; a param or a
    // matching literal is fine. Correct scalar types pass too.
    assert_clean(
        r#"
        Org { id: Id, name: text }
        Doc { id: Id, org: Org, count: int, title: text }
        shape D from Doc { title }
        mutation make(o: Org, t: text) -> D {
          create Doc { org = $o, count = 3, title = $t };
        }
        "#,
    );
}

#[test]
fn tx_step_ref_type_mismatch_rejected() {
    // `$batch.count` reads an int off the bound create; assigning it to a text column
    // is a family clash (`E0153`), typed through the binding reference.
    let (_, d) = analyze(
        r#"
        Batch { id: Id, count: int }
        Doc { id: Id, label: text, batch: Batch }
        shape D from Doc { label }
        mutation run(n: int) -> D {
          tx {
            create Batch { count = $n } as batch;
            create Doc { label = $batch.count, batch = $batch.id };
          }
        }
        "#,
    );
    assert_eq!(errors(&d), ["E0153"]);
}
