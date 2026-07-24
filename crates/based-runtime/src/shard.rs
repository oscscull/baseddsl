//! Backend-agnostic shard-routing primitives.
//!
//! The logical-shard space, the per-shard pool sizing, and the stable routing hash are the
//! same for every concrete backend (MariaDB [`crate::driver::ShardRouter`], Postgres
//! [`crate::postgres::PgRouter`]) — a shard key must land on the same logical shard
//! regardless of which database dialect serves it. So they live here, feature-free, rather
//! than in a driver module gated behind one backend's crate.

use std::time::Duration;

/// A physical shard's identity: its index into a router's shard list.
pub type ShardId = usize;

/// The fixed size of the logical-shard space. Routing hashes a key into `[0,
/// LOGICAL_SHARDS)`, then a `logical → physical` map (built at startup) sends it to a
/// pool. This number is **permanent** — it is the granularity at which data can be
/// rebalanced between physical shards, so it is chosen large once (4096 logical shards ⇒ up
/// to 4096 physical shards, and any split moves whole logical shards, never rehashes keys).
pub const LOGICAL_SHARDS: usize = 4096;

/// Bounded per-shard pool sizing + the live-DB hardening timeouts. The `max` is the
/// concurrency ceiling against one database box (protecting it under load); `min` keeps warm
/// connections ready. The two timeouts turn "wait forever" hangs into fast, retryable `503`s.
#[derive(Debug, Clone, Copy)]
pub struct PoolConfig {
    pub min: usize,
    pub max: usize,
    /// Max time a checkout waits for a free connection before failing with a
    /// [`DbErrorKind::PoolExhausted`](crate::run::DbErrorKind::PoolExhausted) `503`.
    /// Bounds pool saturation to a fast failure the client/LB retries, never an unbounded
    /// hang that ties up a worker thread.
    pub checkout_timeout: Duration,
    /// Server-side per-statement timeout: a query/mutation running longer is aborted by the
    /// server and surfaces as a `503` rather than hanging a connection (MariaDB
    /// `max_statement_time`, Postgres `statement_timeout`). `ZERO` disables it.
    pub statement_timeout: Duration,
}

impl Default for PoolConfig {
    /// A conservative default: a small warm floor, a bounded ceiling well under a database
    /// box's connection limit (scale for load by adding shards + instances), a few-second
    /// checkout wait, and a generous per-statement ceiling (a hardening backstop for a
    /// runaway query, not a tight SLA; a deployment tightens it).
    fn default() -> Self {
        Self {
            min: 4,
            max: 32,
            checkout_timeout: Duration::from_secs(5),
            statement_timeout: Duration::from_secs(30),
        }
    }
}

/// FNV-1a (64-bit) — a **stable** hash for shard routing. `DefaultHasher` is explicitly not
/// stable across releases; a shard key must hash the same forever, so we pin the algorithm
/// here. Shared by every backend router so a key routes identically across dialects.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// The logical → physical shard a key routes to, given the per-logical-shard assignment.
pub fn shard_for(assign: &[ShardId], key: &str) -> ShardId {
    let logical = (fnv1a_64(key.as_bytes()) % LOGICAL_SHARDS as u64) as usize;
    assign[logical]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_is_stable_and_in_range() {
        // Stable: the same key always lands on the same logical shard (regression guard on
        // the pinned FNV constants — a routing change would strand data).
        let expect = {
            let mut h = 0xcbf2_9ce4_8422_2325u64;
            for &b in b"org-1" {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
            h % LOGICAL_SHARDS as u64
        };
        assert_eq!(fnv1a_64(b"org-1") % LOGICAL_SHARDS as u64, expect);
    }
}
