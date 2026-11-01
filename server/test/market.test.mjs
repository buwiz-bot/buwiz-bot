// The plugin market and private skills (Rust broker plan, phase 5 item
// 5.8) as Node answers them: the catalog, a plugin's page, installs from
// git and uninstalls (files, installed_plugins, mcp_servers rows), ${KEY}
// setup values (plugin_settings, the shared @google row), connector
// accounts, a skill's SKILL.md, POST /skills, /skills/state,
// /skills/delete, the six skill tools (module level), an installed plugin
// no catalog lists, and the bundled first-party catalog.
//
// The broker reads a synthetic catalog (CATALOG below, written to a temp
// file and named by BUWIZ_PLUGIN_CATALOG); its plugins and hosts are made up.
// No network: the catalog's repositories are local bare repos built here,
// reached through git's own `url.<file://…>.insteadOf` (GIT_CONFIG_*), with
// only the file protocol allowed. The catalog also probes some connectors'
// OAuth servers (plugins.mjs dcrSupport); every broker here sends HTTP(S)
// through a proxy sink on the second test port, which records the CONNECT
// and refuses it.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { DatabaseSync } from "node:sqlite";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { FIXTURE, PORT, ROOT, dbRows, nodeIn, offlineEnv, postJson, request, startServer, tempDir, waitFor } from "./helpers.mjs";

const SINK_PORT = Number(process.env.TEST_SINK_PORT || PORT + 3);
const ORBIT = "https://github.com/example/orbit-plugin";
const PLUGINS_REPO = "https://github.com/example/plugins";
const orbitSkills = ["orbit-cloud", "orbit-developer", "orbit-ops"];
const CATALOG = {
  name: "test-catalog",
  owner: { name: "Test Owner" },
  version: "t1",
  plugins: [
    {
      id: "101", key: "orbit", name: "orbit", pluginName: "orbit", displayName: "Orbit", description: "Run and operate Orbit workflows.",
      category: "Infrastructure", categoryKeys: ["INFRASTRUCTURE"], repositoryUrl: ORBIT, websiteUrl: "https://orbit.example",
      connectors: [], publisher: { name: "orbit", displayName: "Orbit" },
      skills: orbitSkills.map((n) => ({ name: n, description: `What ${n} does.`, sourceUrl: `${ORBIT}/blob/main/skills/${n}/SKILL.md` })),
      install: { repo: ORBIT, sha: "1".repeat(40), path: "", via: "manifest", servers: {}, skills: orbitSkills.map((n) => `skills/${n}/SKILL.md`) },
    },
    {
      id: "102", key: "mailbox", name: "mailbox", pluginName: "mailbox", displayName: "Mailbox", description: "Mail on Google's hosted MCP server.",
      category: "Featured", categoryKeys: ["FEATURED", "INBOX_AND_COLLABORATION"], repositoryUrl: PLUGINS_REPO, websiteUrl: "https://plugins.example",
      connectors: [{ name: "mailbox", description: "" }], skills: [], publisher: { name: "example", displayName: "Example" },
      install: { repo: PLUGINS_REPO, sha: "2".repeat(40), path: "mailbox", via: "marketplace", servers: { mailbox: { type: "http", url: "https://gmailmcp.googleapis.com/mcp/v1" } }, skills: [] },
    },
    {
      id: "103", key: "sketch-docs", name: "sketch-docs", pluginName: "sketch-docs", displayName: "Sketch Docs", description: "Render docs as a sketch.",
      category: "Canvas", categoryKeys: ["CANVAS"], repositoryUrl: PLUGINS_REPO, websiteUrl: "https://plugins.example", connectors: [],
      skills: [{ name: "sketch-docs", description: "Render docs.", sourceUrl: `${PLUGINS_REPO}/blob/main/sketch-docs/skills/sketch-docs/SKILL.md` }],
      publisher: { name: "example", displayName: "Example" },
      install: { repo: PLUGINS_REPO, sha: "3".repeat(40), path: "sketch-docs", via: "marketplace", servers: {}, skills: ["sketch-docs/skills/sketch-docs/SKILL.md"] },
    },
    {
      id: "104", key: "relay", name: "relay", pluginName: "relay", displayName: "Relay", description: "A connector that pins its vendor's own OAuth client.",
      categoryKeys: ["SALES"], connectors: [{ name: "relay" }], skills: [], publisher: { name: "relay", displayName: "Relay" },
      install: { repo: "https://github.com/example/relay", path: "", servers: { relay: { type: "http", url: "https://mcp.relay.example/mcp", auth: { CLIENT_ID: "relay-own-client" } } }, skills: [] },
    },
    {
      name: "lumen", description: "Query logs with Lumen.", category: "observability", homepage: "https://lumen.example",
      source: { source: "url", url: "https://github.com/example/lumen-skills.git", sha: "4".repeat(40) }, keywords: ["lumen"],
    },
  ],
};
const entry = (key) => CATALOG.plugins.find((e) => (e.key || e.name) === key);

const dir = tempDir("market");
const CATALOG_FILE = join(dir, "..", `${dir.split("/").pop()}-catalog.json`);
const repos = join(dir, "..", `${dir.split("/").pop()}-repos`);
const PLUGINS = join(dir, "plugins");
// git with no user or system config (an insteadOf there could send a URL
// elsewhere) and only file:// allowed
const GIT_ENV = { GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null", GIT_ALLOW_PROTOCOL: "file", GIT_TERMINAL_PROMPT: "0" };
const git = (cwd, ...args) => execFileSync("git", ["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "init.defaultBranch=main", ...args], { cwd, env: { ...process.env, ...GIT_ENV }, stdio: "pipe" });

/** A bare repo at <repos>/<name>.git holding `files` ({path: text}). */
function bareRepo(name, files) {
  const work = join(repos, `${name}-work`);

  for (const [p, text] of Object.entries(files)) {
    mkdirSync(join(work, p, ".."), { recursive: true });
    writeFileSync(join(work, p), text);
  }
  git(work, "init", "-q");
  git(work, "add", "-A");
  git(work, "commit", "-q", "-m", "plugin");
  git(repos, "clone", "-q", "--bare", work, join(repos, `${name}.git`));

  return `file://${join(repos, `${name}.git`)}`;
}

const skillMd = (name, desc) => `---\nname: ${name}\ndescription: ${desc}\n---\n\n# ${name}\n\nDo the ${name} thing.\n`;

// orbit: the catalog's path ("") is the repo's root; two connectors, one
// on its own ${KEY}s, one on the shared Google client
const ORBIT_MCP = {
  mcpServers: {
    "orbit-mcp": { command: "node", args: [FIXTURE], env: { API_KEY: "${ORBIT_API_KEY}", ADDRESS: "${ORBIT_ADDRESS:-localhost:7233}", ROOT: "${CLAUDE_PLUGIN_ROOT}" } },
    "orbit-google": { command: "node", args: [FIXTURE], env: { GOOGLE_CLIENT_ID: "${GOOGLE_CLIENT_ID}", GOOGLE_CLIENT_SECRET: "${GOOGLE_CLIENT_SECRET}" } },
  },
};
const orbitFiles = () => ({
  ".cursor-plugin/plugin.json": JSON.stringify({ name: "orbit", version: "1.2.3", author: { name: "Orbit Tester" }, license: "MIT" }),
  ...Object.fromEntries(entry("orbit").install.skills.map((f) => [f, skillMd(f.split("/").at(-2), `The ${f.split("/").at(-2)} skill.`)])),
  "commands/run.md": "---\ndescription: Run a workflow\n---\nRun it.\n",
  "agents/helper.md": "---\ndescription: >-\n  A helper that\n  spans lines\n---\nHelp.\n",
  "hooks/hooks.json": "{}",
  ".mcp.json": JSON.stringify(ORBIT_MCP),
  "README.md": "orbit\n",
});
// example/plugins: sketch-docs is not at the catalog's path, so the install
// resolves it through the repo's marketplace.json (plugin_resolve.mjs)
const sketchFiles = () => ({
  ".cursor-plugin/marketplace.json": JSON.stringify({ plugins: [{ name: "sketch-docs", source: "plugins/sketch-docs" }] }),
  "plugins/sketch-docs/.cursor-plugin/plugin.json": JSON.stringify({ name: "sketch-docs", version: "0.1.0", author: "Sketch Co" }),
  "plugins/sketch-docs/skills/sketch-docs/SKILL.md": skillMd("sketch-docs", "|\n  Render docs."),
  "plugins/sketch-docs/.mcp.json": JSON.stringify({ mcpServers: { canvas: { command: "node", args: [FIXTURE] } } }),
  "plugins/other/plugin.json": JSON.stringify({ name: "other" }),
});
// lumen: listed by its source, cloned whole
const lumenFiles = () => ({ "skills/apl/SKILL.md": skillMd("apl", "Query with APL."), ".claude-plugin/plugin.json": JSON.stringify({ name: "lumen" }) });

let sink;
const sinkHits = [];
let server;
let gitEnv;

const env = () => ({
  ...offlineEnv(join(dir, "stubs.log")),
  ...GIT_ENV,
  ...gitEnv,
  // the catalog's OAuth probes go to the sink (Node: NODE_USE_ENV_PROXY)
  NODE_USE_ENV_PROXY: "1",
  HTTPS_PROXY: `http://127.0.0.1:${SINK_PORT}`,
  HTTP_PROXY: `http://127.0.0.1:${SINK_PORT}`,
  NO_PROXY: "127.0.0.1,localhost",
  // buwiz-broker hosts the MCP servers (Node ignores these)
  BUWIZ_BROKER_MCP_HOST: "1",
  BUWIZ_PLUGIN_CATALOG: CATALOG_FILE,
});

before(async () => {
  mkdirSync(repos, { recursive: true });
  writeFileSync(CATALOG_FILE, JSON.stringify(CATALOG));
  const urls = {
    "https://github.com/example/orbit-plugin": bareRepo("orbit", orbitFiles()),
    "https://github.com/example/plugins": bareRepo("example", sketchFiles()),
    "https://github.com/example/lumen-skills": bareRepo("lumen", lumenFiles()),
  };
  gitEnv = { GIT_CONFIG_COUNT: String(Object.keys(urls).length) };
  Object.entries(urls).forEach(([from, to], i) => {
    gitEnv[`GIT_CONFIG_KEY_${i}`] = `url.${to}.insteadOf`;
    gitEnv[`GIT_CONFIG_VALUE_${i}`] = from;
  });
  sink = createServer((s) => {
    s.on("data", (d) => {
      for (const l of String(d).split("\r\n")) if (/^[A-Z]+ \S+ HTTP\//.test(l)) sinkHits.push(l);
      s.end("HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\n\r\n");
    });
    s.on("error", () => {});
  });
  await new Promise((ok, bad) => sink.once("error", bad).listen(SINK_PORT, "127.0.0.1", ok));
  server = await startServer(dir, { env: env() });
});

after(async () => {
  await server?.stop();
  await new Promise((ok) => (sink ? sink.close(ok) : ok()));
});

const get = (path) => request("GET", path);
const catalogRow = (r, key) => r.json.plugins.find((p) => p.name === key);
const PUBLIC = `http://127.0.0.1:${PORT}`;
const rows = (sql, ...a) => dbRows(dir, sql, ...a);
const files = (root) => {
  const out = [];
  const walk = (d, rel) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      if (e.name === ".git") continue;

      if (e.isDirectory()) walk(join(d, e.name), `${rel}${e.name}/`);
      else out.push(rel + e.name);
    }
  };
  walk(root, "");

  return out.sort();
};

test("MKT-01 the catalog: every entry in the catalog file's order, its rev, and {unchanged} for a matching rev", async () => {
  const r = await get("/plugins/catalog");

  assert.equal(r.status, 200);
  assert.equal(r.headers["content-type"], "application/json");
  assert.deepEqual(Object.keys(r.json), ["marketplace", "rev", "categories", "plugins"]);
  assert.equal(r.json.marketplace, "buwiz-bot");
  assert.equal(r.json.rev, "t1||0");
  assert.deepEqual(r.json.categories.map((c) => c.id), ["credentials", "productivity", "communication", "design", "code", "data", "sales", "marketing", "finance", "research", "support"]);
  assert.equal(r.json.categories[0].label, "Password Managers");
  assert.deepEqual(r.json.plugins.map((p) => [p.name, p.category, p.cats, p.setup]), [
    ["orbit", "Code", ["Code"], false],
    ["mailbox", "Communication", ["Communication"], true],
    ["sketch-docs", "Design", ["Design"], false],
    ["relay", "Sales", ["Sales"], true],
    ["lumen", "observability", [], false],
  ]);
  assert.deepEqual(catalogRow(r, "orbit"), {
    name: "orbit",
    displayName: "Orbit",
    description: entry("orbit").description,
    category: "Code",
    cats: ["Code"],
    homepage: "https://orbit.example",
    publisher: "Orbit",
    keywords: ["orbit", "orbit", ...orbitSkills],
    domains: [],
    private: false,
    setup: false,
    unavailable: null,
    installed: false,
    meta: null,
  });
  // an entry listed by its source: its record's fields, the catalog owner as publisher
  assert.deepEqual(catalogRow(r, "lumen"), {
    name: "lumen", displayName: "Lumen", description: "Query logs with Lumen.", category: "observability", cats: [], homepage: "https://lumen.example",
    publisher: "Test Owner", keywords: ["lumen", "lumen", "lumen"], domains: [], private: false, setup: false, unavailable: null, installed: false, meta: null,
  });
  const same = await get(`/plugins/catalog?rev=${encodeURIComponent(r.json.rev)}`);

  assert.equal(same.text, JSON.stringify({ rev: r.json.rev, unchanged: true }));
  // the connector with a vendor's own client id was probed for dynamic
  // registration, through the proxy (never reached)
  const probed = () => [...new Set(sinkHits)].sort();
  const PROBED = ["CONNECT mcp.relay.example:443 HTTP/1.1"];

  assert.ok(await waitFor(() => probed().length >= PROBED.length), sinkHits.join("\n"));
  assert.deepEqual(probed(), PROBED);
});

test("MKT-02 a plugin's page before install: the catalog's skills, its servers from the install hints, setup fields; 404 for an unknown one", async () => {
  const t = await get("/plugins/detail?name=orbit");

  assert.equal(t.status, 200);
  assert.deepEqual(t.json, {
    name: "orbit", displayName: "Orbit", description: entry("orbit").description, category: "Code",
    homepage: "https://orbit.example", source: ORBIT, publisher: "Orbit", availability: "Public", unavailable: null,
    installed: false, meta: null,
    skills: entry("orbit").skills.map((s) => ({ name: s.name, description: s.description, source: s.sourceUrl })),
    commands: [], agents: [], hooks: false, lsp: false, manifest: { author: "Orbit" },
    mcp: [], fields: [],
  });
  const g = await get("/plugins/detail?name=mailbox");

  assert.deepEqual(g.json.mcp, [{
    name: "mailbox", transport: "remote", id: null, status: "not-added", tools: 0, err: null, accounts: [], unavailable: null,
    needs: ["Google Client ID", "Google Client Secret"], callbackUrl: `${PUBLIC}/oauth/callback/mailbox`,
  }]);
  assert.deepEqual(g.json.fields.map((f) => [f.key, f.required, f.secret, f.set, f.value, f.oauth, f.shared]), [
    ["GOOGLE_CLIENT_ID", true, false, false, "", true, "google"],
    ["GOOGLE_CLIENT_SECRET", true, true, false, "", true, "google"],
  ]);
  const none = await get("/plugins/detail?name=nope");

  assert.equal(none.status, 404);
  assert.equal(none.text, '{"error":"unknown plugin"}');
});

let orbitMeta;

test("MKT-03 install: the repo's files under plugins/<key>, the record, a row for each connector with its values; one waiting for Setup", async () => {
  const r = await postJson("/plugins/install", { name: "orbit" });

  assert.equal(r.status, 200, r.text);
  assert.equal(r.text, JSON.stringify({ ok: true, name: "orbit", mcp: ["orbit-mcp"], kinds: ["skills", "commands", "agents", "hooks/hooks.json"], waiting: ["orbit-google"] }));
  assert.deepEqual(files(join(PLUGINS, "orbit")), [".cursor-plugin/plugin.json", ".mcp.json", "README.md", "agents/helper.md", "commands/run.md", "hooks/hooks.json", ...entry("orbit").install.skills].sort());
  // the blobless clone the install read the repo through
  assert.ok(existsSync(join(PLUGINS, ".repos", "example__orbit-plugin", ".git")));
  const [rec] = rows("SELECT * FROM installed_plugins");

  orbitMeta = {
    root: "orbit",
    skills: entry("orbit").install.skills.map((f) => ({ name: f.split("/").at(-2), file: `orbit/${f}` })),
    repo: "https://github.com/example/orbit-plugin",
    sha: entry("orbit").install.sha,
    catalogId: "101",
    mcp: ["orbit-mcp"],
    created: ["orbit-mcp"],
    kinds: ["skills", "commands", "agents", "hooks/hooks.json"],
  };
  assert.equal(rec.name, "orbit");
  assert.equal(rec.meta, JSON.stringify(orbitMeta));
  assert.ok(Math.abs(rec.created_at - Date.now()) < 60000);
  const [row] = rows("SELECT * FROM mcp_servers");

  assert.deepEqual({ ...row, created_at: 0 }, {
    id: "orbit-mcp", name: "orbit-mcp", command: "node", args: JSON.stringify([FIXTURE]),
    env: JSON.stringify({ ADDRESS: "localhost:7233", ROOT: join(PLUGINS, "orbit") }),
    enabled: 1, bot: null, created_at: 0, url: null, disabled_tools: null, auth: null, account: null,
    headers: "{}", oauth_client: null, transport: null, agent_pending: null,
  });
  // the catalog shows it installed, with setup fields now
  const c = await get("/plugins/catalog");
  const t = catalogRow(c, "orbit");

  assert.deepEqual([t.installed, t.setup, t.meta], [true, true, orbitMeta]);
  assert.equal(c.json.rev, `t1|orbit|${JSON.stringify(orbitMeta).length}`);
});

test("MKT-04 an installed plugin's page: its inventory and manifest, each connector's accounts (an idle one is probed), the ${KEY} fields", async () => {
  const d = await get("/plugins/detail?name=orbit");

  assert.deepEqual(d.json.meta, orbitMeta);
  assert.deepEqual(d.json.skills, entry("orbit").install.skills.map((f) => ({ name: f.split("/").at(-2), description: `The ${f.split("/").at(-2)} skill.` })));
  assert.deepEqual(d.json.commands, [{ name: "run", description: "Run a workflow" }]);
  assert.deepEqual(d.json.agents, [{ name: "helper", description: "A helper that spans lines" }]);
  assert.deepEqual([d.json.hooks, d.json.lsp, d.json.manifest], [true, false, { author: "Orbit Tester", version: "1.2.3", license: "MIT" }]);
  assert.deepEqual(d.json.fields, [
    { key: "ORBIT_API_KEY", label: "Orbit API Key", type: "string", placeholder: "ORBIT_API_KEY", required: false, secret: true, hint: "Referenced as `${ORBIT_API_KEY}` in this plugin's MCP configuration.", options: [], set: false, value: "" },
    { key: "ORBIT_ADDRESS", label: "Orbit Address", type: "string", placeholder: "ORBIT_ADDRESS", required: false, secret: false, hint: "Referenced as `${ORBIT_ADDRESS}` in this plugin's MCP configuration. Defaults to `localhost:7233` when left blank.", options: [], set: false, value: "" },
    { key: "GOOGLE_CLIENT_ID", label: "Google Client ID", placeholder: "xxxxxxxx.apps.googleusercontent.com", secret: false, hint: "Shared by every Google plugin (Gmail, Google Calendar, Drive, Docs, Sheets, Slides) — saving it here updates all of them. Create it in Google Cloud Console → APIs & Services → Credentials → OAuth client ID, type Desktop app.", type: "string", required: true, options: [], oauth: true, shared: "google", set: false, value: "" },
    { key: "GOOGLE_CLIENT_SECRET", label: "Google Client Secret", placeholder: "GOCSPX-…", secret: true, hint: "The Desktop app client's secret (Google issues one; it's sent on the token request). Shared by every Google plugin.", type: "string", required: true, options: [], oauth: true, shared: "google", set: false, value: "" },
  ]);
  const [mcp, google] = d.json.mcp;

  assert.deepEqual(google, { name: "orbit-google", transport: "command", id: null, status: "not-added", tools: 0, err: null, accounts: [], unavailable: null, needs: ["Google Client ID", "Google Client Secret"], callbackUrl: null });
  assert.equal(mcp.name, "orbit-mcp");
  // the first answer comes before the probe it started
  const connected = await waitFor(async () => {
    const m = (await get("/plugins/detail?name=orbit")).json.mcp[0];

    return m.status === "connected" && m;
  });

  assert.deepEqual(connected, {
    name: "orbit-mcp", transport: "command", id: "orbit-mcp", status: "connected", tools: 2, err: null,
    accounts: [{ id: "orbit-mcp", label: "default", status: "connected", tools: 2, err: null, authed: false, note: "" }],
    unavailable: null, needs: [], callbackUrl: null,
  });
});

test("MKT-05 setup values: Google keys go to the shared @google row, the rest to the plugin's; connectors re-applied, a waiting one added", async () => {
  const r = await postJson("/plugins/settings", { plugin: "orbit", values: { ORBIT_API_KEY: " k1 ", ORBIT_ADDRESS: "db:7233", GOOGLE_CLIENT_ID: "cid", GOOGLE_CLIENT_SECRET: "sec", NOT_A_FIELD: "x" } });

  assert.equal(r.text, '{"ok":true}');
  const vals = () => Object.fromEntries(rows("SELECT plugin, vals FROM plugin_settings ORDER BY plugin").map((s) => [s.plugin, JSON.parse(s.vals)]));

  assert.deepEqual(vals(), { "@google": { GOOGLE_CLIENT_ID: "cid", GOOGLE_CLIENT_SECRET: "sec" }, orbit: { ORBIT_API_KEY: "k1", ORBIT_ADDRESS: "db:7233" } });
  assert.deepEqual(rows("SELECT id, env, enabled FROM mcp_servers ORDER BY id"), [
    { id: "orbit-google", env: JSON.stringify({ GOOGLE_CLIENT_ID: "cid", GOOGLE_CLIENT_SECRET: "sec" }), enabled: 1 },
    { id: "orbit-mcp", env: JSON.stringify({ API_KEY: "k1", ADDRESS: "db:7233", ROOT: join(PLUGINS, "orbit") }), enabled: 1 },
  ]);
  orbitMeta = { ...orbitMeta, mcp: ["orbit-mcp", "orbit-google"], created: ["orbit-mcp", "orbit-google"] };
  assert.equal(rows("SELECT meta FROM installed_plugins")[0].meta, JSON.stringify(orbitMeta));
  // a blank secret keeps the stored one, a blank plain value clears; clear drops
  await postJson("/plugins/settings", { plugin: "orbit", values: { ORBIT_API_KEY: "", ORBIT_ADDRESS: " " } });
  assert.deepEqual(vals().orbit, { ORBIT_API_KEY: "k1" });
  await postJson("/plugins/settings", { plugin: "orbit", values: {}, clear: ["ORBIT_API_KEY"] });
  assert.deepEqual(vals(), { "@google": { GOOGLE_CLIENT_ID: "cid", GOOGLE_CLIENT_SECRET: "sec" } });
  assert.equal(JSON.parse(rows("SELECT env FROM mcp_servers WHERE id='orbit-mcp'")[0].env).API_KEY, undefined);
  await postJson("/plugins/settings", { plugin: "orbit", values: { ORBIT_API_KEY: "k2" } });
  const d = (await get("/plugins/detail?name=orbit")).json;

  assert.deepEqual(d.fields.map((f) => [f.key, f.set, f.value]), [["ORBIT_API_KEY", true, ""], ["ORBIT_ADDRESS", false, ""], ["GOOGLE_CLIENT_ID", true, "cid"], ["GOOGLE_CLIENT_SECRET", true, ""]]);
  const bad = await postJson("/plugins/settings", { plugin: "nope", values: {} });

  assert.equal(bad.status, 400);
  assert.equal(bad.text, '{"ok":false,"error":"unknown plugin nope"}');
});

test("MKT-06 connect: a labelled account is a new row under the connector's name, probed at once; reserved and taken labels refused", async () => {
  const r = await postJson("/plugins/connect", { plugin: "orbit", server: "orbit-mcp", label: "Work" });

  assert.equal(r.text, JSON.stringify({ ok: true, status: "connected", err: null, token: false, id: "orbit-mcp-work" }));
  const [row] = rows("SELECT id, name, account, enabled, agent_pending, env FROM mcp_servers WHERE id='orbit-mcp-work'");

  assert.deepEqual(row, { id: "orbit-mcp-work", name: "orbit-mcp", account: "Work", enabled: 1, agent_pending: null, env: JSON.stringify({ API_KEY: "k2", ADDRESS: "localhost:7233", ROOT: join(PLUGINS, "orbit") }) });
  orbitMeta = { ...orbitMeta, mcp: [...orbitMeta.mcp, "orbit-mcp-work"], created: [...orbitMeta.created, "orbit-mcp-work"] };
  assert.equal(rows("SELECT meta FROM installed_plugins")[0].meta, JSON.stringify(orbitMeta));
  const accounts = (await get("/plugins/detail?name=orbit")).json.mcp[0].accounts;

  assert.deepEqual(accounts.map((a) => [a.id, a.label]), [["orbit-mcp", "default"], ["orbit-mcp-work", "Work"]]);
  for (const [body, error] of [
    [{ plugin: "orbit", server: "orbit-mcp", label: "work" }, "an account labeled work already exists"],
    [{ plugin: "orbit", server: "orbit-mcp", label: "Default" }, "Default is a reserved account label"],
    [{ plugin: "orbit", server: "nope" }, "orbit declares no server nope"],
    [{ plugin: "lumen", server: "x" }, "plugin lumen is not installed"],
  ]) {
    const bad = await postJson("/plugins/connect", body);

    assert.equal(bad.status, 400);
    assert.equal(bad.text, JSON.stringify({ ok: false, error }));
  }
  // the default account again: the existing row, switched on
  const again = await postJson("/plugins/connect", { plugin: "orbit", server: "orbit-mcp" });

  assert.equal(again.json.id, "orbit-mcp");
  assert.equal(again.json.status, "connected");
});

test("MKT-07 a skill's SKILL.md: an installed plugin's file, else the catalog's description; 404 for neither", async () => {
  const r = await get("/plugins/skill?plugin=orbit&skill=orbit-ops");

  assert.equal(r.text, JSON.stringify({ name: "orbit-ops", body: skillMd("orbit-ops", "The orbit-ops skill."), description: "The orbit-ops skill." }));
  const c = await get("/plugins/skill?plugin=sketch-docs&skill=sketch-docs");

  assert.deepEqual(c.json, { name: "sketch-docs", description: entry("sketch-docs").skills[0].description, body: "", source: entry("sketch-docs").skills[0].sourceUrl });
  const none = await get("/plugins/skill?plugin=orbit&skill=nope");

  assert.equal(none.status, 404);
  assert.equal(none.text, '{"error":"no such skill"}');
});

test("MKT-08 no plugin logos: /plugins/icon/<key> is gone (404), and no row or page names one", async () => {
  assert.equal((await get("/plugins/icon/orbit")).status, 404);
  const c = await get("/plugins/catalog");

  assert.ok(c.json.plugins.every((p) => !("icon" in p) && !("iconUrl" in p)));
  const d = await get("/plugins/detail?name=orbit");

  assert.ok(!("icon" in d.json) && !("iconUrl" in d.json));
});

test("MKT-09 an install that resolves the plugin through the repo's marketplace.json; a connector row it didn't create stays off, and stays", async () => {
  // a row named like the plugin's connector, made before and switched off
  const db = new DatabaseSync(join(dir, "generated", "app.db"), { timeout: 5000 });

  db.prepare("INSERT INTO mcp_servers(id,name,command,args,env,enabled,created_at) VALUES('my-canvas','canvas','old','[]','{}',0,1)").run();
  db.close();
  const r = await postJson("/plugins/install", { name: "sketch-docs" });

  assert.equal(r.text, JSON.stringify({ ok: true, name: "sketch-docs", mcp: ["my-canvas"], kinds: ["skills"], waiting: [] }));
  assert.deepEqual(files(join(PLUGINS, "sketch-docs")), ["plugins/sketch-docs/.cursor-plugin/plugin.json", "plugins/sketch-docs/.mcp.json", "plugins/sketch-docs/skills/sketch-docs/SKILL.md"]);
  const meta = { root: "sketch-docs/plugins/sketch-docs", skills: [{ name: "sketch-docs", file: "sketch-docs/plugins/sketch-docs/skills/sketch-docs/SKILL.md" }], repo: "https://github.com/example/plugins", sha: entry("sketch-docs").install.sha, catalogId: entry("sketch-docs").id, mcp: ["my-canvas"], created: [], kinds: ["skills"] };

  assert.equal(rows("SELECT meta FROM installed_plugins WHERE name='sketch-docs'")[0].meta, JSON.stringify(meta));
  assert.deepEqual(rows("SELECT id, name, command, args, enabled, created_at FROM mcp_servers WHERE name='canvas'"), [{ id: "my-canvas", name: "canvas", command: "node", args: JSON.stringify([FIXTURE]), enabled: 0, created_at: 1 }]);
  const d = (await get("/plugins/detail?name=sketch-docs")).json;

  assert.deepEqual(d.mcp[0].accounts, [{ id: "my-canvas", label: "default", status: "off", tools: 0, err: null, authed: false, note: "" }]);
  assert.equal(d.mcp[0].status, "off");
  assert.deepEqual(d.manifest, { author: "Sketch Co", version: "0.1.0", license: null });
  assert.deepEqual(d.skills, [{ name: "sketch-docs", description: "Render docs." }]);
  // uninstall: the record and files go, the row it found stays
  assert.equal((await postJson("/plugins/uninstall", { name: "sketch-docs" })).text, '{"ok":true}');
  assert.ok(!existsSync(join(PLUGINS, "sketch-docs")));
  assert.deepEqual(rows("SELECT id, enabled FROM mcp_servers WHERE name='canvas'"), [{ id: "my-canvas", enabled: 0 }]);
});

test("MKT-10 uninstall: the rows its install and connects created, the record, its own setup values and its files go; @google stays", async () => {
  const r = await postJson("/plugins/uninstall", { name: "orbit" });

  assert.equal(r.text, '{"ok":true}');
  assert.deepEqual(rows("SELECT id FROM mcp_servers ORDER BY id"), [{ id: "my-canvas" }]);
  assert.deepEqual(rows("SELECT name FROM installed_plugins"), []);
  assert.deepEqual(rows("SELECT plugin FROM plugin_settings"), [{ plugin: "@google" }]);
  assert.ok(!existsSync(join(PLUGINS, "orbit")));
  assert.ok(existsSync(join(PLUGINS, ".repos")), "the repo cache stays");
  // its clients were dropped: no server of its left in the status list
  const servers = (await get("/mcp/servers")).json.servers.map((s) => s.id);

  assert.deepEqual(servers, ["my-canvas"]);
  // uninstalling what isn't installed is still ok
  assert.equal((await postJson("/plugins/uninstall", { name: "nope" })).text, '{"ok":true}');
  assert.equal((await postJson("/plugins/install", { name: "nope" })).text, '{"ok":false,"error":"unknown plugin nope"}');
});

test("MKT-11 a legacy plugin: cloned whole at plugins/<name>, its record keeps the source path", async () => {
  const r = await postJson("/plugins/install", { name: "lumen" });

  assert.equal(r.text, JSON.stringify({ ok: true, name: "lumen", mcp: [], kinds: ["skills"], waiting: [] }));
  assert.deepEqual(files(join(PLUGINS, "lumen")), [".claude-plugin/plugin.json", "skills/apl/SKILL.md"]);
  assert.ok(existsSync(join(PLUGINS, "lumen", ".git")));
  assert.equal(rows("SELECT meta FROM installed_plugins WHERE name='lumen'")[0].meta, JSON.stringify({ path: "", mcp: [], created: [], kinds: ["skills"] }));
  const s = await get("/plugins/skill?plugin=lumen&skill=apl");

  assert.equal(s.json.description, "Query with APL.");
  await postJson("/plugins/uninstall", { name: "lumen" });
  assert.ok(!existsSync(join(PLUGINS, "lumen")));
});

test("MKT-11b an installed plugin no catalog lists: listed from its record, its page renders, settings save, uninstall cleans up", async () => {
  // installed from a catalog Buwiz no longer ships: the record, its files
  // and the connector row it made
  const root = join(PLUGINS, "gone", "plug");

  mkdirSync(join(root, ".claude-plugin"), { recursive: true });
  writeFileSync(join(root, ".claude-plugin", "plugin.json"), JSON.stringify({ name: "gone", displayName: "Gone Plugin", description: "From before.", author: { name: "Gone Co" } }));
  mkdirSync(join(root, "skills", "tidy"), { recursive: true });
  writeFileSync(join(root, "skills", "tidy", "SKILL.md"), skillMd("tidy", "Tidies."));
  const db = new DatabaseSync(join(dir, "generated", "app.db"), { timeout: 5000 });

  db.prepare("INSERT INTO installed_plugins(name, meta, created_at) VALUES(?,?,?)").run("gone", JSON.stringify({ root: "gone/plug", catalogId: "9", mcp: ["gone-web"], created: ["gone-web"] }), 1);
  db.prepare("INSERT INTO mcp_servers(id,name,command,args,env,enabled,created_at,url) VALUES('gone-web','gone-web','','[]','{}',0,1,'https://mcp.gone.example/mcp')").run();
  db.close();
  const row = catalogRow(await get("/plugins/catalog"), "gone");

  assert.deepEqual([row.displayName, row.description, row.publisher, row.installed, row.cats], ["Gone Plugin", "From before.", "Gone Co", true, []]);
  const d = (await get("/plugins/detail?name=gone")).json;

  assert.deepEqual([d.displayName, d.installed, d.skills], ["Gone Plugin", true, [{ name: "tidy", description: "Tidies." }]]);
  assert.deepEqual(d.mcp.map((m) => [m.name, m.transport, m.id, m.status, m.unavailable, m.accounts.map((a) => a.id)]), [["gone-web", "remote", "gone-web", "off", null, ["gone-web"]]]);
  assert.equal((await postJson("/plugins/settings", { plugin: "gone", values: {} })).text, '{"ok":true}');
  assert.equal((await get("/plugins/skill?plugin=gone&skill=tidy")).json.description, "Tidies.");
  assert.equal((await postJson("/plugins/uninstall", { name: "gone" })).text, '{"ok":true}');
  assert.deepEqual(rows("SELECT name FROM installed_plugins WHERE name='gone'"), []);
  assert.deepEqual(rows("SELECT id FROM mcp_servers WHERE id='gone-web'"), []);
  assert.ok(!existsSync(join(PLUGINS, "gone")));
  assert.equal(catalogRow(await get("/plugins/catalog"), "gone"), undefined);
});

test("MKT-12 POST /skills creates by slug and edits in place; /skills/state and /skills/delete; GET /skills reads them back", async () => {
  const mk = await postJson("/skills", { name: "Weekly Report!", description: "d1", instructions: "Do it." });

  assert.equal(mk.text, '{"ok":true,"id":"weekly-report"}');
  assert.equal((await postJson("/skills", { name: "x" })).text, '{"ok":false,"error":"skill needs instructions"}');
  assert.equal((await postJson("/skills", { name: "x" })).status, 400);
  // an edit by id keeps what it doesn't name, and the enabled flag and age
  assert.equal((await postJson("/skills/state", { id: "weekly-report", enabled: false })).text, '{"ok":true}');
  const [before] = rows("SELECT * FROM skills");

  assert.equal((await postJson("/skills", { id: "weekly-report", description: "d2" })).text, '{"ok":true,"id":"weekly-report"}');
  assert.deepEqual(rows("SELECT * FROM skills"), [{ ...before, description: "d2" }]);
  assert.deepEqual([before.name, before.body, before.enabled], ["Weekly Report!", "Do it.", 0]);
  // BUG: an empty name is kept as the name ("" is not nullish); the id
  // falls back to "skill"
  await postJson("/skills", { name: "", body: "b" });
  await postJson("/skills", { name: "n".repeat(100), description: "e".repeat(500), body: "x" });
  const list = (await get("/skills")).json.skills;

  assert.deepEqual(list.map((s) => [s.id, s.name.length, s.description.length, s.enabled]), [["weekly-report", 14, 2, false], ["skill", 0, 0, true], ["n".repeat(48), 80, 400, true]]);
  assert.equal((await postJson("/skills/state", { id: "skill", enabled: 1 })).text, '{"ok":true}');
  assert.equal((await postJson("/skills/state", { id: "nope", enabled: true })).text, '{"ok":true}');
  const broken = await request("POST", "/skills/state", { headers: { "Content-Type": "application/json" }, body: "{" });

  assert.deepEqual([broken.status, broken.text], [400, '{"ok":false}']);
  // (BUG: a /skills/delete body that isn't JSON throws outside any try and
  // takes Node down: not sent here; Rust answers it, D8)
  assert.equal((await postJson("/skills/delete", { id: "skill" })).text, '{"ok":true}');
  assert.deepEqual(rows("SELECT id FROM skills ORDER BY created_at, id").map((s) => s.id), ["weekly-report", "n".repeat(48)]);
});

test("MKT-13 the six skill tools (module level): create, list, update, disable, enable, delete, and what each answers", () => {
  const out = JSON.parse(nodeIn(tempDir("market-tools"), `
    const { toolSkillAdmin, privateSkillsInject } = await import("./src/tools.mjs");
    const { db } = await import("./src/state.mjs");
    const r = [];
    const t = (n, i) => r.push(toolSkillAdmin(n, i));
    t("skill_list", {});
    t("skill_create", { name: "Tone Guide", description: "How to write", instructions: "Be brief." });
    t("skill_create", { name: "tone guide", instructions: "Be briefer." });
    t("skill_create", { body: "no name" });
    t("skill_update", { name: "Tone Guide", description: "new" });
    t("skill_update", { id: "nope" });
    t("skill_update", {});
    t("skill_disable", { id: "tone-guide" });
    t("skill_list", {});
    t("skill_enable", { name: "tone-guide" });
    r.push(privateSkillsInject());
    t("skill_delete", { name: "skill" });
    t("skill_delete", { id: "tone-guide" });
    r.push(privateSkillsInject());
    r.push(db.prepare("SELECT id, name, description, body, enabled FROM skills").all().map((x) => ({ ...x })));
    console.log(JSON.stringify(r));
  `));

  assert.deepEqual(out, [
    { content: "no private skills yet" },
    { content: 'created private skill "tone-guide" — its instructions apply to every bot\'s next step' },
    { content: 'updated private skill "tone-guide" — its instructions apply to every bot\'s next step' },
    { content: 'created private skill "skill" — its instructions apply to every bot\'s next step' },
    { content: 'skill "tone-guide" updated — applies from the next step' },
    { content: "no private skill matches nope — skill_list shows the ids", is_error: true },
    { content: "no private skill matches ? — skill_list shows the ids", is_error: true },
    { content: 'skill "tone-guide" disabled — instructions withheld' },
    { content: "tone-guide — Tone Guide (disabled) · new\nskill — skill · " },
    { content: 'skill "tone-guide" enabled — instructions apply' },
    "\nPrivate skills (active — follow when applicable):\n### Tone Guide\nnew\nBe briefer.\n\n### skill\n\nno name",
    { content: 'deleted private skill "skill"' },
    { content: 'deleted private skill "tone-guide"' },
    "",
    [],
  ]);
});

test("MKT-14 installed plugin skills as the agent's index (module level)", () => {
  const d = tempDir("market-inject");
  const root = join(d, "plugins", "p1");

  mkdirSync(join(root, "skills", "s1"), { recursive: true });
  writeFileSync(join(root, "skills", "s1", "SKILL.md"), skillMd("s1", "First skill."));
  mkdirSync(join(root, "skills", "s2"), { recursive: true });
  const out = JSON.parse(nodeIn(d, `
    const { pluginSkillsInject } = await import("./src/plugins.mjs");
    const { db } = await import("./src/state.mjs");
    const a = pluginSkillsInject();
    db.prepare("INSERT INTO installed_plugins(name, meta, created_at) VALUES(?,?,?)").run("p1", JSON.stringify({ root: "p1" }), 1);
    db.prepare("INSERT INTO installed_plugins(name, meta, created_at) VALUES(?,?,?)").run("gone", JSON.stringify({ root: "gone" }), 2);
    console.log(JSON.stringify([a, pluginSkillsInject()]));
  `));

  assert.deepEqual(out, ["", `\nPlugin skills (installed): when a request matches a skill's description, read its SKILL.md with shell_run (cat the file) and follow it; files it references sit beside it.\n- p1/s1: First skill. (file: ${join(root, "skills", "s1", "SKILL.md")})`]);
});

test("MKT-15 the bundled catalog lists the three vendored plugins; no catalog file is an empty catalog (module level)", () => {
  const read = (env) => JSON.parse(nodeIn(tempDir("market-catalog"), `
    const { catalogEntries, entryFor, pluginRoot } = await import("./src/plugins.mjs");
    const { db } = await import("./src/state.mjs");
    db.prepare("INSERT INTO installed_plugins(name, meta, created_at) VALUES(?,?,?)").run("kept", JSON.stringify({ root: "kept" }), 1);
    console.log(JSON.stringify({
      rows: catalogEntries().map((e) => [e.key, e.displayName, e.publisher.displayName, e.cats, !!pluginRoot(e.key) || null]),
      kept: entryFor("kept") && entryFor("kept").displayName,
    }));
  `, env));
  const bundled = read({});

  assert.deepEqual(bundled.rows.map((r) => r.slice(0, 4)), [
    ["calcom", "Cal.com", "Cal.com", ["communication"]],
    ["neon", "Neon", "Neon", ["data"]],
    ["google-workspace", "Google Workspace", "taylorwilsdon", ["productivity"]],
  ]);
  for (const name of ["calcom", "neon", "google-workspace"]) assert.ok(existsSync(join(ROOT, "src", "plugins", "external_plugins", name, ".mcp.json")), name);
  const none = read({ BUWIZ_PLUGIN_CATALOG: join(dir, "no-such-catalog.json") });

  assert.deepEqual(none, { rows: [], kept: "Kept" });
});
