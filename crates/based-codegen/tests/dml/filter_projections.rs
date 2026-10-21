use super::*;

// ---------- named filters projected as bool shape fields ----------

const SCHEMA: &str = r#"
    enum TaskStatus { waiting, done }
    User { id: Id, name: text, active: bool, tasks: Task[] }
    Task {
      id:     Id
      status: TaskStatus
      since:  timestamp
      owner:  User
    }
    filter overdue(before) = status = waiting and since <= $before;
    filter active_owner = owner.active = true;
    shape TaskCard from Task {
      id
      is_overdue = overdue($before)
      owner_ok   = active_owner()
    }
    shape OwnerCard from User { name, tasks -> TaskCard }
    shape TaskOwner from Task { id, owner -> UserFlag }
    shape UserFlag from User { name, owner_active = active_owner_of() }
    filter active_owner_of = active = true;
    query due(before: timestamp) -> TaskCard[] { list Task where (overdue($before)); }
    query owners(before: timestamp) -> OwnerCard[] { list User; }
    query with_owner() -> TaskOwner[] { list Task; }
"#;

#[test]
fn filter_projection_is_a_null_safe_bool_per_dialect() {
    let pg = query_section(&gen_pg(SCHEMA), "due").to_string();
    assert!(
        pg.contains(
            r#"CASE WHEN (("task"."status" = 'waiting' AND "task"."since" <= :before)) THEN TRUE ELSE FALSE END AS "is_overdue""#
        ),
        "\n{pg}"
    );
    // The same filter still narrows the rows.
    assert!(pg.contains("WHERE"), "\n{pg}");
    let maria = query_section(&gen(SCHEMA), "due").to_string();
    assert!(
        maria.contains("THEN TRUE ELSE FALSE END AS `is_overdue`"),
        "\n{maria}"
    );
    let lite = query_section(&gen_for(SCHEMA, Dialect::Sqlite), "due").to_string();
    assert!(
        lite.contains("THEN 1 ELSE 0 END AS `is_overdue`"),
        "\n{lite}"
    );
}

#[test]
fn zero_arg_filter_projection_reaches_through_a_join() {
    let pg = query_section(&gen_pg(SCHEMA), "due").to_string();
    assert!(pg.contains(r#"JOIN "user""#), "\n{pg}");
    assert!(pg.contains(r#"END AS "owner_ok""#), "\n{pg}");
}

#[test]
fn filter_projection_in_nested_to_many_binds_the_query_param() {
    let pg = query_section(&gen_pg(SCHEMA), "owners").to_string();
    assert!(pg.contains(":before"), "\n{pg}");
    assert!(pg.contains("'is_overdue', CASE WHEN"), "\n{pg}");
}

#[test]
fn filter_projection_in_nested_to_one_resolves_against_the_joined_row() {
    let pg = query_section(&gen_pg(SCHEMA), "with_owner").to_string();
    assert!(pg.contains(r#"END AS "owner.owner_active""#), "\n{pg}");
    // The filter's column is the joined owner's, not the root task's.
    assert!(!pg.contains(r#""task"."active""#), "\n{pg}");
}
