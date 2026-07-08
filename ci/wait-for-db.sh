#!/usr/bin/env bash
# Portable DB readiness-wait. Blocks until a TCP connection to the host:port in a
# database URL succeeds, or a timeout elapses. Dependency-free: uses bash's built-in
# /dev/tcp, so it runs anywhere bash does (CI runner, laptop) with no psql/mysql client.
#
# It is the pre-`based migrate apply` guard for the example scenarios: a fresh service
# container (GitHub Actions `services:`, `docker run`) accepts TCP a moment after it starts,
# and this loop keeps the apply from racing that boot. The live Rust suites have their own
# in-process readiness poll (support/docker_*.rs `wait_ready`), so this is only needed where
# an external command (`based`) connects.
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

authority="${url#*://}"       # strip scheme
authority="${authority#*@}"   # strip user:pass@ if present
hostport="${authority%%/*}"   # drop /database and anything after
host="${hostport%%:*}"
port="${hostport##*:}"
[[ "$port" == "$host" ]] && port=""   # no explicit port in the URL

if [[ -z "$port" ]]; then
  case "$url" in
    mysql://*|mariadb://*) port=3306 ;;
    postgres://*|postgresql://*) port=5432 ;;
    *) echo "wait-for-db: no port in '$url' and unknown scheme" >&2; exit 1 ;;
  esac
fi

echo "wait-for-db: waiting up to ${timeout}s for ${host}:${port} ..."
deadline=$(( $(date +%s) + timeout ))
until (exec 3<>"/dev/tcp/${host}/${port}") 2>/dev/null; do
  if [[ $(date +%s) -ge $deadline ]]; then
    echo "wait-for-db: ${host}:${port} not reachable within ${timeout}s" >&2
    exit 1
  fi
  sleep 1
done
exec 3>&- 2>/dev/null || true
# A DB that just opened its socket may still be finishing auth setup; a short grace avoids a
# first-connection flake. `based migrate apply` itself is the real connect that follows.
sleep 2
echo "wait-for-db: ${host}:${port} is accepting connections"
