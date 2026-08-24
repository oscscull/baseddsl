//! End-to-end proof of composite (multi-column) primary keys `@key(f1, f2, …)` against a
//! **real** SQLite engine — no mock.
//!
//! A composite-key model has no surrogate `id`: two-or-more declared columns *are* the
//! primary key. A relation into a composite-key model is a multi-column foreign key that
//! auto-expands to `<field>_<part>` columns. This runs the verbatim `based gen sql` DDL +
//! the runtime's lowered DML against a live in-memory SQLite database, so a passing run
//! means the whole composite-key path — composite `PRIMARY KEY`, composite create +
//! read-back, get-by-composite-key, an inbound multi-column FK write, and a to-many nest
//! correlated on the parent's composite key — actually executes.

#![cfg(feature = "sqlite")]

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen, SqliteBackend};
use based_sema::check;

const SCHEMA: &str = r#"
@key(code)
Course { code: text, title: text }

@key(sid)
Student { sid: text, name: text }

@key(course, student)
Enrollment {
  course:  Course
  student: Student
  grade:   int
  sessions: Session[]
}

Session { id: Id, enrollment: Enrollment, note: text, @index enrollment }

shape CourseCard  from Course  { code, title }
shape StudentCard from Student { sid, name }
shape EnrollmentCard from Enrollment { course = course.code, student = student.sid, grade }
shape EnrollmentWithSessions from Enrollment {
  course = course.code, student = student.sid, sessions { note }
}
shape SessionCard from Session { note, enrollment }

mutation create_course(code, title) -> CourseCard {
  create Course { code = $code, title = $title };
}
mutation create_student(sid, name) -> StudentCard {
  create Student { sid = $sid, name = $name };
}
mutation enroll(course -> course, student -> student, grade, note) -> EnrollmentCard {
  tx {
    create Enrollment { course = $course, student = $student, grade = $grade } as e;
    create Session { enrollment = $e, note = $note };
  }
}
query enrollment_by_key(course, student) -> EnrollmentCard;
query enrollment_sessions(course, student) -> EnrollmentWithSessions;
query sessions() -> SessionCard[];
"#;

async fn backend() -> (Compiled, SqliteBackend) {
    let sf = parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error),
        "schema should check clean: {diags:?}"
    );
    let ddl = sql::ddl(&schema, Dialect::Sqlite);
    let compiled = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("composite-key DDL failed to execute: {e:?}\n{ddl}"));
    (compiled, backend)
}

async fn call(
    compiled: &Compiled,
    backend: &SqliteBackend,
    path: &str,
    args: serde_json::Value,
) -> based_runtime::WireResponse {
    let ids = SeqIdGen::default();
    dispatch(
        compiled,
        backend,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        path,
        args,
        json!({}),
        None,
    )
    .await
}

#[tokio::test]
async fn composite_key_create_get_fk_and_nest() {
    let (c, backend) = backend().await;

    // Parents with natural single-column keys, so the composite key parts are known ids.
    call(
        &c,
        &backend,
        "/m/create_course",
        json!({ "code": "CS101", "title": "Intro" }),
    )
    .await;
    call(
        &c,
        &backend,
        "/m/create_student",
        json!({ "sid": "s1", "name": "Ada" }),
    )
    .await;

    // Composite create: the Enrollment row is keyed on (course_id, student_id) and read back
    // on that whole tuple (no surrogate id). The same mutation writes a Session whose
    // multi-column FK references the new enrollment's composite key.
    let made = call(
        &c,
        &backend,
        "/m/enroll",
        json!({ "course": "CS101", "student": "s1", "grade": 90, "note": "kickoff" }),
    )
    .await;
    assert_eq!(made.status, 200, "{:?}", made.body);
    assert_eq!(
        made.body,
        json!({ "course": "CS101", "student": "s1", "grade": 90 })
    );

    // Get-by-composite-key: the two key columns filter the row back.
    let got = call(
        &c,
        &backend,
        "/q/enrollment_by_key",
        json!({ "course": "CS101", "student": "s1" }),
    )
    .await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    assert_eq!(
        got.body,
        json!({ "course": "CS101", "student": "s1", "grade": 90 })
    );

    // To-many nest correlated on the parent's composite key: the session reads back only
    // because both FK columns (enrollment_course_id, enrollment_student_id) hold the
    // enrollment's key and the correlation ANDs both against the parent.
    let with = call(
        &c,
        &backend,
        "/q/enrollment_sessions",
        json!({ "course": "CS101", "student": "s1" }),
    )
    .await;
    assert_eq!(with.status, 200, "{:?}", with.body);
    assert_eq!(with.body["course"], json!("CS101"));
    assert_eq!(with.body["student"], json!("s1"));
    assert_eq!(with.body["sessions"], json!([{ "note": "kickoff" }]));

    // A composite FK projected bare crosses the wire as the structured id object — the two
    // FK columns reassembled into `{ course, student }`, honest to the key parts.
    let sessions = call(&c, &backend, "/q/sessions", json!({})).await;
    assert_eq!(sessions.status, 200, "{:?}", sessions.body);
    assert_eq!(
        sessions.body,
        json!([{ "note": "kickoff", "enrollment": { "course": "CS101", "student": "s1" } }])
    );
}
