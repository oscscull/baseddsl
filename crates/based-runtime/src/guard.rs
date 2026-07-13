//! Host guard hooks (auth.md Handle 3): the app registers a named async function per
//! declared `guard`, and dispatch invokes it before a mutation's write body. The
//! engine owns *that* the check runs; the app owns what it decides.
//!
//! Guards are host code, so they live outside the schema: a guard function receives
//! the callable's name, its decoded JSON arguments, and the server-derived `$ctx`,
//! and returns a [`GuardVerdict`]. It is async and owns its own resources — it may
//! read the database through a captured pool, or call the typed client itself.
//!
//! A schema that declares a guard nobody registered must fail when the engine is
//! *built* ([`Guards::missing_for`] backs that check), never pass silently at request
//! time; the request-time backstop for a raw dispatch is a loud `500`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::load::Compiled;

/// What a guard receives: the callable being invoked, its decoded JSON argument
/// object, and the server-derived request `$ctx` (never client-supplied). Owned, so a
/// registered `async move` closure needs no borrow gymnastics.
#[derive(Debug, Clone)]
pub struct GuardRequest {
    /// The mutation's name — one registered fn may guard several callables.
    pub callable: String,
    pub args: serde_json::Value,
    pub ctx: serde_json::Value,
}

/// A guard's decision. There is no third state: a guard that cannot decide (its own
/// lookup failed) should deny — fail closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardVerdict {
    /// Run the mutation.
    Allow,
    /// Reject the call: a `403` with the stable wire code `guard_denied` and this
    /// reason as the message. The reason is mandatory — a denial is never silent.
    Deny { message: String },
}

impl GuardVerdict {
    /// Deny with a reason (the wire message the caller sees).
    pub fn deny(message: impl Into<String>) -> GuardVerdict {
        GuardVerdict::Deny {
            message: message.into(),
        }
    }
}

/// A registered guard implementation, boxed so the registry is object-safe.
type GuardFn =
    Arc<dyn Fn(GuardRequest) -> Pin<Box<dyn Future<Output = GuardVerdict> + Send>> + Send + Sync>;

/// The registered-guard registry: guard name → host async fn. Built by the embedding
/// app and handed to [`crate::Engine::with_guards`]; cheap to clone (the fns are
/// shared). An empty registry is correct for a schema that declares no guards.
#[derive(Clone, Default)]
pub struct Guards {
    map: HashMap<String, GuardFn>,
}

impl Guards {
    /// An empty registry. Register implementations with [`Guards::register`].
    pub fn new() -> Guards {
        Guards::default()
    }

    /// Register the implementation for one declared guard name (builder-style).
    /// Registering a name twice keeps the last implementation.
    pub fn register<F, Fut>(mut self, name: impl Into<String>, guard: F) -> Guards
    where
        F: Fn(GuardRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = GuardVerdict> + Send + 'static,
    {
        self.map
            .insert(name.into(), Arc::new(move |req| Box::pin(guard(req))));
        self
    }

    /// The registered implementation for `name`, if any.
    pub(crate) fn get(&self, name: &str) -> Option<&GuardFn> {
        self.map.get(name)
    }

    /// Every `(mutation, guard)` the schema declares that this registry lacks — the
    /// engine-build check: non-empty means the engine must not come up.
    pub fn missing_for(&self, compiled: &Compiled) -> Vec<(String, String)> {
        compiled
            .declared_guards()
            .filter(|(_, g)| !self.map.contains_key(*g))
            .map(|(m, g)| (m.to_string(), g.to_string()))
            .collect()
    }
}

/// The schema declares guards this registry does not cover — building an engine (or
/// starting a listener) over that pairing is refused, so a declared check can never
/// silently not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardSetupError {
    /// The uncovered `(mutation, guard)` pairs.
    pub missing: Vec<(String, String)>,
}

impl std::fmt::Display for GuardSetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let list = self
            .missing
            .iter()
            .map(|(m, g)| format!("mutation `{m}` declares guard `{g}`"))
            .collect::<Vec<_>>()
            .join("; ");
        write!(
            f,
            "{list}, but no guard with that name is registered — register every declared guard when building the engine"
        )
    }
}

impl std::error::Error for GuardSetupError {}
