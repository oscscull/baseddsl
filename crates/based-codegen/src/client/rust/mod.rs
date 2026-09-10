//! The Rust client target: lower the schema to Rust source, one file per concern.

use super::ClientOptions;
use based_ast::*;
use based_sema::{CheckedSchema, CtxField, CtxReq, MemberKind, RModel, RQuery};

mod callable;
mod collect;
mod find_shape;
mod naming;
mod output_struct;
mod params;
mod relations;
mod render;
mod render_enum;
mod render_method;
mod render_struct;
mod templates;
mod types;

use callable::*;
use collect::*;
use find_shape::*;
use naming::*;
use output_struct::*;
use params::*;
use relations::*;
use render_enum::*;
use render_method::*;
use render_struct::*;
use templates::*;
use types::*;

pub(crate) use render::render;
