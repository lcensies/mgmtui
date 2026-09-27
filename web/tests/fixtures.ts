// Playwright fixtures: start an isolated `mgmt web` server per test against a fresh, EMPTY temp
// vault (loopback, no auth), serving the built web/dist. Each test gets its own clean data + URL,
// so tests never touch real data and never interfere with each other.

import { test as base } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "..", "..");
const BIN = process.env.MGMT_BIN || join(REPO, "target", "debug", "mgmt");
const DIST = resolve(HERE, "..", "dist");
// MGMT_COVERAGE=1: dump raw V8 JS coverage per test into web/coverage/raw (see scripts/coverage.mjs).
const COVERAGE = process.env.MGMT_COVERAGE === "1";
const RAW_DIR = resolve(HERE, "..", "coverage", "raw");

interface App {
  base: string;
  dataDir: string;
}

async function startServer(): Promise<{ base: string; dataDir: string; proc: ChildProcess }> {
  const dataDir = mkdtempSync(join(tmpdir(), "mgmt-e2e-"));
  const proc = spawn(BIN, ["--data-dir", dataDir, "web", "serve", "--bind", "127.0.0.1:0", "--assets-dir", DIST], {
    // Keep config writes (calendar metadata lands in config.yaml) inside the temp dir too.
    env: { ...process.env, XDG_CONFIG_HOME: join(dataDir, "config") },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const base = await new Promise<string>((res, rej) => {
    const to = setTimeout(() => rej(new Error("server did not start")), 15_000);
    const onData = (buf: Buffer) => {
      const m = /listening on (http:\/\/\S+)/.exec(buf.toString());
      if (m) {
        clearTimeout(to);
        proc.stdout?.off("data", onData);
        proc.stderr?.off("data", onData);
        res(m[1]);
      }
    };
    proc.stdout?.on("data", onData);
    proc.stderr?.on("data", onData);
    proc.on("exit", (code) => rej(new Error(`server exited early (${code})`)));
  });
  return { base, dataDir, proc };
}

export const test = base.extend<{ app: App; jsCoverage: void }>({
  app: async ({}, use) => {
    const { base, dataDir, proc } = await startServer();
    await use({ base, dataDir });
    const exited = new Promise((r) => proc.once("exit", r));
    proc.kill("SIGINT");
    // The instrumented binary writes its .profraw only on a clean exit.
    if (COVERAGE) await exited;
    try {
      rmSync(dataDir, { recursive: true, force: true });
    } catch {
      /* ignore */
    }
  },
  jsCoverage: [
    async ({ page }, use, testInfo) => {
      if (!COVERAGE) return use();
      await page.coverage.startJSCoverage({ resetOnNavigation: false });
      await use();
      const entries = (await page.coverage.stopJSCoverage())
        .filter((e) => e.url.includes("/assets/"))
        .map((e) => {
          const m = /\/\/# sourceMappingURL=(\S+)/.exec(e.source ?? "");
          const sourceMap = m ? JSON.parse(readFileSync(join(DIST, "assets", basename(m[1])), "utf8")) : undefined;
          return { ...e, sourceMap };
        });
      mkdirSync(RAW_DIR, { recursive: true });
      writeFileSync(join(RAW_DIR, `${testInfo.testId}-${testInfo.retry}.json`), JSON.stringify(entries));
    },
    { auto: true },
  ],
});

export { expect } from "@playwright/test";
