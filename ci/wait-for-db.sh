#!/usr/bin/env bash
# Portable DB readiness-wait. Blocks until the database server is genuinely accepting
# connections, or a timeout elapses. It runs anywhere bash does (CI runner, laptop).
#
# It is the pre-`based migrate apply` guard for the example scenarios: a fresh service
# container (GitHub Actions `services:`, `docker run`) accepts TCP a moment after it starts,
# and this loop keeps the apply from racing that boot. The live Rust suites have their own
# in-process readiness poll (support/docker_*.rs `wait_ready`), so this is only needed where
# an external command (`based`) connects.
#
# "Ready" means genuinely-ready, not merely a TCP accept: with OrbStack port forwarding a bare
# connect can succeed for a moment *after* the DB container has already exited. So a TCP accept
# is only the first gate — it is then confirmed with a protocol-level ping (`pg_isready` /
# `mysqladmin ping`) when a client is on PATH, and with `docker` liveness of the container
# publishing this port when a daemon is reachable; neither can pass for a dead server. When
# neither is available the TCP accept stands (best effort, no worse than a raw port check).
#
# Usage: ci/wait-for-db.sh <database-url> [timeout-seconds]
#   sqlite URLs (a bare file path, no `://host`) return immediately — nothing to wait for.
set -euo pipefail

url="${1:?usage: wait-for-db.sh <database-url> [timeout-seconds]}"
timeout="${2:-60}"

# Extract host:port from a `scheme://user:pass@host:port/db` URL. A URL with no `://`
# authority (a SQLite file path) has nothing to wait for.
if [[ "$url" != *"://"* ]]; then
  echo "wait-for-db: '$url' is not a server URL (sqlite file path?) — nothing to wait for"
  exit 0
fi

scheme="${url%%://*}"
authority="${url#*://}"        # strip scheme
authority="${authority#*@}"    # strip user:pass@ if present
hostport="${authority%%/*}"    # drop /database and anything after
host="${hostport%%:*}"
port="${hostport##*:}"
[[ "$port" == "$host" ]] && port=""   # no explicit port in the URL

if [[ -z "$port" ]]; then
  case "$scheme" in
    mysql|mariadb) port=3306 ;;
    postgres|postgresql) port=5432 ;;
    *) echo "wait-for-db: no port in '$url' and unknown scheme" >&2; exit 1 ;;
  esac
fi

# Does a bare TCP connection to the server open? Necessary but not sufficient for "ready".
tcp_open() { (exec 3<>"/dev/tcp/${host}/${port}") 2>/dev/null && exec 3>&- 2>/dev/null; }

# Is a container that *publishes* this host port actually running? A dead container may still
# have a briefly-lingering port forward, so a `false`/absent State rules "not ready". Returns
# success (don't block) when docker isn't reachable or no such container is found — this is a
# best-effort liveness signal layered on top of the protocol ping, not the sole gate.
container_live() {
  command -v docker >/dev/null 2>&1 || return 0
  local cid state
  cid=$(docker ps -q --filter "publish=${port}" 2>/dev/null | head -n1) || return 0
  [[ -z "$cid" ]] && return 0
  state=$(docker inspect -f '{{.State.Running}}' "$cid" 2>/dev/null) || return 0
  [[ "$state" == "true" ]]
}

# Confirm the server actually answers, not just that the port accepts. A protocol ping cannot
# succeed against a dead backend; when no client is installed we rely on TCP + container liveness.
protocol_ready() {
  case "$scheme" in
    postgres|postgresql)
      if command -v pg_isready >/dev/null 2>&1; then
        pg_isready -q -h "$host" -p "$port" && return 0 || return 1
      fi ;;
    mysql|mariadb)
      for ping in mysqladmin mariadb-admin; do
        if command -v "$ping" >/dev/null 2>&1; then
          "$ping" --protocol=tcp -h "$host" -P "$port" ping >/dev/null 2>&1 && return 0 || return 1
        fi
      done ;;
  esac
  # No protocol client available: fall back to container liveness (docker) or the TCP accept.
  container_live
}

ready() { tcp_open && protocol_ready; }

echo "wait-for-db: waiting up to ${timeout}s for ${host}:${port} ..."
deadline=$(( $(date +%s) + timeout ))
until ready; do
  if [[ $(date +%s) -ge $deadline ]]; then
    echo "wait-for-db: ${host}:${port} not ready within ${timeout}s (server down?)" >&2
    exit 1
  fi
  sleep 1
done
# A server that just answered may still be finishing auth setup; a short grace avoids a
# first-connection flake. `based migrate apply` itself is the real connect that follows.
sleep 2
echo "wait-for-db: ${host}:${port} is ready"
