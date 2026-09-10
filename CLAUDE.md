# CLAUDE.md

DB-first DSL + engine. Rust. MySQL/MariaDB first; dialect = compile target.
Audience = LLMs + human reviewers. Optimize tokens-to-comprehend, not keystrokes.
When terse vs readable conflict: readable wins.

Source comments: only where a reader can't infer *what* the code does from the code itself —
most code needs none; a few key entry points get a short doc block. No cross-refs to
D#/M#/PLAN/DoD/decisions in source. No WIP/rationale/narration — the spec says what the
language is; work status lives in GitHub issues + git history.

`spec/principles.md` is always-loaded tiebreakers, in order. Read before any decision.

Source: one extension `.bsl`; uniform grammar (any decl in any file). Layout is the user's
choice — compiler globs `**/*.bsl`. Recommended convention: `<domain>/model.bsl` (model + shapes)
+ `<domain>/queries.bsl` (access layer). See spec/examples/commerce.

Repo layout:
- `spec/` — language design docs (prose). Source of truth for *what* the language is.
- `spec/grammar.ebnf` — canonical grammar. Source of truth for *what parses*; resolves prose ambiguity.
- `crates/` — Rust cargo workspace (compiler + runtime).
- `tests/conformance/` — golden (input.bsl, expected) pairs.

Work is tracked in GitHub issues; the "why" behind shipped work lives in git history + closed issues, not in a checked-in log.

Spec file map:
- `spec/principles.md` — design rules. Always in context.
- `spec/syntax/models.md` — models, fields, decorators, types
- `spec/syntax/enums.md` — enum type (string + numeric kinds, explicit variant values, column + CHECK representation)
- `spec/syntax/relations.md` — relations, inverses, joins
- `spec/syntax/soft-delete.md` — soft-delete decorator + ops
- `spec/syntax/queries.md` — query signatures, get/list, filters
- `spec/syntax/mutations.md` — create/update/delete/restore, tx
- `spec/syntax/shapes.md` — return projections
- `spec/syntax/sorting.md` — default sorts + override cascade
- `spec/syntax/pagination.md` — keyset/offset
- `spec/syntax/streaming.md` — `-> stream` queries: NDJSON wire + `Stream` client method
- `spec/syntax/indexing.md` — index decl + lint
- `spec/syntax/raw.md` — raw SQL escape hatch
- `spec/syntax/migrations.md` — migration generation (snapshot diff, neutral steps, `@was`, ledger)
- `spec/syntax/calling.md` — generated client + wire surface
- `spec/syntax/auth.md` — scope/context/guard handles
- `spec/examples/commerce/` — full worked reference, in the recommended by-domain layout

Each syntax file lists its governing principles at top. Load that file + principles.md for a task.

Note: `->` has two meanings, both "connects to": relation declaration (models.md/relations.md) and param-binding (queries.md). Expect both.
