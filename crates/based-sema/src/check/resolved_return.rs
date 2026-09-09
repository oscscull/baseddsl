//! Resolve a return type to its underlying model.

use super::*;

pub(crate) struct Resolved {
    pub(crate) model: String,
    pub(crate) shape: Option<String>,
}

/// Resolve a return type to its underlying model. A shape resolves via its `from`;
/// a bare model resolves to itself; `full` needs a block body naming the model.
pub(crate) fn resolve_return(
    ret: &RetType,
    body_model: Option<&str>,
    cx: &Cx,
    sink: &mut Sink,
) -> Option<Resolved> {
    let name = ret.ty.node.as_str();
    if name == "full" {
        return match body_model {
            Some(m) if cx.find(m).is_some() => Some(Resolved {
                model: m.to_string(),
                shape: Some("full".to_string()),
            }),
            Some(m) => {
                sink.error(
                    code::UNKNOWN_MODEL,
                    ret.ty.span,
                    format!("unknown model `{m}`"),
                );
                None
            }
            None => {
                sink.error(
                    code::FULL_NEEDS_MODEL,
                    ret.ty.span,
                    "`full` return needs a block body that names the model",
                );
                None
            }
        };
    }
    if let Some(from) = cx.shapes.get(name) {
        return Some(Resolved {
            model: from.clone(),
            shape: Some(name.to_string()),
        });
    }
    if cx.find(name).is_some() {
        return Some(Resolved {
            model: name.to_string(),
            shape: None,
        });
    }
    sink.error(
        code::UNKNOWN_RETURN,
        ret.ty.span,
        format!("unknown return type `{name}` (not a declared shape or model)"),
    );
    None
}
