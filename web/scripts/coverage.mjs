// Turn the raw V8 coverage dumped by tests/fixtures.ts (MGMT_COVERAGE=1) into web/coverage/
// (lcov.info + console summary), mapped back to src/ through the build's sourcemaps.
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { CoverageReport } from "monocart-coverage-reports";

const WEB = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const RAW = join(WEB, "coverage", "raw");

const mcr = new CoverageReport({
  name: "mgmt PWA",
  outputDir: join(WEB, "coverage"),
  reports: ["console-summary", "json-summary", "lcovonly"],
  sourceFilter: "**/src/**",
  // Sourcemap paths are relative to dist/assets; collapse them to repo-relative web/src/...
  sourcePath: (p) => p.replace(/^.*?src\//, "web/src/"),
  all: { dir: join(WEB, "src"), filter: "**/*.{ts,tsx}" },
  cleanCache: true,
  // tests/coverage.sh wipes web/coverage itself; MCR's own clean would delete raw/ mid-run.
  clean: false,
});

let files = [];
try {
  files = readdirSync(RAW).filter((f) => f.endsWith(".json"));
} catch {
  console.error(`no raw coverage in ${RAW} — run playwright with MGMT_COVERAGE=1 first`);
  process.exit(1);
}
for (const f of files) {
  const entries = JSON.parse(readFileSync(join(RAW, f), "utf8"));
  if (entries.length) await mcr.add(entries);
}
await mcr.generate();
