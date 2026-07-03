#!/usr/bin/env bash
# Build the recorder image and produce the demo video + screenshots in demo/out/.
# Usage:  ./demo/run.sh           (build if needed, then record)
#         ./demo/run.sh --rebuild (force a clean image rebuild)
set -euo pipefail

DEMO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$DEMO_DIR/.." && pwd)"
IMAGE="mgmt-demo"
OUT="$DEMO_DIR/out"

# `docker` may be podman; both accept the same flags used here.
DOCKER="${DOCKER:-docker}"

BUILD_ARGS=(build -f "$DEMO_DIR/Dockerfile" --ignorefile "$DEMO_DIR/.dockerignore" -t "$IMAGE" "$REPO_ROOT")
[ "${1:-}" = "--rebuild" ] && BUILD_ARGS=(build --no-cache "${BUILD_ARGS[@]:1}")

echo ">> building $IMAGE"
"$DOCKER" "${BUILD_ARGS[@]}"

mkdir -p "$OUT"
echo ">> recording (artifacts -> $OUT)"
"$DOCKER" run --rm --shm-size=512m -v "$OUT:/out" "$IMAGE"

echo
echo ">> done. Output:"
ls -la "$OUT"
