// The MCP routes that move with the chat routes (Rust broker plan, phase 5
// item 5.2): POST /mcp/servers (add and edit, agent_pending kept until the
// user enables), POST /mcp/attach and GET /mcp/attachments, as Node answers
// them. MOCK on; MCP servers are the stdio fixtures.
import assert from "node:assert/strict";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { FIXTURE, ROOT, dbRows, nodeIn, offlineEnv, postJson, request, startServer, tempDir, waitFor } from "./helpers.mjs";

const dir = tempDir("mcp-chat");
const APPS = join(ROOT, "test", "fixtures", "mcp-apps-server.mjs");
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

let server;
let pending;

before(async () => {
  // a server the agent installed (mcp_install): off, waiting for the user
  pending = JSON.parse(nodeIn(dir, `
    const { mcpUpsertServer } = await import("./src/tools.mjs");
    console.log(JSON.stringify(await mcpUpsertServer({ name: "Agent Pick", command: process.execPath, args: [${JSON.stringify(FIXTURE)}], byAgent: true })));
  `));
  server = await startServer(dir, { env: offlineEnv(join(dir, "stubs.log")) });
});

after(async () => {
  await server?.stop();
});

const listed = async (id) => (await request("GET", "/mcp/servers")).json.servers.find((s) => s.id === id);
const row = (id) => dbRows(dir, "SELECT * FROM mcp_servers WHERE id=?", id)[0];
const bad = (r, error) => {
  assert.equal(r.status, 400, r.text);
  assert.deepEqual(r.json, { ok: false, error });
};

test("MCHAT-01 POST /mcp/servers refuses a server without a name, a command or url, or with a bad new id", async () => {
  bad(await postJson("/mcp/servers", {}), "server needs a name");
  bad(await postJson("/mcp/servers", { name: "  " , command: "x" }), "server needs a name");
  bad(await postJson("/mcp/servers", { name: "x" }), "server needs a command or url");
  bad(await postJson("/mcp/servers", { id: "Bad_Id", name: "x", command: "y" }), "server id Bad_Id must be [a-z0-9-]+");
});

test("MCHAT-02 a server added from the UI is probed at once; its id comes from its name, made unique", async () => {
  const r = await postJson("/mcp/servers", { name: "My Fixture!", command: process.execPath, args: [FIXTURE], env: "A=1\nB=two\nnot a pair" });

  assert.equal(r.status, 200, r.text);
  assert.deepEqual(r.json, { ok: true, id: "my-fixture", status: "connected", tools: 2, toolNames: ["mcp_my-fixture_echo", "mcp_my-fixture_secret"], err: null });
  const again = (await postJson("/mcp/servers", { name: "My Fixture!", command: process.execPath, args: `${FIXTURE} ` })).json;

  assert.equal(again.id, "my-fixture-2");
  const r1 = row("my-fixture");

  assert.deepEqual({ name: r1.name, command: r1.command, args: JSON.parse(r1.args), env: JSON.parse(r1.env), enabled: r1.enabled, bot: r1.bot, url: r1.url, agent_pending: r1.agent_pending },
    { name: "My Fixture!", command: process.execPath, args: [FIXTURE], env: { A: "1", B: "two" }, enabled: 1, bot: null, url: null, agent_pending: null });
  assert.deepEqual(JSON.parse(row("my-fixture-2").args), [FIXTURE]);
  assert.deepEqual(await listed("my-fixture"), {
    id: "my-fixture", name: "My Fixture!", command: process.execPath, url: null, args: [FIXTURE], envKeys: ["A", "B"], enabled: true, bot: null, status: "connected", tools: 2, err: null,
  });
});

test("MCHAT-03 an edit keeps what it doesn't name: command, args, env, created_at, the switch and per-tool switches", async () => {
  const before = row("my-fixture");

  assert.deepEqual((await postJson("/mcp/tools/state", { id: "my-fixture", tool: "secret", enabled: false })).json, { ok: true });
  const r = await postJson("/mcp/servers", { id: "my-fixture", name: "Renamed" });

  assert.equal(r.json.status, "connected");
  const after = row("my-fixture");

  assert.deepEqual({ ...after, name: before.name }, { ...before, disabled_tools: '["secret"]' });
  assert.equal(after.name, "Renamed");
  // a bot the store doesn't know is no scope
  await postJson("/mcp/servers", { id: "my-fixture", bot: "nope" });
  assert.equal(row("my-fixture").bot, null);
  const bot = (await postJson("/bot/new", { name: "Ada" })).json.bot;

  await postJson("/mcp/servers", { id: "my-fixture", bot: bot.id });
  assert.equal(row("my-fixture").bot, bot.id);
  assert.equal((await listed("my-fixture")).bot, bot.id);
});

test("MCHAT-04 a switched-off server answers off on an edit and is not started", async () => {
  assert.deepEqual((await postJson("/mcp/server/state", { id: "my-fixture-2", enabled: false })).json, { ok: true });
  assert.deepEqual((await postJson("/mcp/servers", { id: "my-fixture-2", args: [FIXTURE, "--x"] })).json, { ok: true, id: "my-fixture-2", status: "off", tools: 0, toolNames: [], err: null });
  assert.equal((await listed("my-fixture-2")).status, "off");
  bad(await postJson("/mcp/server/state", { id: "nope", enabled: true }), "no such server");
});

test("MCHAT-05 agent_pending: the agent's server stays off through an edit; the user's enable clears it and connects", async () => {
  assert.deepEqual(pending, { id: "agent-pick", status: "off", tools: 0, toolNames: [], err: null });
  assert.deepEqual([row("agent-pick").enabled, row("agent-pick").agent_pending], [0, 1]);
  assert.deepEqual((await postJson("/mcp/servers", { id: "agent-pick", name: "Agent Pick 2" })).json, { ok: true, id: "agent-pick", status: "off", tools: 0, toolNames: [], err: null });
  assert.deepEqual([row("agent-pick").enabled, row("agent-pick").agent_pending], [0, 1]);
  assert.deepEqual(await listed("agent-pick"), {
    id: "agent-pick", name: "Agent Pick 2", command: process.execPath, url: null, args: [FIXTURE], envKeys: [], enabled: false, bot: null, status: "off", tools: 0, err: null,
  });
  assert.deepEqual((await postJson("/mcp/server/state", { id: "agent-pick", enabled: true })).json, { ok: true });
  assert.deepEqual([row("agent-pick").enabled, row("agent-pick").agent_pending], [1, null]);
  assert.ok(await waitFor(async () => (await listed("agent-pick")).status === "connected"), "connected after the enable");
});

test("MCHAT-06 POST /mcp/attach resolves a token, stores it and posts its card; the same token again is the one there", async () => {
  assert.equal((await postJson("/mcp/servers", { id: "apps", name: "Apps", command: process.execPath, args: [APPS] })).json.status, "connected");
  const thread = (await postJson("/thread/new", {})).json.id;
  const r = await postJson("/mcp/attach", { server: "apps", token: " @apps:item:42 ", thread });

  assert.equal(r.status, 200, r.text);
  const a = r.json.attachment;

  assert.match(a.id, UUID);
  assert.match(a.turn, UUID);
  assert.deepEqual(r.json, {
    ok: true,
    attachment: {
      id: a.id, server: "apps", token: "@apps:item:42", scope: "item", title: "Item 42", resolved_title: "Item 42", open: "buwiz://app/apps/hello",
      key: "mcp:apps/hello", details: { got: { name: "item", args: "42", complete: false } }, thread, bot: null, turn: a.turn, created_at: a.created_at,
    },
    existing: false,
  });
  const card = (await request("GET", `/turns?thread=${thread}`)).json.turns.at(-1);

  assert.equal(card.id, a.turn);
  assert.deepEqual([card.role, card.text, card.label], ["ai", "", ""]);
  assert.deepEqual(card.links, [{
    provider: "mcp-app", id: "mcp:apps/hello", name: "Item 42", link: "buwiz://app/apps/hello", action: "attached", kind: "app", file_type: "An item",
    token: "@apps:item:42", attachment: a.id, at: a.created_at, type: "An item",
  }]);
  const again = (await postJson("/mcp/attach", { token: "@apps:item:42", thread })).json;

  assert.deepEqual(again, { ok: true, attachment: a, existing: true });
  // a second token: the chat's attachments ride the resolve as context
  const b = (await postJson("/mcp/attach", { token: "@apps:item:7", thread })).json.attachment;

  assert.deepEqual(b.details, { got: { name: "item", args: "7", complete: false, context: { attached: ["@apps:item:42"] } } });
  // a rename names every card and attachment of the token
  assert.deepEqual((await postJson("/mcp/attachment/rename", { token: "@apps:item:42", title: "  The answer " })).json, { ok: true });
  assert.equal((await request("GET", `/turns?thread=${thread}`)).json.turns.find((t) => t.id === a.turn).links[0].name, "The answer");
  assert.equal((await request("GET", `/mcp/attachments?token=${encodeURIComponent("@apps:item:42")}`)).json.attachments[0].title, "The answer");
});

test("MCHAT-07 a bot's attachment lands in its chat; refusals", async () => {
  const bot = (await postJson("/bot/new", { name: "Bea" })).json.bot;
  const r = (await postJson("/mcp/attach", { token: "@apps:item:9", bot: bot.id })).json;

  assert.equal(r.attachment.thread, bot.threadId);
  assert.equal(r.attachment.bot, bot.id);
  bad(await postJson("/mcp/attach", { token: "@apps:item:1", bot: "nope" }), "unknown bot nope");
  bad(await postJson("/mcp/attach", { token: "@apps:item:1", thread: "t9999" }), "unknown chat t9999");
  bad(await postJson("/mcp/attach", { token: "@apps:item:1" }), "unknown chat ");
  bad(await postJson("/mcp/attach", { token: "apps:item", thread: bot.threadId }), "not a token: apps:item");
  bad(await postJson("/mcp/attach", { token: "@nobody:item:1", thread: bot.threadId }), "no such server nobody");
  bad(await postJson("/mcp/attach", { token: "@apps:go:1", thread: bot.threadId }), "@apps:go tokens can't be attached");
  await postJson("/thread/delete", { id: bot.threadId });
  bad(await postJson("/mcp/attach", { token: "@apps:item:1", bot: bot.id }), "Bea has no chat");
});

test("MCHAT-08 GET /mcp/attachments filters by thread, bot and token, oldest first, with where each is", async () => {
  const all = (await request("GET", "/mcp/attachments")).json.attachments;

  assert.deepEqual(all.map((a) => [a.token, a.botName]), [["@apps:item:42", ""], ["@apps:item:7", ""], ["@apps:item:9", "Bea"]]);
  assert.deepEqual(Object.keys(all[0]), ["id", "server", "token", "scope", "title", "resolved_title", "open", "key", "details", "thread", "bot", "turn", "created_at", "threadTitle", "botName"]);
  assert.equal(all[0].threadTitle, "new chat");
  // the bot's chat is gone: its attachment stays, with no chat title
  assert.equal(all[2].threadTitle, "");
  assert.deepEqual((await request("GET", `/mcp/attachments?thread=${all[0].thread}`)).json.attachments.map((a) => a.token), ["@apps:item:42", "@apps:item:7"]);
  assert.deepEqual((await request("GET", `/mcp/attachments?bot=${all[2].bot}`)).json.attachments.map((a) => a.token), ["@apps:item:9"]);
  assert.deepEqual((await request("GET", `/mcp/attachments?thread=${all[0].thread}&token=${encodeURIComponent("@apps:item:7")}`)).json.attachments.map((a) => a.token), ["@apps:item:7"]);
  assert.deepEqual((await request("GET", "/mcp/attachments?thread=t9999")).json, { attachments: [] });
});
