/// The client compile target (manifest `client`). Rust is the only target; the
/// enum exists so the entry point can branch when a second target lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientTarget {
    Rust,
}

impl ClientTarget {
    /// Parse a previously validated target, retaining the legacy default for API callers.
    pub fn parse(s: &str) -> Self {
        Self::try_parse(s).unwrap_or(Self::Rust)
    }

    /// Validate an explicit client target without silently replacing it.
    pub fn try_parse(s: &str) -> Option<Self> {
        match s {
            "rust" => Some(Self::Rust),
            _ => None,
        }
    }
}

/// Emit options beyond the target language.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClientOptions {
    /// Also emit the in-process **embedded bridge** — an `Embedded` `Transport` over
    /// `based_runtime::Engine` plus an `embedded(&engine)` constructor. Off by default so
    /// a pure-wire/HTTP client need not depend on based-runtime. An embedding consumer
    /// (the quickstarts, `tests/embed.rs`) turns it on to get a working `Client` with no
    /// hand-written bridge.
    pub embedded: bool,
    /// The compile-target dialect, when known (`based gen client` passes it from the
    /// manifest). It gates the one per-driver bring-your-own transaction constructor
    /// (`adopt_postgres` / `adopt_mariadb` / `adopt_sqlite`) the embedded bridge emits: the
    /// generated client is compiled for one dialect, and each `adopt_*` names that driver's
    /// concrete `sqlx::Transaction<DB>` (the types don't unify), so exactly the matching one
    /// is emitted. `None` (e.g. a wire-only client, or a test that doesn't set it) emits no
    /// `adopt_*`.
    pub dialect: Option<crate::Dialect>,
}
