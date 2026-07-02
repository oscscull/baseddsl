# syntax/indexing.md

Principles: 1, 2, 5, 6, 8.

## Declaration
```
@index field
@index(a, b)          # composite; parens bound the column list
@index(a, b) unique
```

## Inference (shown, not written)
Engine infers a baseline set from generated queries: join keys, filter paths, soft-delete columns. Soft-delete columns -> partial/predicate-leading indexes (engine knows it always filters `deleted_at IS NULL`). Inferred indexes shown in LSP.

## Lint (bidirectional)
- missing-index: a query will scan
- useless-index: declared but no query uses it -> drop (pure write-tax)

## unindexed lint
Fires on consequential unindexed queries. Default = warn (legacy onboarding must not hit a wall; ratchet to CI-error as schema gets healthy). Fire only on plausibly-consequential tables (unknown/marked-large, or above prod-stats floor) so the annotation stays meaningful, not rote.

Satisfy by index OR annotation:
- `unindexed(max_rows: N)` — checked assertion: bounded-and-fine; re-fires if prod stats show N exceeded. Self-policing, not a mute.
- `unindexed(unsafe)` / `unindexed(unsafe, "reason")` — unbounded, uncheckable. Permitted but greppable + surfaced in audit, never silently satisfied. Means "guarantees end here, human vouches."

The annotation is a query clause (grammar `unindexed_clause`), legal wherever `where`/`order`/`page` are. A stale annotation — the query turns out indexed — is itself linted. Lint semantics + inference boundaries: decisions.md D15.
