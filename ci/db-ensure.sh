#!/usr/bin/env bash
# Ensure one named DB container is running on the expected image — reuse it if it already
# is, (re)create it only when missing, stopped, or on a stale image. This replaces the old
# "docker rm -fv + fresh docker run every time" so a warm container is reused across runs
# and across `make check`'s two db phases; container cold-start is the gate's biggest cost.
#
# Usage: ci/db-ensure.sh <name> <image> [docker-run-flags...]
#   The image is appended as the final `docker run` argument automatically.
set -euo pipefail

name="${1:?usage: db-ensure.sh <name> <image> [run-flags...]}"
image="${2:?usage: db-ensure.sh <name> <image> [run-flags...]}"
shift 2
flags=("$@")

if ! command -v docker >/dev/null 2>&1; then
  echo "  db-ensure: docker not found — skipping $name (live suites will self-skip)" >&2
  exit 0
fi

running=$(docker inspect -f '{{.State.Running}}' "$name" 2>/dev/null || echo missing)
cur_image=$(docker inspect -f '{{.Config.Image}}' "$name" 2>/dev/null || echo "")

if [[ "$running" == "true" && "$cur_image" == "$image" ]]; then
  echo "  reuse    $name  ($image, already up)"
  exit 0
fi

if [[ "$running" != "missing" ]]; then
  reason=$([[ "$cur_image" != "$image" ]] && echo "stale image: $cur_image" || echo "not running")
  echo "  recreate $name  ($reason)"
  docker rm -fv "$name" >/dev/null 2>&1 || true
else
  echo "  create   $name  ($image)"
fi

docker run --rm -d --name "$name" "${flags[@]}" "$image" >/dev/null
echo "  started  $name"
