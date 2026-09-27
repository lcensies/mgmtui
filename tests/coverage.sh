#!/usr/bin/env bash
# Line coverage for the whole stack. One instrumented build in target/cov is driven by the unit
# tests, the blackbox suite (CLI/TUI/web over subprocess + pty) and the Playwright GUI suite;
# every spawned `mgmt` writes a .profraw, and they all merge into one report.
#
#   tests/coverage.sh [unit] [bb] [web]      default: all three
#
# Outputs: target/cov/report.txt, target/cov/lcov.info (Rust);
#          web/coverage/lcov.info + console summary (PWA, only with `web`).
#
# The profraw format follows rustc's bundled LLVM, so llvm-profdata/llvm-cov of that same major
# are run via `nix shell` (system LLVM is newer and refuses the files). LLVM_BIN=<dir> overrides.
set -euo pipefail
cd "$(dirname "$0")/.."
suites=" ${*:-unit bb web} "

export CARGO_TARGET_DIR="$PWD/target/cov"
export RUSTFLAGS="-C instrument-coverage"
prof="$CARGO_TARGET_DIR/prof"
export LLVM_PROFILE_FILE="$prof/%p-%m.profraw"

# Build everything first so build-script/proc-macro runs don't leave stray profiles behind.
cargo build --offline -p mgmt-cli
mapfile -t objects < <(cargo test --offline --workspace --no-run --message-format=json \
  | jq -r 'select(.executable != null) | .executable')
export MGMT_BIN="$CARGO_TARGET_DIR/debug/mgmt"
objects+=("$MGMT_BIN")
rm -rf "$prof" web/coverage
mkdir -p "$prof"

# A failing suite still yields a report; the exit code carries the failure.
status=0
if [[ $suites == *" unit "* ]]; then cargo test --offline --workspace || status=1; fi
if [[ $suites == *" bb "* ]]; then tests/blackbox/.venv/bin/python -m pytest tests/blackbox -q || status=1; fi
if [[ $suites == *" web "* ]]; then
  (cd web && npm run build:cov && MGMT_COVERAGE=1 npx playwright test) || status=1
  (cd web && npm run coverage)
fi

major=$(rustc -vV | sed -n 's/^LLVM version: \([0-9]*\).*/\1/p')
llvm() {
  if [[ -n ${LLVM_BIN:-} ]]; then "$LLVM_BIN/$1" "${@:2}"
  else nix shell "nixpkgs#llvmPackages_${major}.llvm" --command "$@"
  fi
}
merged="$CARGO_TARGET_DIR/merged.profdata"
llvm llvm-profdata merge -sparse "$prof"/*.profraw -o "$merged"
args=("${objects[0]}")
for o in "${objects[@]:1}"; do args+=(-object "$o"); done
ignore='/\.cargo/|/rustc/|/nix/store/|/vendor/|/target/'
llvm llvm-cov report "${args[@]}" -instr-profile="$merged" -ignore-filename-regex="$ignore" \
  | tee "$CARGO_TARGET_DIR/report.txt"
llvm llvm-cov export -format=lcov "${args[@]}" -instr-profile="$merged" -ignore-filename-regex="$ignore" \
  > "$CARGO_TARGET_DIR/lcov.info"
echo "rust: $CARGO_TARGET_DIR/report.txt  lcov: $CARGO_TARGET_DIR/lcov.info"
tests/coverage-summary.sh
exit $status
