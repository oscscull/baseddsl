//! Reprint a model's header and members: decorators and their args, a field line
//! (name/type/inverse alignment, modifiers, relation and FK annotations), and the
//! `field_of` member accessor.

use based_ast::*;

use crate::*;

pub(crate) fn decorator(d: &Decorator) -> String {
    if d.args.is_empty() {
        format!("@{}", d.name.node)
    } else {
        format!(
            "@{}({})",
            d.name.node,
            d.args.iter().map(deco_arg).collect::<Vec<_>>().join(", ")
        )
    }
}

fn deco_arg(a: &DecoArg) -> String {
    match a {
        DecoArg::Sort(s) => sort_term(s),
        DecoArg::Pred(p) => predicate(p, 0),
        DecoArg::Ident(i) => i.node.clone(),
        DecoArg::Path(p) => path(p),
        DecoArg::Lit(l) => literal(l),
    }
}

/// Render one field line body (no indent). `name_w` aligns the type column across
/// the model's fields; `inverse_w` aligns the inverse-ref column across the fields
/// that carry one.
pub(crate) fn field(f: &Field, name_w: usize, inverse_w: usize) -> String {
    let ty = type_expr(&f.ty);
    let mut s = format!(
        "{:<width$} {}",
        format!("{}:", f.name.node),
        ty,
        width = name_w + 1
    );
    if let Some(iv) = &f.inverse {
        let pad = inverse_w.saturating_sub(ty.len());
        s.push_str(&" ".repeat(pad + 1));
        s.push_str(&format!("({}.{})", iv.model.node, iv.field.node));
    }
    if !f.modifiers.is_empty() {
        s.push(' ');
        s.push_str(&modifiers_group(&f.modifiers));
    }
    if let Some(pred) = &f.relation_on {
        s.push_str(&format!(" (on: {})", predicate(pred, 0)));
    }
    if let Some(fk) = &f.fk {
        s.push_str(&fk_annot(fk));
    }
    if let Some(no_fk) = &f.no_fk {
        s.push_str(&match &no_fk.reason {
            Some(r) => format!(" @no_fk(\"{}\")", esc(&r.node)),
            None => " @no_fk".to_string(),
        });
    }
    if let Some(w) = &f.was {
        s.push_str(&format!(" @was(\"{}\")", esc(&w.node)));
    }
    if let Some(sort) = &f.sort {
        s.push_str(&format!(
            " @sort({})",
            sort.iter().map(sort_term).collect::<Vec<_>>().join(", ")
        ));
    }
    s
}

/// Reprint `@fk` / `@fk("reason", on_delete: cascade, on_update: cascade)`. Bare when it
/// carries no reason and no actions; the reason (if any) leads, then the action kwargs.
fn fk_annot(fk: &FkAnnot) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(r) = &fk.reason {
        parts.push(format!("\"{}\"", esc(&r.node)));
    }
    if let Some(a) = &fk.on_delete {
        parts.push(format!("on_delete: {}", a.node));
    }
    if let Some(a) = &fk.on_update {
        parts.push(format!("on_update: {}", a.node));
    }
    if parts.is_empty() {
        " @fk".to_string()
    } else {
        format!(" @fk({})", parts.join(", "))
    }
}

fn modifiers_group(mods: &[Modifier]) -> String {
    format!(
        "({})",
        mods.iter().map(modifier).collect::<Vec<_>>().join(", ")
    )
}

fn modifier(m: &Modifier) -> String {
    match m {
        Modifier::Unique => "unique".to_string(),
        Modifier::Default(v) => format!("default {}", default_val(v)),
        Modifier::Column(c) => format!("column \"{}\"", esc(c)),
    }
}

pub(crate) fn field_of(m: &Member) -> Option<&Field> {
    match m {
        Member::Field(f) => Some(f),
        _ => None,
    }
}
