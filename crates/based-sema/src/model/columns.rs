use super::*;

/// The model-level `@no_fk` opt-out (whole table): whether it is present, plus its optional
/// reason string and decorator span (the divergence check needs both). Only the last
/// `@no_fk` decorator is recorded — repeating it is meaningless.
pub(crate) fn model_no_fk(m: &Model) -> Option<(Option<String>, Span)> {
    m.decorators
        .iter()
        .rev()
        .find(|d| d.name.node == "no_fk")
        .map(|d| {
            let reason = match d.args.first() {
                Some(DecoArg::Lit(Literal::Str(s))) if !s.trim().is_empty() => Some(s.clone()),
                _ => None,
            };
            (reason, d.span)
        })
}

/// Whether the model carries `@no_id("reason")` (a keyless legacy table). The reason is
/// mandatory — an empty or missing one is, so a forfeited primary key is never
/// silent in review.
pub(crate) fn model_no_id(m: &Model, sink: &mut Sink) -> bool {
    let mut keyless = false;
    for d in &m.decorators {
        if d.name.node != "no_id" {
            continue;
        }
        keyless = true;
        let reason = match d.args.first() {
            Some(DecoArg::Lit(Literal::Str(s))) => Some(s.trim()),
            _ => None,
        };
        if reason.is_none_or(str::is_empty) {
            sink.error(
                code::NO_ID_REASON,
                d.span,
                format!(
                    "`@no_id` on `{}` needs a reason — `@no_id(\"why this table has no primary key\")`",
                    m.name.node
                ),
            );
        }
    }
    keyless
}

/// The model-level `@was("old_table")` rename directive's old table name, if declared.
/// A generic decorator (`@was` is not a distinct grammar form model-side).
pub(crate) fn model_was(m: &Model) -> Option<String> {
    for d in &m.decorators {
        if d.name.node == "was" {
            if let Some(DecoArg::Lit(Literal::Str(s))) = d.args.first() {
                return Some(s.clone());
            }
        }
    }
    None
}

/// Physical table name: `@table("…")` override else `snake_case(Name)`. A `.` in the
/// override is a namespace prefix in the wrong place — the schema/database qualifier is
/// `@schema`, so the table name is emitted as a single quoted identifier.
pub(crate) fn table_name(m: &Model, sink: &mut Sink) -> String {
    for d in &m.decorators {
        if d.name.node == "table" {
            if let Some(DecoArg::Lit(Literal::Str(s))) = d.args.first() {
                if s.contains('.') {
                    sink.error_note(
                        code::TABLE_QUALIFIED,
                        d.span,
                        format!("`@table(\"{s}\")` contains a `.`"),
                        "a schema/database qualifier belongs in `@schema(\"…\")`; `@table` names one table",
                    );
                }
                return s.clone();
            }
        }
    }
    snake_case(&m.name.node)
}

/// The SQL schema (Postgres) / database (MySQL/MariaDB) namespace from `@schema("name")`,
/// or `None` for the default namespace. The name must be a single bare identifier — a
/// dotted, empty, or whitespace value is (a multi-level `db.schema` qualifier is
/// not modelled; one namespace level).
pub(crate) fn model_schema(m: &Model, sink: &mut Sink) -> Option<String> {
    for d in &m.decorators {
        if d.name.node != "schema" {
            continue;
        }
        let Some(DecoArg::Lit(Literal::Str(s))) = d.args.first() else {
            sink.error_note(
                code::SCHEMA_INVALID,
                d.span,
                "`@schema` needs a name".to_string(),
                "write the schema/database as a string: `@schema(\"analytics\")`",
            );
            return None;
        };
        let valid = !s.is_empty() && !s.contains('.') && !s.chars().any(char::is_whitespace);
        if !valid {
            sink.error_note(
                code::SCHEMA_INVALID,
                d.span,
                format!("`@schema(\"{s}\")` is not a valid namespace name"),
                "use a single bare identifier — a schema (Postgres) or database (MySQL/MariaDB) name",
            );
            return None;
        }
        return Some(s.clone());
    }
    None
}

/// Classify a field as a scalar column, a forward relation (FK here), or an
/// inverse edge (`[]`, or an explicit `(Model.field)` pairing). An UpperCamel type
/// that names a declared `enum` is a scalar column (carrying `enum_name`), not a
/// relation — sema disambiguates by what the name resolves to. The stored type follows
/// the enum's kind: text for a string enum, integer for an int enum.
pub(crate) fn classify(f: &Field, enums: &HashMap<String, EnumKind>) -> MemberKind {
    let default = || {
        f.modifiers.iter().find_map(|m| match m {
            Modifier::Default(dv) => Some(dv.clone()),
            _ => None,
        })
    };
    let unique = f.modifiers.iter().any(|m| matches!(m, Modifier::Unique));
    let column = || column_override(f).unwrap_or_else(|| f.name.node.clone());
    match &f.ty.base {
        BaseType::Primitive(p) => MemberKind::Scalar {
            ty: *p,
            optional: f.ty.optional,
            many: f.ty.many,
            column: column(),
            unique,
            default: default(),
            enum_name: None,
            raw_type: None,
            generated: None,
        },
        // An opaque column: the engine carries the literal type string into DDL and the
        // snapshot and treats the value as text everywhere else.
        BaseType::Raw(spec) => MemberKind::Scalar {
            ty: Primitive::Text,
            optional: f.ty.optional,
            many: false,
            column: column(),
            unique,
            default: default(),
            enum_name: None,
            raw_type: Some(spec.clone()),
            generated: None,
        },
        BaseType::Model(target) if enums.contains_key(&target.node) => MemberKind::Scalar {
            // An enum column stores its variant's wire value: text for a string enum,
            // integer for an int enum. `enum_name` marks it for the DB CHECK constraint,
            // the client's real enum, and variant membership checks.
            ty: match enums[&target.node] {
                EnumKind::Int => Primitive::Int,
                EnumKind::Str => Primitive::Text,
            },
            optional: f.ty.optional,
            many: f.ty.many,
            column: column(),
            unique,
            default: default(),
            enum_name: Some(target.node.clone()),
            raw_type: None,
            generated: None,
        },
        BaseType::Model(target) => {
            // A to-many model edge, or one carrying an explicit inverse ref, is a
            // back edge; its FK lives on the target. `via` is filled in `validate`
            // (explicit ref, or inferred from the target's forward edges).
            if f.ty.many || f.inverse.is_some() {
                MemberKind::Inverse {
                    target: target.node.clone(),
                    via: f
                        .inverse
                        .as_ref()
                        .map(|iv| iv.field.node.clone())
                        .unwrap_or_default(),
                }
            } else {
                let fk_col = column_override(f).unwrap_or_else(|| format!("{}_id", f.name.node));
                MemberKind::Forward {
                    target: target.node.clone(),
                    optional: f.ty.optional,
                    fk_col,
                    custom_on: f.relation_on.clone(),
                    fk: fk_decl(f),
                }
            }
        }
    }
}

/// Carry a field's `@fk`/`@no_fk` intent into the resolved member. Presence is left
/// unresolved (it needs the `foreign_keys` convention); an unknown action maps to `None`
/// and is flagged in [`validate_fk`].
pub(crate) fn fk_decl(f: &Field) -> FkDecl {
    FkDecl {
        fk: f.fk.is_some(),
        fk_reason: f
            .fk
            .as_ref()
            .and_then(|a| a.reason.as_ref())
            .map(|r| r.node.clone()),
        fk_span: f.fk.as_ref().map(|a| a.span),
        no_fk: f.no_fk.is_some(),
        no_fk_reason: f
            .no_fk
            .as_ref()
            .and_then(|a| a.reason.as_ref())
            .map(|r| r.node.clone()),
        no_fk_span: f.no_fk.as_ref().map(|a| a.span),
        on_delete: f
            .fk
            .as_ref()
            .and_then(|a| a.on_delete.as_ref())
            .and_then(|s| FkAction::parse(&s.node)),
        on_update: f
            .fk
            .as_ref()
            .and_then(|a| a.on_update.as_ref())
            .and_then(|s| FkAction::parse(&s.node)),
    }
}

pub(crate) fn column_override(f: &Field) -> Option<String> {
    f.modifiers.iter().find_map(|m| match m {
        Modifier::Column(c) => Some(c.clone()),
        _ => None,
    })
}
