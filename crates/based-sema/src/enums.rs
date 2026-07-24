//! Enum resolution + variant checks.
//!
//! An `enum Name { … }` is a closed set of variants and a first-class scalar type. Its
//! name shares the type-name namespace with models/shapes/scopes, so a collision is a
//! duplicate. Kind is inferred from the variant values: a string enum (bare or
//! explicit-string variants) stores text + CHECK; an int enum (every variant an int)
//! stores an integer column + CHECK and additionally allows ordered comparison. A field
//! typed by an enum name is a scalar column (classified in `model`); this module
//! resolves the decls and checks the enum-specific value sites the shared checker can't:
//! a field's `default <variant>` and (via `resolve::check_enum_operand`) a
//! `where`/`create`/`update` variant.

use std::collections::HashMap;

use based_ast::{DefaultVal, EnumDecl, EnumVariant, VariantValue};

use crate::ir::*;
use crate::resolve::Cx;

/// Resolve the `enum` decls into `REnum`s + a name→index map. `taken` is the set of
/// names already claimed by models/shapes/scopes; an enum colliding with one of those,
/// or with another enum, is `E0106` (the type-name namespace is shared). Per enum: the
/// kind is inferred (mixing an int variant with a bare/string one is `E0156`), duplicate
/// variant names are `E0104`, and two variants sharing a wire value are `E0157`.
pub fn resolve_decls(
    decls: &[&EnumDecl],
    taken: &std::collections::HashSet<String>,
    sink: &mut Sink,
) -> (Vec<REnum>, HashMap<String, usize>) {
    let mut enums = Vec::new();
    let mut index = HashMap::new();
    for d in decls {
        let name = d.name.node.clone();
        if taken.contains(&name) || index.contains_key(&name) {
            sink.error(
                code::DUP_ENUM,
                d.name.span,
                format!("`{name}` is already declared (a model, shape, scope, or enum)"),
            );
            continue;
        }
        index.insert(name.clone(), enums.len());
        enums.push(resolve_enum(d, sink));
    }
    (enums, index)
}

/// Infer an enum's kind and resolve each variant's wire value. A variant with no explicit
/// value is a string variant whose value is its own name.
fn resolve_enum(d: &EnumDecl, sink: &mut Sink) -> REnum {
    let any_int = d.variants.iter().any(|v| {
        matches!(
            v.value.as_ref().map(|s| &s.node),
            Some(VariantValue::Int(_))
        )
    });
    let kind = if any_int {
        EnumKind::Int
    } else {
        EnumKind::Str
    };

    let mut variants: Vec<REnumVariant> = Vec::new();
    let mut seen_names: HashMap<String, ()> = HashMap::new();
    for v in &d.variants {
        // Duplicate variant name (a repeated member) — E0104, the model/field dup code.
        if seen_names.insert(v.name.node.clone(), ()).is_some() {
            sink.error(
                code::DUP_FIELD,
                v.name.span,
                format!(
                    "duplicate variant `{}` in enum `{}`",
                    v.name.node, d.name.node
                ),
            );
            continue;
        }
        let value = resolve_variant_value(d, v, kind, sink);
        variants.push(REnumVariant {
            name: v.name.node.clone(),
            span: v.name.span,
            value,
        });
    }

    check_duplicate_values(d, &variants, sink);

    REnum {
        name: d.name.node.clone(),
        span: d.span,
        kind,
        variants,
    }
}

/// The wire value of one variant, given the enum's inferred kind. An int enum rejects any
/// bare/string variant (`E0156`); a string enum rejects any int variant (`E0156`).
fn resolve_variant_value(
    d: &EnumDecl,
    v: &EnumVariant,
    kind: EnumKind,
    sink: &mut Sink,
) -> EnumValue {
    match (kind, v.value.as_ref().map(|s| &s.node)) {
        // Int enum: every variant must carry an int.
        (EnumKind::Int, Some(VariantValue::Int(n))) => EnumValue::Int(*n),
        (EnumKind::Int, other) => {
            let span = v.value.as_ref().map_or(v.name.span, |s| s.span);
            sink.error(
                code::ENUM_MIXED,
                span,
                format!(
                    "enum `{}` is numeric (a variant has an int value), so `{}` must also \
                     have an int value",
                    d.name.node, v.name.node
                ),
            );
            // Recover with the value written (a string) or 0, so downstream stays sound.
            match other {
                Some(VariantValue::Str(s)) => EnumValue::Str(s.clone()),
                _ => EnumValue::Int(0),
            }
        }
        // String enum: bare (value = name) or an explicit string.
        (EnumKind::Str, None) => EnumValue::Str(v.name.node.clone()),
        (EnumKind::Str, Some(VariantValue::Str(s))) => EnumValue::Str(s.clone()),
        (EnumKind::Str, Some(VariantValue::Int(n))) => {
            // Unreachable in practice (an int makes the enum Int), kept exhaustive.
            EnumValue::Int(*n)
        }
    }
}

/// Two variants sharing a wire value make the enum's stored value ambiguous — `E0157`.
fn check_duplicate_values(d: &EnumDecl, variants: &[REnumVariant], sink: &mut Sink) {
    for (i, v) in variants.iter().enumerate() {
        if variants[..i].iter().any(|w| w.value == v.value) {
            let show = match &v.value {
                EnumValue::Str(s) => format!("\"{s}\""),
                EnumValue::Int(n) => n.to_string(),
            };
            sink.error(
                code::ENUM_DUP_VALUE,
                v.span,
                format!(
                    "variant `{}` reuses the value {show} already used in enum `{}`",
                    v.name, d.name.node
                ),
            );
        }
    }
}

/// Check every enum-typed field's `default <variant>` names a member of its enum, and
/// that a bare-identifier default appears only on an enum column. Reads the resolved
/// models directly — the variant default rides on `MemberKind::Scalar.default` with its
/// original span, so no AST walk is needed.
pub fn check_field_defaults(cx: &Cx, sink: &mut Sink) {
    for m in cx.models {
        for mem in &m.members {
            let MemberKind::Scalar {
                default: Some(DefaultVal::Variant(v)),
                enum_name,
                ..
            } = &mem.kind
            else {
                continue;
            };
            match enum_name {
                Some(en_name) => {
                    if let Some(en) = cx.enum_(en_name) {
                        if !en.has_variant(&v.node) {
                            sink.error(
                                code::ENUM_DEFAULT,
                                v.span,
                                format!(
                                    "default `{}` is not a variant of enum `{}` (expected one of: {})",
                                    v.node,
                                    en.name,
                                    en.variant_names().join(", ")
                                ),
                            );
                        }
                    }
                }
                None => sink.error(
                    code::ENUM_DEFAULT,
                    v.span,
                    format!(
                        "`default {}` is a bare identifier, valid only on an `enum` column",
                        v.node
                    ),
                ),
            }
        }
    }
}
