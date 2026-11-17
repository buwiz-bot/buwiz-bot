// Avatar design over HTTP: POST /avatar/design (MOCK, then over the wire to
// the fake opencodex, and on the OpenAI-compatible path), the bot routes'
// `outline` field through a broker restart, POST /avatar/picture and
// /avatar/image/import. The recipe checks themselves are pinned in
// src/hub/library/avatar_shape/tests.rs.
import assert from "node:assert/strict";
import { existsSync, mkdirSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";
import { after, before, test } from "node:test";
import { PNG_B64, say, startFakeOpencodex } from "../fixtures/fake-opencodex.mjs";
import { modelEnv, postJson, request, startServer, tempDir } from "./helpers.mjs";

// the design prompt the broker sends, as pinned beside the avatar checks
const DESIGN_SYSTEM = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "pins", "avatar_shape", "design_system.txt"), "utf8");
const KIT_SHAPES = ["blob", "cloud", "square", "sparkle", "clover", "heart", "flower", "teardrop", "tablet", "wedge", "house", "star", "hex"];
// a candidate the way the designer must return it: a kit shape, or a custom outline with its colour
const validCandidate = (c) =>
  typeof c.name === "string" && /^#[0-9A-Fa-f]{6}$/.test(c.color) && (c.shape === "custom" ? /^[MLCQZmlcqz0-9., -]+$/.test(c.outline) && /Z\s*$/i.test(c.outline) : KIT_SHAPES.includes(c.shape));

const dir = tempDir("avatar-design");
const log = join(dir, "stubs.log");
const avatars = join(dir, "generated", "avatars");
let fake;
let server;
const env = (extra = {}) => modelEnv(fake, log, { MOCK: "1", ...extra });

before(async () => {
  fake = await startFakeOpencodex();
  server = await startServer(dir, { env: env() });
});

after(async () => {
  await server?.stop();
  await fake?.close();
});

const bad = (r, error) => {
  assert.equal(r.status, 400, r.text);
  assert.equal(r.json.ok, false);
  if (error instanceof RegExp) assert.match(r.json.error, error);
  else assert.equal(r.json.error, error);
};
const threads = async () => (await request("GET", "/threads")).json;
const botOf = async (id) => (await threads()).bots.find((b) => b.id === id);

const GOOD = {
  pebble: "M50 16 C73 16 88 31 88 52 C88 73 72 86 50 86 C28 86 12 73 12 52 C12 31 27 16 50 16 Z",
  cat: "M18 44 L22 14 L42 28 Q50 26 58 28 L78 14 L82 44 C88 66 72 86 50 86 C28 86 12 66 18 44 Z",
  receipt: "M26 14 L74 14 L74 80 L66 88 L58 80 L50 88 L42 80 L34 88 L26 80 Z",
  heart: "M50 85 C20 65 10 45 15 30 C20 15 40 12 50 28 C60 12 80 15 85 30 C90 45 80 65 50 85 Z",
};
const kit = (name, shape = "blob", color = "#1084FE") => ({ name, shape, color, why: `${name}.` });
const cust = (name, outline = GOOD.pebble, color = "#00C972") => ({ name, shape: "custom", color, outline, why: `${name}.` });

test("AVD-09 POST /avatar/design under MOCK: three valid candidates, a kit one without an image, a custom first with one", async () => {
  const r = await postJson("/avatar/design", { prompt: "reads my receipts and files my BIR forms", name: "Tax Fox" });
  assert.equal(r.status, 200, r.text);
  assert.deepEqual(Object.keys(r.json), ["ok", "candidates"]);
  const { candidates } = r.json;
  assert.equal(candidates.length, 3);
  for (const c of candidates) {
    assert.ok(validCandidate(c), c.name);
    assert.deepEqual(Object.keys(c), c.shape === "custom" ? ["name", "shape", "color", "outline", "why"] : ["name", "shape", "color", "why"]);
  }
  assert.ok(candidates.some((c) => KIT_SHAPES.includes(c.shape)));
  assert.equal(candidates.filter((c) => c.shape === "custom").length, 2);
  mkdirSync(avatars, { recursive: true });
  const img = join(avatars, "m1.png");
  writeFileSync(img, Buffer.from(PNG_B64, "base64"));
  const w = await postJson("/avatar/design", { prompt: "make it orange", image: pathToFileURL(img).href });
  assert.equal(w.status, 200, w.text);
  assert.equal(w.json.candidates.length, 3);
  assert.equal(w.json.candidates[0].shape, "custom");
  for (const c of w.json.candidates) assert.ok(validCandidate(c), c.name);
  // deterministic
  assert.deepEqual((await postJson("/avatar/design", { prompt: "reads my receipts and files my BIR forms", name: "Tax Fox" })).json, r.json);
  bad(await postJson("/avatar/design", { prompt: "  " }), "describe the bot or add a picture");
  bad(await postJson("/avatar/design", {}), "describe the bot or add a picture");
});

test("AVD-10 /avatar/design takes an image only from inside the avatar folder", async () => {
  const refused = "image must be an avatar image from /avatar/image/pick or /avatar/image/paste";
  const outside = join(dir, "outside.png");
  writeFileSync(outside, Buffer.from(PNG_B64, "base64"));
  const link = join(avatars, "link.png");
  symlinkSync(outside, link);
  for (const image of [
    pathToFileURL(outside).href,
    `file://${avatars}/../outside.png`,
    `file://${avatars}/%2E%2E/outside.png`,
    pathToFileURL(link).href,
    pathToFileURL(avatars).href,
    outside,
    "https://example.com/x.png",
    42,
  ]) bad(await postJson("/avatar/design", { prompt: "x", image }), refused);
  bad(await postJson("/avatar/design", { prompt: "x", image: pathToFileURL(join(avatars, "nope.png")).href }), "image not found");
  writeFileSync(join(avatars, "text.png"), "not an image");
  bad(await postJson("/avatar/design", { prompt: "x", image: pathToFileURL(join(avatars, "text.png")).href }), "image is not a PNG, JPEG, GIF or WebP");
});

test("AVD-11 /bot/new and /bot/update store an outline, a kit shape clears it, bad ones are refused; it survives a restart", async () => {
  const made = await postJson("/bot/new", { name: "Tax Fox", shape: "custom", color: "#FF6700", outline: GOOD.cat });
  assert.equal(made.status, 200, made.text);
  const { bot } = made.json;
  assert.deepEqual([bot.shape, bot.color, bot.outline], ["custom", "#FF6700", GOOD.cat]);
  let b = await botOf(bot.id);
  assert.deepEqual([b.shape, b.outline], ["custom", GOOD.cat]);
  assert.deepEqual((await threads()).list.find((t) => t.id === bot.threadId).bot, { id: bot.id, name: "Tax Fox", shape: "custom", color: "#FF6700", outline: GOOD.cat });
  // refusals: no outline, a bad one, a bad shape or colour
  bad(await postJson("/bot/new", { name: "X", shape: "custom" }), "a custom shape needs an outline");
  bad(await postJson("/bot/new", { name: "X", outline: "M0 0 L1 0 L1 1 Z" }), /covers 0% of the box/);
  bad(await postJson("/bot/new", { name: "X", shape: "moon" }), /^shape must be one of/);
  bad(await postJson("/bot/new", { name: "X", color: "blue" }), "color must be #RRGGBB");
  bad(await postJson("/bot/update", { id: bot.id, outline: "M10 10 H90 V90 Z" }), "outline may hold only M/L/C/Q/Z commands and numbers");
  bad(await postJson("/bot/update", { id: bot.id, shape: "custom", outline: "M15 15 L85 15 L85 85 L15 85" }), "outline must be closed (end with Z)");
  assert.equal((await botOf(bot.id)).outline, GOOD.cat, "a refused update changes nothing");
  // a colour alone keeps the outline; a new outline replaces it
  assert.deepEqual((await postJson("/bot/update", { id: bot.id, color: "#9159FE" })).json, { ok: true });
  assert.deepEqual([(b = await botOf(bot.id)).color, b.outline], ["#9159FE", GOOD.cat]);
  await postJson("/bot/update", { id: bot.id, outline: GOOD.receipt });
  assert.deepEqual([(b = await botOf(bot.id)).shape, b.outline], ["custom", GOOD.receipt]);
  // a kit shape clears it
  await postJson("/bot/update", { id: bot.id, shape: "heart" });
  b = await botOf(bot.id);
  assert.equal(b.shape, "heart");
  assert.ok(!("outline" in b));
  assert.ok(!("outline" in (await threads()).list.find((t) => t.id === bot.threadId).bot));
  // back to custom without an outline is refused; with one it takes
  bad(await postJson("/bot/update", { id: bot.id, shape: "custom" }), "a custom shape needs an outline");
  await postJson("/bot/update", { id: bot.id, shape: "custom", outline: GOOD.heart });
  const kitBot = (await postJson("/bot/new", { name: "Plain", shape: "hex" })).json.bot;
  assert.ok(!("outline" in kitBot));
  await server.stop();
  server = await startServer(dir, { env: env() });
  b = await botOf(bot.id);
  assert.deepEqual([b.shape, b.color, b.outline], ["custom", "#9159FE", GOOD.heart]);
  assert.ok(!("outline" in (await botOf(kitBot.id))));
});

test("AVD-12 /avatar/design over the wire: the system prompt, the image block, a retry with the errors, the failures dropped", async () => {
  await server.stop();
  server = await startServer(dir, { env: env({ MOCK: undefined }) });
  fake.reset();
  const reply = (cands) => say(`Here:\n${JSON.stringify({ candidates: cands })}`);
  fake.script("design", [reply([kit("A"), cust("Bad", "M0 0 L5 0 L5 5 Z"), cust("B")]), reply([kit("A"), cust("Bad", "M0 0 L5 0 L5 5 Z"), cust("B")])]);
  const img = join(avatars, "m1.png");
  const r = await postJson("/avatar/design", { prompt: "[[script:design]] a fox", name: "Fox" });
  assert.equal(r.status, 200, r.text);
  assert.deepEqual(r.json.candidates, [kit("A"), cust("B")]);
  const calls = fake.calls();
  assert.equal(calls.length, 2);
  assert.equal(calls[0].path, "/v1/messages");
  assert.deepEqual({ ...calls[0].body, messages: undefined }, { model: "claude-haiku-5-5", max_tokens: 6000, system: DESIGN_SYSTEM, messages: undefined }, "nothing picked: the chat's default model");
  assert.match(calls[1].body.messages[2].content, /candidate 2 \(Bad\): outline covers/);
  fake.reset();
  fake.script("design-img", [reply([cust("Fox", GOOD.cat, "#FF6700"), kit("A"), cust("B")])]);
  const w = await postJson("/avatar/design", { prompt: "[[script:design-img]] keep the ears", image: pathToFileURL(img).href });
  assert.equal(w.status, 200, w.text);
  assert.equal(w.json.candidates[0].outline, GOOD.cat);
  const [call] = fake.calls();
  assert.deepEqual(call.body.messages[0].content[0], { type: "image", source: { type: "base64", media_type: "image/png", data: PNG_B64 } });
  // the picker's model, routed as a bot's chosen one is (a --fast id keeps its suffix)
  for (const model of ["gpt-5.5", "claude-haiku-5-5--fast", "  claude-sonnet-5  "]) {
    fake.reset();
    fake.script("design-pick", [reply([kit("A"), cust("B"), cust("C", GOOD.cat)])]);
    const p = await postJson("/avatar/design", { prompt: "[[script:design-pick]] a fox", model });
    assert.equal(p.status, 200, p.text);
    assert.deepEqual([fake.calls()[0].body.model, fake.calls()[0].body.system], [model.trim(), DESIGN_SYSTEM]);
  }
  fake.reset();
  fake.script("design-default", [reply([kit("A"), cust("B"), cust("C", GOOD.cat)])]);
  assert.equal((await postJson("/avatar/design", { prompt: "[[script:design-default]] a fox", model: "" })).status, 200);
  assert.equal(fake.calls()[0].body.model, "claude-haiku-5-5", "the picker's Default is the chat's default");
  // the picker's reasoning effort comes along
  fake.reset();
  fake.script("design-effort", [reply([kit("A"), cust("B"), cust("C", GOOD.cat)])]);
  assert.equal((await postJson("/avatar/design", { prompt: "[[script:design-effort]] a fox", model: "claude-sonnet-5", effort: "high" })).status, 200);
  assert.deepEqual([fake.calls()[0].body.model, fake.calls()[0].body.output_config], ["claude-sonnet-5", { effort: "high" }]);
  // GEN_AVATAR_MODEL picks Design's model; GEN_MODEL (the chat's) does not
  await server.stop();
  server = await startServer(dir, { env: env({ MOCK: undefined, GEN_MODEL: "claude-haiku-5-5", GEN_AVATAR_MODEL: "claude-opus-5" }) });
  fake.reset();
  fake.script("design-env", [reply([kit("A"), cust("B"), cust("C", GOOD.cat)])]);
  assert.equal((await postJson("/avatar/design", { prompt: "[[script:design-env]] a fox" })).status, 200);
  assert.equal(fake.calls()[0].body.model, "claude-opus-5");
});

test("AVD-14 /avatar/design on the OpenAI-compatible path: chat/completions with the image as image_url", async () => {
  await server.stop();
  server = await startServer(dir, { env: env({ MOCK: undefined, GEN_PROVIDER: "openai", GEN_BASE_URL: `${fake.url}/v1`, OPENROUTER_API_KEY: "test-key" }) });
  fake.reset();
  fake.script("design-oai", [say(JSON.stringify({ candidates: [cust("Fox", GOOD.cat, "#FF6700"), kit("A"), cust("B")] }))]);
  const r = await postJson("/avatar/design", { prompt: "[[script:design-oai]] a fox", image: pathToFileURL(join(avatars, "m1.png")).href });
  assert.equal(r.status, 200, r.text);
  assert.deepEqual(r.json.candidates, [cust("Fox", GOOD.cat, "#FF6700"), kit("A"), cust("B")]);
  const [call] = fake.calls();
  assert.equal(call.path, "/v1/chat/completions");
  // off Anthropic, Design's model defaults to the provider's (GEN_MODEL's)
  assert.equal(call.body.model, "openai/gpt-4o-mini");
  assert.equal(call.headers.authorization, "Bearer test-key");
  assert.deepEqual(call.body.messages[0], { role: "system", content: DESIGN_SYSTEM });
  assert.deepEqual(call.body.messages[1].content[0], { type: "image_url", image_url: { url: `data:image/png;base64,${PNG_B64}` } });
  assert.equal(call.body.messages[1].content[1].type, "text");
});

test("AVD-16 /avatar/picture under MOCK: the reference (or a fixed square) as the draft, in the avatar folder; the same image rules", async () => {
  await server.stop();
  server = await startServer(dir, { env: env() });
  const keys = ["ok", "via", "uri", "name", "w", "h"];
  const plain = await postJson("/avatar/picture", { prompt: "a violet ogre" });
  assert.equal(plain.status, 200, plain.text);
  assert.deepEqual(Object.keys(plain.json), keys);
  assert.deepEqual({ ...plain.json, uri: "" }, { ok: true, via: "mock", uri: "", name: "Bot picture", w: 64, h: 64 });
  assert.match(plain.json.uri, new RegExp(`^file://${avatars}/[0-9a-z]+-bot\\.png$`));
  assert.ok(existsSync(plain.json.uri.slice(7)));
  const img = join(avatars, "m1.png");
  const w = await postJson("/avatar/picture", { image: pathToFileURL(img).href });
  assert.equal(w.status, 200, w.text);
  assert.deepEqual(readFileSync(w.json.uri.slice(7)), readFileSync(img), "the stub sips copies the reference");
  bad(await postJson("/avatar/picture", { prompt: "  " }), "add a picture or describe the character");
  bad(await postJson("/avatar/picture", {}), "add a picture or describe the character");
  bad(await postJson("/avatar/picture", { image: pathToFileURL(join(dir, "outside.png")).href }), "image must be an avatar image from /avatar/image/pick or /avatar/image/paste");
});

test("AVD-17 /avatar/picture over the wire: the picked model draws SVG; a refused SVG falls back to the image relay", async () => {
  await server.stop();
  server = await startServer(dir, { env: env({ MOCK: undefined }) });
  fake.reset();
  fake.script("pic", [say('Here:\n<svg viewBox="0 0 1024 1024"><rect width="1024" height="1024" fill="#333"/></svg>')]);
  const img = join(avatars, "m1.png");
  const r = await postJson("/avatar/picture", { prompt: "[[script:pic]] make it blue", image: pathToFileURL(img).href, model: "claude-sonnet-5", effort: "high" });
  assert.equal(r.status, 200, r.text);
  assert.deepEqual({ ...r.json, uri: "" }, { ok: true, via: "claude-sonnet-5", uri: "", name: "Bot picture", w: 64, h: 64 });
  assert.ok(existsSync(r.json.uri.slice(7)));
  assert.ok(!existsSync(r.json.uri.slice(7, -4) + ".svg"), "the SVG is cleaned up");
  const [call] = fake.calls();
  assert.deepEqual([call.body.model, call.body.max_tokens, call.body.system, call.body.output_config], ["claude-sonnet-5", 16000, "You are an illustrator who draws clean vector avatars as SVG.", { effort: "high" }]);
  assert.deepEqual(call.body.messages[0].content[0], { type: "image", source: { type: "base64", media_type: "image/png", data: PNG_B64 } });
  assert.match(call.body.messages[0].content[1].text, /^Turn the subject of the reference picture into a minimal flat "Bot" avatar\.[\s\S]*\n\nAlso, from the user: \[\[script:pic\]\] make it blue\n\nDraw it as one SVG: viewBox="0 0 1024 1024"/);
  // an SVG with an outside image: the image relay edits the reference instead
  fake.reset();
  fake.script("pic-bad", [say('<svg><image href="https://example.com/a.png"/></svg>')]);
  const f = await postJson("/avatar/picture", { prompt: "[[script:pic-bad]] x", image: pathToFileURL(img).href });
  assert.equal(f.status, 200, f.text);
  assert.equal(f.json.via, "chatgpt");
  assert.equal(fake.calls()[0].body.model, "claude-haiku-5-5", "nothing picked: the chat's default model");
  assert.ok(fake.requests.some((q) => q.path === "/v1/images/edits"));
});

test("AVD-24 /avatar/image/import copies a file the app's open panel chose into the avatar folder", async () => {
  const src = join(dir, "picked-by-panel.png");
  writeFileSync(src, Buffer.from(PNG_B64, "base64"));
  const r = await postJson("/avatar/image/import", { path: src });
  assert.equal(r.status, 200, r.text);
  assert.equal(r.json.name, "picked-by-panel.png");
  assert.match(r.json.uri, /^file:\/\/.*\/avatars\/[^/]+\.png$/);
  bad(await postJson("/avatar/image/import", { path: "relative.png" }), "import needs an absolute file path");
  bad(await postJson("/avatar/image/import", { path: join(dir, "nope.png") }), "image not found");
  bad(await postJson("/avatar/image/import", { path: dir }), "image not found");
});
