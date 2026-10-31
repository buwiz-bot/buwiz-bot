// The bot routes (Rust broker plan, phase 5 item 5.2), as Node answers
// them today: new, update, memories, integrations, delete, the /threads bot
// fields, avatars and the attachment picker. MOCK on, but MOCK does not
// guard /avatar/gen or image models: those reach the fake opencodex
// (fixtures/fake-opencodex.mjs). The Mac tools are PATH stubs.
import assert from "node:assert/strict";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { PNG_B64, say, startFakeOpencodex } from "./fixtures/fake-opencodex.mjs";
import { modelEnv, postJson, request, startServer, stubCalls, tempDir, waitFor } from "./helpers.mjs";

const dir = tempDir("bots-routes");
const log = join(dir, "stubs.log");
const picks = join(dir, "osascript-answer");
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

let fake;
let server;
const env = () => modelEnv(fake, log, { MOCK: "1", OSASCRIPT_STUB_FILE: picks });

before(async () => {
  fake = await startFakeOpencodex();
  server = await startServer(dir, { env: env() });
});

after(async () => {
  await server?.stop();
  await fake?.close();
});

const threads = async () => (await request("GET", "/threads")).json;
const botOf = async (id) => (await threads()).bots.find((b) => b.id === id);
const bad = (r, error) => {
  assert.equal(r.status, 400, r.text);
  assert.deepEqual(r.json, { ok: false, error });
};
const defaults = (name) => ({
  description: `${name} is a personal assistant bot in Buwiz Bot.`,
  instructions: `Stay in character as ${name}. Be helpful, concise and friendly; ask one short clarifying question when a request is ambiguous.`,
});
const done = (id) => waitFor(async () => (await request("GET", `/status?id=${id}`)).json.done, 30000);

test("BOT-01 /bot/new makes a bot with its own chat, seeded context, and makes that chat active", async () => {
  const r = await postJson("/bot/new", { name: "  Ada  " });

  assert.equal(r.status, 200);
  assert.deepEqual(Object.keys(r.json), ["ok", "bot", "turns"]);
  const { bot } = r.json;

  assert.match(bot.id, /^b[0-9a-z]+$/);
  assert.match(bot.threadId, /^t\d+$/);
  assert.deepEqual(bot, { id: bot.id, name: "Ada", shape: "blob", color: "#1084FE", threadId: bot.threadId, ...defaults("Ada"), memories: [], integrations: [] });
  assert.deepEqual(r.json.turns, []);
  const t = await threads();

  assert.equal(t.active, bot.threadId);
  assert.deepEqual(t.list[0], { id: bot.threadId, uid: t.activeUid, title: "Ada", count: 0, hidden: false, archived: false, lastApp: "", bot: { id: bot.id, name: "Ada", shape: "blob", color: "#1084FE" } });
  const own = (await postJson("/bot/new", { name: "Cy", shape: "star", color: "#FF309B" })).json.bot;

  assert.deepEqual([own.shape, own.color], ["star", "#FF309B"]);
  bad(await postJson("/bot/new", {}), "bot needs a name");
  bad(await postJson("/bot/new", { name: "   " }), "bot needs a name");
});

test("BOT-02 /threads lists each bot with every field, preview and updated_at", async () => {
  const id = (await postJson("/bot/new", { name: "Dee" })).json.bot.id;
  const b = await botOf(id);

  assert.deepEqual(b, {
    id, name: "Dee", shape: "blob", color: "#1084FE", threadId: b.threadId, pinned: false, section: "", hidden: false, image: "", crop: null,
    model: "", effort: "", fast: false, label: "", ...defaults("Dee"), memories: [], integrations: [], updated_at: 0, preview: "",
  });
});

test("BOT-03 /bot/update: a rename carries untouched defaults and the chat's title; every field; sections grow", async () => {
  const bot = (await postJson("/bot/new", { name: "Eve" })).json.bot;

  assert.deepEqual((await postJson("/bot/update", { id: bot.id, name: " Fay " })).json, { ok: true });
  let b = await botOf(bot.id);

  assert.equal(b.name, "Fay");
  assert.equal(b.description, defaults("Fay").description);
  assert.equal(b.instructions, defaults("Fay").instructions);
  assert.equal(b.updated_at, 0);
  assert.equal((await threads()).list.find((t) => t.id === bot.threadId).title, "Fay");
  await postJson("/bot/update", {
    id: bot.id, description: "  Writes poems.  ", shape: "hex", color: "#00C972", pinned: 1, hidden: true, image: "file:///x.png",
    crop: { x: 1, y: 2, w: 3, h: 4 }, model: "gpt-5.5", effort: "high", fast: true, label: "x".repeat(50), section: " Work ",
  });
  b = await botOf(bot.id);
  assert.deepEqual(
    { shape: b.shape, color: b.color, pinned: b.pinned, hidden: b.hidden, image: b.image, crop: b.crop, model: b.model, effort: b.effort, fast: b.fast, label: b.label, section: b.section, description: b.description },
    { shape: "hex", color: "#00C972", pinned: true, hidden: true, image: "file:///x.png", crop: { x: 1, y: 2, w: 3, h: 4 }, model: "gpt-5.5", effort: "high", fast: true, label: "x".repeat(40), section: "Work", description: "Writes poems." },
  );
  assert.ok(b.updated_at > 0, "an edited description touches updated_at");
  assert.deepEqual((await threads()).sections, ["Work"]);
  // an edited description stays through a rename; the untouched instructions follow
  await postJson("/bot/update", { id: bot.id, name: "Gus", section: "" });
  b = await botOf(bot.id);
  assert.equal(b.description, "Writes poems.");
  assert.equal(b.instructions, defaults("Gus").instructions);
  assert.equal(b.section, "");
  assert.deepEqual((await threads()).sections, ["Work"]);
  bad(await postJson("/bot/update", { id: "nope", name: "x" }), "unknown bot nope");
});

test("BOT-04 /bot/memory adds (a duplicate returns the one there), updates and removes; refusals", async () => {
  const id = (await postJson("/bot/new", { name: "Hal" })).json.bot.id;
  const add = await postJson("/bot/memory", { id, op: "add", text: "  Likes tea  " });

  assert.equal(add.status, 200);
  assert.deepEqual(Object.keys(add.json), ["ok", "memories"]);
  const [m] = add.json.memories;

  assert.match(m.id, UUID);
  assert.deepEqual(Object.keys(m), ["id", "text", "at"]);
  assert.equal(m.text, "Likes tea");
  assert.equal((await postJson("/bot/memory", { id, op: "add", text: "LIKES TEA" })).json.memories.length, 1);
  assert.equal((await postJson("/bot/memory", { id, op: "add", text: "y".repeat(1200) })).json.memories[1].text.length, 1000);
  const upd = (await postJson("/bot/memory", { id, op: "update", memoryId: m.id, text: "Likes green tea" })).json.memories;

  assert.equal(upd[0].text, "Likes green tea");
  assert.deepEqual((await postJson("/bot/memory", { id, op: "remove", memoryId: m.id })).json.memories.map((x) => x.text.length), [1000]);
  assert.equal((await botOf(id)).memories.length, 1);
  bad(await postJson("/bot/memory", { id, op: "add", text: "  " }), "memory needs text");
  bad(await postJson("/bot/memory", { id, op: "update", memoryId: "zzz", text: "x" }), "bot has no memory zzz");
  bad(await postJson("/bot/memory", { id, op: "remove", memoryId: "zzz" }), "no memory zzz");
  bad(await postJson("/bot/memory", { id, op: "wipe" }), "unknown op wipe");
  bad(await postJson("/bot/memory", { id: "nope", op: "add", text: "x" }), "unknown bot nope");
});

test("BOT-05 /bot/integration adds a skill once and removes it; refusals", async () => {
  const id = (await postJson("/bot/new", { name: "Ivy" })).json.bot.id;

  assert.deepEqual((await postJson("/skills", { name: "Tidy Notes", description: "Keeps notes tidy", instructions: "Tidy them." })).json, { ok: true, id: "tidy-notes" });
  const one = { id: "tidy-notes", name: "Tidy Notes", kind: "skill", description: "Keeps notes tidy", icon: "" };

  assert.deepEqual((await postJson("/bot/integration", { id, op: "add", integration: "skill:tidy-notes" })).json, { ok: true, integrations: [one] });
  // no kind: mcp, then plugin, then skill
  assert.deepEqual((await postJson("/bot/integration", { id, op: "add", integration: "tidy-notes" })).json, { ok: true, integrations: [one] });
  assert.deepEqual((await botOf(id)).integrations, [one]);
  bad(await postJson("/bot/integration", { id, op: "add", integration: "mcp:zzz" }), "no integration matches mcp:zzz");
  bad(await postJson("/bot/integration", { id, op: "add", integration: "zzz" }), "no integration matches zzz");
  bad(await postJson("/bot/integration", { id, op: "add", integration: "" }), "integration needs an id");
  bad(await postJson("/bot/integration", { id, op: "add", integration: { kind: "app", id: "y" } }), "unknown integration kind app");
  assert.deepEqual((await postJson("/bot/integration", { id, op: "remove", integration: { kind: "skill", id: "tidy-notes" } })).json, { ok: true, integrations: [] });
  bad(await postJson("/bot/integration", { id, op: "remove", integration: "skill:tidy-notes" }), "bot has no integration skill:tidy-notes");
  bad(await postJson("/bot/integration", { id, op: "swap", integration: "x" }), "unknown op swap");
  bad(await postJson("/bot/integration", { id: "nope", op: "add", integration: "x" }), "unknown bot nope");
});

test("BOT-06 the preview is the chat's latest text, markdown marks and runs of space flattened", async () => {
  const bot = (await postJson("/bot/new", { name: "Jo" })).json.bot;

  await done((await postJson("/chat", { prompt: "/mock zzz", thread: bot.threadId })).json.id);
  const b = await botOf(bot.id);
  const text = (await request("GET", `/turns?thread=${bot.threadId}`)).json.turns.at(-1).text;

  assert.match(text, /^No mock for `zzz` — try one of: /);
  assert.equal(b.preview, text.replace(/[#*`>_]/g, "").replace(/\s+/g, " ").trim().slice(0, 160));
  assert.equal(b.preview.length, 160);
});

test("BOT-07 updated_at is never persisted: a restart reads 0 while the edit stays", async () => {
  const id = (await postJson("/bot/new", { name: "Kai" })).json.bot.id;

  await postJson("/bot/update", { id, instructions: "Be brief." });
  assert.ok((await botOf(id)).updated_at > 0);
  await server.stop();
  server = await startServer(dir, { env: env() });
  const b = await botOf(id);

  assert.equal(b.updated_at, 0);
  assert.equal(b.instructions, "Be brief.");
});

test("BOT-08 /avatar/gen asks the model for a shape and color even under MOCK; a choice off the list keeps the default", async () => {
  fake.reset();
  fake.script("avatar", [say('Sure: {"shape":"star","color":"#FF309B"}'), say('{"shape":"moon","color":"#123456"}'), say("no json at all")]);
  const desc = "[[script:avatar]] a happy cat";

  assert.deepEqual((await postJson("/avatar/gen", { description: `  ${desc}  ` })).json, { ok: true, shape: "star", color: "#FF309B" });
  assert.deepEqual((await postJson("/avatar/gen", { description: desc })).json, { ok: true, shape: "blob", color: "#1084FE" });
  assert.deepEqual((await postJson("/avatar/gen", { description: desc })).json, { ok: true, shape: "blob", color: "#1084FE" });
  const [call] = fake.calls();

  assert.equal(call.path, "/v1/messages");
  assert.equal(call.stream, false);
  assert.deepEqual(call.headers, { "content-type": "application/json", "x-api-key": "opencodex", "anthropic-version": "2023-06-01" });
  assert.deepEqual(call.body, {
    model: "claude-haiku-5-5",
    max_tokens: 120,
    system: 'Pick an avatar shape and color for a blob character from a description. Reply ONLY with JSON {"shape":<one of blob|cloud|square|sparkle|clover|heart|flower|teardrop|tablet|wedge|house|star|hex>,"color":<one of #FFFFFF|#936439|#FF263C|#FF6700|#FF9800|#00C972|#00BCA6|#1084FE|#9159FE|#FF309B|#777777>}.',
    messages: [{ role: "user", content: desc }],
  });
  bad(await postJson("/avatar/gen", { description: "  " }), "describe the avatar");
});

test("BOT-09 /avatar/image/<gen|pick|paste>: the image model, the picker (cancel is not an error), the clipboard", async () => {
  fake.reset();
  const gen = await postJson("/avatar/image/gen", { description: " a red fox " });
  const avatars = join(dir, "generated", "avatars");

  assert.equal(gen.status, 200, gen.text);
  assert.match(gen.json.uri, new RegExp(`^file://${avatars}/[0-9a-z]+\\.png$`));
  assert.deepEqual({ ...gen.json, uri: "" }, { ok: true, uri: "", name: "a red fox", w: 64, h: 64 });
  assert.deepEqual(readFileSync(gen.json.uri.slice(7)).toString("base64"), PNG_B64);
  assert.deepEqual(fake.requests.map((r) => [r.method, r.path, r.body]), [
    ["POST", "/v1/images/generations", { prompt: "A square profile avatar image, centered subject filling the frame, clean simple background: a red fox", size: "1024x1024", n: 1 }],
  ]);
  const src = join(dir, "picked.png");

  writeFileSync(src, Buffer.from(PNG_B64, "base64"));
  writeFileSync(picks, src);
  const pick = await postJson("/avatar/image/pick", {});

  assert.deepEqual({ ...pick.json, uri: "" }, { ok: true, uri: "", name: "picked.png", w: 64, h: 64 });
  assert.ok(existsSync(pick.json.uri.slice(7)));
  writeFileSync(picks, "!cancel");
  const cancelled = await postJson("/avatar/image/pick", {});

  assert.equal(cancelled.status, 200);
  assert.deepEqual(cancelled.json, { ok: false, cancelled: true });
  writeFileSync(picks, "");
  assert.deepEqual({ ...(await postJson("/avatar/image/paste", {})).json, uri: "" }, { ok: true, uri: "", name: "Pasted image", w: 64, h: 64 });
  bad(await postJson("/avatar/image/gen", {}), "describe the avatar");
  bad(await postJson("/avatar/image/bogus", {}), "unknown avatar source bogus");
});

test("BOT-10 /attach/pick stages each chosen file: images copied with a thumbnail, documents as they are; cancel answers no files", async () => {
  const png = join(dir, "photo.png");
  const txt = join(dir, "notes.txt");

  writeFileSync(png, Buffer.from(PNG_B64, "base64"));
  writeFileSync(txt, "some notes\n");
  writeFileSync(picks, `${png}\n${txt}\n`);
  const n = stubCalls(log).length;
  const r = await postJson("/attach/pick", {});

  assert.equal(r.status, 200, r.text);
  const [img, doc] = r.json.files;
  const att = join(dir, "generated", "attachments");

  assert.match(img.uri, new RegExp(`^file://${att}/[0-9a-z]+-[0-9a-z]{1,4}\\.png$`));
  assert.deepEqual(img, { uri: img.uri, thumb: `${img.uri.slice(0, -4)}.thumb.png`, feed: "", name: "photo.png", ext: "png", kind: "image", size: 70, width: 64, height: 64 });
  assert.match(doc.uri, new RegExp(`^file://${att}/[0-9a-z]+-[0-9a-z]{1,4}-notes\\.txt$`));
  assert.deepEqual(doc, { uri: doc.uri, thumb: "", feed: "", name: "notes.txt", ext: "txt", kind: "file", size: 11 });
  const out = img.uri.slice(7);
  const sips = stubCalls(log).slice(n).filter((c) => c.bin === "sips").map((c) => c.argv);

  assert.deepEqual(sips.slice(0, 2).sort(), [
    ["-s", "format", "png", "-Z", "1120", out, "--out", `${out.slice(0, -4)}.feed.png`],
    ["-s", "format", "png", "-Z", "256", out, "--out", `${out.slice(0, -4)}.thumb.png`],
  ].sort());
  // the feed is no smaller than the original: dropped
  assert.ok(!existsSync(`${out.slice(0, -4)}.feed.png`));
  writeFileSync(picks, "!cancel");
  assert.deepEqual((await postJson("/attach/pick", {})).json, { ok: false, cancelled: true, files: [] });
});

test("BOT-11 /bot/delete drops the bot and its chat; the last chat gone leaves a new one", async () => {
  const bot = (await postJson("/bot/new", { name: "Lu" })).json.bot;

  assert.deepEqual((await postJson("/bot/delete", { id: bot.id })).json, { ok: true });
  const t = await threads();

  assert.ok(!t.bots.some((b) => b.id === bot.id));
  assert.ok(!t.list.some((x) => x.id === bot.threadId));
  assert.notEqual(t.active, bot.threadId);
  bad(await postJson("/bot/delete", { id: bot.id }), `unknown bot ${bot.id}`);

  for (const x of t.list) {
    const owner = t.bots.find((b) => b.threadId === x.id);

    await postJson(owner ? "/bot/delete" : "/thread/delete", { id: owner ? owner.id : x.id });
  }

  const after = await threads();

  assert.equal(after.list.length, 1);
  assert.equal(after.list[0].title, "new chat");
  assert.deepEqual(after.bots.map((b) => b.threadId), after.bots.map(() => null));
});
