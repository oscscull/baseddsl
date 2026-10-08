#!/usr/bin/env bash
# Matched consumer builds in scratch projects only; never edit or restore workspace sources.
# Usage: ci/measure-incremental.sh --based /path/to/release/based --output /path/to/results.json
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 "$root/benchmarks/consumer-build/measure.py" "$@"
