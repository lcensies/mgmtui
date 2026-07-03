#!/usr/bin/env bash
# Container entrypoint: seed a fresh dataset, record the tape with VHS, publish artifacts.
set -euo pipefail

OUT_DIR="${OUT_DIR:-/out}"

# Always start from a clean, reproducible dataset.
rm -rf "${XDG_DATA_HOME:?}/mgmt"
mkdir -p "${XDG_DATA_HOME}/mgmt" "${XDG_CONFIG_HOME}/mgmt"
cp -f /demo/config.yaml "${XDG_CONFIG_HOME}/mgmt/config.yaml"

echo ">> seeding demo data"
bash /demo/seed.sh

echo ">> recording with VHS"
mkdir -p /demo/out
cd /demo/out                 # VHS writes Output/Screenshot files (bare names) here
vhs /demo/demo.tape

echo ">> artifacts in /demo/out:"
ls -la /demo/out

# Publish to the bind-mounted volume if one is present.
if [ -d "$OUT_DIR" ] && [ "$OUT_DIR" != "/demo/out" ]; then
  cp -rf /demo/out/. "$OUT_DIR"/ 2>/dev/null || true
  echo ">> copied artifacts to $OUT_DIR"
fi
