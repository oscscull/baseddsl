//! Shape-declaration checks: bodies, nesting, `-> Shape` refs, junction flatten, cycles.
use super::*;

mod body;
mod check_shape;
mod flatten_path;
mod nest_ref;
mod nest_target;
mod ref_cycle;

use body::*;
pub(crate) use check_shape::check_shape;
use flatten_path::*;
use nest_ref::*;
use nest_target::*;
use ref_cycle::*;
