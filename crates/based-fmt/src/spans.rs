//! Span/header accessors shared across the printer: the start byte a comment
//! anchors to for each construct, a declaration's span, and the model-header
//! item (decorator or `@scope` ref) the header lays out in source order.

use crate::*;

/// The leading ident's start byte of a shape field, used to place comments between the
/// fields of a block-form shape body.
pub(crate) fn shape_field_start(f: &ShapeField) -> Option<u32> {
    Some(match f {
        ShapeField::Bare(id) => id.span.start,
        ShapeField::Rename { out, .. } | ShapeField::Flatten { out, .. } => out.span.start,
        ShapeField::Nest { field, .. } | ShapeField::NestRef { field, .. } => field.span.start,
        ShapeField::Spread { shape } => shape.span.start,
    })
}

pub(crate) fn member_span(m: &Member) -> Span {
    match m {
        Member::Field(f) => f.span,
        Member::Index(i) => i.span,
        Member::SoftOverride(s) => s.raw.span,
        Member::Generated(g) => g.span,
    }
}

pub(crate) fn decl_span(d: &Decl) -> Span {
    match d {
        Decl::Model(m) => m.span,
        Decl::Shape(s) => s.span,
        Decl::Scope(s) => s.span,
        Decl::Enum(e) => e.span,
        Decl::Query(q) => q.span,
        Decl::Mutation(m) => m.span,
        Decl::Filter(f) => f.span,
    }
}

pub(crate) fn decl_start(d: &Decl) -> u32 {
    decl_span(d).start
}

pub(crate) enum HeaderItem<'a> {
    Deco(&'a Decorator),
    Scope(&'a ScopeRef),
}

impl HeaderItem<'_> {
    pub(crate) fn start(&self) -> u32 {
        match self {
            HeaderItem::Deco(d) => d.span.start,
            HeaderItem::Scope(s) => s.span.start,
        }
    }
    pub(crate) fn end(&self) -> u32 {
        match self {
            HeaderItem::Deco(d) => d.span.end,
            HeaderItem::Scope(s) => s.span.end,
        }
    }
    pub(crate) fn render(&self) -> String {
        match self {
            HeaderItem::Deco(d) => decorator(d),
            HeaderItem::Scope(s) => format!(
                "@scope {}",
                s.names
                    .iter()
                    .map(|n| n.node.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}
