use super::*;

// ---------- shapes ---------------------------------------------------------

#[test]
fn shape_bare_relation_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, org: Org }
        Org { id: Id, name: text }
        shape U from User { org }
        "#,
    );
    assert_eq!(errors(&d), ["E0130"]);
}

#[test]
fn shape_nest_scalar_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        shape U from User { name { x } }
        "#,
    );
    assert_eq!(errors(&d), ["E0131"]);
}

#[test]
fn shape_nest_and_reach_ok() {
    assert_clean(
        r#"
        User { id: Id, name: text, org: Org }
        Org { id: Id, name: text, slug: text }
        shape U from User {
          name
          city = org.name
          org { name slug }
        }
        "#,
    );
}

#[test]
fn shape_nest_ref_ok_to_one_and_to_many() {
    // A named-shape nest works on a forward (to-one) and an inverse (to-many)
    // relation, and a referenced shape may itself nest (inline or by name).
    assert_clean(
        r#"
        User { id: Id, name: text, org: Org, placed_orders: Order[] (Order.placed_by) }
        Org { id: Id, name: text, slug: text }
        Order { id: Id, placed_by: User, total: int }
        shape OrgRef from Org { name, slug }
        shape UserRef from User { name, org -> OrgRef }
        shape OrderDetail from Order {
          total
          placed_by -> UserRef
        }
        shape UserOrders from User {
          name
          placed_orders -> OrderRow
        }
        shape OrderRow from Order { total }
        query detail(id) -> OrderDetail;
        "#,
    );
}

#[test]
fn shape_nest_ref_unknown_shape() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        Order { id: Id, placed_by: User }
        shape D from Order { placed_by -> Missing }
        "#,
    );
    assert_eq!(errors(&d), ["E0132"]);
}

#[test]
fn shape_nest_ref_model_mismatch() {
    // `OrgRef` projects `Org`, but `placed_by` relates to `User` — never silent.
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        Org { id: Id, name: text }
        Order { id: Id, placed_by: User }
        shape OrgRef from Org { name }
        shape D from Order { placed_by -> OrgRef }
        "#,
    );
    assert_eq!(errors(&d), ["E0133"]);
}

#[test]
fn shape_nest_ref_on_scalar_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text }
        shape N from User { name }
        shape D from User { name -> N }
        "#,
    );
    assert_eq!(errors(&d), ["E0131"]);
}

#[test]
fn shape_nest_ref_cycle_rejected() {
    // `UserRef` -> `OrderRow` -> `UserRef` would expand forever; each decl reports
    // the reference that closes the cycle from its own root.
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text, placed_orders: Order[] (Order.placed_by) }
        Order { id: Id, placed_by: User }
        shape UserRef from User { placed_orders -> OrderRow }
        shape OrderRow from Order { placed_by -> UserRef }
        "#,
    );
    assert!(
        errors(&d).contains(&"E0134"),
        "expected a cycle error, got: {:?}",
        codes(&d)
    );
}

#[test]
fn shape_nest_ref_self_cycle_rejected() {
    let (_, d) = analyze(
        r#"
        User { id: Id, name: text, invited_by: User? }
        shape UserTree from User { name, invited_by -> UserTree }
        "#,
    );
    assert_eq!(errors(&d), ["E0134"]);
}

#[test]
fn shape_flatten_far_side_ok() {
    // `courses = enrollments.course { title }` — a to-many hop into the junction, then a
    // forward hop to the far side; the body projects the far model. Checks clean.
    assert_clean(
        r#"
        Student { id: Id, name: text, enrollments: Enrollment[] (Enrollment.student) }
        Enrollment { id: Id, student: Student, course: Course, @index (student, course) }
        Course { id: Id, title: text }
        shape StudentCourses from Student { name, courses = enrollments.course { title } }
        query student_by_id(id) -> StudentCourses;
        "#,
    );
}

#[test]
fn shape_flatten_first_segment_must_be_to_many() {
    // A forward (to-one) first segment has no junction to flatten through → E0300.
    let (_, d) = analyze(
        r#"
        Student { id: Id, name: text, school: School, @index school }
        School { id: Id, name: text }
        shape S from Student { name, x = school.name { name } }
        "#,
    );
    assert!(errors(&d).contains(&"E0300"), "{:?}", codes(&d));
}

#[test]
fn shape_flatten_later_segment_must_be_forward() {
    // After the junction hop, a non-forward segment (a scalar) is E0301.
    let (_, d) = analyze(
        r#"
        Student { id: Id, name: text, enrollments: Enrollment[] (Enrollment.student) }
        Enrollment { id: Id, student: Student, note: text, @index student }
        shape S from Student { name, x = enrollments.note { title } }
        "#,
    );
    assert!(errors(&d).contains(&"E0301"), "{:?}", codes(&d));
}

#[test]
fn shape_flatten_single_segment_has_no_far_side() {
    // A one-segment path never reaches a far side (nothing to flatten to) → E0301.
    let (_, d) = analyze(
        r#"
        Student { id: Id, name: text, enrollments: Enrollment[] (Enrollment.student) }
        Enrollment { id: Id, student: Student, note: text, @index student }
        shape S from Student { name, x = enrollments { note } }
        "#,
    );
    assert!(errors(&d).contains(&"E0301"), "{:?}", codes(&d));
}

#[test]
fn shape_flatten_keyless_far_side_rejected() {
    // A `@no_id` far model has no primary key to dedup the distinct set on → E0302.
    let (_, d) = analyze(
        r#"
        Student { id: Id, name: text, enrollments: Enrollment[] (Enrollment.student) }
        Enrollment { id: Id, student: Student, course: Course, @index (student, course) }
        @no_id("legacy view without a key")
        Course { title: text }
        shape S from Student { name, courses = enrollments.course { title } }
        "#,
    );
    assert!(errors(&d).contains(&"E0302"), "{:?}", codes(&d));
}

#[test]
fn shape_flatten_scoped_far_side_is_touched_e0185() {
    // Flattening into a scoped far side (or junction) counts as touching it: a callable
    // that doesn't satisfy that scope alternative fails at compile time.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        Region { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        scope Region (region: Region = $ctx.region)
        @scope Tenant
        Student { id: Id, org: Org, name: text, enrollments: Enrollment[] (Enrollment.student), @index org }
        @scope Tenant
        Enrollment { id: Id, org: Org, student: Student, course: Course, @index (student, course), @index org }
        @scope Region
        Course { id: Id, region: Region, title: text, @index region }
        shape S from Student { name, courses = enrollments.course { title } }
        query student_by_id(id) -> S scoped Tenant;
        "#,
    );
    assert!(errors(&d).contains(&"E0185"), "{:?}", codes(&d));

    // Naming both axes satisfies every touched model → clean.
    let (_, d) = analyze(
        r#"
        Org { id: Id, name: text }
        Region { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        scope Region (region: Region = $ctx.region)
        @scope Tenant
        Student { id: Id, org: Org, name: text, enrollments: Enrollment[] (Enrollment.student), @index org }
        @scope Tenant
        Enrollment { id: Id, org: Org, student: Student, course: Course, @index (student, course), @index org }
        @scope Region
        Course { id: Id, region: Region, title: text, @index region }
        shape S from Student { name, courses = enrollments.course { title } }
        query student_by_id(id) -> S scoped Tenant, Region;
        "#,
    );
    assert!(errors(&d).is_empty(), "{:?}", codes(&d));
}
