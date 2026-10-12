use based_sema::{CheckedSchema, MemberKind, RMember, RModel};

use super::{FieldValue, FixtureField, Placeholder, Role};

/// The fixture field for one member: `None` when a fixture never sets it (an inverse
/// edge, a custom join, a generated column, the soft-delete marker, or an optional
/// opaque column). A required column a fixture cannot write is an error.
pub(super) fn classify<'s>(
    schema: &'s CheckedSchema,
    model: &'s RModel,
    member: &'s RMember,
) -> Result<Option<FixtureField<'s>>, String> {
    match &member.kind {
        MemberKind::Inverse { .. } => Ok(None),
        MemberKind::Forward {
            target,
            optional,
            custom_on,
            ..
        } => {
            if custom_on.is_some() {
                return Ok(None);
            }
            let Some(target) = schema
                .model(target)
                .filter(|t| !t.pk_field_names().is_empty())
            else {
                return if *optional {
                    Ok(None)
                } else {
                    Err(format!(
                        "required relation `{}` points at a keyless model",
                        member.name
                    ))
                };
            };
            Ok(Some(FixtureField {
                name: &member.name,
                role: if *optional {
                    Role::Optional
                } else {
                    Role::Argument
                },
                value: FieldValue::Link {
                    target,
                    optional: *optional,
                },
            }))
        }
        MemberKind::Scalar {
            ty,
            optional,
            many,
            default,
            enum_name,
            raw_type,
            generated,
            ..
        } => {
            let required = !*optional && default.is_none();
            if generated.is_some() || is_tombstone(model, member) {
                return Ok(None);
            }
            if raw_type.is_some() {
                return if required {
                    Err(format!(
                        "required column `{}` has an opaque raw type",
                        member.name
                    ))
                } else {
                    Ok(None)
                };
            }
            let role = if required && !engine_managed(model, member) {
                let enum_decl = enum_name.as_deref().and_then(|e| schema.enum_(e));
                Role::Placeholder(Placeholder::of(*ty, enum_decl, *many))
            } else {
                Role::Optional
            };
            Ok(Some(FixtureField {
                name: &member.name,
                role,
                value: FieldValue::Scalar(member),
            }))
        }
    }
}

/// A keyless model reads its row back by a unique column, so one must always be set.
pub(super) fn check_readback_key(
    model: &RModel,
    fields: &[FixtureField<'_>],
) -> Result<(), String> {
    if !model.pk_field_names().is_empty() {
        return Ok(());
    }
    let keyed = fields
        .iter()
        .any(|f| matches!(f.role, Role::Placeholder(_)) && model.is_unique(f.name));
    if keyed {
        Ok(())
    } else {
        Err("a keyless model needs a required unique column to read the row back by".into())
    }
}

/// Columns the engine fills itself when a fixture leaves them unset.
fn engine_managed(model: &RModel, member: &RMember) -> bool {
    let name = member.name.as_str();
    (model.pk_strategy().is_some() && name == "id")
        || model.created.as_deref() == Some(name)
        || model.updated.as_deref() == Some(name)
        || model
            .serial_key_member()
            .is_some_and(|m| m.name == member.name)
}

/// A fixture is always a live row, so it never sets the soft-delete marker.
fn is_tombstone(model: &RModel, member: &RMember) -> bool {
    model
        .soft_delete
        .as_ref()
        .is_some_and(|s| s.field == member.name)
}
