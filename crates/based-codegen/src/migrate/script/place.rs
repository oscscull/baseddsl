//! Interleave the snapshot-derived structural steps with the `raw` and data items of an
//! in-order script (`up.mig` / `down.mig`), each item after the structural step it follows.

use super::super::diff::Step;
use super::super::up_mig::render_step;
use super::scan::{is_canonical, ItemKind, Scanned};
use super::ScriptProblem;

/// One entry of an in-order migration, in execution order.
pub(crate) enum Placed<'a> {
    Step(&'a Step),
    Raw(&'a Step),
    Data { line: usize, text: &'a str },
}

pub(crate) fn place<'a>(
    file: &'static str,
    steps: &'a [Step],
    scanned: &'a Scanned<'_>,
) -> Result<Vec<Placed<'a>>, ScriptProblem> {
    let mut pending = steps.iter().map(|s| (s, canonical_lines(s))).peekable();
    let mut seen = 0usize;
    let mut out = Vec::with_capacity(steps.len() + scanned.items.len());
    for item in &scanned.items {
        while let Some((step, n)) = pending.next_if(|&(_, n)| seen + n <= item.after) {
            seen += n;
            out.push(Placed::Step(step));
        }
        if seen != item.after {
            return Err(ScriptProblem {
                file,
                line: item.line,
                message: "a data step or raw line can't sit inside a multi-line structural step"
                    .to_string(),
            });
        }
        out.push(match &item.kind {
            ItemKind::Raw(step) => Placed::Raw(step),
            ItemKind::Data(text) => Placed::Data {
                line: item.line,
                text,
            },
        });
    }
    out.extend(pending.map(|(s, _)| Placed::Step(s)));
    Ok(out)
}

/// How many canonical `up.mig` lines a step renders to.
fn canonical_lines(step: &Step) -> usize {
    let mut text = String::new();
    render_step(&mut text, step);
    text.lines().filter(|l| is_canonical(l)).count()
}
