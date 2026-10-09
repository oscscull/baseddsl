//! Physical catalog facts and the read-only discovery seam shared by import adapters.
//! This crate performs no SQL, connection management, BSL emission, or policy inference.

mod canonical;
mod capability;
mod catalog;
mod check;
mod column;
mod diagnostic;
mod discovery;
mod foreign_key;
mod identity;
mod index;
mod key;
mod native_type;
mod reader;
mod selection;
mod source;
mod table;
mod validation;

#[cfg(feature = "test-support")]
pub mod test_support;

pub use catalog::*;
pub use check::*;
pub use column::*;
pub use diagnostic::*;
pub use discovery::*;
pub use foreign_key::*;
pub use identity::*;
pub use index::*;
pub use key::*;
pub use native_type::*;
pub use reader::*;
pub use selection::*;
pub use source::*;
pub use table::*;
