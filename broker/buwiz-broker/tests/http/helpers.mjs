// Shared by the HTTP tests: a buwiz-broker on its own port and data
// directory (MOCK by default, so no model calls; a test can turn it off
// through startServer's env), and raw HTTP requests to it. Plain `node`,
// no packages (only office.test.mjs needs office/'s, see run.sh): run.sh passes the broker binary as BUWIZ_BROKER_BIN.
import { spawn } from "node:child_process";
import { DatabaseSync } from "node:sqlite";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { request as httpRequest } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

export const BROKER = process.env.BUWIZ_BROKER_BIN || "";
export const PORT = Number(process.env.TEST_PORT || 8797);
/** The broker's own assets (templates, bundled plugins, trust list). */
export const ASSETS = join(HERE, "..", "..", "assets");
export const FIXTURE = join(HERE, "fixtures", "mcp-server.mjs");

export const tempDir = (label) => mkdtempSync(join(tmpdir(), `buwiz-http-${label}-`));

// PATH stubs for the Mac tools the broker shells out to (nsh and the
// nsh-macos-host beside it, pbcopy, open, osascript, sips, qlmanage,
// secret, aerospace): each file's header says what it does and which env
// vars steer it.
export const STUB_BIN = join(HERE, "..", "fixtures", "bin");

/** The env that puts the stubs first on the broker's PATH, NSH at the nsh
 *  stub, and their call log at `log` (BUWIZ_STUB_LOG). */
export const stubEnv = (log) => ({ PATH: `${STUB_BIN}:${process.env.PATH}`, NSH: join(STUB_BIN, "nsh"), BUWIZ_STUB_LOG: log });

/** The stubs' calls so far: [{bin, argv, stdin?}], oldest first. */
export const stubCalls = (log) =>
  existsSync(log) ? readFileSync(log, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : [];

/** The stubs plus every model URL at a closed port: MOCK does not guard
 *  /models, /avatar/gen, image models or app icons, and nothing a test
 *  starts may reach a real model gateway. */
export const offlineEnv = (log) => ({ ...stubEnv(log), OPENCODEX_URL: "http://127.0.0.1:9", ANTHROPIC_BASE_URL: "http://127.0.0.1:9", ANTHROPIC_API_KEY: undefined });

/** Rows of `sql` in data dir `dataDir`'s app.db, read-only. */
export function dbRows(dataDir, sql, ...args) {
  const db = new DatabaseSync(join(dataDir, "generated", "app.db"), { readOnly: true, timeout: 5000 });

  try {
    return db.prepare(sql).all(...args).map((r) => ({ ...r }));
  } finally {
    db.close();
  }
}

/** The env for a broker that talks to a fake model gateway (the fake
 *  opencodex in broker/buwiz-broker/tests/fixtures) with MOCK off: every
 *  provider URL at the fake, the provider keys and model overrides of this
 *  shell removed, the stubs on PATH. `extra` goes over it. */
export const modelEnv = (fake, log, extra = {}) => ({
  MOCK: undefined,
  OPENCODEX_URL: fake.url,
  ANTHROPIC_BASE_URL: undefined,
  ANTHROPIC_API_KEY: undefined,
  ANTHROPIC_AUTH_TOKEN: undefined,
  GEN_PROVIDER: undefined,
  GEN_BASE_URL: undefined,
  GEN_MODEL: undefined,
  GEN_AVATAR_MODEL: undefined,
  GEN_MAX_TOKENS: undefined,
  OPENROUTER_API_KEY: undefined,
  OPENAI_API_KEY: undefined,
  GEMINI_API_KEY: undefined,
  LINEAR_API_KEY: undefined,
  ...stubEnv(log),
  ...extra,
});

// The routes the app reaches with the host token (X-Buwiz-Host) and a Bot's
// shell does not: request() adds the token of the broker started last to
// them, unless the test names the header itself or asks for none.
const LOCKED = new Set(["/mcp/servers", "/plugins/install", "/plugins/connect", "/plugins/settings", "/skills", "/skills/state", "/skills/delete", "/bot/new", "/bot/update", "/bot/memory", "/bot/integration", "/bot/delete", "/job/stop", "/mcp/server/state", "/mcp/tools/state", "/mcp/server/delete", "/file/write", "/file/open", "/file/save"]);
const LOCKED_READS = new Set(["/jobs/running"]);
let hostToken = "";

/** One request: {status, headers, text, json}. `headers` as given (an
 *  Origin is sent as is); `body` a string or a Buffer; `noToken` keeps the
 *  host token off a locked route; a broker that does not answer in `timeout`
 *  ms fails the request. */
export function request(method, path, { headers = {}, body, noToken = false, timeout = 60000 } = {}) {
  const route = path.split("?")[0];
  const locked = method === "POST" ? LOCKED.has(route) : method === "GET" && LOCKED_READS.has(route);

  if (locked && hostToken && !noToken && !Object.keys(headers).some((k) => k.toLowerCase() === "x-buwiz-host")) headers = { ...headers, "X-Buwiz-Host": hostToken };

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

    req.setTimeout(timeout, () => req.destroy(new Error(`${method} ${path}: no answer from the broker in ${timeout} ms`)));
    req.on("error", reject);

    if (body !== undefined) req.write(body);
    req.end();
  });
}

/** A POST as the app sends it: JSON body, Content-Type application/json, no Origin. */
export const postJson = (path, obj) =>
  request("POST", path, { headers: { "Content-Type": "application/json" }, body: JSON.stringify(obj) });

const up = () => request("GET", "/health", { timeout: 2000 }).then((r) => r.status === 200, () => false);

/** The environment startServer gives the broker: this process's, the test
 *  defaults (PORT, BUWIZ_BROKER_DATA_DIR, MOCK=1, no Keychain, no extra origins, a HOME of its own), then
 *  `env` over them; a key set to `undefined` is removed (`{MOCK: undefined}` turns MOCK off). */
export function serverEnv(dataDir, env = {}) {
  const home = join(dataDir, "home");
  // no model provider's setting of the caller's reaches a broker
  const inherited = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(GEN_|ANTHROPIC_|OPENAI_|OPENROUTER_|GEMINI_|OPENCODEX_|LINEAR_)/.test(k)));
  const out = {
    ...inherited, HOME: home, PORT: String(PORT), BUWIZ_BROKER_DATA_DIR: dataDir, MOCK: "1", BUWIZ_ALLOWED_ORIGINS: "",
    BUWIZ_NO_KEYCHAIN: "1",
  };

  for (const [k, v] of Object.entries(env)) {
    if (v === undefined) delete out[k];
    else out[k] = String(v);
  }

  return out;
}

/** Starts the broker on PORT with BUWIZ_BROKER_DATA_DIR=dataDir; resolves to
 *  {log, stop, pid} (pid leads the broker's process group). `env`
 *  overrides the environment (serverEnv). */
export async function startServer(dataDir, { env, args = [] } = {}) {
  if (!BROKER) throw new Error("BUWIZ_BROKER_BIN names no broker (run.sh sets it)");
  if (await up()) throw new Error(`port ${PORT} is already serving; stop that first`);

  mkdirSync(join(dataDir, "home"), { recursive: true });
  mkdirSync(join(dataDir, "widgets"), { recursive: true });

  // its own process group, so stop() also ends the MCP children it spawned
  const child = spawn(BROKER, ["--assets", ASSETS, ...args], {
    cwd: dataDir,
    env: serverEnv(dataDir, env),
    stdio: ["ignore", "pipe", "pipe"],
    detached: true,
  });

  let log = "";
  child.stdout.on("data", (d) => (log += d));
  child.stderr.on("data", (d) => (log += d));
  const exited = new Promise((r) => child.once("exit", r));
  const stop = async () => {
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {}

    const t = setTimeout(() => {
      try {
        process.kill(-child.pid, "SIGKILL");
      } catch {}
    }, 3000);

    if (child.exitCode === null && child.signalCode === null) await exited;
    clearTimeout(t);
  };

  // a broker built a moment ago can take a minute to start the first time (the OS scans a new binary)
  for (let i = 0; i < 1200 && !(await up()); i++) {
    if (child.exitCode !== null) throw new Error(`broker exited early:\n${log}`);
    await new Promise((r) => setTimeout(r, 100));
  }

  if (!(await up())) {
    await stop();
    throw new Error(`broker did not answer on ${PORT}:\n${log}`);
  }

  hostToken = "";

  for (let i = 0; i < 50 && !hostToken; i++) {
    try {
      hostToken = readFileSync(join(dataDir, "host-token"), "utf8").trim();
    } catch {
      await new Promise((r) => setTimeout(r, 100));
    }
  }

  return { log: () => log, stop, pid: child.pid };
}

/** The app.db of `dataDir` open for writing, to seed rows before a test
 *  starts its broker: when the db is not there yet, a broker is started on
 *  the dir once, to make its tables, and stopped. */
export async function seedDb(dataDir, { env } = {}) {
  const file = join(dataDir, "generated", "app.db");

  if (!existsSync(file)) await (await startServer(dataDir, { env })).stop();

  return new DatabaseSync(file, { timeout: 5000 });
}

/** What the broker's build pipeline leaves for one version of module `name`
 *  in `dataDir` (`db` its app.db from seedDb): the version file, the head
 *  mirror, a `versions` row and the head. Returns the version number. */
export function writeVersion(dataDir, db, name, code, parent, prompt) {
  const gen = join(dataDir, "generated");
  const v = (db.prepare("SELECT MAX(v) AS v FROM versions WHERE name=?").get(name).v || 0) + 1;

  mkdirSync(join(gen, name), { recursive: true });
  writeFileSync(join(gen, name, `${name}.v${v}.tsx`), code);
  writeFileSync(join(gen, `${name}.tsx`), code);
  db.prepare("INSERT INTO versions(name, v, parent, prompt, created_at, label) VALUES(?,?,?,?,?,NULL)").run(name, v, parent, prompt, Date.now());
  db.prepare("INSERT OR REPLACE INTO heads(name, v) VALUES(?,?)").run(name, v);

  return v;
}

/** The person's "allow for this chat" on a tool's ask (`key` names the
 *  tool's group): without it the broker asks first in a plain chat. */
export function allow(dataDir, thread, key) {
  const db = new DatabaseSync(join(dataDir, "generated", "app.db"), { timeout: 5000 });

  try {
    db.prepare("INSERT OR REPLACE INTO ask_grant(thread, key, at) VALUES(?,?,?)").run(thread, key, Date.now());
  } finally {
    db.close();
  }
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
