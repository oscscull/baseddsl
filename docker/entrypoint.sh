#!/bin/sh
# Entrypoint for the `based serve` image. Config is env-driven (see docker/Dockerfile);
# with no command it serves the project at $BASED_PROJECT, binding $BASED_LISTEN (which
# `based serve` reads natively — no flag plumbing here).
set -e

# An explicit command that isn't a flag runs verbatim — so you can override, e.g.
#   docker run IMG based migrate status
# while `docker run IMG --pool-max 64` still passes flags through to `based serve`.
if [ "$#" -gt 0 ] && [ "${1#-}" = "$1" ]; then
  exec "$@"
fi

# Opt-in schema setup: apply the project's migrations before serving. Off by default
# (safe by default; principle 1) — a deploy that applies migrations out of band leaves it
# unset. Reads BASED_DATABASE_URL / DATABASE_URL like serve does.
if [ "${BASED_MIGRATE_ON_START:-0}" = "1" ]; then
  based migrate apply "$BASED_PROJECT"
fi

exec based serve "$BASED_PROJECT" "$@"
