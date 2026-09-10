//! The `_based_migrations` ledger's content hash and drift check.

use super::*;

/// A stable content hash of an `up.mig`'s canonical bytes — the `_based_migrations`
/// ledger's tamper guard. Canonicalization drops comment (`#…`) and blank
/// lines and trims each remaining line, so a cosmetic whitespace/comment edit doesn't trip
/// the guard but any change to a step does. FNV-1a-64 (the same family the runtime uses for
/// request fingerprints), rendered as 16 lowercase hex digits — collision resistance
/// is not security-critical here (it guards against an accidental post-apply edit, not an
/// adversary), so a fast non-cryptographic hash is the right tool.
pub fn content_hash(up_text: &str) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for line in up_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        mix(line.as_bytes());
        mix(b"\n");
    }
    format!("{h:016x}")
}

/// Does the stored `up.mig`'s structural (non-`raw`) residue still match the steps its
/// `schema.snap` chain implies? The snapshot-authoritative drift check shared by `apply`,
/// `render`, and `verify`: structural steps derive from `schema.snap`, so a hand-edit that
/// changes a structural line's canonical form is drift and must be refused rather than
/// silently ignored. Canonicalized like [`content_hash`] (cosmetic whitespace/comment edits
/// are tolerated; `raw(dialect)` escapes ride separately and are stripped before compare).
pub fn up_mig_matches_snapshot(up_text: &str, steps: &[Step]) -> bool {
    content_hash(&strip_raw_steps(up_text)) == content_hash(&render_up(steps))
}
