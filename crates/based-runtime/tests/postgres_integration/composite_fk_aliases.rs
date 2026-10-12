//! A multi-column FK on per-part `(column part = "…")` aliases — quoted, listed out of key
//! order — round-trips through create, get, a to-many nest and a bare projection on live
//! Postgres.

use super::*;

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

Session {
  id: Id
  enrollment: Enrollment (column student = "ses student", course = "ses\"course")
  note: text
  @index enrollment
}

shape EnrollmentCard from Enrollment { course = course.code, student = student.sid, grade }
shape EnrollmentWithSessions from Enrollment {
  course = course.code, student = student.sid, sessions { note }
}
shape SessionCard from Session { note, enrollment }

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

#[tokio::test]
async fn aliased_composite_fk_columns_round_trip_live_postgres() {
    let Some((c, router, container)) = live_schema(SCHEMA).await else {
        return;
    };
    let ddl = sql::ddl(&c.schema, Dialect::Postgres);
    assert!(ddl.contains(r#"("ses""course", "ses student")"#), "\n{ddl}");
    container
        .exec_batch(
            "INSERT INTO \"course\" (\"code\", \"title\") VALUES ('CS101', 'Intro');\n\
             INSERT INTO \"student\" (\"sid\", \"name\") VALUES ('s1', 'Ada');",
        )
        .await;

    let made = call(
        &c,
        &router,
        "POST",
        "/m/enroll",
        json!({ "course": "CS101", "student": "s1", "grade": 90, "note": "kickoff" }),
        json!({}),
    )
    .await;
    assert_eq!(made.status, 200, "{:?}", made.body);
    assert_eq!(
        made.body,
        json!({ "course": "CS101", "student": "s1", "grade": 90 })
    );

    let got = call(
        &c,
        &router,
        "POST",
        "/q/enrollment_by_key",
        json!({ "course": "CS101", "student": "s1" }),
        json!({}),
    )
    .await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    assert_eq!(got.body, made.body);

    let with = call(
        &c,
        &router,
        "POST",
        "/q/enrollment_sessions",
        json!({ "course": "CS101", "student": "s1" }),
        json!({}),
    )
    .await;
    assert_eq!(with.status, 200, "{:?}", with.body);
    assert_eq!(
        with.body,
        json!({ "course": "CS101", "student": "s1", "sessions": [{ "note": "kickoff" }] })
    );

    let sessions = call(&c, &router, "POST", "/q/sessions", json!({}), json!({})).await;
    assert_eq!(sessions.status, 200, "{:?}", sessions.body);
    assert_eq!(
        sessions.body,
        json!([{ "note": "kickoff", "enrollment": { "course": "CS101", "student": "s1" } }])
    );
}
