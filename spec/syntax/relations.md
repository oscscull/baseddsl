# syntax/relations.md

Principles: 1, 2, 4 (point at the forward edge), 8.

## Forward-only by default
A relation is a one-directional edge. No inverse synthesized or required unless you traverse back. "Not traversable from here" is a true, safe state — not hidden.
```
# order/model.bsl — forward edges only; User's file untouched
Order {
  org:          Org
  placed_by:    User
  fulfilled_by: User?
}
```

## Inverse (opt-in)
Declare an ordinary field in the file that wants the traversal; modifier points at the forward edge via `(Model.field)`.
```
# user/model.bsl
User {
  invited_by:    User?
  invited_users: User[] (User.invited_by)   # self-ref inverse
  placed_orders: Order[] (Order.placed_by)  # foreign-edge inverse
  # fulfilled_orders absent: not traversed -> does not exist
}
```
`(Model.field)` is the pairing key. Unambiguous for self-refs and multiple-relations-to-same-model (each inverse points at its specific forward field). No tags, no two-sided declaration, no ghosts.

"What points at User?" = tooling query (find-references), not source.

## No foreign keys
No DB FK emitted by default (bad at scale / often banned). Relations are app-level; engine knows how to join, emits no FK constraint unless asked.

## Many-to-many
A many-to-many relationship is modeled by an **explicit junction model** — a model with a
forward edge to each side. Its two inverse edges make the collection traversable from either
end, so m2m needs no new relation syntax: it is two forward edges + two to-many inverses.
```
# a student enrolls in many courses; a course has many students
Enrollment {
  student: Student
  course:  Course
  @index (student, course) unique   # one enrollment per pair
}
Student {
  name:        text
  enrollments: Enrollment[] (Enrollment.student)
}
Course {
  title:       text
  enrollments: Enrollment[] (Enrollment.course)
}
```
The junction is a **declared model with real, reviewable columns** — its table, its two FK
columns, its uniqueness, and any FK-opt-in all read directly from the source (principle 2:
nothing consequential true by omission; an auto-created join table would be invisible DDL —
the same NF11 tension the owner flagged for inferred indexes). Extra columns on the link
(`enrolled_at`, `role`) live on the junction like any field. Association is an ordinary
`create` / `delete` of a junction row; a shape reaches the far side through the junction
(`enrollments { course { title } }`), reusing the to-many nesting machinery.

*Deferred (the next T5 slice):* a **far-side flattening projection** that skips the junction
in a shape (`courses = enrollments.course { title }` → a flat `Vec<Course>` rather than an
array of enrollment objects), and any decision on implicit-junction sugar (`courses:
Course[] <-> students`). The sugar is held because an engine-generated join table is real
DDL a reviewer must see in the PR — it wants the same explicit-in-source resolution NF11 is
weighing for inferred indexes, not a silent default.

## Custom join condition
Stays inside the guarantee — engine still understands the join, still injects soft-delete, still types it. For legacy keys:
```
placed_by: User (on: orders.user_ref = users.legacy_id)
```

## Note on ->
`->` also appears in query param-binding (queries.md). Both mean "connects to."
