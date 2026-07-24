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

## Foreign keys — opt-in

A relation is app-level: the engine knows how to join and stores the `<field>_id` FK
**column**, but whether it emits a DB `FOREIGN KEY` **constraint** is a deliberate choice
(FK constraints are often banned at scale). The default is safe — no constraint — and every
divergence is visible in source.

**The convention (`based.toml`).**

```toml
[schema]
foreign_keys = "none"   # default: a relation gets an FK only if it writes @fk
# foreign_keys = "all"  # every forward relation gets a bare FK unless it writes @no_fk
```

**`@fk` — opt a forward (to-one) relation into a constraint**, with optional standard-SQL
referential actions:

```
placed_by: User @fk(on_delete: cascade)
org:       Org  @fk(on_delete: restrict, on_update: cascade)
author:    User @fk                       # bare: DB-default action (no ON DELETE/UPDATE clause)
```

Actions: `cascade`, `restrict`, `set_null`, `no_action`. `on_delete:`/`on_update:` are
independent optional kwargs. `on_delete: set_null` requires the relation be optional
(nullable FK) — `E0293`.

**`@no_fk` — opt out of the constraint**, on one forward edge or a whole model:

```
Order @no_fk { org: Org  placed_by: User }   # whole table — no FKs on any forward relation
Event { org: Org  actor: User @no_fk }        # just this edge
```

`@fk`/per-edge `@no_fk` are valid **only on a forward to-one relation** (an inverse/`[]`
edge or a scalar is `E0290`; a custom-join `on:` relation, which owns no conventional FK
column, is `E0291`; both on one edge is `E0292`; an unknown action is `E0294`).

**The reason rule.** A **reason string is required exactly when a decorator flips FK
presence *against* the `foreign_keys` convention** — spelled and handled like
`@no_id("reason")` / `unscoped("reason")`, so a forfeited or against-convention guarantee is
never silent in review:

| convention | `@fk` (adds an FK) | `@no_fk` (removes an FK) |
|------------|--------------------|--------------------------|
| `"none"` (norm: absent) | **diverges** → reason required (`E0295`): `@fk("orders die with their org", on_delete: cascade)` | concordant → **redundant** `W0110` |
| `"all"` (norm: present) | concordant (an action refines) / bare is **redundant** `W0110` | **diverges** → reason required (`E0295`): `@no_fk("legacy table, FKs banned at scale")` |

Referential actions never trigger a reason on their own — only flipping presence does. A
resolved FK is recorded in `schema.snap` (a `fk` line), so adding / removing / changing one
diffs into a migration step (migrations.md).

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

### Far-side flattening projection

A shape can skip the junction and return the far side directly as a flat, **distinct**
list — a derived field (`=`) naming a relation **path** through the to-many junction edge,
then a forward edge to the far model, with a projection body:
```
shape StudentCourses from Student {
  name
  courses = enrollments.course { title }   # -> courses: [ { title }, … ], junction hidden
}
```
`enrollments.course` hops into the to-many junction (`enrollments`, an inverse edge), then
out along a forward edge (`course`) to the far model — so the field is `Vec<Course>`, not a
`Vec` of enrollment wrappers. The list is the **set** of related far rows, each once
(distinct on the far primary key): a junction with duplicate links, or a filter, never
multiplies a far row into the result. It generalizes to more hops (an inverse edge first,
then forward edges; the last segment's model is the element type), but the junction-skip is
the primary form. **Order is unspecified** unless the far model declares `@sort` (portable
JSON aggregation has no cross-dialect ordered form) — the same rule as a to-many nest. The
far model's *and* the junction's `@scope` / `@soft_delete` ride the flattening subquery, so a
tombstoned link, a tombstoned far row, and an out-of-scope far row are all excluded; nesting
into a scoped far side counts as touching it. The body composes — it may nest or flatten
further. A `@no_id` (keyless) far model is a compile error (no primary key to dedup on).

*Implicit-junction sugar* (`courses: Course[] <-> students`) stays **rejected**: an
engine-generated join table is real DDL a reviewer must see in the PR, so it wants the same
explicit-in-source resolution NF11 settled for inferred indexes, not a silent default.

## Custom join condition
Stays inside the guarantee — engine still understands the join, still injects soft-delete, still types it. For legacy keys:
```
placed_by: User (on: orders.user_ref = users.legacy_id)
```

## Note on ->
`->` also appears in query param-binding (queries.md). Both mean "connects to."
