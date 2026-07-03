//! Engine id generation (D1: the app generates a model's `id`, no SQL default).
//!
//! A `create` binds its `id` to an engine-generated value (`:id` / `:id_<step>`);
//! the runtime fills it from an [`IdGen`]. The trait is the seam: production uses a
//! uuid generator (arriving with the concrete MariaDB driver, the next slice), while
//! tests use the deterministic [`SeqIdGen`] so a planned INSERT's bound id is
//! predictable — the write path's twin of the read path's `MockDb`.

/// Produces fresh ids for engine-generated `id` columns. Called once per `create`
/// (and once more is never needed — a `^.id` back-reference *reuses* the value the
/// prior create already generated, it does not draw a new one).
pub trait IdGen {
    fn next_id(&mut self) -> String;
}

/// A deterministic generator for tests: `<prefix>-0`, `<prefix>-1`, … in call order.
/// Not for production (ids must be unpredictable + globally unique) — the uuid
/// generator lands with the driver slice.
pub struct SeqIdGen {
    prefix: String,
    n: u64,
}

impl SeqIdGen {
    /// A generator yielding `<prefix>-0`, `<prefix>-1`, …
    pub fn new(prefix: impl Into<String>) -> Self {
        SeqIdGen {
            prefix: prefix.into(),
            n: 0,
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
    fn next_id(&mut self) -> String {
        let id = format!("{}-{}", self.prefix, self.n);
        self.n += 1;
        id
    }
}
