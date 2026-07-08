#!/usr/bin/env bash
# Smoke-boot the `based serve` image: run the container against the
# bundled-SQLite quickstart — no external DB service — apply its migrations on start, and
# assert /healthz + /readyz both answer 200. Proves the packaged image actually boots and
# serves, self-contained enough to run on any CI runner or a laptop.
set -euo pipefail

IMAGE="${1:?usage: smoke-image.sh <image>}"
PORT="${SMOKE_PORT:-8099}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NAME="based-serve-smoke-$$"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

# SQLite is bundled (no service). The project is read-only; the DB file lives in the
# writable /tmp so `migrate apply` (BASED_MIGRATE_ON_START) can create it.
docker run -d --name "$NAME" -p "$PORT:8080" \
  -v "$ROOT/examples/sqlite-quickstart:/app:ro" \
  -e DATABASE_URL=/tmp/smoke.db \
  -e BASED_MIGRATE_ON_START=1 \
  "$IMAGE" >/dev/null

# Wait for the listener to come up (bounded ~30s).
ok=
for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1; then ok=1; break; fi
  sleep 1
done
if [ -z "$ok" ]; then
  echo "smoke: /healthz never came up"; docker logs "$NAME"; exit 1
fi

code_h=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/healthz")
code_r=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/readyz")
echo "smoke: /healthz=$code_h /readyz=$code_r"
if [ "$code_h" != 200 ] || [ "$code_r" != 200 ]; then
  echo "smoke: probe failed"; docker logs "$NAME"; exit 1
fi
echo "smoke: image boots + serves OK"
