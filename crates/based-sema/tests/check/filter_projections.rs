use super::*;

// ---------- named filters projected as bool shape fields ----------

const MODELS: &str = r#"
        enum TaskStatus { waiting, done }
        User { id: Id, name: text, active: bool, tasks: Task[] }
        Task {
          id:     Id
          status: TaskStatus
          since:  timestamp
          owner:  User
          @index status
        }
        filter overdue(before) = status = waiting and since <= $before;
        filter active_owner = owner.active = true;
        filter mine = owner = $ctx.user;
"#;

fn diags(body: &str) -> (CheckedSchema, Vec<Diagnostic>) {
    let mut src = MODELS.to_string();
    src.push_str(body);
    analyze(&src)
}

/// Error codes, minus the index-coverage lint these snippets don't care about.
fn errs(d: &[Diagnostic]) -> Vec<&str> {
    errors(d).into_iter().filter(|c| *c != "E0260").collect()
}

#[test]
fn filter_projection_bound_by_query_param_is_clean() {
    let (_, d) = diags(
        r#"
        shape TaskCard from Task {
          id
          is_overdue = overdue($before)
          owner_ok   = active_owner()
        }
        query due(before: timestamp) -> TaskCard[] { list Task where (overdue($before)); }
        "#,
    );
    assert!(errs(&d).is_empty(), "{:?}", codes(&d));
}

#[test]
fn shape_param_missing_on_query_is_e0355() {
    let (_, d) = diags(
        r#"
        shape TaskCard from Task { id, is_overdue = overdue($before) }
        query all() -> TaskCard[] { list Task; }
        "#,
    );
    assert!(errs(&d).contains(&"E0355"), "{:?}", codes(&d));
}

#[test]
fn shape_param_type_mismatch_is_e0355() {
    let (_, d) = diags(
        r#"
        shape TaskCard from Task { id, is_overdue = overdue($before) }
        query due(before: int) -> TaskCard[] { list Task; }
        "#,
    );
    assert!(errs(&d).contains(&"E0355"), "{:?}", codes(&d));
}

#[test]
fn shape_param_reached_through_nested_shape_is_checked() {
    let (_, d) = diags(
        r#"
        shape TaskRef from Task { id, is_overdue = overdue($before) }
        shape UserCard from User { name, tasks -> TaskRef }
        query users() -> UserCard[] { list User; }
        "#,
    );
    assert!(errs(&d).contains(&"E0355"), "{:?}", codes(&d));
}

#[test]
fn mutation_returning_shape_binds_its_params() {
    let (_, d) = diags(
        r#"
        shape TaskCard from Task { id, is_overdue = overdue($before) }
        mutation finish(id) -> TaskCard { update Task where (id = $id) { status = done }; }
        "#,
    );
    assert!(errs(&d).contains(&"E0355"), "{:?}", codes(&d));
}

#[test]
fn unknown_filter_and_arity_in_shape() {
    let (_, d) = diags(
        r#"
        shape A from Task { id, x = nope($before) }
        shape B from Task { id, y = overdue() }
        "#,
    );
    let e = errs(&d);
    assert!(e.contains(&"E0114"), "{:?}", codes(&d));
    assert!(e.contains(&"E0115"), "{:?}", codes(&d));
}

#[test]
fn filter_projection_in_aggregate_shape_is_e0324() {
    let (_, d) = diags(
        r#"
        shape Counts from Task { status, n = count(), late = overdue($before) }
        "#,
    );
    assert!(errs(&d).contains(&"E0324"), "{:?}", codes(&d));
}

#[test]
fn filter_projection_ctx_read_is_required() {
    let (schema, d) = diags(
        r#"
        shape TaskCard from Task { id, is_mine = mine() }
        query all() -> TaskCard[] { list Task; }
        "#,
    );
    assert!(errs(&d).is_empty(), "{:?}", codes(&d));
    let q = schema.queries.iter().find(|q| q.name == "all").unwrap();
    assert!(
        q.ctx_requires.iter().any(|r| r.field == "user"),
        "{:?}",
        q.ctx_requires
    );
}
