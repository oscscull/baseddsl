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

## Custom join condition
Stays inside the guarantee — engine still understands the join, still injects soft-delete, still types it. For legacy keys:
```
placed_by: User (on: orders.user_ref = users.legacy_id)
```

## Note on ->
`->` also appears in query param-binding (queries.md). Both mean "connects to."
