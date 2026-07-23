# syntax/indexing.md

Principles: 1, 2, 5, 6, 8.

## Declaration
```
@index field
@index(a, b)          # composite; parens bound the column list
@index(a, b) unique
```

## Required in source (not inferred)
An index carries real write + disk cost, so it is *written*, never silently derived
(principle 8; principle 2 governs its consequential omission). The engine tells you which
indexes the access layer needs and errors when one is missing:

- A relation **join key** some query/shape traverses with no covering `@index`, or a query
  whose root filter (its eq/range/leading-sort columns) no available index leads with, is an
  error **`E0260`** — the query will scan. Satisfy it with an `@index` (the editor autofix
  inserts one) or the visible `unindexed(…)` opt-out below. Closed-world (D5) makes "this
  query will scan" a fact, not a guess.

A written `@index` on a `@soft_delete` model is **rendered predicate-leading**: the
always-filtered tombstone column is physically prepended (`(deleted_at, order_id)`) because
MariaDB/SQLite have no partial indexes, so the declared index still leads with the column
that selects. That is a rendering of the *written* index, not a second silent one; a unique
`@index` is a constraint and is never reshaped.

## Exotic indexes
An opaque column you cannot index is dead weight — a `geometry` without GIST, a `tsvector`
without GIN, is unusable. Two tiers ride the same declaration.

**Access method — `using <method>`:**
```
@index location using gist
@index(body, title) using fulltext
```
The method set is closed and per-target, checked **loudly** at generation time (`E0272`,
never a silent downgrade to a plain index):

| method | targets |
|--------|---------|
| `btree`, `hash` | postgres, mariadb |
| `gist`, `spgist`, `gin`, `brin` | postgres |
| `fulltext`, `spatial` | mariadb |

SQLite has no access-method syntax at all, so **every** `using` is an error there. Postgres
renders the leading `USING <m>`; MariaDB spells `fulltext`/`spatial` as index kinds
(`FULLTEXT KEY`) and `btree`/`hash` as a trailing `USING`.

**Opaque index — `@index raw("…")`:** the long tail (expression indexes, opclasses, partial
`WHERE`) the neutral vocabulary does not model:
```
@index raw("(lower(email))")
@index raw({ postgres: "USING gin (tags jsonb_path_ops)", mariadb: "(tags(64))" })
```
The body is **everything after `ON <table>`** and is emitted verbatim as a standalone
`CREATE INDEX` on every dialect (never a MariaDB inline `KEY`). The snapshot records it as a
literal string and diffs it by string compare, so create/drop/rebuild lifecycle stays
in-system exactly as for an opaque column — no orphaned index, nothing lost to a sqlite table
rebuild, nothing invisible to review. Its name is derived from the body
(`idx_<table>_raw_<hash>`), so reordering a model's indexes never reads as a rename. An empty
body is `E0274`.

The engine cannot know what an exotic index covers, so it never satisfies `E0260` — it is the
author's assertion, not a derived fact.

## Lint (declared indexes)
- **`W0104` useless-index**: declared but no query filters, sorts, or joins on it -> drop (pure write-tax).
- **`W0105` stale-annotation**: an `unindexed(…)` on a query that turns out indexed -> drop it.

## `unindexed(…)` opt-out
The visible escape hatch that satisfies `E0260` without an index — for a query the author
knows stays small. Satisfy `E0260` by an `@index` OR this annotation:
- `unindexed(max_rows: N)` — checked assertion: bounded-and-fine; re-fires if prod stats show N exceeded. Self-policing, not a mute.
- `unindexed(unsafe)` / `unindexed(unsafe, "reason")` — unbounded, uncheckable. Permitted but greppable + surfaced in audit, never silently satisfied. Means "guarantees end here, human vouches."

The annotation is a query clause (grammar `unindexed_clause`), legal wherever `where`/`order`/`page` are. A bulk write (`update`/`delete`/`restore` `where`) carries no such clause, so it must be indexed. A stale annotation — the query turns out indexed — is `W0105`. Lint semantics + requirement boundaries: decisions.md D15, D103.
