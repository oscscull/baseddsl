#!/usr/bin/env bash
# The infra-free gate must not compile server DB drivers through feature unification.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo_bin="${CARGO:-cargo}"
tree=$("$cargo_bin" tree --workspace --no-default-features --features sqlite,serve \
    -e normal,build,dev --prefix none)
if hits=$(printf '%s\n' "$tree" | grep -E '^(sqlx-mysql|sqlx-postgres|rust_decimal) '); then
    printf 'fast feature boundary violated:\n%s\n' "$hits" >&2
    exit 1
fi
echo 'ci-fast-features: no server drivers or codec-spike dependencies'

# An embedded production generator must remain usable without the HTTP listener.
embedded_tree=$("$cargo_bin" tree -p based-runtime --no-default-features \
    --features sqlite,id-gen -e normal,build --prefix none)
if hits=$(printf '%s\n' "$embedded_tree" | grep -E '^(axum|hyper|tower|sqlx-mysql|sqlx-postgres) '); then
    printf 'embedded production feature boundary violated:\n%s\n' "$hits" >&2
    exit 1
fi
echo 'ci-fast-features: embedded production IDs have no HTTP or server-driver dependencies'
