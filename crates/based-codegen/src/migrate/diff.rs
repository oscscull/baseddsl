//! Neutral migration step vocabulary (`up.mig`) and the diff engine.
//!
//! The [`Step`] / [`ScopeChange`] / [`ColumnChange`] types, the destructive-marker
//! policy ([`Step::destructive`]), and [`diff`] — which compares a prior [`Snapshot`] to
//! the current schema and emits the neutral step list. Dialect-neutral throughout; SQL
//! rendering is [`super::sql`], neutral `up.mig` text is [`super::up_mig`].

use super::model::{ColumnSnap, IndexSnap, ScopeDeclSnap, Snapshot, TableSnap};
use based_sema::CheckedSchema;

// ---------- neutral step vocabulary (`up.mig`) ----------------------------

/// One neutral migration step (migrations.md's `up.mig` vocabulary). Dialect-neutral:
/// E3 renders each to per-dialect DDL over the `Dialect` seam. A [`Step`] carrying
/// `destructive: true` (drops, type-narrowing, a new `not_null` without a default, a new
/// unique over existing data) is *marked* so apply (E4) can gate it on an acknowledgement
/// — this engine marks, never applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Full `CREATE TABLE` — `0001_init` is entirely these.
    CreateTable(TableSnap),
    /// `drop table <name>` — DESTRUCTIVE.
    DropTable(String),
    /// `add column <table>.<col> …`.
    AddColumn { table: String, column: ColumnSnap },
    /// `drop column <table>.<col>` — DESTRUCTIVE.
    DropColumn { table: String, column: String },
    /// `alter column <table>.<col> …` — one or more changes.
    AlterColumn {
        table: String,
        column: String,
        changes: Vec<ColumnChange>,
        /// The resulting column state. Carried because MariaDB alters a column via a
        /// full `MODIFY COLUMN <full definition>` — it cannot express a piecemeal
        /// null/type change — so the renderer (E3) needs the whole target column, not
        /// just the deltas. Postgres/SQLite render from the `changes` alone.
        after: ColumnSnap,
    },
    /// `add index <name> (<cols>)`.
    AddIndex { table: String, index: IndexSnap },
    /// `drop index <name>`.
    DropIndex { table: String, name: String },
    /// `add unique <name> (<cols>)` — DESTRUCTIVE over existing data.
    AddUnique { table: String, index: IndexSnap },
    /// `drop unique <name>`.
    DropUnique { table: String, name: String },
    /// A scope-contract change (auth.md Handle 2): a scope decl added/dropped/retyped, or a
    /// model joining/leaving a scope. A scope emits **no DDL** (it is an injected filter in
    /// generated code, not a DB object), so this renders as a neutral note and produces no
    /// SQL — it exists so the change lands in a reviewable migration and advances the
    /// snapshot, keeping the offline drift check honest.
    ScopeChange(ScopeChange),
}

/// The kinds of scope-contract change a diff surfaces (auth.md / DNF). None emit SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeChange {
    /// A new `scope Name (…)` decl.
    Add(ScopeDeclSnap),
    /// A `scope Name` decl removed.
    Drop(String),
    /// A surviving `scope Name` decl whose terms changed; carries the new state.
    Alter(ScopeDeclSnap),
    /// A surviving table's `@scope` alternative set changed (joined/left/re-shaped a scope).
    Table {
        table: String,
        alts: Vec<Vec<String>>,
    },
}

/// One field-level change inside an [`Step::AlterColumn`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnChange {
    /// `type <new>` — DESTRUCTIVE when narrowing (see [`is_narrowing`]).
    Type { from: String, to: String },
    /// `null` — always safe (relaxing a constraint).
    SetNull,
    /// `not_null` — DESTRUCTIVE without a default (existing NULLs violate it).
    SetNotNull { has_default: bool },
    /// `default=<lit>` — safe.
    SetDefault(String),
    /// `drop default` — safe.
    DropDefault,
}

impl Step {
    /// Does this step risk losing or rejecting data (migrations.md's destructive
    /// policy)? Apply (E4) gates a destructive step on `--allow-destructive` /
    /// `unsafe("reason")`; this engine only reports the marker.
    pub fn destructive(&self) -> bool {
        match self {
            Step::DropTable(_) | Step::DropColumn { .. } | Step::AddUnique { .. } => true,
            Step::AlterColumn { changes, .. } => changes.iter().any(|c| match c {
                ColumnChange::Type { from, to } => is_narrowing(from, to),
                ColumnChange::SetNotNull { has_default } => !has_default,
                _ => false,
            }),
            _ => false,
        }
    }
}

/// Is a type change `from -> to` narrowing (potentially truncating/failing)? Widening
/// is safe (`int -> text`); a same-family no-op is not a change at all. Anything that is
/// not a recognized safe widening is treated as narrowing (conservative — principle 1).
fn is_narrowing(from: &str, to: &str) -> bool {
    if from == to {
        return false;
    }
    // The only unambiguously safe widenings: any scalar -> text (text holds any value),
    // and int -> its wider selves (we have one `int`, so this is really the text case).
    !matches!(to, "text")
}

// ---------- diff ----------------------------------------------------------

/// Diff a *prior* snapshot against the *current* schema, producing the neutral `up.mig`
/// step list. An empty prior snapshot (`0001_init`) yields a full create set — exactly
/// what `based gen sql` builds from scratch. Renames are never auto-guessed: a changed
/// name is a drop + add pair (the `@was`-driven rename is E5).
pub fn diff(prev: &Snapshot, schema: &CheckedSchema) -> Vec<Step> {
    diff_snapshots(prev, &Snapshot::from_schema(schema))
}

/// Diff two neutral snapshots (the pure core — used by [`diff`] and by verify/drift). The
/// step order is: drops last within a table isn't required, but table creates/drops are
/// grouped and columns/indexes are emitted in a stable name order so the `up.mig` is
/// deterministic and reviewable.
pub fn diff_snapshots(prev: &Snapshot, now: &Snapshot) -> Vec<Step> {
    let mut steps = Vec::new();

    // Tables added (present now, absent before) → full CREATE. Sorted by name.
    for t in &now.tables {
        if prev.table(&t.name).is_none() {
            steps.push(Step::CreateTable(t.clone()));
        }
    }

    // Tables surviving → column + index deltas. Sorted by name (both snapshots are).
    for now_t in &now.tables {
        if let Some(prev_t) = prev.table(&now_t.name) {
            diff_table(prev_t, now_t, &mut steps);
        }
    }

    // Tables dropped (present before, absent now) → DROP. Sorted by name.
    for t in &prev.tables {
        if now.table(&t.name).is_none() {
            steps.push(Step::DropTable(t.name.clone()));
        }
    }

    diff_scopes(prev, now, &mut steps);

    steps
}

/// Diff the scope contract (top-level decls + membership on surviving tables). Emits
/// no-DDL [`Step::ScopeChange`] steps so a scope-only change still produces a reviewable
/// migration. Skipped for a from-scratch `0001_init` (empty prior): the initial scopes ride
/// in `schema.snap` and each table's `scope_alts` rides its `CreateTable`, so init stays
/// create-only (its `up.mig` matches `based gen sql` from scratch, which emits no scope SQL).
fn diff_scopes(prev: &Snapshot, now: &Snapshot, steps: &mut Vec<Step>) {
    if prev.tables.is_empty() && prev.scopes.is_empty() {
        return;
    }
    // Scope decls added / retyped (present now).
    for s in &now.scopes {
        match prev.scope(&s.name) {
            None => steps.push(Step::ScopeChange(ScopeChange::Add(s.clone()))),
            Some(old) if old != s => steps.push(Step::ScopeChange(ScopeChange::Alter(s.clone()))),
            Some(_) => {}
        }
    }
    // Scope decls dropped (present before, absent now).
    for s in &prev.scopes {
        if now.scope(&s.name).is_none() {
            steps.push(Step::ScopeChange(ScopeChange::Drop(s.name.clone())));
        }
    }
    // Surviving tables whose `@scope` membership changed (joined/left/re-shaped).
    for now_t in &now.tables {
        if let Some(prev_t) = prev.table(&now_t.name) {
            if prev_t.scope_alts != now_t.scope_alts {
                steps.push(Step::ScopeChange(ScopeChange::Table {
                    table: now_t.name.clone(),
                    alts: now_t.scope_alts.clone(),
                }));
            }
        }
    }
}

fn diff_table(prev: &TableSnap, now: &TableSnap, steps: &mut Vec<Step>) {
    // Columns added.
    for c in &now.columns {
        if prev.column(&c.name).is_none() {
            steps.push(Step::AddColumn {
                table: now.name.clone(),
                column: c.clone(),
            });
        }
    }
    // Columns altered (present in both, changed).
    for c in &now.columns {
        if let Some(old) = prev.column(&c.name) {
            let changes = column_changes(old, c);
            if !changes.is_empty() {
                steps.push(Step::AlterColumn {
                    table: now.name.clone(),
                    column: c.name.clone(),
                    changes,
                    after: c.clone(),
                });
            }
        }
    }
    // Columns dropped.
    for c in &prev.columns {
        if now.column(&c.name).is_none() {
            steps.push(Step::DropColumn {
                table: now.name.clone(),
                column: c.name.clone(),
            });
        }
    }

    // Indexes added. A unique index is its own `add unique` step (destructive over
    // existing data); a plain index is `add index` (safe).
    for i in &now.indexes {
        if prev.index(&i.name).map(|p| p == i) != Some(true) && prev.index(&i.name).is_none() {
            if i.unique {
                steps.push(Step::AddUnique {
                    table: now.name.clone(),
                    index: i.clone(),
                });
            } else {
                steps.push(Step::AddIndex {
                    table: now.name.clone(),
                    index: i.clone(),
                });
            }
        }
    }
    // Indexes changed (same name, different columns/unique) → drop + re-add. Renaming
    // an index isn't auto-guessed either; a definition change is a drop then an add.
    for i in &now.indexes {
        if let Some(old) = prev.index(&i.name) {
            if old != i {
                drop_index_step(old, now, steps);
                if i.unique {
                    steps.push(Step::AddUnique {
                        table: now.name.clone(),
                        index: i.clone(),
                    });
                } else {
                    steps.push(Step::AddIndex {
                        table: now.name.clone(),
                        index: i.clone(),
                    });
                }
            }
        }
    }
    // Indexes dropped.
    for i in &prev.indexes {
        if now.index(&i.name).is_none() {
            drop_index_step(i, now, steps);
        }
    }
}

fn drop_index_step(idx: &IndexSnap, table: &TableSnap, steps: &mut Vec<Step>) {
    if idx.unique {
        steps.push(Step::DropUnique {
            table: table.name.clone(),
            name: idx.name.clone(),
        });
    } else {
        steps.push(Step::DropIndex {
            table: table.name.clone(),
            name: idx.name.clone(),
        });
    }
}

/// The field-level changes turning column `old` into `now`. Empty when identical.
fn column_changes(old: &ColumnSnap, now: &ColumnSnap) -> Vec<ColumnChange> {
    let mut changes = Vec::new();
    if old.ty != now.ty {
        changes.push(ColumnChange::Type {
            from: old.ty.clone(),
            to: now.ty.clone(),
        });
    }
    if old.nullable != now.nullable {
        if now.nullable {
            changes.push(ColumnChange::SetNull);
        } else {
            changes.push(ColumnChange::SetNotNull {
                has_default: now.default.is_some(),
            });
        }
    }
    if old.default != now.default {
        match &now.default {
            Some(d) => changes.push(ColumnChange::SetDefault(d.clone())),
            None => changes.push(ColumnChange::DropDefault),
        }
    }
    changes
}
