// The sidebar's groups (Rust broker plan, phase 5 item 5.2): /groups and
// /group/new|update|delete|item as Node answers them, and the one-time
// migration of bot sections and pinned Mini Apps into groups. MOCK on.
import assert from "node:assert/strict";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { dbRows, nodeIn, offlineEnv, postJson, request, startServer, tempDir, waitFor } from "./helpers.mjs";

const dir = tempDir("groups");
const COLORS = ["#1884fc", "#32bb78", "#f2994a", "#eb5757", "#9b51e0", "#2d9cdb", "#f2c94c", "#8e8e93"];

let server;

before(async () => {
  server = await startServer(dir, { env: offlineEnv(join(dir, "stubs.log")) });
});

after(async () => {
  await server?.stop();
});

const groups = async () => (await request("GET", "/groups")).json;
const group = async (id) => (await groups()).groups.find((g) => g.id === id);
const bad = (r, error) => {
  assert.equal(r.status, 400, r.text);
  assert.deepEqual(r.json, { ok: false, error });
};
const make = async (body) => {
  const r = await postJson("/group/new", body);

  assert.equal(r.status, 200, r.text);

  return r.json.id;
};

test("GRP-01 no groups: the list is empty and names the colors", async () => {
  assert.deepEqual(await groups(), { groups: [], colors: COLORS });
});

test("GRP-02 /group/new: a trimmed, clipped name; the next color unless a valid one is given", async () => {
  const r = await postJson("/group/new", { name: "  Team  " });

  assert.deepEqual(Object.keys(r.json), ["ok", "id"]);
  assert.match(r.json.id, /^g[0-9a-z]+$/);
  const b = await make({ name: "n".repeat(70), color: "#AABBCC" });
  const c = await make({ name: "Third", color: "red" });
  const { groups: gs } = await groups();

  assert.deepEqual(gs.map(({ name, color, order, collapsed, items }) => ({ name, color, order, collapsed, items })), [
    { name: "Team", color: COLORS[0], order: 0, collapsed: false, items: [] },
    { name: "n".repeat(60), color: "#aabbcc", order: 1, collapsed: false, items: [] },
    { name: "Third", color: COLORS[2], order: 2, collapsed: false, items: [] },
  ]);
  assert.deepEqual(gs.map((g) => g.id), [r.json.id, b, c]);
  bad(await postJson("/group/new", { name: "  " }), "a group needs a name");
  bad(await postJson("/group/new", {}), "a group needs a name");
  bad(await postJson("/group/new", { name: "x", item: { kind: "bot", ref: "nope" } }), "unknown bot nope");
  assert.equal((await groups()).groups.length, 3, "a refused item makes no group");

  for (const g of gs) await postJson("/group/delete", { id: g.id });
});

test("GRP-03 members resolve: a bot, a chat (hidden or archived reads hidden), an artifact's record, an app's title", async () => {
  const bot = (await postJson("/bot/new", { name: "Ada" })).json.bot;
  const chat = (await postJson("/thread/new", {})).json.id;

  const job = (await postJson("/chat", { prompt: "make a counter", thread: chat })).json.id;

  await waitFor(async () => (await request("GET", `/status?id=${job}`)).json.done, 30000);
  // (renamed after its first message, which names an empty chat)
  await postJson("/thread/update", { id: chat, title: "Plans", archived: true });
  const card = (await request("GET", `/turns?thread=${chat}`)).json.turns[1].name;
  const id = await make({ name: "Mixed", item: { kind: "bot", ref: bot.id } });

  assert.deepEqual((await postJson("/group/item", { id, op: "add", kind: "chat", ref: chat })).json, { ok: true });
  await postJson("/group/item", { id, op: "add", kind: "artifact", ref: `${card}@v1`, thread: chat });
  await postJson("/group/item", { id, op: "add", kind: "app", ref: "mcp:apps/hello|{\"a\":1}", title: "  Hello app  " });
  await postJson("/group/item", { id, op: "add", kind: "app", ref: "mcp:apps/plain" });
  const g = await group(id);

  assert.deepEqual(g.items.map(({ artifact: _a, ...it }) => it), [
    { kind: "bot", ref: bot.id, title: "Ada", hidden: false },
    { kind: "chat", ref: chat, title: "Plans", hidden: true },
    { kind: "artifact", ref: card, thread: chat, title: g.items[2].artifact.title },
    { kind: "app", ref: "mcp:apps/hello|{\"a\":1}", title: "Hello app" },
    { kind: "app", ref: "mcp:apps/plain", title: "apps/plain" },
  ]);
  assert.equal(g.items[2].artifact.id, card);
  assert.deepEqual(dbRows(dir, "SELECT kind, ref, thread, ord, title FROM group_items WHERE group_id=? ORDER BY ord", id), [
    { kind: "bot", ref: bot.id, thread: null, ord: 0, title: null },
    { kind: "chat", ref: chat, thread: null, ord: 1, title: null },
    { kind: "artifact", ref: card, thread: chat, ord: 2, title: null },
    { kind: "app", ref: "mcp:apps/hello|{\"a\":1}", thread: null, ord: 3, title: "Hello app" },
    { kind: "app", ref: "mcp:apps/plain", thread: null, ord: 4, title: null },
  ]);
});

test("GRP-04 /group/item moves an item already there to its index, removes one, and refuses what it can't add", async () => {
  const bot = (await postJson("/bot/new", { name: "Bo" })).json.bot;
  const chat = (await postJson("/thread/new", {})).json.id;
  const id = await make({ name: "Order" });

  await postJson("/group/item", { id, op: "add", kind: "bot", ref: bot.id });
  await postJson("/group/item", { id, op: "add", kind: "chat", ref: chat });
  await postJson("/group/item", { id, op: "add", kind: "artifact", ref: "file:m1/a.txt", thread: "t9999" });
  // a move, to the front
  await postJson("/group/item", { id, op: "add", kind: "artifact", ref: "file:m1/a.txt", index: 0 });
  assert.deepEqual(dbRows(dir, "SELECT ref, thread FROM group_items WHERE group_id=? ORDER BY ord", id), [
    { ref: "file:m1/a.txt", thread: null },
    { ref: bot.id, thread: null },
    { ref: chat, thread: null },
  ]);
  await postJson("/group/item", { id, op: "add", kind: "chat", ref: chat, index: -5 });
  assert.deepEqual(dbRows(dir, "SELECT ref FROM group_items WHERE group_id=? ORDER BY ord", id).map((r) => r.ref), [chat, "file:m1/a.txt", bot.id]);
  assert.deepEqual((await postJson("/group/item", { id, op: "remove", kind: "artifact", ref: "file:m1/a.txt" })).json, { ok: true });
  assert.deepEqual(dbRows(dir, "SELECT ref, ord FROM group_items WHERE group_id=? ORDER BY ord", id), [{ ref: chat, ord: 0 }, { ref: bot.id, ord: 1 }]);
  bad(await postJson("/group/item", { id: "nope", op: "add", kind: "chat", ref: chat }), "unknown group nope");
  bad(await postJson("/group/item", { id, op: "add", kind: "thing", ref: "x" }), "bad kind thing");
  bad(await postJson("/group/item", { id, op: "remove", kind: "thing", ref: "x" }), "bad kind thing");
  bad(await postJson("/group/item", { id, op: "pin", kind: "chat", ref: chat }), "unknown op pin");
  bad(await postJson("/group/item", { id, op: "add", kind: "bot", ref: "nope" }), "unknown bot nope");
  bad(await postJson("/group/item", { id, op: "add", kind: "chat", ref: "t9999" }), "unknown chat t9999");
  bad(await postJson("/group/item", { id, op: "add", kind: "artifact", ref: "x1" }), "bad artifact id x1");
  bad(await postJson("/group/item", { id, op: "add", kind: "app", ref: "apps/hello" }), "bad app apps/hello");
});

test("GRP-05 /group/update renames, recolors, folds and reorders; refusals", async () => {
  const { groups: before } = await groups();
  const id = before[0].id;

  assert.deepEqual((await postJson("/group/update", { id, name: " Renamed ", color: "#ABCDEF", collapsed: 1, order: 99 })).json, { ok: true });
  const { groups: gs } = await groups();
  const g = gs.at(-1);

  assert.deepEqual([g.id, g.name, g.color, g.collapsed, g.order], [id, "Renamed", "#abcdef", true, gs.length - 1]);
  await postJson("/group/update", { id, order: 0, collapsed: false });
  assert.equal((await groups()).groups[0].id, id);
  bad(await postJson("/group/update", { id: "nope", name: "x" }), "unknown group nope");
  bad(await postJson("/group/update", { id, name: " " }), "a group needs a name");
  bad(await postJson("/group/update", { id, color: "red" }), "bad color red");
  assert.equal((await group(id)).name, "Renamed");
});

test("GRP-06 a member whose bot or chat is gone is dropped for good when the list is read", async () => {
  const bot = (await postJson("/bot/new", { name: "Gone" })).json.bot;
  const chat = (await postJson("/thread/new", {})).json.id;
  const id = await make({ name: "Leaving", item: { kind: "bot", ref: bot.id } });

  await postJson("/group/item", { id, op: "add", kind: "chat", ref: chat });
  await postJson("/bot/delete", { id: bot.id });
  await postJson("/thread/delete", { id: chat });
  assert.equal(dbRows(dir, "SELECT * FROM group_items WHERE group_id=?", id).length, 2);
  assert.deepEqual((await group(id)).items, []);
  assert.equal(dbRows(dir, "SELECT * FROM group_items WHERE group_id=?", id).length, 0);
});

test("GRP-07 /group/delete removes the group and renumbers the rest; its members stay", async () => {
  const { groups: before } = await groups();
  const gone = before[0];

  assert.deepEqual((await postJson("/group/delete", { id: gone.id })).json, { ok: true });
  const { groups: gs } = await groups();

  assert.deepEqual(gs.map((g) => g.id), before.slice(1).map((g) => g.id));
  assert.deepEqual(dbRows(dir, "SELECT ord FROM groups ORDER BY ord").map((r) => r.ord), gs.map((_, k) => k));
  assert.equal(dbRows(dir, "SELECT * FROM group_items WHERE group_id=?", gone.id).length, 0);
  bad(await postJson("/group/delete", { id: gone.id }), `unknown group ${gone.id}`);
});

test("GRP-08 the first start makes each bot section a group and the pinned Mini Apps one more, once", async () => {
  await server.stop();
  const old = tempDir("groups-migrate");
  const bots = JSON.parse(nodeIn(old, `
    const { store, nextThreadSeq, uuidv7, writeIndex, db } = await import("./src/state.mjs");
    const mk = (id, name, section) => {
      const t = "t" + nextThreadSeq();
      store.list.push({ id: t, uid: uuidv7(), title: name, turns: [] });
      store.bots.push({ id, name, shape: "blob", color: "#1084FE", threadId: t, section, memories: [], integrations: [] });
    };
    mk("bwork1", "Ann", "Work");
    mk("bhome1", "Ben", "Home");
    mk("bwork2", "Cal", "Work");
    mk("bnone1", "Dan", "");
    store.sections = ["Home", "Work"];
    writeIndex();
    db.prepare("INSERT INTO kv(name,data) VALUES('rox-pinned-apps',?)").run(JSON.stringify({ apps: [{ app: "R3@v2", thread: "t1" }, { app: "" }, { app: "R4" }] }));
    console.log(JSON.stringify(store.bots.map((b) => b.id)));
  `));

  assert.equal(bots.length, 4);
  server = await startServer(old, { env: offlineEnv(join(old, "stubs.log")) });
  assert.deepEqual(dbRows(old, "SELECT name, color, ord FROM groups ORDER BY ord"), [
    { name: "Home", color: COLORS[0], ord: 0 },
    { name: "Work", color: COLORS[1], ord: 1 },
    { name: "Mini Apps", color: COLORS[2], ord: 2 },
  ]);
  assert.deepEqual(dbRows(old, "SELECT g.name, i.kind, i.ref, i.thread, i.ord FROM group_items i JOIN groups g ON g.id=i.group_id ORDER BY g.ord, i.ord"), [
    { name: "Home", kind: "bot", ref: "bhome1", thread: null, ord: 0 },
    { name: "Work", kind: "bot", ref: "bwork1", thread: null, ord: 0 },
    { name: "Work", kind: "bot", ref: "bwork2", thread: null, ord: 1 },
    { name: "Mini Apps", kind: "artifact", ref: "R3", thread: "t1", ord: 0 },
    { name: "Mini Apps", kind: "artifact", ref: "R4", thread: null, ord: 1 },
  ]);
  assert.match(dbRows(old, "SELECT v FROM meta WHERE k='groups_migrated'")[0].v, /^\d{13}$/);
  // artifacts the Library can't find are skipped in the list, and kept
  assert.deepEqual((await groups()).groups.map((g) => [g.name, g.items.map((i) => i.title)]), [["Home", ["Ben"]], ["Work", ["Ann", "Cal"]], ["Mini Apps", []]]);
  await server.stop();
  server = await startServer(old, { env: offlineEnv(join(old, "stubs.log")) });
  assert.equal(dbRows(old, "SELECT * FROM groups").length, 3, "it runs once");
});
