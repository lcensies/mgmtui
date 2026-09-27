#!/usr/bin/env bash
# Compact per-crate line-coverage summary from the files tests/coverage.sh leaves behind.
set -euo pipefail
cd "$(dirname "$0")/.."
test -s target/cov/report.txt || { echo 'no target/cov/report.txt — run `just cov` first' >&2; exit 1; }
echo '## Rust line coverage (unit + blackbox + playwright)'
# llvm-cov report columns: file regions missed cover% functions missed executed% lines missed cover% ...
# File paths are relative to crates/ (llvm-cov strips the common prefix), so $1's first component is the crate.
awk '$1 ~ /^mgmt-/ && NF >= 10 { split($1, p, "/"); c = p[1]; lines[c] += $8; missed[c] += $9 }
     END { for (c in lines) printf "%-16s %5.1f%%  (%d/%d lines)\n", c, 100 * (lines[c] - missed[c]) / lines[c], lines[c] - missed[c], lines[c] }' \
  target/cov/report.txt | sort
awk '$1 == "TOTAL" { printf "%-16s %s  (%d/%d lines)\n", "TOTAL", $10, $8 - $9, $8 }' target/cov/report.txt
echo
echo '## PWA line coverage (V8 via Playwright)'
if [[ -s web/coverage/coverage-summary.json ]]; then
  jq -r '.total.lines | "web/src          \(.pct)%  (\(.covered)/\(.total) lines)"' web/coverage/coverage-summary.json
else
  echo 'none (run `just cov web`)'
fi
