use super::*;

// ---------- date buckets: `date(x)` / `week(x)` ------------------------------

const BUCKET_MODELS: &str = r#"
        User {
          id: Id
          created_at: timestamp
          seen_at: timestamp?
          born_on: date
          name: text
        }
"#;

fn with(body: &str) -> String {
    format!("{BUCKET_MODELS}{body}")
}

#[test]
fn day_and_week_buckets_group_cleanly() {
    assert_clean(&with(
        r#"
        shape PerDay from User { day = date(created_at), n = count() }
        shape PerWeek from User { week = week(created_at), n = count() }
        query per_day() -> PerDay[] { list User group by (date(created_at)) order (day); }
        query per_week() -> PerWeek[] { list User group by (week(created_at)) order (week desc); }
        "#,
    ));
}

#[test]
fn bucket_of_a_date_column_is_clean() {
    assert_clean(&with(
        r#"
        shape PerBirthWeek from User { week = week(born_on), n = count() }
        query per_birth_week() -> PerBirthWeek[] { list User group by (week(born_on)); }
        "#,
    ));
}

#[test]
fn bucket_beside_a_plain_group_column_is_clean() {
    assert_clean(&with(
        r#"
        shape PerNameDay from User { name, day = date(created_at), n = count() }
        query per_name_day() -> PerNameDay[] {
          list User group by (name, date(created_at)) having (n > 1);
        }
        "#,
    ));
}

#[test]
fn bucket_in_a_row_shape_is_clean() {
    assert_clean(&with(
        r#"
        shape Joined from User { id, day = date(created_at), week = week(seen_at) }
        query joined() -> Joined[] { list User order (id); }
        "#,
    ));
}

#[test]
fn bucket_of_text_column_is_e0246() {
    let (_, d) = analyze(&with(
        r#"
        shape Bad from User { day = date(name), n = count() }
        query bad() -> Bad[] { list User group by (date(name)); }
        "#,
    ));
    assert!(errors(&d).contains(&"E0246"), "{:?}", codes(&d));
}

#[test]
fn bucket_without_argument_is_e0246() {
    let (_, d) = analyze(&with(
        r#"
        shape Bad from User { id, day = date() }
        query bad() -> Bad[] { list User; }
        "#,
    ));
    assert!(errors(&d).contains(&"E0246"), "{:?}", codes(&d));
}

#[test]
fn bucket_with_two_arguments_is_e0246() {
    let (_, d) = analyze(&with(
        r#"
        shape Bad from User { day = week(created_at, seen_at), n = count() }
        query bad() -> Bad[] { list User group by (week(created_at, seen_at)); }
        "#,
    ));
    assert!(errors(&d).contains(&"E0246"), "{:?}", codes(&d));
}

#[test]
fn group_key_bucket_of_text_is_e0246() {
    let (_, d) = analyze(&with(
        r#"
        shape Bad from User { n = count() }
        query bad() -> Bad[] { list User group by (week(name)); }
        "#,
    ));
    assert!(errors(&d).contains(&"E0246"), "{:?}", codes(&d));
}

#[test]
fn bucket_grouped_by_the_raw_column_is_e0242() {
    let (_, d) = analyze(&with(
        r#"
        shape Bad from User { day = date(created_at), n = count() }
        query bad() -> Bad[] { list User group by (created_at); }
        "#,
    ));
    assert!(errors(&d).contains(&"E0242"), "{:?}", codes(&d));
}

#[test]
fn bucket_grouped_by_the_other_bucket_is_e0242() {
    let (_, d) = analyze(&with(
        r#"
        shape Bad from User { day = date(created_at), n = count() }
        query bad() -> Bad[] { list User group by (week(created_at)); }
        "#,
    ));
    assert!(errors(&d).contains(&"E0242"), "{:?}", codes(&d));
}

#[test]
fn raw_column_grouped_by_its_bucket_is_e0242() {
    let (_, d) = analyze(&with(
        r#"
        shape Bad from User { created_at, n = count() }
        query bad() -> Bad[] { list User group by (date(created_at)); }
        "#,
    ));
    assert!(errors(&d).contains(&"E0242"), "{:?}", codes(&d));
}

#[test]
fn bucket_group_by_without_aggregate_is_e0243() {
    let (_, d) = analyze(&with(
        r#"
        shape Days from User { day = date(created_at) }
        query bad() -> Days[] { list User group by (date(created_at)); }
        "#,
    ));
    assert!(errors(&d).contains(&"E0243"), "{:?}", codes(&d));
}

#[test]
fn bucket_as_a_where_operand_is_e0246() {
    let (_, d) = analyze(&with(
        r#"
        shape Names from User { name }
        query bad() -> Names[] { list User where (born_on = date(created_at)); }
        "#,
    ));
    assert!(errors(&d).contains(&"E0246"), "{:?}", codes(&d));
}

#[test]
fn bucket_in_create_from_input_is_e0327() {
    let (_, d) = analyze(&with(
        r#"
        shape NewUser from User { name, joined = date(created_at) }
        mutation add(row: NewUser) -> ok { create User from $row; }
        "#,
    ));
    assert!(errors(&d).contains(&"E0327"), "{:?}", codes(&d));
}
