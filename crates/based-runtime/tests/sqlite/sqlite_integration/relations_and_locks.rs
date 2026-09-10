use super::*;

/// **Live FK enforcement + `on_delete: cascade`** against real SQLite. Proves two things
/// end to end: (1) the SQLite `foreign_keys` pragma is on (a bad-parent insert is rejected),
/// and (2) `@fk(on_delete: cascade)` in the generated DDL actually cascades — deleting the
/// parent row removes the child. Without the pragma SQLite silently ignores the FK, so this
/// test also guards the connection-setup pragma from regressing.
#[tokio::test]
async fn fk_on_delete_cascade_is_enforced_live() {
    use based_runtime::fetch_all;

    let src = "Org { id: Id  name: text }\n\
               Order { id: Id  org: Org @fk(on_delete: cascade) }";
    let sf = parse_file(src, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(
        diags
            .iter()
            .all(|d| d.severity != based_diagnostics::Severity::Error),
        "sema errors: {diags:#?}"
    );
    let ddl = sql::ddl(&schema, Dialect::Sqlite);
    assert!(ddl.contains("ON DELETE CASCADE"), "\n{ddl}");

    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            "INSERT INTO `org` (`id`, `name`) VALUES ('o1', 'Acme');\n\
             INSERT INTO `order` (`id`, `org_id`) VALUES ('r1', 'o1');",
        )
        .await
        .expect("seed");

    // (1) The FK is enforced: a child pointing at a non-existent parent is rejected.
    let bad = backend
        .execute_batch("INSERT INTO `order` (`id`, `org_id`) VALUES ('r2', 'ghost');")
        .await;
    assert!(bad.is_err(), "FK not enforced — the pragma is off");

    // (2) Deleting the parent cascades: the child row disappears.
    let mut db = backend.checkout("").await.expect("checkout");
    db.execute(
        "DELETE FROM `org` WHERE `id` = ?",
        &[based_runtime::SqlValue::Text("o1".into())],
    )
    .await
    .expect("delete parent");
    let rows = fetch_all(db.fetch("SELECT `id` FROM `order`", &[]))
        .await
        .expect("count orders");
    assert!(
        rows.is_empty(),
        "cascade did not remove the child rows: {rows:?}"
    );
}

/// **Far-side flattening projection** (`courses = enrollments.course { title }`) end to
/// end against a real engine: the many-to-many is flattened past its junction to a flat,
/// **distinct** `Vec<Course>`. Proves (1) the junction is hidden — the response carries
/// courses, never enrollment rows; (2) a course shared by two students appears for each;
/// (3) a duplicate junction link (two enrollments, same student→course) dedups to one far
/// row; (4) a soft-deleted junction row excludes its link; (5) a soft-deleted far course
/// is excluded entirely.
#[tokio::test]
async fn far_side_flattening_projection_returns_distinct_far_rows() {
    let c = compile_sqlite(
        r#"
        @sort(id asc)
        Student { id: Id, name: text, enrollments: Enrollment[] (Enrollment.student) }
        @sort(id asc)
        @soft_delete(deleted_at)
        Enrollment { id: Id, student: Student, course: Course, deleted_at: timestamp? }
        @sort(title asc)
        @soft_delete(deleted_at)
        Course { id: Id, title: text, deleted_at: timestamp? }
        shape StudentCourses from Student { name, courses = enrollments.course { title } }
        query student_by_id(id) -> StudentCourses;
        query students() -> StudentCourses[];
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `student` (`id`, `name`) VALUES ('s1', 'Ann'), ('s2', 'Bob');
            INSERT INTO `course` (`id`, `title`, `deleted_at`) VALUES
                ('c1', 'Math', NULL), ('c2', 'Physics', NULL),
                -- a soft-deleted far course is excluded entirely.
                ('c3', 'Chemistry', '2020-01-01 00:00:00');
            INSERT INTO `enrollment` (`id`, `student_id`, `course_id`, `deleted_at`) VALUES
                ('e1', 's1', 'c1', NULL),
                -- a DUPLICATE link (s1 -> c1 again) must dedup to one Math.
                ('e2', 's1', 'c1', NULL),
                ('e3', 's1', 'c2', NULL),
                -- s1 -> c3 (Chemistry) — link is live, but the course is soft-deleted.
                ('e4', 's1', 'c3', NULL),
                -- a soft-deleted link s1 -> c2: c2 still reached via the live e3.
                ('e5', 's1', 'c2', '2020-01-01 00:00:00'),
                -- c1 (Math) shared with s2.
                ('e6', 's2', 'c1', NULL),
                -- s2 -> c2 link is soft-deleted → Physics absent for s2.
                ('e7', 's2', 'c2', '2020-01-01 00:00:00');
            "#,
        )
        .await
        .expect("seed");

    // Ann (s1): Math (deduped from e1+e2), Physics (live e3, though e5 is tombstoned);
    // Chemistry excluded (course soft-deleted). Junction hidden — flat course objects,
    // ordered by the far model's `@sort` (title asc).
    let ann = call(
        &c,
        &backend,
        "POST",
        "/q/student_by_id",
        json!({ "id": "s1" }),
        json!({}),
    )
    .await;
    assert_eq!(ann.status, 200, "{:?}", ann.body);
    assert_eq!(
        ann.body,
        json!({ "name": "Ann", "courses": [{ "title": "Math" }, { "title": "Physics" }] }),
        "distinct far courses, junction hidden, soft-deleted course + link excluded"
    );

    // Bob (s2): Math (shared c1 via e6); Physics absent (his only c2 link is tombstoned).
    let bob = call(
        &c,
        &backend,
        "POST",
        "/q/student_by_id",
        json!({ "id": "s2" }),
        json!({}),
    )
    .await;
    assert_eq!(bob.status, 200, "{:?}", bob.body);
    assert_eq!(
        bob.body,
        json!({ "name": "Bob", "courses": [{ "title": "Math" }] }),
        "shared course appears for the second student; soft-deleted link excludes Physics"
    );
}

/// On SQLite `for update nowait` and `for update skip locked` are no-ops (SQLite has no
/// row-level lock — its transaction locks the whole database), consistent with plain
/// `for update`. The modifier emits no SQL, so the query still runs and returns its rows.
#[tokio::test]
async fn for_update_wait_modes_are_a_noop_on_sqlite() {
    let c = compile_sqlite(
        r#"
        Product { id: text, sku: text, stock: int }
        shape ProductRow from Product { sku, stock }
        query available(max) -> ProductRow[] {
            list Product where (stock <= $max) order (sku) for update skip locked;
        }
        query one_nowait(id) -> ProductRow {
            get Product where (id = $id) for update nowait;
        }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&sql::ddl(&c.schema, Dialect::Sqlite))
        .await
        .expect("generated DDL");
    backend
        .execute_batch(
            "INSERT INTO `product` (`id`, `sku`, `stock`) VALUES ('p1', 'a', 0), ('p2', 'b', 0);",
        )
        .await
        .expect("seed");

    let skip = call(
        &c,
        &backend,
        "POST",
        "/q/available",
        json!({ "max": 100 }),
        json!({}),
    )
    .await;
    assert_eq!(
        skip.status, 200,
        "`skip locked` no-op runs: {:?}",
        skip.body
    );
    assert_eq!(
        skip.body,
        json!([{ "sku": "a", "stock": 0 }, { "sku": "b", "stock": 0 }])
    );

    let nowait = call(
        &c,
        &backend,
        "POST",
        "/q/one_nowait",
        json!({ "id": "p1" }),
        json!({}),
    )
    .await;
    assert_eq!(nowait.status, 200, "`nowait` no-op runs: {:?}", nowait.body);
    assert_eq!(nowait.body, json!({ "sku": "a", "stock": 0 }));
}
