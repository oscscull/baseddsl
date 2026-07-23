//! Engine id generation.
//!
//! A `create` binds its `id` to an engine-generated value (`:id` / `:id_<step>`);
//! the runtime fills it from an [`IdGen`]. The trait is the seam: production uses the
//! uuid generator ([`UuidGen`], behind the `serve` feature), while tests use the
//! deterministic [`SeqIdGen`] so a planned INSERT's bound id is predictable.

/// Produces fresh ids for engine-generated `id` columns. Called once per `create`
/// (and once more is never needed — a `$name.id` step reference *reuses* the value the
/// bound create already generated, it does not draw a new one). Minting takes `&self`
/// and may run from any number of concurrent requests, so an implementation with
/// state synchronizes it internally (an atomic or a short std `Mutex`) — dispatch
/// never wraps the generator in a lock of its own, which is what lets a guard call
/// back into its own engine.
pub trait IdGen: Send + Sync {
    fn next_id(&self) -> String;
}

/// A deterministic generator for tests: `<prefix>-0`, `<prefix>-1`, … in call order.
/// Not for production (ids must be unpredictable + globally unique).
pub struct SeqIdGen {
    prefix: String,
    n: std::sync::atomic::AtomicU64,
}

impl SeqIdGen {
    /// A generator yielding `<prefix>-0`, `<prefix>-1`, …
    pub fn new(prefix: impl Into<String>) -> Self {
        SeqIdGen {
            prefix: prefix.into(),
            n: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl Default for SeqIdGen {
    /// Ids of the form `id-0`, `id-1`, …
    fn default() -> Self {
        SeqIdGen::new("id")
    }
}

impl IdGen for SeqIdGen {
    fn next_id(&self) -> String {
        let n = self.n.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("{}-{}", self.prefix, n)
    }
}

/// The production generator: a fresh random v4 uuid per `create`. Unpredictable and
/// globally unique — no coordination with the database, so a `create`'s id is known
/// before the INSERT, which is what lets a `$name.id` step reference bind the same value
/// the INSERT used. Stateless, so concurrent mints need no synchronization.
#[cfg(feature = "serve")]
#[derive(Default)]
pub struct UuidGen;

#[cfg(feature = "serve")]
impl IdGen for UuidGen {
    fn next_id(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }
}
