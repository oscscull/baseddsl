# syntax=docker/dockerfile:1
#
# Container image for `based serve`. Multi-stage: a Rust builder
# compiles the release `based` binary; a slim runtime carries just the binary + its
# entrypoint. The served project (based.toml + **/*.bsl [+ migrations/]) is mounted at
# /app, and everything else is env-configured — see docker/README.md for the run recipe.
#
# Build from the repo root:  docker build -f docker/Dockerfile -t based-serve .

# --- build stage --------------------------------------------------------------------
# Tracks stable 1.x (matches CI's `dtolnay/rust-toolchain@stable`); the workspace floor is
# 1.85 but the resolved lockfile pulls deps needing a newer stable, so we don't pin below it.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
# BuildKit cache mounts keep the dependency compile off the critical path: the cargo
# registry, git checkouts, and the target dir persist across rebuilds, so an unchanged
# dependency set is never recompiled — only touched crates rebuild. The binary is copied
# out of the (cache-mounted, non-persisted) target dir within the same layer.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release -p based-cli \
 && cp target/release/based /usr/local/bin/based

# --- runtime stage ------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime
# ca-certificates: DB endpoints behind TLS termination. curl: the HEALTHCHECK probe.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*

COPY --from=build /usr/local/bin/based /usr/local/bin/based
COPY docker/entrypoint.sh /usr/local/bin/docker-entrypoint.sh
COPY docker/healthcheck.sh /usr/local/bin/docker-healthcheck.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh /usr/local/bin/docker-healthcheck.sh

# Config surface, all env-driven:
#   BASED_DATABASE_URL     — one shard URL, or comma-separated for a sharded fleet (required;
#                            the standard DATABASE_URL is also honored)
#   BASED_PROJECT          — the served schema root (based.toml + **/*.bsl), mounted at /app
#   BASED_LISTEN           — bind address, read natively by `based serve`; default 0.0.0.0:8080
#                            so the port is reachable from outside the container
#   BASED_MIGRATE_ON_START — "1" runs `based migrate apply` before serving (opt-in)
ENV BASED_PROJECT=/app \
    BASED_LISTEN=0.0.0.0:8080
WORKDIR /app
VOLUME ["/app"]
EXPOSE 8080

# Liveness via docker-healthcheck.sh (probes /healthz on BASED_LISTEN's port). The
# probe lives in a script so the port derivation runs in the shell at container runtime,
# not Docker's build-time parser.
HEALTHCHECK --interval=15s --timeout=3s --start-period=10s --retries=3 \
  CMD ["/usr/local/bin/docker-healthcheck.sh"]

# Run as an unprivileged user (the binary needs no root; the schema is read-only).
RUN useradd --uid 10001 --no-create-home --shell /usr/sbin/nologin based
USER based

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
