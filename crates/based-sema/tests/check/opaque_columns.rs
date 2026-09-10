use super::*;

// ---------- opaque `raw(…)` columns + exotic indexes -----------------------

const PLACE: &str = r#"
    Place {
      id:       Id
      name:     text
      location: raw("geometry(Point,4326)")?
      tags:     raw({ postgres: "tsvector", mariadb: "text", sqlite: "text" })?
      @index name
      @index location using gist
      @index raw("(lower(name))")
    }

    shape PlaceRow from Place {
      id
      name
      location
      area = raw`ST_Area(location)`
    }

    query place(id) -> PlaceRow;
    query by_name(name) -> PlaceRow[] order (name);
    "#;

#[test]
fn opaque_column_projects_and_indexes_clean() {
    assert_clean(PLACE);
    let (schema, _) = analyze(PLACE);
    let place = schema.model("Place").unwrap();
    let loc = place.member("location").unwrap();
    let spec = loc.kind.opaque().expect("opaque column");
    assert_eq!(spec.for_dialect("mariadb"), Some("geometry(Point,4326)"));
    // Ordinary columns stay unmarked.
    assert!(place.member("name").unwrap().kind.opaque().is_none());
    assert_eq!(place.indexes[1].method.as_deref(), Some("gist"));
    assert!(place.indexes[2].raw.is_some());
}

#[test]
fn filtering_sorting_or_grouping_an_opaque_column_errors() {
    let (_, d) = analyze(
        r#"
        Place { id: Id, location: raw("geometry")? }
        shape Row from Place { id }
        query q() -> Row[] { list Place where (location = "x") unindexed(unsafe); }
        query s() -> Row[] { list Place order (location) unindexed(unsafe); }
        "#,
    );
    assert_eq!(errors(&d), vec!["E0271", "E0271"]);
}

#[test]
fn writing_an_opaque_column_errors() {
    let (_, d) = analyze(
        r#"
        Place { id: Id, location: raw("geometry")? }
        shape Row from Place { id }
        mutation m(v) -> Row { create Place { location = $v } }
        "#,
    );
    assert_eq!(errors(&d), vec!["E0273"]);
}

#[test]
fn a_required_opaque_column_makes_create_impossible() {
    let (_, d) = analyze(
        r#"
        Place { id: Id, name: text, location: raw("geometry") }
        shape Row from Place { id }
        mutation m(n) -> Row { create Place { name = $n } }
        "#,
    );
    // E0273 replaces the ordinary "missing required field" (E0146) — there is no
    // value the caller could have supplied.
    assert_eq!(errors(&d), vec!["E0273"]);
}

#[test]
fn empty_raw_body_and_unknown_method_error() {
    let (_, d) = analyze(r#"Place { id: Id, a: raw("")?, @index raw("") @index a using nope }"#);
    let mut got = errors(&d);
    got.sort_unstable();
    assert_eq!(got, vec!["E0272", "E0274", "E0274"]);
}

#[test]
fn a_raw_map_naming_an_unknown_dialect_errors() {
    let (_, d) = analyze(r#"Place { id: Id, a: raw({ oracle: "clob" })? }"#);
    assert_eq!(errors(&d), vec!["E0270"]);
}

#[test]
fn check_target_reports_the_missing_dialect_and_unavailable_method() {
    let (schema, _) = analyze(PLACE);
    // The map covers all three targets and `gist` is a Postgres method.
    assert!(based_sema::check_target(&schema, "postgres").is_empty());
    let sqlite = based_sema::check_target(&schema, "sqlite");
    assert_eq!(
        codes(&sqlite),
        vec!["E0272"],
        "sqlite has no access methods"
    );

    let (partial, _) = analyze(r#"Place { id: Id, a: raw({ postgres: "tsvector" })? }"#);
    assert_eq!(
        codes(&based_sema::check_target(&partial, "mariadb")),
        vec!["E0270"]
    );
    assert!(based_sema::check_target(&partial, "postgres").is_empty());
}
