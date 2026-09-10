use super::*;

// ---------- far-side flattening projection (`courses = enrollments.course { … }`) ----

#[test]
fn flatten_skips_junction_to_distinct_far_side_all_dialects() {
    // `courses = enrollments.course { title }` flattens the many-to-many to the *distinct*
    // set of far-side courses, hiding the junction: a `FROM course WHERE course.id IN
    // (SELECT enrollment.course_id …)` — iterating far rows gives DISTINCT-on-PK for free,
    // so a duplicate junction link never multiplies. Ordered by the far model's `@sort`.
    let src = r#"
        @sort(id asc)
        Student { id: Id, name: text, enrollments: Enrollment[] (Enrollment.student) }
        @sort(id asc)
        Enrollment { id: Id, student: Student, course: Course, @index (student, course) }
        @sort(title asc)
        Course { id: Id, title: text }
        shape StudentCourses from Student { name, courses = enrollments.course { title } }
        query student_by_id(id) -> StudentCourses;
        "#;

    let lite = gen_for(src, Dialect::Sqlite);
    assert!(
        lite.contains("(SELECT json_group_array(json_object('title', `s2_course`.`title`) ORDER BY `s2_course`.`title` ASC) FROM `course` AS `s2_course` WHERE `s2_course`.`id` IN (SELECT `s1_enrollment`.`course_id` FROM `enrollment` AS `s1_enrollment` WHERE `s1_enrollment`.`student_id` = `student`.`id`)) AS `courses[]`"),
        "\n{lite}"
    );

    let maria = gen_for(src, Dialect::MariaDb);
    assert!(
        maria.contains("COALESCE(JSON_ARRAYAGG(JSON_OBJECT('title', `s2_course`.`title`) ORDER BY `s2_course`.`title` ASC), JSON_ARRAY()) FROM `course` AS `s2_course` WHERE `s2_course`.`id` IN (SELECT `s1_enrollment`.`course_id` FROM `enrollment` AS `s1_enrollment` WHERE `s1_enrollment`.`student_id` = `student`.`id`)) AS `courses[]`"),
        "\n{maria}"
    );

    let pg = gen_for(src, Dialect::Postgres);
    assert!(
        pg.contains(r#"COALESCE(json_agg(json_build_object('title', "s2_course"."title") ORDER BY "s2_course"."title" ASC), '[]'::json) FROM "course" AS "s2_course" WHERE "s2_course"."id" IN (SELECT "s1_enrollment"."course_id" FROM "enrollment" AS "s1_enrollment" WHERE "s1_enrollment"."student_id" = "student"."id")) AS "courses[]""#),
        "\n{pg}"
    );
    // Junction never enters the outer FROM — it is hidden inside the IN-subquery.
    assert!(
        !pg.contains(
            r#"FROM "student"
JOIN "enrollment""#
        ),
        "junction leaked into FROM\n{pg}"
    );
}

#[test]
fn flatten_injects_both_junction_and_far_soft_delete() {
    // The junction's soft-delete rides the inner `IN` (excludes tombstoned links); the far
    // model's rides the outer `WHERE` (excludes tombstoned far rows).
    let ddl = gen(r#"
        @sort(id asc)
        Student { id: Id, name: text, enrollments: Enrollment[] (Enrollment.student) }
        @sort(id asc)
        @soft_delete(deleted_at)
        Enrollment { id: Id, student: Student, course: Course, deleted_at: timestamp? }
        @sort(id asc)
        @soft_delete(deleted_at)
        Course { id: Id, title: text, deleted_at: timestamp? }
        shape StudentCourses from Student { name, courses = enrollments.course { title } }
        query student_by_id(id) -> StudentCourses;
        "#);
    // far tombstone in the outer WHERE, junction tombstone in the inner IN.
    assert!(
        ddl.contains("WHERE `s2_course`.`id` IN (SELECT `s1_enrollment`.`course_id` FROM `enrollment` AS `s1_enrollment` WHERE `s1_enrollment`.`student_id` = `student`.`id` AND `s1_enrollment`.`deleted_at` IS NULL) AND `s2_course`.`deleted_at` IS NULL) AS `courses[]`"),
        "\n{ddl}"
    );
}

#[test]
fn flatten_injects_scope_on_junction_and_far() {
    // A scoped junction *and* a scoped far side each carry their `@scope` into the right
    // level: the junction's into the inner IN, the far's into the outer WHERE.
    let ddl = gen(r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @sort(id asc)
        Student { id: Id, name: text, enrollments: Enrollment[] (Enrollment.student) }
        @scope Tenant
        @sort(id asc)
        Enrollment { id: Id, student: Student, course: Course, org: Org }
        @scope Tenant
        @sort(id asc)
        Course { id: Id, title: text, org: Org }
        shape StudentCourses from Student { name, courses = enrollments.course { title } }
        query student_by_id(id) -> StudentCourses scoped Tenant;
        "#);
    assert!(
        ddl.contains("WHERE `s2_course`.`id` IN (SELECT `s1_enrollment`.`course_id` FROM `enrollment` AS `s1_enrollment` WHERE `s1_enrollment`.`student_id` = `student`.`id` AND `s1_enrollment`.`org_id` = :ctx_org) AND `s2_course`.`org_id` = :ctx_org) AS `courses[]`"),
        "\n{ddl}"
    );
}

#[test]
fn flatten_self_referential_m2m_aliases_distinctly() {
    // A self-referential m2m (`User.friends` through a `Friendship` junction): the far
    // alias (`s2_user`), the junction alias (`s1_friendship`), and the outer `user` row
    // must all differ so the two-level correlation is unambiguous.
    let ddl = gen(r#"
        @sort(id asc)
        User { id: Id, name: text, links: Friendship[] (Friendship.owner) }
        @sort(id asc)
        Friendship { id: Id, owner: User, friend: User }
        shape UserFriends from User { name, friends = links.friend { name } }
        query user_by_id(id) -> UserFriends;
        "#);
    assert!(
        ddl.contains("FROM `user` AS `s2_user` WHERE `s2_user`.`id` IN (SELECT `s1_friendship`.`friend_id` FROM `friendship` AS `s1_friendship` WHERE `s1_friendship`.`owner_id` = `user`.`id`)) AS `friends[]`"),
        "\n{ddl}"
    );
}
