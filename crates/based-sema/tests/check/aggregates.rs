use super::*;

// ---------- aggregations + group by + having (T4) --------------------------

const AGG_MODELS: &str = r#"
        Buyer { id: Id, name: text }
        @soft_delete(deleted_at)
        Order {
          id: Id
          deleted_at: timestamp?
          buyer: Buyer
          total: decimal(12, 2)
          qty: int
          note: text
        }
"#;

#[test]
fn aggregate_query_group_by_is_clean() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape BuyerStats from Order {
          who = buyer
          orders = count()
          revenue = sum(total)
          avg_qty = avg(qty)
          biggest = max(total)
        }
        query buyer_stats() -> BuyerStats[] {
          list Order group by (buyer) having (revenue > 100) order (revenue desc);
        }
        "#,
    );
    assert_clean(&src);
}

#[test]
fn global_aggregate_get_is_clean() {
    // No group by, all-aggregate shape → one whole-table row (a `get`), not
    // flagged as an unkeyed get.
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape OrderTotals from Order { orders = count(), revenue = sum(total) }
        query order_totals() -> OrderTotals { get Order; }
        "#,
    );
    assert_clean(&src);
}

#[test]
fn sum_of_text_column_is_e0241() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Bad from Order { who = buyer, x = sum(note) }
        query bad() -> Bad[] { list Order group by (buyer); }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0241"), "{:?}", codes(&d));
}

#[test]
fn count_with_argument_is_e0240() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Bad from Order { who = buyer, n = count(total) }
        query bad() -> Bad[] { list Order group by (buyer); }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0240"), "{:?}", codes(&d));
}

#[test]
fn unknown_aggregate_is_e0240() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Bad from Order { who = buyer, n = median(total) }
        query bad() -> Bad[] { list Order group by (buyer); }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0240"), "{:?}", codes(&d));
}

#[test]
fn ungrouped_projected_column_is_e0242() {
    // `note` is projected but not aggregated and not in `group by`.
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Bad from Order { who = buyer, note, orders = count() }
        query bad() -> Bad[] { list Order group by (buyer); }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0242"), "{:?}", codes(&d));
}

#[test]
fn having_reference_not_projected_is_e0242() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Stats from Order { who = buyer, orders = count() }
        query bad() -> Stats[] { list Order group by (buyer) having (revenue > 10); }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0242"), "{:?}", codes(&d));
}

#[test]
fn group_by_on_non_aggregate_query_is_e0243() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Plain from Order { note }
        query bad() -> Plain[] { list Order group by (buyer); }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0243"), "{:?}", codes(&d));
}

#[test]
fn page_on_aggregate_query_is_e0244() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Stats from Order { who = buyer, orders = count() }
        query bad() -> Stats[] { list Order group by (buyer) page (20); }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0244"), "{:?}", codes(&d));
}

#[test]
fn aggregate_shape_nested_is_e0245() {
    // An aggregate shape must be flat.
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Bad from Order { orders = count(), buyer { name } }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0245"), "{:?}", codes(&d));
}

#[test]
fn aggregate_shape_as_mutation_return_is_e0245() {
    let mut src = AGG_MODELS.to_string();
    src.push_str(
        r#"
        shape Stats from Order { orders = count() }
        mutation touch(id: Id) -> Stats { update Order where (id = $id) { qty = 1 }; }
        "#,
    );
    let (_, d) = analyze(&src);
    assert!(errors(&d).contains(&"E0245"), "{:?}", codes(&d));
}
