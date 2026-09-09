//! Resolve a dotted DSL path to `(table_alias, column)`, materializing a JOIN per step.

use super::*;

impl<'a> Select<'a> {
    /// Resolve a dotted path from `root` to `(table_alias, column)`, materializing
    /// a JOIN for each relation step. A terminal relation resolves to its FK column
    /// (so `where (org = $org)` compares `org_id`), never a join.
    pub(crate) fn resolve(&mut self, path: &Path, root: &RModel) -> (String, String) {
        let root_alias = self.root_alias.clone();
        self.resolve_from(path, &root_alias, "", root)
    }

    /// Resolve a dotted path starting from `start_model` (aliased `start_alias`, at join
    /// path `start_prefix`) to `(table_alias, column)`, materializing a JOIN per relation
    /// step. [`resolve`](Self::resolve) is this rooted at the query's root; a nested shape
    /// body resolves its paths from the joined relation's alias/prefix instead.
    pub(crate) fn resolve_from(
        &mut self,
        path: &Path,
        start_alias: &str,
        start_prefix: &str,
        start_model: &RModel,
    ) -> (String, String) {
        let mut cur = start_model;
        let mut alias = start_alias.to_string();
        let mut prefix = start_prefix.to_string();
        let n = path.segments.len();
        for (i, seg) in path.segments.iter().enumerate() {
            let name = &seg.node;
            let Some(mem) = cur.member(name) else {
                return (alias, name.clone()); // sema already flagged this
            };
            let last = i + 1 == n;
            match &mem.kind {
                MemberKind::Scalar { column, .. } => return (alias, column.clone()),
                MemberKind::Forward {
                    fk_col, optional, ..
                } => {
                    if last {
                        return (alias, fk_col.clone());
                    }
                    let (next_alias, next) =
                        self.join_forward(&alias, cur, &mut prefix, name, *optional);
                    alias = next_alias;
                    cur = next;
                }
                MemberKind::Inverse { target, via } => {
                    if last {
                        // Equality against a to-many edge has no local column; fall
                        // back to the key (unusual; kept resolvable).
                        return (alias, "id".to_string());
                    }
                    let (next_alias, next) =
                        self.join_inverse(&alias, cur, &mut prefix, name, target, via);
                    alias = next_alias;
                    cur = next;
                }
            }
        }
        (alias, "id".to_string())
    }
}
