// Playwright fixtures: start an isolated `mgmt web` server per test against a fresh, EMPTY temp
// vault (loopback, no auth), serving the built web/dist. Each test gets its own clean data + URL,
// so tests never touch real data and never interfere with each other.

import { test as base } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "..", "..");
const BIN = join(REPO, "target", "debug", "mgmt");
const DIST = resolve(HERE, "..", "dist");

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

export const test = base.extend<{ app: App }>({
  app: async ({}, use) => {
    const { base, dataDir, proc } = await startServer();
    await use({ base, dataDir });
    proc.kill("SIGINT");
    try {
      rmSync(dataDir, { recursive: true, force: true });
    } catch {
      /* ignore */
    }
  },
});

export { expect } from "@playwright/test";
