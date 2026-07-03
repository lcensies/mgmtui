# =============================================================================
#  mgmt demo recorder — builds the app from source and records a terminal video
#  with VHS (ttyd + headless chromium + ffmpeg), fully self-contained.
#
#  Build (context = repo root):
#     docker build -f demo/Dockerfile --ignorefile demo/.dockerignore -t mgmt-demo .
#  Run (writes artifacts to the mounted ./out):
#     docker run --rm --shm-size=512m -v "$PWD/demo/out:/out" mgmt-demo
#
#  Both stages use Debian bookworm (glibc 2.36) so the binary built in stage 1
#  runs unchanged in the recorder stage.
# =============================================================================

# ---- stage 1: build the mgmt binary -----------------------------------------
FROM rust:slim-bookworm AS builder

# `ring` (TLS) needs a C toolchain; ca-certificates for the crates.io index.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p mgmt-cli \
    && cp target/release/mgmt /usr/local/bin/mgmt \
    && strip /usr/local/bin/mgmt || true

# ---- stage 2: the recorder --------------------------------------------------
FROM debian:bookworm-slim AS recorder
ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
        chromium ffmpeg ca-certificates curl bash procps \
        fontconfig fonts-dejavu-core fonts-noto-color-emoji \
    && fc-cache -f \
    && rm -rf /var/lib/apt/lists/*

# ttyd (static single binary that VHS drives).
ARG TTYD_VERSION=1.7.7
RUN curl -fsSL -o /usr/local/bin/ttyd \
        "https://github.com/tsl0922/ttyd/releases/download/${TTYD_VERSION}/ttyd.x86_64" \
    && chmod +x /usr/local/bin/ttyd

# VHS.
ARG VHS_VERSION=0.11.0
RUN curl -fsSL -o /tmp/vhs.tgz \
        "https://github.com/charmbracelet/vhs/releases/download/v${VHS_VERSION}/vhs_${VHS_VERSION}_Linux_x86_64.tar.gz" \
    && tar -xzf /tmp/vhs.tgz -C /tmp \
    && install -m0755 "$(find /tmp -name vhs -type f | head -n1)" /usr/local/bin/vhs \
    && rm -rf /tmp/vhs*

# Help go-rod/VHS locate chromium and run it headless as root.
ENV VHS_NO_SANDBOX=true
RUN ln -sf /usr/bin/chromium /usr/local/bin/google-chrome-stable \
    && ln -sf /usr/bin/chromium /usr/local/bin/chrome

# The app binary.
COPY --from=builder /usr/local/bin/mgmt /usr/local/bin/mgmt

# Demo assets.
WORKDIR /demo
COPY demo/config.yaml  /demo/config.yaml
COPY demo/seed.sh      /demo/seed.sh
COPY demo/demo.tape    /demo/demo.tape
COPY demo/entrypoint.sh /demo/entrypoint.sh

# One shared data/config root for both the CLI seeding and the TUI.
ENV HOME=/root
ENV XDG_CONFIG_HOME=/demo/xdg/config
ENV XDG_DATA_HOME=/demo/xdg/data
ENV TERM=xterm-256color
RUN mkdir -p /demo/xdg/config/mgmt /demo/xdg/data /demo/out \
    && cp /demo/config.yaml /demo/xdg/config/mgmt/config.yaml \
    && chmod +x /demo/seed.sh /demo/entrypoint.sh

ENTRYPOINT ["/demo/entrypoint.sh"]
