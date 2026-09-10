use super::*;

/// What a single query/mutation lowers to on the client: an input struct, a wire
/// route, an output type, and the method that ties them together.
pub(super) struct Callable<'a> {
    /// signature name (also the method name and the route tail) — already snake_case.
    pub(super) name: &'a str,
    /// `/q/<name>` for a query, `/m/<name>` for a mutation.
    pub(super) route: String,
    pub(super) params: &'a [Param],
    /// model the params resolve against (query target / mutation return model);
    /// `None` when it could not be resolved (a mutation with no model return).
    pub(super) root: Option<&'a RModel>,
    /// the concrete output type expression, e.g. `Vec<OrderCard>` or `Page<Product>`.
    pub(super) output: String,
    /// a `-> stream` query: the method calls the transport's streaming door and
    /// returns a live `RowStream`.
    pub(super) stream: bool,
    /// a mutation: it additionally gets a `<name>_with_key` method carrying a
    /// mutation idempotency key through the transport's keyed door.
    pub(super) is_mutation: bool,
    /// an `-> ok` mutation: the method returns unit — the wire body is the empty
    /// acknowledgement (`{}`), decoded through the shared `Ack` type.
    pub(super) ack: bool,
    /// the output *struct* to emit (name + fields), deduped across callables.
    pub(super) out_struct: OutStruct,
    /// the `$ctx.<field>`s this callable requires, inferred per callable.
    /// Empty for a public callable (no context); non-empty callables get a typed
    /// `<Name>Ctx` struct the method takes and the `Transport` carries.
    pub(super) ctx_requires: &'a [CtxReq],
    /// how this callable paginates, so the input struct carries the right page
    /// control: a keyset page a `cursor`, an offset page an `offset`.
    pub(super) page: PageInput,
    /// a `get|list … for update` locking read: its method is emitted in the confined
    /// `impl<T: Transport + TxBound>` block, so it is callable only on a transaction-bound
    /// client (a lock outside a transaction releases immediately) — a compile error on the
    /// auto-commit `Client<Embedded>`.
    pub(super) for_update: bool,
    /// Params that resolve to an **entity id** → the model they identify (a Forward FK's
    /// target, or the model's own `id`). Drives the `Id<entity::M>` param type; a param
    /// absent here (and not model-annotated) is a plain scalar.
    pub(super) param_entities: std::collections::HashMap<String, String>,
}

/// How a callable paginates, driving its extra input field.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PageInput {
    /// Not paginated (a `get`, or a `list` with no `page`) — no page-control input.
    None,
    /// Keyset (`page` without `offset`): an opaque `cursor: Option<Cursor>` (absent =
    /// the first page); the response's `Page.cursor` is fed straight back for the next.
    Keyset,
    /// Explicit offset (`page … offset`): an `offset: Option<i64>` (absent = offset 0).
    Offset,
}

/// A named output struct: a shape projection or a bare-model row.
pub(super) struct OutStruct {
    pub(super) name: String,
    pub(super) fields: Vec<(String, String)>, // (field name, rust type)
    /// Auxiliary structs for to-one nested sub-objects (`buyer { … }`), each the
    /// projection of one nested relation, emitted alongside the parent and referenced by
    /// the parent's field type. Empty for a flat shape.
    pub(super) nested: Vec<Self>,
}
