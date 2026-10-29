// Shared by the server tests: a server of this checkout on its own port
// and data directory (MOCK by default, so no model calls; a test can turn
// it off through startServer's env), and raw HTTP requests to it.
import { execFileSync, spawn } from "node:child_process";
import { DatabaseSync } from "node:sqlite";
import { existsSync, mkdirSync, mkdtempSync, readFileSync } from "node:fs";
import { request as httpRequest } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
export const PORT = Number(process.env.TEST_PORT || 8797);
export const FIXTURE = join(ROOT, "test", "fixtures", "mcp-server.mjs");

/** The broker the HTTP tests start: `ROX_BROKER_CMD` (a command line,
 *  relative paths from server/) when set, else Node as it ships. The tests
 *  are the contract, whichever broker answers (the Rust broker plan, R4). */
export function brokerCommand() {
  const cmd = (process.env.ROX_BROKER_CMD || "").trim();

  if (!cmd) return [process.execPath, [join(ROOT, "src", "app-server.mjs")]];
  const [bin, ...args] = cmd.split(/\s+/);

  return [bin === "node" ? process.execPath : bin, args];
}

export const tempDir = (label) => mkdtempSync(join(tmpdir(), `grok-test-${label}-`));

// PATH stubs for the Mac tools the broker shells out to (nsh, pbcopy,
// open, osascript, sips, qlmanage, secret): each file's header says what it
// does and which env vars steer it.
export const STUB_BIN = join(ROOT, "test", "fixtures", "bin");

/** The env that puts the stubs first on the broker's PATH, NSH at the nsh
 *  stub, and their call log at `log` (ROX_STUB_LOG). */
export const stubEnv = (log) => ({ PATH: `${STUB_BIN}:${process.env.PATH}`, NSH: join(STUB_BIN, "nsh"), ROX_STUB_LOG: log });

/** The stubs' calls so far: [{bin, argv, stdin?}], oldest first. */
export const stubCalls = (log) =>
  existsSync(log) ? readFileSync(log, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : [];

/** The stubs plus every model URL at a closed port: MOCK does not guard
 *  /models, /avatar/gen, image models or app icons, and nothing a test
 *  starts may reach a real opencodex. */
export const offlineEnv = (log) => ({ ...stubEnv(log), OPENCODEX_URL: "http://127.0.0.1:9", ANTHROPIC_BASE_URL: "http://127.0.0.1:9", ANTHROPIC_API_KEY: undefined });

/** Runs `code` (an ES module body, cwd server/) in a Node of its own on
 *  data dir `dataDir` (its widgets/ made first), as the broker's modules see it (seeding a store
 *  through state.mjs, say); returns its last line of output. */
export const nodeIn = (dataDir, code, env = {}) => (
  mkdirSync(join(dataDir, "widgets"), { recursive: true }),
  execFileSync(process.execPath, ["--input-type=module", "-e", `${code}\nprocess.exit(0);`], {
    cwd: ROOT,
    env: { ...process.env, GROK_DATA_DIR: dataDir, MOCK: "1", ...env },
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim().split("\n").at(-1)
);

/** Rows of `sql` in data dir `dataDir`'s app.db, read-only. */
export function dbRows(dataDir, sql, ...args) {
  const db = new DatabaseSync(join(dataDir, "generated", "app.db"), { readOnly: true, timeout: 5000 });

  try {
    return db.prepare(sql).all(...args);
  } finally {
    db.close();
  }
}

/** The env for a broker that talks to a fake opencodex (fixtures/
 *  fake-opencodex.mjs) with MOCK off: every provider URL at the fake, the
 *  provider keys and model overrides of this shell removed, the stubs on
 *  PATH. `extra` goes over it. */
export const modelEnv = (fake, log, extra = {}) => ({
  MOCK: undefined,
  OPENCODEX_URL: fake.url,
  ANTHROPIC_BASE_URL: undefined,
  ANTHROPIC_API_KEY: undefined,
  ANTHROPIC_AUTH_TOKEN: undefined,
  GEN_PROVIDER: undefined,
  GEN_BASE_URL: undefined,
  GEN_MODEL: undefined,
  GEN_MAX_TOKENS: undefined,
  OPENROUTER_API_KEY: undefined,
  OPENAI_API_KEY: undefined,
  GEMINI_API_KEY: undefined,
  LINEAR_API_KEY: undefined,
  ...stubEnv(log),
  ...extra,
});

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

/** The environment startServer gives the broker: this process's, the test
 *  defaults (PORT, GROK_DATA_DIR, MOCK=1, no extra origins), then `env`
 *  over them; a key set to `undefined` is removed (`{MOCK: undefined}`
 *  turns MOCK off). */
export function serverEnv(dataDir, env = {}) {
  const out = { ...process.env, PORT: String(PORT), GROK_DATA_DIR: dataDir, MOCK: "1", GROK_ALLOWED_ORIGINS: "" };

  for (const [k, v] of Object.entries(env)) {
    if (v === undefined) delete out[k];
    else out[k] = String(v);
  }

  return out;
}

/** Starts the server on PORT with GROK_DATA_DIR=dataDir; resolves to a
 *  stop(). `env` overrides the environment (serverEnv). */
export async function startServer(dataDir, { env } = {}) {
  if (await up()) throw new Error(`port ${PORT} is already serving; stop that first`);

  // its own process group, so stop() also ends the MCP children it spawned
  const [bin, args] = brokerCommand();
  const child = spawn(bin, args, {
    cwd: ROOT,
    env: serverEnv(dataDir, env),
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
