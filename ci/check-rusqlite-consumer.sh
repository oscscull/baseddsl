#!/usr/bin/env bash
# An app on the newest rusqlite can add based-runtime's rusqlite driver: one libsqlite3-sys,
# no sqlx, and the two share a database at run time.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo_bin="${CARGO:-cargo}"
manifest=ci/fixtures/rusqlite-consumer/Cargo.toml
tree=$("$cargo_bin" tree --manifest-path "$manifest" -e normal,build --prefix none)
sys=$(printf '%s\n' "$tree" | grep -E '^libsqlite3-sys ' | sort -u)
if [ "$(printf '%s\n' "$sys" | wc -l)" -ne 1 ] || ! printf '%s\n' "$sys" | grep -q '^libsqlite3-sys v0\.38\.'; then
    printf 'expected exactly libsqlite3-sys 0.38.x, got:\n%s\n' "$sys" >&2
    exit 1
fi
if hits=$(printf '%s\n' "$tree" | grep -E '^sqlx'); then
    printf 'the rusqlite driver pulled sqlx:\n%s\n' "$hits" >&2
    exit 1
fi
"$cargo_bin" run --manifest-path "$manifest" --target-dir target/rusqlite-consumer
