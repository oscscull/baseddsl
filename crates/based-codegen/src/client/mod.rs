//! Client codegen: a `CheckedSchema` -> a typed Rust client module (`based gen
//! client`). Per signature it emits a typed input struct, a typed output type (shape
//! struct, bare-model struct, or the `Page<T>` pagination envelope), and one wire route
//! plus a `Client` method that posts the input and decodes the output. A `-> stream`
//! query's method returns a `RowStream<Shape>` (per-item `Result`, drop = cancel)
//! through the transport's streaming call; the NDJSON decoder is emitted with the
//! module so every HTTP transport shares one framing implementation. A mutation
//! additionally gets a `<name>_with_key` twin carrying a mutation idempotency key
//! through the transport's keyed call (HTTP: the `Idempotency-Key` header).
//!
//! Transport is abstract: `Client<T>` is generic over a `Transport` trait (post JSON to
//! a route, decode JSON back), which the runtime crate implements. Entity ids map to a
//! phantom-typed `Id<E>` newtype and the keyset cursor to an opaque `Cursor`, both
//! `#[serde(transparent)]` so the wire stays a plain string. `$ctx` is carried out of
//! band as request context. Shape projections nest to matching structs.
//!
//! When [`ClientOptions::embedded`] is set, the module also emits an in-process bridge
//! over `based_runtime::Engine` (an `Embedded` transport plus an `embedded(&engine)`
//! constructor), giving an embedding consumer a working `Client` with no bridge code.
//! Opt-in so a pure-wire client need not depend on based-runtime.

use based_ast::Decl;
use based_sema::CheckedSchema;

mod format_rust;
mod rust;
mod target;

pub use format_rust::format_rust;
pub use target::{ClientOptions, ClientTarget};

/// Render the whole schema as a typed *wire* client module for `target` (no embedded
/// bridge). The socket-free embed path uses [`client_with`] with
/// [`ClientOptions::embedded`] instead.
pub fn client(schema: &CheckedSchema, decls: &[Decl], target: ClientTarget) -> String {
    client_with(schema, decls, target, ClientOptions::default())
}

/// Render the whole schema as a typed client module for `target`, honoring `opts` (e.g.
/// [`ClientOptions::embedded`] to append the in-process bridge over `based_runtime::Engine`).
pub fn client_with(
    schema: &CheckedSchema,
    decls: &[Decl],
    target: ClientTarget,
    opts: ClientOptions,
) -> String {
    let ClientTarget::Rust = target;
    rust::render(schema, decls, opts)
}
