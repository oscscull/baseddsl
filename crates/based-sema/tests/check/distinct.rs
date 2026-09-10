use super::*;

// ---------- `list distinct` ------------------------------------------------

#[test]
fn distinct_list_projecting_non_key_is_clean() {
    let (_, diags) = analyze(
        r#"
        City { id: Id, name: text, region: text }
        shape RegionName from City { region }
        query regions() -> RegionName[] { list distinct City order (region); }
        "#,
    );
    // no distinct-related diagnostics: projects only `region`, ordered by a projected col.
    for c in ["E0310", "E0311", "E0312", "W0111"] {
        assert!(
            !codes(&diags).contains(&c),
            "unexpected {c}: {:?}",
            codes(&diags)
        );
    }
}

#[test]
fn distinct_on_aggregate_query_is_e0311() {
    let (_, diags) = analyze(
        r#"
        City { id: Id, name: text, region: text }
        shape RegionCount from City { region, n = count() }
        query counts() -> RegionCount[] {
          list distinct City group by (region);
        }
        "#,
    );
    assert!(errors(&diags).contains(&"E0311"), "{:?}", codes(&diags));
}

#[test]
fn distinct_with_keyset_page_is_e0310() {
    let (_, diags) = analyze(
        r#"
        City { id: Id, name: text, region: text }
        shape RegionName from City { region }
        query regions() -> RegionName[] { list distinct City order (region) page (20); }
        "#,
    );
    assert!(errors(&diags).contains(&"E0310"), "{:?}", codes(&diags));
}

#[test]
fn distinct_order_by_unprojected_column_is_e0312() {
    let (_, diags) = analyze(
        r#"
        City { id: Id, name: text, region: text }
        shape RegionName from City { region }
        query regions() -> RegionName[] { list distinct City order (name); }
        "#,
    );
    assert!(errors(&diags).contains(&"E0312"), "{:?}", codes(&diags));
}

#[test]
fn distinct_on_bare_model_return_is_noop_w0111() {
    let (_, diags) = analyze(
        r#"
        City { id: Id, name: text, region: text }
        query all_cities() -> City[] { list distinct City order (name); }
        "#,
    );
    assert!(codes(&diags).contains(&"W0111"), "{:?}", codes(&diags));
}

#[test]
fn distinct_projecting_primary_key_is_noop_w0111() {
    let (_, diags) = analyze(
        r#"
        City { id: Id, name: text, region: text }
        shape CityRow from City { id, region }
        query rows() -> CityRow[] { list distinct City order (region); }
        "#,
    );
    assert!(codes(&diags).contains(&"W0111"), "{:?}", codes(&diags));
}
