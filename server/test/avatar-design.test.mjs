// Avatar design: the recipe checks (avatar-shape.mjs), the designer's
// retry-then-drop logic with the model stubbed, POST /avatar/design (MOCK,
// then over the wire to the fake opencodex), and the bot routes' `outline`
// field, through a broker restart.
import assert from "node:assert/strict";
import { existsSync, mkdirSync, readdirSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { after, before, test } from "node:test";
import { PNG_B64, say, startFakeOpencodex } from "./fixtures/fake-opencodex.mjs";
import { modelEnv, postJson, request, startServer, tempDir } from "./helpers.mjs";
import {
  DESIGN_SYSTEM, KIT, KIT_SHAPES, MOCK_OUTLINES, designCandidates, flattenPath, outlineErrors, parseCandidates, validateAvatar,
} from "../src/avatar-shape.mjs";

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
  pebble: MOCK_OUTLINES.pebble,
  cat: MOCK_OUTLINES.cat,
  receipt: MOCK_OUTLINES.receipt,
  relative: "m50 15 q35 0 35 35 q0 35 -35 35 q-35 0 -35 -35 q0 -35 35 -35 z",
  sparkle: "M50 2 Q60 40 98 50 Q60 60 50 98 Q40 60 2 50 Q40 40 50 2 Z",
  star: "M50 4 L58 36 L90 27 L66 50 L90 73 L58 64 L50 96 L42 64 L10 73 L34 50 L10 27 L42 36 Z",
  heart: "M50 85 C20 65 10 45 15 30 C20 15 40 12 50 28 C60 12 80 15 85 30 C90 45 80 65 50 85 Z",
  implicit: "M15 15 85 15 85 85 15 85 Z",
};

test("AVD-01 the validator accepts kit shapes and good outlines, absolute and relative", () => {
  for (const shape of KIT_SHAPES) assert.deepEqual(validateAvatar({ shape, color: "#1084FE" }), [], shape);
  assert.deepEqual(validateAvatar({ shape: "blob", color: "#ff309b" }), []);
  for (const [k, outline] of Object.entries(GOOD)) assert.deepEqual(validateAvatar({ shape: "custom", color: "#00C972", outline }), [], k);
  // a kit shape ignores any outline
  assert.deepEqual(validateAvatar({ shape: "heart", color: "#FF263C", outline: "nonsense" }), []);
  // relative and absolute data flatten alike; pairs after M are linetos
  const abs = flattenPath("M50 15 Q85 15 85 50 Q85 85 50 85 Q15 85 15 50 Q15 15 50 15 Z").points;
  const rel = flattenPath(GOOD.relative).points;
  assert.equal(abs.length, rel.length);
  abs.forEach((p, k) => assert.ok(Math.hypot(p[0] - rel[k][0], p[1] - rel[k][1]) < 1e-9));
  assert.deepEqual(flattenPath(GOOD.implicit).points, [[15, 15], [85, 15], [85, 85], [15, 85]]);
});

test("AVD-02 the validator rejects each bad case", () => {
  const one = (recipe, re) => {
    const errs = validateAvatar(recipe);
    assert.ok(errs.some((e) => re.test(e)), `${JSON.stringify(recipe).slice(0, 80)} → ${JSON.stringify(errs)}`);
  };
  const custom = (outline, re) => one({ shape: "custom", color: "#1084FE", outline }, re);
  one({ shape: "moon", color: "#1084FE" }, /^shape must be one of blob, cloud/);
  one({ color: "#1084FE" }, /^shape must be/);
  for (const color of ["#FFF", "red", "#12345G", "1084FE", undefined]) one({ shape: "blob", color }, /^color must be #RRGGBB$/);
  one({ shape: "custom", color: "#1084FE" }, /^a custom shape needs an outline$/);
  custom("M10 10 H90 V90 H10 Z", /only M\/L\/C\/Q\/Z/);
  custom("M10 10 A40 40 0 1 1 90 90 Z", /only M\/L\/C\/Q\/Z/);
  custom("M1e1 10 L90 10 L90 90 Z", /only M\/L\/C\/Q\/Z/);
  custom(`M10 10 ${"L90 10 L90 90 L10 90 ".repeat(200)}Z`, /longer than 4000/);
  custom("M10 10 L90 10 L90", /missing numbers/);
  custom("L10 10 L90 10 L90 90 Z", /must start with M/);
  custom("M10.5.5 10 L90 10 L90 90 Z", /malformed number/);
  custom("M-20 10 L90 10 L90 90 L10 90 Z", /within -5\.\.105/);
  custom("M15 15 L85 15 L85 85 L15 85", /must be closed/);
  custom("M10 10 L40 10 L40 40 Z M60 60 L90 60 L90 90 Z", /one shape/);
  custom("M40 40 L60 40 L60 60 L40 60 Z", /covers 4% of the box/);
  custom("M5 40 L95 40 L95 70 L5 70 Z", /width\/height is 3\.00/);
  // a crescent folds back: rays from its centroid cross the edge 0 or 2 times
  custom("M60 10 C20 10 10 50 30 80 C40 95 70 95 85 75 C55 80 40 60 45 40 C50 20 60 15 60 10 Z", /star-shaped/);
  // a bow tie crosses itself
  custom("M15 15 L85 85 L85 15 L15 85 Z", /covers 0%/);
  // a square with a long, 3-unit-thin arm
  custom("M30 20 L70 20 L70 48 L98 48 L98 51 L70 51 L70 80 L30 80 Z", /thinner than ~6 units/);
  assert.deepEqual(outlineErrors(42), ["a custom shape needs an outline"]);
});

test("AVD-03 parseCandidates reads JSON out of prose, fences, bare arrays; tidies each", () => {
  const c = { name: "Fox", shape: "custom", color: "#ff6700", outline: `  ${GOOD.cat}\n`, why: "A fox." };
  const want = [{ name: "Fox", shape: "custom", color: "#FF6700", outline: GOOD.cat, why: "A fox." }];
  assert.deepEqual(parseCandidates(`Here you go:\n\`\`\`json\n${JSON.stringify({ candidates: [c] })}\n\`\`\`\nEnjoy!`), want);
  assert.deepEqual(parseCandidates(JSON.stringify([c])), want);
  // an outline without a shape is custom; a kit shape drops the outline
  assert.deepEqual(parseCandidates(JSON.stringify([{ ...c, shape: "" }, { ...c, shape: "Heart" }])), [want[0], { name: "Fox", shape: "heart", color: "#FF6700", why: "A fox." }]);
  assert.deepEqual(parseCandidates("no json"), []);
  assert.deepEqual(parseCandidates("{broken"), []);
});

// a stub model: its replies in order, the messages each call got
function stub(...replies) {
  const calls = [];
  const ask = async (messages) => (calls.push(structuredClone(messages)), typeof replies[calls.length - 1] === "string" ? replies[calls.length - 1] : JSON.stringify({ candidates: replies[calls.length - 1] }));
  const picks = [];
  const pick = async (p) => (picks.push(p), { shape: "cloud", color: "#00BCA6" });
  return { ask, pick, calls, picks };
}
const kit = (name, shape = "blob", color = "#1084FE") => ({ name, shape, color, why: `${name}.` });
const cust = (name, outline = GOOD.pebble, color = "#00C972") => ({ name, shape: "custom", color, outline, why: `${name}.` });
const IMG = { type: "image", source: { type: "base64", media_type: "image/png", data: PNG_B64 } };

test("AVD-04 designCandidates: three good ones take one call; the request carries the name and the kit rule", async () => {
  const s = stub([kit("A"), cust("B"), cust("C", GOOD.cat)]);
  const out = await designCandidates({ prompt: "files my BIR forms", name: "Tax Fox" }, s);
  assert.deepEqual(out, [kit("A"), cust("B"), cust("C", GOOD.cat)]);
  assert.equal(s.calls.length, 1);
  assert.deepEqual(s.calls[0], [{
    role: "user",
    content: "The bot's name: Tax Fox\nThe request (what the bot is or does, or how it should look): files my BIR forms\nCandidates may be kit shapes or custom outlines; at least one must be a kit shape. Use a custom outline when the bot has an obvious emblem (a receipt, a leaf, a cat's head).",
  }]);
});

test("AVD-05 designCandidates: a failing candidate retries once with the errors listed; the retry's good ones stand", async () => {
  const s = stub([kit("A"), cust("Thin", "M15 15 L85 85 L85 15 L15 85 Z"), kit("C", "moon")], [kit("A"), cust("B"), kit("C", "star")]);
  const out = await designCandidates({ prompt: "x" }, s);
  assert.deepEqual(out, [kit("A"), cust("B"), kit("C", "star")]);
  assert.equal(s.calls.length, 2);
  const retry = s.calls[1];
  assert.equal(retry.length, 3);
  assert.equal(retry[1].role, "assistant");
  assert.match(retry[2].content, /^These problems need fixing:\n- candidate 2 \(Thin\): outline covers 0% of the box.*\n- candidate 3 \(C\): shape must be one of/);
  assert.match(retry[2].content, /Reply with all 3 candidates again, in the same order, fixed\. JSON only\.$/);
});

test("AVD-06 designCandidates: still failing after the retry, the failures drop (topped up from round one's good ones)", async () => {
  const s = stub([kit("A"), cust("B", "M0 0 Z"), cust("C", "M0 0 Z")], [cust("B2", "M0 0 Z"), cust("C2"), kit("D", "hex", "#FFF")]);
  assert.deepEqual(await designCandidates({ prompt: "x" }, s), [cust("C2"), kit("A")]);
  assert.equal(s.calls.length, 2);
  assert.deepEqual(s.picks, []);
});

test("AVD-07 designCandidates: nothing usable twice falls back to one kit pick; all-custom gets a kit", async () => {
  const s = stub("I cannot draw.", "Still no JSON.");
  assert.deepEqual(await designCandidates({ prompt: "reads receipts" }, s), [{ name: "Cloud", shape: "cloud", color: "#FFFFFF", why: "A kit shape picked for the description." }]);
  assert.match(s.calls[1][2].content, /- the reply had no JSON candidates\n- there must be exactly 3 candidates \(got 0\)/);
  assert.deepEqual(s.picks, ["reads receipts"]);
  const t = stub([cust("A"), cust("B"), cust("C")], [cust("A"), cust("B"), cust("C")]);
  const out = await designCandidates({ prompt: "y" }, t);
  assert.match(t.calls[1][2].content, /at least one candidate must be a kit shape/);
  assert.deepEqual(out.map((c) => c.shape), ["custom", "custom", "cloud"]);
});

test("AVD-08 designCandidates with an image: the image block leads the turn, candidate 1 must be custom", async () => {
  const s = stub([kit("A"), cust("B"), cust("C")], [cust("Subject", GOOD.cat, "#FF6700"), kit("A"), cust("C")]);
  const out = await designCandidates({ prompt: "keep the ears", image: IMG }, s);
  assert.deepEqual(out[0], cust("Subject", GOOD.cat, "#FF6700"));
  const [first] = s.calls[0];
  assert.deepEqual(first.content[0], IMG);
  assert.equal(first.content[1].type, "text");
  assert.match(first.content[1].text, /Candidate 1 MUST be that subject made into an avatar: shape "custom"/);
  assert.match(s.calls[1][2].content, /candidate 1 must be a custom outline tracing the image's subject/);
});

test("AVD-09 POST /avatar/design under MOCK: three valid candidates, a kit one without an image, a custom first with one", async () => {
  const r = await postJson("/avatar/design", { prompt: "reads my receipts and files my BIR forms", name: "Tax Fox" });
  assert.equal(r.status, 200, r.text);
  assert.deepEqual(Object.keys(r.json), ["ok", "candidates"]);
  const { candidates } = r.json;
  assert.equal(candidates.length, 3);
  for (const c of candidates) {
    assert.deepEqual(validateAvatar(c), [], c.name);
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
  for (const c of w.json.candidates) assert.deepEqual(validateAvatar(c), []);
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

test("AVD-13 the system prompt names the engine's rules and every kit shape", () => {
  // the prompt is a template file (the Rust hub reads it too): its kit list is KIT's
  assert.deepEqual(DESIGN_SYSTEM.split("\n").filter((l) => /^- [a-z]+: /.test(l) && KIT_SHAPES.includes(l.slice(2).split(":")[0])), KIT_SHAPES.map((k) => `- ${k}: ${KIT[k]}`));
  for (const s of ["no face", "no holes", "y DOWN", "55-80%", "about 10 units", "M, L, C, Q and Z", "JSON only", "By default every candidate is #FFFFFF"]) assert.ok(DESIGN_SYSTEM.includes(s), s);
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

test("AVD-15 rich avatars: MOCK rich designs pass the checks; a bot keeps its layers and eyes, a new outline or a kit shape drops them, across a restart", async () => {
  const { MOCK_RICH, validateAvatar, layerErrors } = await import("../src/avatar-shape.mjs");
  for (const c of MOCK_RICH) assert.deepEqual(validateAvatar(c), [], c.name);
  await server.stop();
  server = await startServer(dir, { env: env() });
  const d = await postJson("/avatar/design", { prompt: "a green giant", style: "rich" });
  assert.equal(d.status, 200, d.text);
  assert.equal(d.json.candidates.length, 3);
  assert.ok(d.json.candidates.every((c) => c.shape === "custom" && c.layers.length && /^#[0-9A-F]{6}$/i.test(typeof c.eyes === "string" ? c.eyes : c.eyes.c)));
  // the checks
  assert.deepEqual(layerErrors([], "#FFFFFF"), ["a rich avatar needs at least one layer (the face)"]);
  assert.match(layerErrors([{ path: "M10 10 L20 10 L20 20 Z", fill: "red" }], "#FFFFFF").join(";"), /fill must be #RRGGBB/);
  assert.match(layerErrors([{ path: "M10 10 L20 10 L20 20 Z", fill: "#FF0000", opacity: 2 }], "#FFFFFF").join(";"), /opacity must be 0..1/);
  assert.match(layerErrors([{ path: "M10 10 H20 V20 Z", fill: "#FF0000" }], "#FFFFFF").join(";"), /only M\/L\/C\/Q\/Z/);
  assert.match(layerErrors([{ path: "M10 10 L20 10 L20 20 Z", fill: "#FF0000" }], "nope").join(";"), /eyes must be #RRGGBB/);
  assert.match(validateAvatar({ shape: "heart", color: "#FF0000", layers: MOCK_RICH[0].layers, eyes: "#FFFFFF" }).join(";"), /only a custom outline can have layers/);
  // stored on a bot
  const r = MOCK_RICH[0];
  const made = await postJson("/bot/new", { name: "Giant", shape: "custom", color: r.color, outline: r.outline, layers: r.layers, eyes: r.eyes });
  assert.equal(made.status, 200, made.text);
  const id = made.json.bot.id;
  let b = await botOf(id);
  assert.deepEqual([b.layers, b.eyes], [r.layers, r.eyes]);
  assert.deepEqual((await threads()).list.find((t) => t.id === b.threadId).bot.layers, r.layers);
  // a colour alone keeps them
  await postJson("/bot/update", { id, color: "#225522" });
  assert.deepEqual((b = await botOf(id)).layers, r.layers);
  // across a restart
  await server.stop();
  server = await startServer(dir, { env: env() });
  assert.deepEqual([(b = await botOf(id)).layers, b.eyes], [r.layers, r.eyes]);
  // a new outline without layers drops them; so does a kit shape
  await postJson("/bot/update", { id, outline: GOOD.cat });
  assert.ok(!("layers" in (b = await botOf(id))) && !("eyes" in b));
  await postJson("/bot/update", { id, outline: r.outline, layers: r.layers, eyes: r.eyes });
  assert.deepEqual((await botOf(id)).layers, r.layers);
  await postJson("/bot/update", { id, shape: "hex" });
  assert.ok(!("layers" in (b = await botOf(id))) && b.shape === "hex");
});

test("AVD-16 /avatar/picture under MOCK: the reference (or a fixed square) as the draft, in the avatar folder; the same image rules", async () => {
  const keys = ["ok", "via", "uri", "name", "w", "h"];
  const plain = await postJson("/avatar/picture", { prompt: "a green giant" });
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

test("AVD-18 rich self-check: the designs drawn flat go back to the model once; a fix that passes replaces its candidate, one that fails keeps the first; GEN_AVATAR_SELFCHECK=0 skips it", async () => {
  const { MOCK_RICH } = await import("../src/avatar-shape.mjs");
  const say2 = (cands) => say(JSON.stringify({ candidates: cands }));
  const fixedEyes = { c: "#58D63A", at: [50, 58], gap: 20, s: 1.2, rim: "#111111", rw: 0.3 };
  const fix = [{ ...MOCK_RICH[0], eyes: fixedEyes }, { ...MOCK_RICH[1], layers: [] }, { ...MOCK_RICH[2], name: "Cat Two" }];
  // the server from AVD-17 runs with MOCK off
  fake.reset();
  fake.script("rich-check", [say2(MOCK_RICH), say2(fix)]);
  const r = await postJson("/avatar/design", { prompt: "[[script:rich-check]] a green giant", style: "rich" });
  assert.equal(r.status, 200, r.text);
  assert.deepEqual(r.json.candidates.map((c) => c.name), ["Green Giant", "Mask Hero", "Cat Two"]);
  assert.deepEqual(r.json.candidates[0].eyes, fixedEyes, "a fix that passes replaces its candidate");
  assert.deepEqual(r.json.candidates[1], MOCK_RICH[1], "a fix without layers keeps the first");
  const calls = fake.calls();
  assert.equal(calls.length, 2);
  assert.equal(calls[1].body.max_tokens, 12000);
  const [first, said, look] = calls[1].body.messages;
  assert.deepEqual(first, calls[0].body.messages[0]);
  assert.deepEqual(JSON.parse(said.content), { candidates: MOCK_RICH });
  assert.deepEqual(look.content.map((b) => b.type), ["text", "image", "text", "image", "text", "image", "text"]);
  assert.equal(look.content[0].text, "Design 1 (Green Giant):");
  assert.equal(look.content[1].source.media_type, "image/png");
  assert.equal(look.content[6].text, 'Here are your three designs drawn flat. For each, does it read as the request "[[script:rich-check]] a green giant"? Fix the outline, layers and eyes where it doesn\'t (missing hair volume, eyes off the lenses or band, features covering the eyes). Reply with all 3, JSON only.');
  assert.deepEqual(readdirSync(avatars).filter((f) => f.startsWith("check-")), [], "the previews are cleaned up");
  // switched off: one call
  await server.stop();
  server = await startServer(dir, { env: env({ MOCK: undefined, GEN_AVATAR_SELFCHECK: "0" }) });
  fake.reset();
  fake.script("rich-off", [say2(MOCK_RICH)]);
  assert.equal((await postJson("/avatar/design", { prompt: "[[script:rich-off]] x", style: "rich" })).status, 200);
  assert.equal(fake.calls().length, 1);
});

test("AVD-19 rich eyes as a placement: checked, stored, carried by /threads and a restart; compact layers parse; the preview draws them", async () => {
  const { eyeErrors, parseCandidates, previewSvg, MOCK_RICH, validateAvatar } = await import("../src/avatar-shape.mjs");
  const r = MOCK_RICH[0];
  const eyes = { c: "#58D63A", at: [50, 58], gap: 20, s: 1.2, rim: "#111111", rw: 0.3 };
  assert.deepEqual(validateAvatar({ ...r, eyes }), []);
  assert.deepEqual(eyeErrors({ c: "red", at: [500, 5], gap: 0, s: 3, rim: 1, rw: 2 }, r.outline), [
    "eyes.c must be #RRGGBB", "eyes.at must sit inside the outline's box", "eyes.gap must be a number 0..100", "eyes.s must be 0.6..2.2", "eyes.rim must be #RRGGBB", "eyes.rw must be 0..1.2",
  ]);
  assert.deepEqual(eyeErrors([1], r.outline), ["eyes must be #RRGGBB or {c, at?, gap?, s?, rim?, rw?}"]);
  assert.deepEqual(eyeErrors({ c: "#FFFFFF", at: [1] }, r.outline), ["eyes.at must be [x, y]"]);
  // the compact layer form, and the placement's colours tidied
  const [c] = parseCandidates(JSON.stringify([{ shape: "custom", outline: r.outline, color: "#3e8e2f", layers: [["M30 60 L70 60 L50 80 Z", "#f5ebd6", 0.5], ["M1 1 L9 1 L9 9 Z", "#000000"]], eyes: { c: "#58d63a", at: [50, 58], rim: " #111111 ", junk: 1 } }]));
  assert.deepEqual(c.layers, [{ path: "M30 60 L70 60 L50 80 Z", fill: "#F5EBD6", opacity: 0.5 }, { path: "M1 1 L9 1 L9 9 Z", fill: "#000000" }]);
  assert.deepEqual(c.eyes, { c: "#58D63A", at: [50, 58], rim: "#111111" });
  const svg = previewSvg({ ...r, eyes });
  assert.match(svg, /^<svg [^>]*viewBox="0 0 100 100"/);
  assert.equal((svg.match(/<rect x=/g) || []).length, 4, "two rims and two eyes");
  assert.ok(svg.includes('fill="#111111"') && svg.includes('fill="#58D63A"'));
  // a bot keeps them
  const made = await postJson("/bot/new", { name: "Placed", shape: "custom", color: r.color, outline: r.outline, layers: r.layers, eyes });
  assert.equal(made.status, 200, made.text);
  const id = made.json.bot.id;
  assert.deepEqual(made.json.bot.eyes, eyes);
  bad(await postJson("/bot/update", { id, eyes: { c: "#58D63A", s: 9 } }), "eyes.s must be 0.6..2.2");
  // eyes alone update a rich bot
  const moved = { ...eyes, at: [50, 60] };
  assert.equal((await postJson("/bot/update", { id, eyes: moved })).status, 200);
  assert.deepEqual((await botOf(id)).eyes, moved);
  await server.stop();
  server = await startServer(dir, { env: env() });
  const b = await botOf(id);
  assert.deepEqual([b.layers, b.eyes], [r.layers, moved]);
  assert.deepEqual((await threads()).list.find((t) => t.id === b.threadId).bot.eyes, moved);
});

test("AVD-20 rich parts: checked (2% pieces, z, count), parsed, drawn in z order, stored and carried across a restart; a new outline or a kit shape drops them", async () => {
  const { MOCK_RICH, parseCandidates, partErrors, previewSvg, validateAvatar } = await import("../src/avatar-shape.mjs");
  const hat = MOCK_RICH.find((c) => c.parts);
  assert.ok(hat, "a multi-part mock");
  assert.deepEqual(validateAvatar(hat), []);
  const brim = { outline: "M30 40 L70 40 L70 46 L30 46 Z", color: "#E8C85A" };
  assert.deepEqual(partErrors([brim]), ["part 1: outline's width/height is 6.67; it must be between 0.4 and 2.5"]);
  const small = { outline: "M44 44 L56 44 L56 56 L44 56 Z", color: "#E8C85A" };
  assert.deepEqual(partErrors([small]), ["part 1: outline covers 1% of the box; it must cover at least 2%"]);
  const antler = { outline: "M40 30 L60 30 L60 50 L40 50 Z", color: "#7A4A22", z: -1 };
  assert.deepEqual(partErrors([antler]), []);
  assert.deepEqual(partErrors([{ ...antler, z: 2, color: "brown", layers: [{ path: "M1 1 Z", fill: "#000000" }] }]), [
    "part 1's color must be #RRGGBB", "part 1: layer 1 encloses no area", "part 1's z must be -1..1",
  ]);
  assert.deepEqual(partErrors(Array(7).fill(antler)), ["at most 6 parts"]);
  assert.deepEqual(partErrors("x"), ["parts must be a list"]);
  assert.deepEqual(partErrors([5]), ["part 1 must be {outline, color}"]);
  assert.deepEqual(validateAvatar({ shape: "heart", color: "#FF0000", parts: [antler] }), ["only a custom outline can have parts"]);
  // parsed (tidied, compact layers), and drawn: behind, the head, the eyes, in front
  const [c] = parseCandidates(JSON.stringify([{ ...hat, parts: [{ outline: ` ${antler.outline}\n`, colour: "#7a4a22", z: -1, layers: [["M42 32 L58 32 L50 40 Z", "#ffffff"]] }, ...hat.parts, 5] }]));
  assert.deepEqual(c.parts[0], { outline: antler.outline, color: "#7A4A22", layers: [{ path: "M42 32 L58 32 L50 40 Z", fill: "#FFFFFF" }], z: -1 });
  assert.equal(c.parts.length, 2);
  const svg = previewSvg(c);
  const at = (s) => svg.indexOf(s);
  assert.ok(at(antler.outline) < at(hat.outline) && at(hat.outline) < at("<rect x=") && at("<rect x=") < at(hat.parts[0].outline), svg);
  // a bot keeps them
  const made = await postJson("/bot/new", { name: "Luffy", shape: "custom", color: hat.color, outline: hat.outline, layers: hat.layers, eyes: hat.eyes, parts: hat.parts });
  assert.equal(made.status, 200, made.text);
  const id = made.json.bot.id;
  assert.deepEqual(made.json.bot.parts, hat.parts);
  bad(await postJson("/bot/update", { id, parts: [{ ...antler, z: 5 }] }), "part 1's z must be -1..1");
  await postJson("/bot/update", { id, color: "#222222" });
  assert.deepEqual((await botOf(id)).parts, hat.parts, "a colour alone keeps them");
  await server.stop();
  server = await startServer(dir, { env: env() });
  let b = await botOf(id);
  assert.deepEqual([b.parts, b.layers, b.eyes], [hat.parts, hat.layers, hat.eyes]);
  assert.deepEqual((await threads()).list.find((t) => t.id === b.threadId).bot.parts, hat.parts);
  await postJson("/bot/update", { id, outline: hat.outline, layers: hat.layers, eyes: hat.eyes });
  assert.ok(!("parts" in (b = await botOf(id))), "a new outline without parts drops them");
  await postJson("/bot/update", { id, outline: hat.outline, layers: hat.layers, eyes: hat.eyes, parts: [antler] });
  assert.deepEqual((await botOf(id)).parts, [antler]);
  await postJson("/bot/update", { id, shape: "hex" });
  assert.ok(!("parts" in (b = await botOf(id))) && b.shape === "hex");
});

test("AVD-21 /avatar/image/import copies a file the app's open panel chose into the avatar folder", async () => {
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
