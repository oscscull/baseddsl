#!/usr/bin/env bash
# Measure the incremental rebuild+relink time that we actually feel when editing one crate.
# Protocol: warm the build once, then make a trivial edit to a leaf source file and time the
# rebuild of a binary that links the full runtime (`based`). Reports wall-clock seconds.
#
# Run it once per linker config (with/without .cargo/config.toml) to A/B the linker; each
# config has its own build cache, so warm first.
#
# Usage: ci/measure-incremental.sh [label]
set -euo pipefail
cd "$(dirname "$0")/.."

label="${1:-build}"
target_crate="based-cli"           # links the full runtime → exercises the linker
touchfile="crates/based-runtime/src/plan.rs"

echo "[$label] warming build (one full compile so the measured run is incremental)…"
cargo build -p "$target_crate" --quiet

echo "[$label] editing $touchfile and timing the incremental rebuild…"
# Append + remove a newline so content is unchanged but mtime bumps → cargo recompiles it.
printf '\n' >> "$touchfile"
start=$(date +%s.%N)
cargo build -p "$target_crate" --quiet
end=$(date +%s.%N)
# Restore the file exactly.
git checkout -- "$touchfile" 2>/dev/null || sed -i '' -e '$d' "$touchfile"

printf '[%s] incremental rebuild+relink: %.2fs\n' "$label" "$(echo "$end - $start" | bc)"
