//! Mutation / write checks: writes, `tx` bindings, assigns, upsert, `create … from` input.
use super::*;

mod check_mutation;
mod write_effect;
mod ack_return;
mod shape_on_delete;
mod keyless_readback;
mod check_write;
mod tx;
mod assign;
mod scope_assign;
mod create_required;
mod is_required;
mod write_model;
mod upsert;
mod create_from;

use write_effect::*;
use ack_return::*;
use shape_on_delete::*;
use keyless_readback::*;
use check_write::*;
use tx::*;
use assign::*;
use scope_assign::*;
use create_required::*;
use is_required::*;
use write_model::*;
use upsert::*;
use create_from::*;
pub(crate) use check_mutation::check_mutation;
