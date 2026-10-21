//! Lower a filter leaf whose path crosses a to-many edge (`comments.resolved = false`) to
//! "some related row satisfies it": a correlated `EXISTS` per to-many hop, never a join, so
//! the parent never fans out and `not` reads as "no related row satisfies it".

use super::super::*;

impl<'a> Select<'a> {
    /// The index of the first to-many edge `path` steps *through* (not its terminal
    /// segment), or `None` when every hop before the terminal is to-one.
    pub(super) fn to_many_hop(&self, path: &Path, model: &RModel) -> Option<usize> {
        let (_, hops) = path.segments.split_last()?;
        let mut cur = self.schema.model(&model.name)?;
        for (i, seg) in hops.iter().enumerate() {
            if self.to_many_edge(&seg.node, cur).is_some() {
                return Some(i);
            }
            cur = self.schema.model(cur.member(&seg.node)?.kind.target()?)?;
        }
        None
    }

    /// Lower the leaf at `path` through `build`, which renders the comparison from its
    /// left-operand SQL. Each to-many hop becomes an `EXISTS` whose rest of the path
    /// resolves inside the subquery; the right-hand side is pre-rendered by the caller in
    /// the outer scope, so a column operand still reads the outer row.
    pub(super) fn leaf(
        &mut self,
        path: &Path,
        model: &RModel,
        build: impl FnOnce(&str) -> String,
    ) -> String {
        let Some(hop) = self.to_many_hop(path, model) else {
            let (alias, col) = self.resolve(path, model);
            return build(&self.qcol(&alias, &col));
        };
        let (head, rest) = path.segments.split_at(hop + 1);
        let head = Path {
            segments: head.to_vec(),
        };
        let rest = Path {
            segments: rest.to_vec(),
        };
        self.relation_exists(&head, model, |sub, child| {
            Some(sub.leaf(&rest, child, build))
        })
    }
}
