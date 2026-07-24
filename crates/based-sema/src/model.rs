//! Model resolution: AST `Model` -> `RModel`.
//!
//! Two phases, because a relation's target and an inverse's forward edge may be
//! declared in any file : [`skeleton`] records every model's fields without
//! cross-checking, then [`validate`] resolves relation targets, inverses,
//! decorators, indexes, and sorts once all skeletons exist.

use based_ast::*;
use std::collections::HashMap;

use crate::ir::*;
use crate::resolve;

/// Build an unvalidated `RModel` from one AST model: implicit `id`, columns, and
/// relations classified forward vs. inverse. No cross-model checks yet. `enums` maps each
/// declared enum name to its kind, so an UpperCamel field type resolving to an enum is a
/// scalar column (text for a string enum, integer for an int enum) rather than a relation.
pub fn skeleton(m: &Model, enums: &HashMap<String, EnumKind>, sink: &mut Sink) -> RModel {
    let mut members: Vec<RMember> = Vec::new();
    let mut seen: HashMap<String, Span> = HashMap::new();

    for mem in &m.members {
        let Member::Field(f) = mem else { continue };
        if seen.contains_key(&f.name.node) {
            sink.error(
                code::DUP_FIELD,
                f.name.span,
                format!(
                    "duplicate field `{}` in model `{}`",
                    f.name.node, m.name.node
                ),
            );
            continue;
        }
        seen.insert(f.name.node.clone(), f.name.span);
        // Field `(default now())` functions are validated against the closed set.
        for m in &f.modifiers {
            if let Modifier::Default(dv) = m {
                resolve::check_default(dv, sink);
            }
        }
        members.push(RMember {
            name: f.name.node.clone(),
            span: f.name.span,
            kind: classify(f, enums),
            was: f.was.as_ref().map(|w| w.node.clone()),
            sort: f.sort.clone().unwrap_or_default(),
        });
    }

    // `@no_id("reason")` opts a genuinely keyless legacy table out of the primary key
    // (the reason is mandatory, so the PR shows why — `E0262`).
    let no_id = model_no_id(m, sink);

    // `@no_fk` opts the whole table out of FK constraints (reason checked, if required,
    // in the manifest-dependent divergence pass).
    let no_fk = model_no_fk(m);

    // A model's primary key is load-bearing and written in source. A model that
    // declares no `id` field is an error (`E0261`) with a one-key autofix; the `id`
    // member is still synthesized so the rest of resolution + codegen has a PK to
    // key on, but the source must name it. A `@no_id` model legitimately has none.
    if !seen.contains_key("id") && !no_id {
        sink.error_fix(
            code::NO_ID,
            m.name.span,
            format!("model `{}` declares no `id`", m.name.node),
            "every model needs a primary key — add an `id: Id` field, or `@no_id(\"reason\")` for a keyless legacy table",
            m.name.node.clone(),
            "id: Id",
        );
        members.insert(
            0,
            RMember {
                name: "id".to_string(),
                span: m.name.span,
                kind: MemberKind::Scalar {
                    ty: Primitive::Id,
                    optional: false,
                    many: false,
                    column: "id".to_string(),
                    unique: false, // PK, expressed as PRIMARY KEY not a UNIQUE constraint
                    default: None, // engine-generated on insert , no SQL default
                    enum_name: None,
                    raw_type: None,
                },
                was: None,
                sort: Vec::new(),
            },
        );
    }

    let table = table_name(m);
    RModel {
        name: m.name.node.clone(),
        span: m.span,
        table,
        members,
        soft_delete: None,
        sort: Vec::new(),
        scope: None,
        scope_alts: Vec::new(),
        created: None,
        updated: None,
        indexes: Vec::new(),
        no_id,
        no_fk: no_fk.is_some(),
        no_fk_reason: no_fk.as_ref().and_then(|(r, _)| r.clone()),
        no_fk_span: no_fk.map(|(_, s)| s),
        unique_cols: Vec::new(),
        was: model_was(m),
    }
}

/// The model-level `@no_fk` opt-out (whole table): whether it is present, plus its optional
/// reason string and decorator span (the divergence check needs both). Only the last
/// `@no_fk` decorator is recorded — repeating it is meaningless.
fn model_no_fk(m: &Model) -> Option<(Option<String>, Span)> {
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
/// mandatory — an empty or missing one is `E0262`, so a forfeited primary key is never
/// silent in review.
fn model_no_id(m: &Model, sink: &mut Sink) -> bool {
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
fn model_was(m: &Model) -> Option<String> {
    for d in &m.decorators {
        if d.name.node == "was" {
            if let Some(DecoArg::Lit(Literal::Str(s))) = d.args.first() {
                return Some(s.clone());
            }
        }
    }
    None
}

/// Physical table name: `@table("…")` override else `snake_case(Name)`.
fn table_name(m: &Model) -> String {
    for d in &m.decorators {
        if d.name.node == "table" {
            if let Some(DecoArg::Lit(Literal::Str(s))) = d.args.first() {
                return s.clone();
            }
        }
    }
    snake_case(&m.name.node)
}

/// Classify a field as a scalar column, a forward relation (FK here), or an
/// inverse edge (`[]`, or an explicit `(Model.field)` pairing). An UpperCamel type
/// that names a declared `enum` is a scalar column (carrying `enum_name`), not a
/// relation — sema disambiguates by what the name resolves to. The stored type follows
/// the enum's kind: text for a string enum, integer for an int enum.
fn classify(f: &Field, enums: &HashMap<String, EnumKind>) -> MemberKind {
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
                    custom_join: f.relation_on.is_some(),
                    fk: fk_decl(f),
                }
            }
        }
    }
}

/// Carry a field's `@fk`/`@no_fk` intent into the resolved member. Presence is left
/// unresolved (it needs the `foreign_keys` convention); an unknown action maps to `None`
/// and is flagged `E0294` in [`validate_fk`].
fn fk_decl(f: &Field) -> FkDecl {
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

fn column_override(f: &Field) -> Option<String> {
    f.modifiers.iter().find_map(|m| match m {
        Modifier::Column(c) => Some(c.clone()),
        _ => None,
    })
}

/// Resolve the structural facts that only touch *this* model plus name lookups:
/// relation targets, inverse pairings, decorator roles, indexes, and uniqueness.
/// Expression resolution (scope/sort paths, which traverse *other* models) is done
/// afterwards in [`resolve_exprs`], once every model is fully built — otherwise the
/// read-only path checker would alias this `&mut` pass.
pub fn validate(
    ast: &Model,
    mi: usize,
    models: &mut [RModel],
    index: &HashMap<String, usize>,
    sink: &mut Sink,
) {
    validate_relations(mi, models, index, sink);
    validate_indexes(ast, mi, models, sink);
    validate_decorators(ast, mi, models, sink);
    validate_fk(ast, mi, models, sink);
    validate_was(ast, mi, models, sink);
    validate_decimals(ast, sink);
    compute_unique(ast, &mut models[mi]);
}

/// The largest `decimal` precision the engine's type map guarantees across dialects.
const DECIMAL_MAX_PRECISION: u32 = 38;

/// Check every `decimal(p, s)` field's precision/scale is in range (`1 ≤ s ≤ p ≤ 38`)
/// and that a decimal column's `default` is a decimal literal (an integer or a fractional
/// literal), not a string/bool — both `E0159`. Purely local (one model's own fields).
fn validate_decimals(ast: &Model, sink: &mut Sink) {
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        let BaseType::Primitive(Primitive::Decimal { precision, scale }) = f.ty.base else {
            continue;
        };
        if !(1 <= scale && scale <= precision && precision <= DECIMAL_MAX_PRECISION) {
            sink.error(
                code::DECIMAL_INVALID,
                f.ty.span,
                format!(
                    "`decimal({precision}, {scale})` is out of range — need \
                     1 ≤ scale ≤ precision ≤ {DECIMAL_MAX_PRECISION}"
                ),
            );
        }
        for m in &f.modifiers {
            let Modifier::Default(DefaultVal::Lit(lit)) = m else {
                continue;
            };
            if !matches!(lit, Literal::Int(_) | Literal::Decimal(_) | Literal::Null) {
                sink.error(
                    code::DECIMAL_INVALID,
                    f.span,
                    format!(
                        "default for decimal column `{}` must be a decimal literal",
                        f.name.node
                    ),
                );
            }
        }
    }
}

/// Validate `@fk`/`@no_fk` on relation fields — the target-independent, convention-free
/// half of the FK checks (the reason-vs-convention divergence rule runs later, in the
/// manifest-dependent pass). Both decorators are valid only on a *forward to-one* relation
/// that owns a conventional `<field>_id` FK column:
///   * on an inverse / `[]` edge, or a scalar column → `E0290`,
///   * on a custom-join (`on:`) relation → `E0291` (no conventional FK column),
///   * both `@fk` and `@no_fk` on one edge → `E0292`,
///   * `@fk(on_delete: set_null)` on a required (non-nullable) relation → `E0293`,
///   * an unknown referential action → `E0294`.
fn validate_fk(ast: &Model, mi: usize, models: &[RModel], sink: &mut Sink) {
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        if f.fk.is_none() && f.no_fk.is_none() {
            continue;
        }
        let span =
            f.fk.as_ref()
                .map(|a| a.span)
                .or_else(|| f.no_fk.as_ref().map(|a| a.span))
                .unwrap_or(f.span);
        // `@fk` + `@no_fk` on the same edge is contradictory.
        if f.fk.is_some() && f.no_fk.is_some() {
            sink.error(
                code::FK_CONFLICT,
                span,
                format!(
                    "`{}` carries both `@fk` and `@no_fk` — an FK is either opted in or out, not both",
                    f.name.node
                ),
            );
            continue;
        }
        // The decorators mean something only on a forward to-one relation column.
        if let Some(MemberKind::Forward { custom_join, .. }) =
            models[mi].member(&f.name.node).map(|m| &m.kind)
        {
            if *custom_join {
                sink.error(
                    code::FK_CUSTOM_JOIN,
                    span,
                    format!(
                        "`@fk`/`@no_fk` on `{}` — a custom-join (`on:`) relation owns no conventional FK column",
                        f.name.node
                    ),
                );
                continue;
            }
        } else {
            sink.error(
                code::FK_TARGET,
                span,
                format!(
                    "`@fk`/`@no_fk` on `{}` — only a forward to-one relation (which owns the `{}_id` FK column) can carry one",
                    f.name.node, f.name.node
                ),
            );
            continue;
        }
        // Referential actions: known spelling, and `set_null` needs a nullable relation.
        if let Some(fk) = &f.fk {
            for act in [&fk.on_delete, &fk.on_update].into_iter().flatten() {
                match FkAction::parse(&act.node) {
                    None => sink.error(
                        code::FK_ACTION,
                        act.span,
                        format!(
                            "unknown referential action `{}` — use cascade, restrict, set_null, or no_action",
                            act.node
                        ),
                    ),
                    Some(FkAction::SetNull) if !f.ty.optional => sink.error(
                        code::FK_SET_NULL_REQUIRED,
                        act.span,
                        format!(
                            "`set_null` on required relation `{}` — make it optional (`{}: {}?`) for the FK to null it",
                            f.name.node,
                            f.name.node,
                            type_base_name(f)
                        ),
                    ),
                    _ => {}
                }
            }
        }
    }
}

/// The relation's target model name, for a diagnostic hint (`field: Target?`).
fn type_base_name(f: &Field) -> String {
    match &f.ty.base {
        BaseType::Model(m) => m.node.clone(),
        _ => "Target".to_string(),
    }
}

/// Validate `@was` rename directives. A `@was` names a *previous*
/// name — one that lives only in the migration snapshot, so sema can't confirm it existed
/// (the diff does). It can catch the two locally-decidable mistakes: a no-op self-rename
/// (`E0190`) and an old name that is still a *live* column/table (`E0191` — then it can't
/// be the rename's source). Field-level `@was` sits in the field modifier position; the
/// model-level form is a generic decorator.
fn validate_was(ast: &Model, mi: usize, models: &[RModel], sink: &mut Sink) {
    // Field-level: `<field>: <ty> @was("old_col")`.
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        let Some(was) = &f.was else { continue };
        let old = &was.node;
        let current = models[mi]
            .member(&f.name.node)
            .map(|m| m.physical_col().to_string());
        if current.as_deref() == Some(old.as_str()) {
            sink.error(
                code::WAS_NOOP,
                was.span,
                format!("`@was(\"{old}\")` renames `{old}` to itself — remove it"),
            );
        } else if models[mi].column(old).is_some() {
            sink.error_note(
                code::WAS_LIVE,
                was.span,
                format!(
                    "`@was(\"{old}\")` names a column that still exists in `{}`",
                    ast.name.node
                ),
                "`@was` names a *previous* column name; a live column can't be a rename source",
            );
        }
    }
    // Model-level: `@was("old_table")` decorator.
    for d in &ast.decorators {
        if d.name.node != "was" {
            continue;
        }
        let Some(DecoArg::Lit(Literal::Str(old))) = d.args.first() else {
            continue;
        };
        if *old == models[mi].table {
            sink.error(
                code::WAS_NOOP,
                d.span,
                format!("`@was(\"{old}\")` renames table `{old}` to itself — remove it"),
            );
        } else if models.iter().any(|m| &m.table == old) {
            sink.error_note(
                code::WAS_LIVE,
                d.span,
                format!("`@was(\"{old}\")` names a table that still exists"),
                "`@was` names a *previous* table name; a live table can't be a rename source",
            );
        }
    }
}

fn validate_relations(
    mi: usize,
    models: &mut [RModel],
    index: &HashMap<String, usize>,
    sink: &mut Sink,
) {
    // Collect fixups without holding an aliasing borrow of `models`.
    let mut infer: Vec<(usize, String)> = Vec::new(); // (member idx, inferred via)
    {
        let m = &models[mi];
        for (i, mem) in m.members.iter().enumerate() {
            match &mem.kind {
                MemberKind::Forward { target, .. } => match index.get(target) {
                    None => sink.error(
                        code::UNKNOWN_MODEL,
                        mem.span,
                        format!("relation `{}` names unknown model `{target}`", mem.name),
                    ),
                    // A to-one relation's FK references the target's `id`; a keyless
                    // target has none, so the edge could never resolve.
                    Some(&ti) if models[ti].no_id => sink.error_note(
                        code::REL_TO_KEYLESS,
                        mem.span,
                        format!(
                            "relation `{}` targets the keyless model `{target}`",
                            mem.name
                        ),
                        "a `@no_id` model has no `id` for a foreign key to reference",
                    ),
                    Some(_) => {}
                },
                MemberKind::Inverse { target, via } => {
                    let Some(&ti) = index.get(target) else {
                        sink.error(
                            code::UNKNOWN_MODEL,
                            mem.span,
                            format!("relation `{}` names unknown model `{target}`", mem.name),
                        );
                        continue;
                    };
                    if via.is_empty() {
                        // Infer: the unique forward edge on `target` back to us.
                        match infer_inverse(&models[ti], &m.name) {
                            Ok(field) => infer.push((i, field)),
                            Err(msg) => sink.error(code::INVERSE_INFER, mem.span, msg),
                        }
                    } else {
                        // Explicit `(Model.field)`: must be a forward edge to us.
                        check_inverse_ref(&models[ti], via, &m.name, mem, sink);
                    }
                }
                MemberKind::Scalar { .. } => {}
            }
        }
    }
    for (i, field) in infer {
        if let MemberKind::Inverse { via, .. } = &mut models[mi].members[i].kind {
            *via = field;
        }
    }
}

/// The unique forward edge on `target` whose type is `me`; error text otherwise.
fn infer_inverse(target: &RModel, me: &str) -> Result<String, String> {
    let candidates: Vec<&str> = target
        .members
        .iter()
        .filter_map(|mem| match &mem.kind {
            MemberKind::Forward { target: t, .. } if t == me => Some(mem.name.as_str()),
            _ => None,
        })
        .collect();
    match candidates.as_slice() {
        [one] => Ok(one.to_string()),
        [] => Err(format!(
            "no forward edge from `{}` back to `{me}` to invert; add one or an explicit `({}.field)`",
            target.name, target.name
        )),
        many => Err(format!(
            "ambiguous inverse: `{}` has {} edges to `{me}` ({}); disambiguate with `({}.field)`",
            target.name,
            many.len(),
            many.join(", "),
            target.name
        )),
    }
}

fn check_inverse_ref(target: &RModel, via: &str, me: &str, mem: &RMember, sink: &mut Sink) {
    match target.member(via).map(|m| &m.kind) {
        Some(MemberKind::Forward { target: t, .. }) if t == me => {}
        Some(_) => sink.error(
            code::INVERSE_REF,
            mem.span,
            format!("`{}.{via}` is not a forward edge to `{me}`", target.name),
        ),
        None => sink.error(
            code::INVERSE_REF,
            mem.span,
            format!("`{}` has no field `{via}`", target.name),
        ),
    }
}

fn validate_indexes(ast: &Model, mi: usize, models: &mut [RModel], sink: &mut Sink) {
    let mut indexes = Vec::new();
    for mem in &ast.members {
        let Member::Index(idx) = mem else { continue };
        for col in &idx.columns {
            if models[mi].member(&col.node).is_none() {
                sink.error(
                    code::INDEX_COLUMN,
                    col.span,
                    format!("index names unknown column `{}`", col.node),
                );
            }
        }
        if let Some(m) = &idx.method {
            if index_method_targets(&m.node).is_none() {
                let known = INDEX_METHODS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ");
                sink.error_note(
                    code::INDEX_METHOD,
                    m.span,
                    format!("unknown index access method `{}`", m.node),
                    format!("known methods: {known}"),
                );
            }
        }
        if let Some(spec) = &idx.raw {
            check_raw_spec(spec, "index", sink);
        }
        indexes.push(RIndex {
            columns: idx.columns.iter().map(|c| c.node.clone()).collect(),
            unique: idx.unique,
            method: idx.method.as_ref().map(|m| m.node.clone()),
            raw: idx.raw.clone(),
            span: idx.span,
        });
    }
    models[mi].indexes = indexes;
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        if let BaseType::Raw(spec) = &f.ty.base {
            check_raw_spec(spec, "type", sink);
        }
    }
}

/// A `raw(…)` body must carry a non-empty literal for every dialect it names — an empty
/// one would emit a syntactically broken column type or index (`E0274`).
fn check_raw_spec(spec: &RawSpec, what: &str, sink: &mut Sink) {
    for lit in spec.literals() {
        if lit.node.trim().is_empty() {
            sink.error(
                code::RAW_EMPTY,
                lit.span,
                format!("an empty `raw(…)` {what} body"),
            );
        }
    }
    if let RawSpecBody::PerDialect(entries) = &spec.body {
        for e in entries {
            if !DIALECTS.contains(&e.dialect.node.as_str()) {
                sink.error_note(
                    code::RAW_TYPE_DIALECT,
                    e.dialect.span,
                    format!("unknown dialect `{}` in a `raw({{…}})` map", e.dialect.node),
                    format!("compile targets: {}", DIALECTS.join(", ")),
                );
            }
        }
    }
}

fn validate_decorators(ast: &Model, mi: usize, models: &mut [RModel], sink: &mut Sink) {
    let mut sort_terms: Vec<SortTerm> = Vec::new();
    for d in &ast.decorators {
        match d.name.node.as_str() {
            "soft_delete" => {
                if let Some(field) = deco_field(d) {
                    resolve_soft_delete(field, mi, models, sink);
                }
            }
            "created" | "updated" => {
                if let Some(field) = deco_field(d) {
                    resolve_managed_ts(&d.name.node, field, mi, models, sink);
                }
            }
            // `@scope Name` is parsed into `Model.scopes`, not `decorators`, and is
            // resolved by the scope pass — it never reaches here.
            "sort" => {
                for a in &d.args {
                    if let Some(t) = deco_sort_term(a) {
                        sort_terms.push(t);
                    }
                }
            }
            "table" => {} // consumed for the table name in `skeleton`
            "was" => {}   // model rename directive — validated in `validate_was`
            "no_id" => {} // keyless opt-out — consumed + validated in `skeleton`
            "no_fk" => {} // whole-table FK opt-out — consumed in `skeleton`, divergence in the manifest pass
            other => sink.warn(
                code::UNKNOWN_DECORATOR,
                d.name.span,
                format!("unknown decorator `@{other}` (ignored)"),
            ),
        }
    }
    models[mi].sort = sort_terms;
}

/// A `@sort` decorator argument as a sort term. A bare path carries no direction
/// token, so the argument scan can't classify it as a sort — it arrives as an
/// `Ident`/`Path` arg and defaults to ascending here (grammar: the direction is
/// optional, bare = `asc`).
fn deco_sort_term(a: &DecoArg) -> Option<SortTerm> {
    match a {
        DecoArg::Sort(t) => Some(t.clone()),
        DecoArg::Ident(id) => Some(SortTerm {
            path: Path {
                segments: vec![id.clone()],
            },
            dir: SortDir::Asc,
        }),
        DecoArg::Path(p) => Some(SortTerm {
            path: p.clone(),
            dir: SortDir::Asc,
        }),
        DecoArg::Pred(_) | DecoArg::Lit(_) => None,
    }
}

/// A decorator's target field, from a bare ident or a single-segment path.
fn deco_field(d: &Decorator) -> Option<&Ident> {
    match d.args.first()? {
        DecoArg::Ident(id) => Some(id),
        DecoArg::Path(p) if p.segments.len() == 1 => Some(&p.segments[0]),
        _ => None,
    }
}

fn resolve_soft_delete(field: &Ident, mi: usize, models: &mut [RModel], sink: &mut Sink) {
    let mode = match models[mi].member(&field.node).map(|m| &m.kind) {
        Some(MemberKind::Scalar {
            ty: Primitive::Timestamp | Primitive::Date,
            optional: true,
            many: false,
            ..
        }) => Some(SoftMode::Timestamp),
        Some(MemberKind::Scalar {
            ty: Primitive::Bool,
            many: false,
            ..
        }) => Some(SoftMode::Bool),
        Some(_) => {
            sink.error_note(
                code::SOFT_DELETE_TYPE,
                field.span,
                format!("`{}` cannot back @soft_delete", field.node),
                "covered subset: nullable `timestamp`/`date`, or `bool` — else drop to a raw override",
            );
            None
        }
        None => {
            sink.error(
                code::DECO_TARGET,
                field.span,
                format!("@soft_delete names unknown field `{}`", field.node),
            );
            None
        }
    };
    if let Some(mode) = mode {
        models[mi].soft_delete = Some(SoftDelete {
            field: field.node.clone(),
            mode,
        });
    }
}

fn resolve_managed_ts(
    deco: &str,
    field: &Ident,
    mi: usize,
    models: &mut [RModel],
    sink: &mut Sink,
) {
    match models[mi].member(&field.node).map(|m| &m.kind) {
        Some(MemberKind::Scalar {
            ty: Primitive::Timestamp | Primitive::Date,
            ..
        }) => {
            if deco == "created" {
                models[mi].created = Some(field.node.clone());
            } else {
                models[mi].updated = Some(field.node.clone());
            }
        }
        Some(_) => sink.error(
            code::DECO_TARGET,
            field.span,
            format!("@{deco} field `{}` must be a timestamp/date", field.node),
        ),
        None => sink.error(
            code::DECO_TARGET,
            field.span,
            format!("@{deco} names unknown field `{}`", field.node),
        ),
    }
}

/// Field names that are individually unique: `id`, `(unique)` scalars, and
/// single-column unique indexes. (Composite unique indexes make no *single*
/// column unique, so they do not count here.)
fn compute_unique(ast: &Model, m: &mut RModel) {
    // A keyless model has no `id`, so it seeds no unique column from one.
    let mut unique: Vec<String> = if m.no_id {
        Vec::new()
    } else {
        vec!["id".to_string()]
    };
    for mem in &ast.members {
        match mem {
            Member::Field(f) if f.modifiers.iter().any(|x| matches!(x, Modifier::Unique)) => {
                unique.push(f.name.node.clone());
            }
            Member::Index(idx) if idx.unique && idx.columns.len() == 1 => {
                unique.push(idx.columns[0].node.clone());
            }
            _ => {}
        }
    }
    unique.dedup();
    m.unique_cols = unique;
}

/// Read-only pass over one model's expression-valued decorators/fields: model
/// `@sort` terms, relation-field `@sort` terms, and custom `on:` joins. Run after
/// every model is built, so path traversal into other models is safe. (`@scope`
/// refs are resolved separately by the scope pass.)
pub fn resolve_exprs(ast: &Model, cx: &resolve::Cx, sink: &mut Sink) {
    let Some(mi) = cx.find(&ast.name.node) else {
        return;
    };
    for d in &ast.decorators {
        // Model `@sort` paths traverse into related models, so resolve them here in the
        // read pass. (`@scope` refs are resolved separately by the scope pass.)
        if d.name.node == "sort" {
            for a in &d.args {
                if let Some(t) = deco_sort_term(a) {
                    resolve::check_sort_term(&t, mi, cx, sink);
                }
            }
        }
    }
    // Custom `on:` joins span two tables (this model + the relation target), so
    // resolve them here in the read pass where other models are reachable .
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        let Some(pred) = &f.relation_on else { continue };
        match &f.ty.base {
            // A to-one forward relation — the only edge that owns a join. `on:` on a
            // scalar, an optional is fine; a `[]` / explicit-inverse edge owns no FK.
            BaseType::Model(target) if !f.ty.many && f.inverse.is_none() => {
                if let Some(fi) = cx.find(&target.node) {
                    resolve::check_relation_on(pred, mi, fi, cx, sink);
                }
            }
            _ => sink.error(
                code::JOIN_FORM,
                f.name.span,
                format!(
                    "`on:` custom join applies only to a to-one relation, not `{}`",
                    f.name.node
                ),
            ),
        }
    }

    // Relation `@sort` sorts the *target* rows; resolve terms against the target.
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        let Some(terms) = &f.sort else { continue };
        let ctx = match cx.model(mi).member(&f.name.node).map(|m| &m.kind) {
            Some(MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. }) => {
                cx.find(target)
            }
            _ => Some(mi),
        };
        if let Some(ci) = ctx {
            for t in terms {
                resolve::check_sort_term(t, ci, cx, sink);
            }
        }
    }
}
