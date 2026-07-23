//! `@was` lifecycle: self-consume spent rename directives, and the rename teach hint.
//!
//! Two offline helpers built on the [`diff`](super::diff) engine:
//!
//! - [`spent_was_edits`] — after `based migrate gen` writes a migration that *consumed* a
//!   `@was` (its `rename` step was emitted), locate that now-spent directive in the `.bsl`
//!   source and return the exact byte range removing it leaves the rest of the declaration
//!   byte-clean. The rename lives durably in the migration ledger (`schema.snap` + the
//!   `rename` step), so the source hint is dead weight once captured (principle 4).
//! - [`rename_hints`] — at the ambiguous drop-X/add-same-family-Y moment (a diff that could
//!   be a rename or a genuine drop+add), the message that teaches `@was` with zero prior
//!   knowledge. Surfaced by `gen`, the `W0108` drift note, and the apply destructive gate.

use based_ast::{Decl, DecoArg, Literal, Member};
use based_sema::CheckedSchema;
use std::collections::HashSet;
use std::ops::Range;

use super::diff::{diff_snapshots, Step};
use super::model::{ColumnSnap, Snapshot};

// ---------- self-consume: retire a spent `@was` from source -----------------

/// A `@was` directive the just-generated migration consumed, and the byte range in its
/// source file to remove to retire it. The range is surgical — only the directive (plus its
/// adjacent separating whitespace, or its whole line for a model-level decorator on its own
/// line) — so the rest of the declaration's formatting is untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpentWas {
    /// Index of the owning source file (the `FileId` the parser stamped on the span).
    pub file: usize,
    /// The byte range to remove from that file's source text.
    pub range: Range<usize>,
    /// A human label for the visible `gen` log line — e.g. `@was("upc") on `Product.barcode``.
    pub label: String,
}

/// Find the `@was` directives the just-written migration consumed and the byte range that
/// retires each. `steps` is the diff `gen` just captured; only a `@was` whose `rename` step
/// was actually emitted is returned — a still-live or spent `@was` (which produces no rename
/// step) is never touched, so the rewrite is conservative and idempotent.
///
/// `sources` is indexed by `FileId` (path, text); the returned [`SpentWas::range`] indexes
/// into the matching file's text. The caller applies the removals (highest offset first) and
/// writes the file back.
pub fn spent_was_edits(
    steps: &[Step],
    schema: &CheckedSchema,
    decls: &[Decl],
    sources: &[(std::path::PathBuf, String)],
) -> Vec<SpentWas> {
    // The renames this migration actually emitted (a spent/inert `@was` emits none).
    let mut table_renames: HashSet<(&str, &str)> = HashSet::new(); // (old, new)
    let mut col_renames: HashSet<(&str, &str, &str)> = HashSet::new(); // (table, old, new)
    for s in steps {
        match s {
            Step::RenameTable { from, to } => {
                table_renames.insert((from.as_str(), to.as_str()));
            }
            Step::RenameColumn { table, from, to } => {
                col_renames.insert((table.as_str(), from.as_str(), to.as_str()));
            }
            _ => {}
        }
    }
    if table_renames.is_empty() && col_renames.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for d in decls {
        let Decl::Model(m) = d else { continue };
        let Some(rmodel) = schema.model(&m.name.node) else {
            continue;
        };
        let table = rmodel.table.as_str();

        // Model-level `@was("old_table")` — a decorator on (usually) its own line.
        for deco in &m.decorators {
            if deco.name.node != "was" {
                continue;
            }
            let Some(DecoArg::Lit(Literal::Str(old))) = deco.args.first() else {
                continue;
            };
            if !table_renames.contains(&(old.as_str(), table)) {
                continue;
            }
            let fid = deco.span.file.0 as usize;
            let Some((_, src)) = sources.get(fid) else {
                continue;
            };
            let range = decorator_removal(src, deco.span.start as usize, deco.span.end as usize);
            out.push(SpentWas {
                file: fid,
                range,
                label: format!("@was(\"{old}\") on model `{}`", m.name.node),
            });
        }

        // Field-level `@was("old_col")` — inline in the field's modifier position.
        for mem in &m.members {
            let Member::Field(f) = mem else { continue };
            let Some(was) = &f.was else { continue };
            let old = was.node.as_str();
            let Some(rmem) = rmodel.member(&f.name.node) else {
                continue;
            };
            let new_col = rmem.physical_col();
            if !col_renames.contains(&(table, old, new_col)) {
                continue;
            }
            let fid = was.span.file.0 as usize;
            let Some((_, src)) = sources.get(fid) else {
                continue;
            };
            let Some(range) =
                field_was_removal(src, was.span.start as usize, was.span.end as usize)
            else {
                continue;
            };
            out.push(SpentWas {
                file: fid,
                range,
                label: format!("@was(\"{old}\") on `{}.{}`", m.name.node, f.name.node),
            });
        }
    }
    out
}

/// Apply a set of [`SpentWas`] removals to one file's source text, highest offset first so
/// earlier ranges stay valid. Returns the rewritten text (unchanged if `edits` is empty).
pub fn apply_spent_was(src: &str, edits: &[SpentWas]) -> String {
    let mut ranges: Vec<Range<usize>> = edits.iter().map(|e| e.range.clone()).collect();
    ranges.sort_by_key(|r| std::cmp::Reverse(r.start));
    let mut out = src.to_string();
    for r in ranges {
        if r.end <= out.len() {
            out.replace_range(r, "");
        }
    }
    out
}

/// The byte range removing a field-level ` @was("old")` — from the `@` through the closing
/// `)`, extended back over the single separating whitespace so `text? @was("x") (unique)`
/// collapses to `text? (unique)`. `lit_start..lit_end` is the string-literal span the parser
/// recorded (the only span it kept for `@was`); the `@was(` prefix and `)` suffix are found
/// by scanning outward. `None` if the surrounding tokens can't be located (never expected).
fn field_was_removal(src: &str, lit_start: usize, lit_end: usize) -> Option<Range<usize>> {
    let at = src.get(..lit_start)?.rfind("@was")?;
    let close_rel = src.get(lit_end..)?.find(')')?;
    let end = lit_end + close_rel + 1;
    let bytes = src.as_bytes();
    let mut start = at;
    while start > 0 && matches!(bytes[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    Some(start..end)
}

/// The byte range removing a model-level `@was("old")` decorator. `ds..de` is the full
/// decorator span. When it sits alone on its line (only indentation before, only whitespace
/// after), the whole line — including its trailing newline — is removed. When it shares a
/// line with other decorators, the minimal edit is taken: the directive plus one trailing
/// space, or a leading space when it is last on the line.
fn decorator_removal(src: &str, ds: usize, de: usize) -> Range<usize> {
    let line_start = src[..ds].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = src[de..]
        .find('\n')
        .map(|i| de + i + 1)
        .unwrap_or_else(|| src.len());
    let before_empty = src[line_start..ds].trim().is_empty();
    let after_empty = src[de..line_end].trim().is_empty();
    if before_empty && after_empty {
        return line_start..line_end;
    }
    // Shares the line — minimal edit around the directive.
    let bytes = src.as_bytes();
    if bytes.get(de) == Some(&b' ') || bytes.get(de) == Some(&b'\t') {
        ds..de + 1
    } else if ds > line_start && matches!(bytes[ds - 1], b' ' | b'\t') {
        ds - 1..de
    } else {
        ds..de
    }
}

// ---------- teach-at-checkpoint: the rename hint ---------------------------

/// The load-bearing teach hint: a single-table diff that drops one column and adds one
/// same-family column is the exact moment a rename is ambiguous with a genuine drop+add.
/// [`rename_hints`] surfaces one of these per such pair so `gen`, the `W0108` drift note,
/// and the apply destructive gate can all say "if this is a rename, declare `@was`".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameHint {
    pub table: String,
    pub dropped: String,
    pub added: String,
}

impl RenameHint {
    /// The teach message — self-revealing at the moment of ambiguity, no prior knowledge.
    pub fn message(&self) -> String {
        format!(
            "if this renames `{drop}` → `{add}` on `{tbl}`, add `@was(\"{drop}\")` on `{add}` \
             and re-run `based migrate gen`; otherwise `{drop}` is dropped (data loss)",
            drop = self.dropped,
            add = self.added,
            tbl = self.table,
        )
    }
}

/// The rename teach hints for a diff `prev → now`: one per table that drops exactly one
/// column and adds exactly one column of the same neutral type family (the unambiguous
/// rename-vs-drop+add signal). A table with a declared `@was` already emits a `rename` step,
/// not a drop+add, so it never trips this. More than one drop or add on a table is left
/// silent (the pairing would be a guess — precisely what `@was` exists to make explicit).
pub fn rename_hints(prev: &Snapshot, now: &Snapshot) -> Vec<RenameHint> {
    use std::collections::BTreeMap;
    let steps = diff_snapshots(prev, now);
    let mut drops: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut adds: BTreeMap<&str, Vec<&ColumnSnap>> = BTreeMap::new();
    for s in &steps {
        match s {
            Step::DropColumn { table, column } => drops.entry(table).or_default().push(column),
            Step::AddColumn { table, column } => adds.entry(table).or_default().push(column),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (table, dropped) in &drops {
        let Some(added) = adds.get(table) else {
            continue;
        };
        if dropped.len() != 1 || added.len() != 1 {
            continue;
        }
        let x = dropped[0];
        let y = added[0];
        let dropped_ty = prev.table(table).and_then(|t| t.column(x)).map(|c| &c.ty);
        if dropped_ty == Some(&y.ty) {
            out.push(RenameHint {
                table: table.to_string(),
                dropped: x.to_string(),
                added: y.name.clone(),
            });
        }
    }
    out
}
