#!/bin/sh
# Container HEALTHCHECK probe: liveness via GET /healthz (D26) — the process is up and its
# worker loop is serving. Never touches the DB, so a DB blip drains via /readyz instead of
# restarting an otherwise-healthy box. Probes BASED_LISTEN's port (`host:port` → `port`),
# the one address serve binds — one source of truth for the port.
exec curl -fsS "http://127.0.0.1:${BASED_LISTEN##*:}/healthz"
