# syntax=docker/dockerfile:1
# =============================================================================
#  Production image for `mgmt web` — the axum API + the PWA baked in (embed-ui),
#  a single self-contained binary on a slim, non-root runtime.
#
#  Build context is the REPO ROOT (see docker-compose.yml `build.context: ../..`):
#     docker build -f infra/docker/Dockerfile -t mgmt-web:latest .
# =============================================================================

# ---- stage 1: build the PWA (web/dist) --------------------------------------
FROM node:22-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build          # -> /web/dist

# ---- stage 2: build the mgmt binary with the PWA embedded -------------------
FROM rust:slim-bookworm AS build
# `ring`/TLS needs a C toolchain; ca-certificates for the crates.io index.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# `--features embed-ui` bakes web/dist into the binary; provide it from stage 1.
COPY --from=web /web/dist ./web/dist
RUN cargo build --release -p mgmt-cli --features embed-ui \
    && strip target/release/mgmt || true

# ---- stage 3: runtime -------------------------------------------------------
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin mgmt \
    && mkdir -p /data && chown mgmt:mgmt /data

COPY --from=build /src/target/release/mgmt /usr/local/bin/mgmt
COPY infra/docker/entrypoint.sh /usr/local/bin/entrypoint
RUN chmod +x /usr/local/bin/entrypoint

# One volume holds config + state; XDG dirs point inside it. `web-auth.yaml`
# (password/TOTP/tokens) lives under $XDG_CONFIG_HOME/mgmt and persists here too.
ENV XDG_CONFIG_HOME=/data/config \
    XDG_DATA_HOME=/data/state \
    MGMT_BIND=0.0.0.0:8321

USER mgmt
WORKDIR /data
VOLUME ["/data"]
EXPOSE 8321

# /api/health is unauthenticated (see mgmt-web middleware::is_public).
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS "http://localhost:8321/api/health" || exit 1

# tini reaps zombies and forwards signals so `docker stop` is clean.
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/entrypoint"]
