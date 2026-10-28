// Shared by the server tests: a server of this checkout on its own port
// and data directory (MOCK, so no model calls), and raw HTTP requests to it.
import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { request as httpRequest } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
export const PORT = Number(process.env.TEST_PORT || 8797);
export const FIXTURE = join(ROOT, "test", "fixtures", "mcp-server.mjs");

export const tempDir = (label) => mkdtempSync(join(tmpdir(), `grok-test-${label}-`));

/** One request: {status, headers, text, json}. `headers` as given (an
 *  Origin is sent as is); `body` a string or a Buffer. */
export function request(method, path, { headers = {}, body } = {}) {
  return new Promise((resolve, reject) => {
    const req = httpRequest({ host: "127.0.0.1", port: PORT, method, path, headers }, (res) => {
      let text = "";
      res.setEncoding("utf8");
      res.on("data", (c) => (text += c));
      res.on("end", () => {
        let json = null;

        try {
          json = JSON.parse(text);
        } catch {}

        resolve({ status: res.statusCode, headers: res.headers, text, json });
      });
    });

    req.on("error", reject);

    if (body !== undefined) req.write(body);
    req.end();
  });
}

/** A POST as Rox sends it: JSON body, Content-Type application/json, no Origin. */
export const postJson = (path, obj) =>
  request("POST", path, { headers: { "Content-Type": "application/json" }, body: JSON.stringify(obj) });

const up = () => request("GET", "/health").then((r) => r.status === 200, () => false);

/** Starts the server on PORT with GROK_DATA_DIR=dataDir; resolves to a stop(). */
export async function startServer(dataDir) {
  if (await up()) throw new Error(`port ${PORT} is already serving; stop that first`);

  // its own process group, so stop() also ends the MCP children it spawned
  const child = spawn(process.execPath, [join(ROOT, "server", "app-server.mjs")], {
    cwd: ROOT,
    env: { ...process.env, PORT: String(PORT), GROK_DATA_DIR: dataDir, MOCK: "1", GROK_ALLOWED_ORIGINS: "" },
    stdio: ["ignore", "pipe", "pipe"],
    detached: true,
  });

  let log = "";
  child.stdout.on("data", (d) => (log += d));
  child.stderr.on("data", (d) => (log += d));
  const exited = new Promise((r) => child.once("exit", r));

  for (let i = 0; i < 200 && !(await up()); i++) {
    if (child.exitCode !== null) throw new Error(`server exited early:\n${log}`);
    await new Promise((r) => setTimeout(r, 100));
  }

  if (!(await up())) throw new Error(`server did not answer on ${PORT}:\n${log}`);

  return {
    log: () => log,
    stop: async () => {
      try {
        process.kill(-child.pid, "SIGTERM");
      } catch {}

      const t = setTimeout(() => {
        try {
          process.kill(-child.pid, "SIGKILL");
        } catch {}
      }, 3000);

      await exited;
      clearTimeout(t);
    },
  };
}

/** Polls `check` until it returns truthy (its value) or `ms` pass (null). */
export async function waitFor(check, ms = 10000) {
  const end = Date.now() + ms;

  while (Date.now() < end) {
    const v = await check();

    if (v) return v;
    await new Promise((r) => setTimeout(r, 100));
  }

  return null;
}
