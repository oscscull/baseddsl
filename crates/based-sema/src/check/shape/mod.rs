//! Shape-declaration checks: bodies, nesting, `-> Shape` refs, junction flatten, cycles.
use super::*;

mod check_shape;
mod body;
mod nest_target;
mod nest_ref;
mod flatten_path;
mod ref_cycle;

use body::*;
use nest_target::*;
use nest_ref::*;
use flatten_path::*;
use ref_cycle::*;
pub(crate) use check_shape::check_shape;
