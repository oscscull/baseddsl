//! The emitted client source, one file per emitted surface.

mod adopted;
mod embedded;
mod preamble;
mod streaming;
mod transactions;
mod transport;

pub(crate) use adopted::*;
pub(crate) use embedded::*;
pub(crate) use preamble::*;
pub(crate) use streaming::*;
pub(crate) use transactions::*;
pub(crate) use transport::*;
