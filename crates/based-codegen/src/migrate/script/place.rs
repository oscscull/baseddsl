//! Interleave the snapshot-derived structural steps with the `raw` and data items of an
//! in-order `up.mig`, each item after the structural step it follows.

use super::super::diff::Step;
use super::super::up_mig::render_step;
use super::scan::{ItemKind, Scanned};
use super::UpProblem;

/// One entry of an in-order migration, in execution order.
pub(crate) enum Placed<'a> {
    Step(&'a Step),
    Raw(&'a Step),
    Data { line: usize, text: &'a str },
}

pub(crate) fn place<'a>(
    steps: &'a [Step],
    scanned: &'a Scanned<'_>,
) -> Result<Vec<Placed<'a>>, UpProblem> {
    let counts: Vec<usize> = steps.iter().map(canonical_lines).collect();
    let mut out = Vec::with_capacity(steps.len() + scanned.items.len());
    let (mut next, mut seen) = (0usize, 0usize);
    for item in &scanned.items {
        while next < steps.len() && seen + counts[next] <= item.after {
            out.push(Placed::Step(&steps[next]));
            seen += counts[next];
            next += 1;
        }
        if seen != item.after {
            return Err(UpProblem {
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
    out.extend(steps[next..].iter().map(Placed::Step));
    Ok(out)
}

/// How many non-blank, non-comment `up.mig` lines a step renders to.
fn canonical_lines(step: &Step) -> usize {
    let mut text = String::new();
    render_step(&mut text, step);
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .count()
}
