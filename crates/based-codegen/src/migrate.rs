//! Migration snapshot + diff engine.
//!
//! Pure, dialect-neutral, deterministic passes over a [`CheckedSchema`](based_sema::CheckedSchema),
//! split by concern:
//!
//! - [`model`] — the neutral [`Snapshot`] type and its `schema.snap` text form.
//!   [`snapshot`] serializes the resolved schema to canonical, stable-ordered,
//!   git-diffable text (`int`/`text`/`uuid`, never `BIGINT`/`TEXT`); [`Snapshot::parse`]
//!   reads it back for the drift check. A pure function of the schema — no wall-clock,
//!   no map iteration order.
//! - [`diff`] — the neutral [`Step`] vocabulary and the [`diff`](diff()) engine. It compares a
//!   *prior* snapshot to the *current* schema and returns the `up.mig` step list.
//!   `0001_init` diffs against the empty schema, so its steps are the full create set —
//!   exactly what `based gen sql` builds from scratch. Renames are never auto-guessed: a
//!   changed name is a drop + add pair (the `@was`-driven rename is the exception). Data-losing
//!   steps are *marked* destructive so apply can gate on the ack — this engine only marks.
//! - [`up_mig`] — renders the neutral step list to the reviewable `up.mig` text.
//! - [`sql`] — [`render_sql`] renders the step list to per-dialect
//!   `CREATE`/`ALTER`/`DROP` SQL over the `Dialect` seam, reusing the DDL type map
//!   ([`crate::sql::sql_type`]) so a migration's SQL can never drift from `based gen sql`;
//!   [`sql_statements`] is its executable twin and [`content_hash`] anchors
//!   the ledger's tamper guard.
//!
//! The snapshot text and step list stay decoupled from SQL: everything outside [`sql`]
//! names no dialect.
//!
//! ## `schema.snap` grammar
//! ```text
//! snapshot v1 dialect=neutral
//! scope <Name> (<col>: <Type> = $ctx.<field>, …)
//!
//! table <name> [soft_delete=<col>:<mode>] [scope=(<Name>, …)]* [sort=(<col> <dir>, …)]
//!   column <name> <type> null|not_null [default=<lit>] [unique] [fk=<Model>]
//!   index  <name> (<col>, …) [unique] [using <method>]
//!   fk     <col> -> <ref_table>.<ref_col> [on_delete=<a>] [on_update=<a>]
//! ```
//! A `fk` line records a resolved foreign-key constraint (the toml `foreign_keys`
//! convention ⊕ per-relation `@fk`/`@no_fk`), so adding/removing/changing an FK diffs.
//! Named scopes serialize once as top-level `scope` decls (sorted by
//! name, before the tables — the one place a scope column's/`$ctx` field's type lives) and
//! are referenced on each governed table by name: one `scope=(…)` group per `@scope`
//! alternative (the DNF — commas inside a group are the AND, separate groups the OR). A
//! scope emits no DDL, so this is header/decl metadata that round-trips for the drift check.
//! Every table opens with a `table` line and closes at the next `table`/EOF; its
//! `column`/`index` lines are indented two spaces. The `id` column is elided when it
//! is the default (`uuid`, not-null, not-unique) — a universally implicit invariant;
//! a model that declares a non-default `id` records it explicitly.

mod diff;
mod lifecycle;
mod model;
mod sql;
mod up_mig;

pub use diff::{
    diff, diff_snapshots, drift, has_raw_step, parse_raw_steps, raw_modeled_tables,
    strip_raw_steps, ColumnChange, ScopeChange, Step,
};
pub use lifecycle::{apply_spent_was, rename_hints, spent_was_edits, RenameHint, SpentWas};
pub use model::{
    fk_spec_text, foreign_key_snaps, snapshot, target_pk_column, ColumnSnap, ForeignKeySnap,
    IndexSnap, ParseError, Rename, ScopeDeclSnap, ScopeTermSnap, Snapshot, TableSnap,
};
pub use sql::{content_hash, render_down, render_sql, sql_statements, up_mig_matches_snapshot};
pub use up_mig::render_up;

#[cfg(test)]
mod tests;
