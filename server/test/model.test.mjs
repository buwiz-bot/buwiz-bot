// The model client (Rust broker plan, phase 5 item 5.3) as Node drives it,
// with MOCK off against the scripted fake opencodex
// (fixtures/fake-opencodex.mjs): routing and effort, /models, the legacy
// generate path (compiler-error, truncation and no-module retries), the
// OpenAI-compatible path, the SDK's retries and the error text a job
// shows. The recorded requests are the contract a port must send too.
import assert from "node:assert/strict";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { say, startFakeOpencodex, use } from "./fixtures/fake-opencodex.mjs";
import { modelEnv, postJson, request, startServer, tempDir, waitFor } from "./helpers.mjs";

const MODEL = "claude-haiku-5-5";
const CODE = 'import { Text } from "nsh";\n\nexport default function R() {\n  return <Text>hi</Text>;\n}';
const FAILING = `${CODE}\n// nsh-stub:fail`;
const NSH_ERR = "error TS2304: Cannot find name 'nope'.\n";

let fake;

before(async () => {
  fake = await startFakeOpencodex();
});

after(async () => {
  await fake?.close();
});

// a broker on its own data dir, talking to the fake
async function broker(label, extra = {}) {
  const dir = tempDir(label);

  return { dir, server: await startServer(dir, { env: modelEnv(fake, join(dir, "stubs.log"), extra) }) };
}

const status = (id) => waitFor(async () => {
  const j = (await request("GET", `/status?id=${id}`)).json;

  return j.done && j;
}, 30000);
// a /chat that runs to its end: its last /status
const ask = async (prompt, thread) => {
  const r = await postJson("/chat", { prompt, ...(thread ? { thread } : {}) });

  assert.equal(r.status, 200, r.text);

  return status(r.json.id);
};
const turnsOf = async (thread) => (await request("GET", `/turns?thread=${thread}`)).json.turns;
const newThread = async () => (await postJson("/thread/new", {})).json.id;
const brief = (c) => ({ path: c.path, stream: c.stream, model: c.body.model, output_config: c.body.output_config, reasoning_effort: c.body.reasoning_effort });

test("MODEL-01 routing: the default goes to ANTHROPIC_BASE_URL, a chosen model to opencodex with --fast and effort; labels credit the bot", async () => {
  const { server } = await broker("model-route", { ANTHROPIC_BASE_URL: `${fake.url}/direct` });

  try {
    const plain = await newThread();

    fake.reset();
    await ask("hello", plain);
    assert.deepEqual(fake.calls().map(brief), [{ path: "/direct/v1/messages", stream: true, model: MODEL, output_config: undefined, reasoning_effort: undefined }]);
    assert.deepEqual(fake.calls()[0].headers, { "content-type": "application/json", "x-api-key": "opencodex", "anthropic-version": "2023-06-01" });
    assert.equal((await turnsOf(plain)).at(-1).label, MODEL);
    // the user's model in /prefs
    await postJson("/prefs", { model: "gpt-5.5", effort: "low", fast: true });
    fake.reset();
    await ask("hello again", plain);
    assert.deepEqual(fake.calls().map(brief), [{ path: "/v1/messages", stream: true, model: "gpt-5.5--fast", output_config: { effort: "low" }, reasoning_effort: "low" }]);
    assert.equal((await turnsOf(plain)).at(-1).label, "gpt-5.5--fast");
    // a bot's own model; its effort; the label is the bot's name
    const ada = (await postJson("/bot/new", { name: "Ada" })).json.bot;

    await postJson("/bot/update", { id: ada.id, model: "claude-x", effort: "high", fast: false });
    fake.reset();
    await ask("hi Ada", ada.threadId);
    assert.deepEqual(fake.calls().map(brief), [{ path: "/v1/messages", stream: true, model: "claude-x", output_config: { effort: "high" }, reasoning_effort: "high" }]);
    assert.equal((await turnsOf(ada.threadId)).at(-1).label, "Ada");
    // a bot on the default model keeps its own effort and fast with the user's model
    const bo = (await postJson("/bot/new", { name: "Bo" })).json.bot;

    await postJson("/bot/update", { id: bo.id, effort: "max", fast: false });
    fake.reset();
    await ask("hi Bo", bo.threadId);
    assert.deepEqual(fake.calls().map(brief), [{ path: "/v1/messages", stream: true, model: "gpt-5.5", output_config: { effort: "max" }, reasoning_effort: "max" }]);
    // no model anywhere: the default route, with the bot's effort
    await postJson("/prefs", { model: "", effort: "", fast: false });
    fake.reset();
    await ask("hi Bo again", bo.threadId);
    assert.deepEqual(fake.calls().map(brief), [{ path: "/direct/v1/messages", stream: true, model: MODEL, output_config: { effort: "max" }, reasoning_effort: "max" }]);
    // BUG: /retry runs outside the thread's model choice (no modelCtx.run in
    // routes/chat.mjs), so a bot's retry goes to the default model and route
    fake.script("route-card", [use("generate_app", { title: "Card", code: CODE }), say("Built."), say(`\`\`\`tsx\n${CODE}\n\`\`\``)]);
    await ask("[[script:route-card]] make a card", ada.threadId);
    const card = (await turnsOf(ada.threadId)).find((t) => t.name);

    assert.equal(card.label, "Ada");
    fake.clear();
    await status((await postJson("/retry", { name: card.name })).json.id);
    assert.deepEqual(fake.calls().map(brief), [{ path: "/direct/v1/messages", stream: true, model: MODEL, output_config: undefined, reasoning_effort: undefined }]);
    assert.equal((await turnsOf(ada.threadId)).at(-1).label, MODEL);
  } finally {
    await server.stop();
  }
});

test("MODEL-02 /models folds --fast twins, names efforts and image models, and the route from opencodex's config", async () => {
  const home = tempDir("model-home");

  mkdirSync(join(home, ".opencodex"));
  writeFileSync(join(home, ".opencodex", "config.json"), JSON.stringify({ defaultProvider: "openai", providers: { openai: { authMode: "chatgpt", label: "ChatGPT" }, "anthropic-apikey": { authMode: "apikey" } } }));
  fake.models = [
    { id: MODEL, owned_by: "anthropic-apikey", reasoning_efforts: [{ value: "low", label: "Low Effort" }, { value: "high", label: "High" }], reasoning_effort: "low" },
    { id: `${MODEL}--fast`, owned_by: "anthropic-apikey" },
    { id: "gpt-5.5", owned_by: "openai", reasoning_efforts: [{ value: "medium" }] },
    { id: "openrouter/qwen-3", owned_by: "openrouter" },
    { id: "painter", owned_by: "xai", capabilities: { output_modalities: ["text", "image"] } },
    { id: "plain" },
  ];
  const { server } = await broker("model-list", { HOME: home });

  try {
    fake.reset();
    const r = await request("GET", "/models");

    assert.equal(r.status, 200);
    assert.deepEqual(r.json, {
      ok: true,
      default: MODEL,
      models: [
        // only authMode "key" (or none, on a key-like name) reads as an API
        // connection: "apikey" counts as a subscription
        { id: MODEL, owner: "anthropic-apikey", fast: true, efforts: [{ value: "low", label: "Low" }, { value: "high", label: "High" }], defaultEffort: "low", image: false, connection: "anthropic-apikey", group: "Anthropic", subscription: true },
        { id: "gpt-5.5", owner: "openai", fast: false, efforts: [{ value: "medium", label: "medium" }], defaultEffort: "", image: true, connection: "openai", group: "ChatGPT", subscription: true },
        { id: "openrouter/qwen-3", owner: "openrouter", fast: false, efforts: [], defaultEffort: "", image: false, connection: "openrouter", group: "OpenRouter", subscription: true },
        { id: "painter", owner: "xai", fast: false, efforts: [], defaultEffort: "", image: true, connection: "xai", group: "xAI", subscription: true },
        { id: "plain", owner: "", fast: false, efforts: [], defaultEffort: "", image: false, connection: "openai", group: "ChatGPT", subscription: true },
      ],
    });
    assert.deepEqual(fake.requests.map((x) => [x.method, x.path]), [["GET", "/v1/models"], ["GET", "/v1/models"]]);
    fake.modelsFail = true;
    assert.deepEqual((await request("GET", "/models")).json, { ok: false, default: MODEL, models: [], error: `opencodex at ${fake.url} unreachable` });
  } finally {
    fake.modelsFail = false;
    fake.models = [];
    await server.stop();
  }
});

test("MODEL-03 the legacy generate path (/retry) without MOCK: compiler error, truncation and no module retry with their own follow-ups; a dead API falls back to the mock", async () => {
  const { server } = await broker("model-legacy");

  try {
    const thread = await newThread();
    const card = (k) => [use("generate_app", { title: `Card ${k}`, code: CODE }), say("Built.")];

    fake.script("legacy-fix", [...card(1), say(`First try\n\`\`\`tsx\n${FAILING}\n\`\`\``), say(`Fixed it\n\`\`\`tsx\n${CODE}\n\`\`\``)]);
    fake.script("legacy-trunc", [...card(2), { content: [{ type: "text", text: "```tsx\nexport default function R() { return <" }], stop_reason: "max_tokens" }, say(`\`\`\`tsx\n${CODE}\n\`\`\``)]);
    fake.script("legacy-nomod", [...card(3), say("I would rather chat."), say(`\`\`\`tsx\n${CODE}\n\`\`\``)]);
    fake.script("legacy-dead", [...card(4), { status: 400, error: "bad request" }]);
    const names = {};

    for (const k of ["fix", "trunc", "nomod", "dead"]) {
      await ask(`[[script:legacy-${k}]] build ${k}`, thread);
      names[k] = (await turnsOf(thread)).filter((t) => t.name).at(-1).name;
    }

    const retry = async (k) => {
      fake.clear();
      const st = await status((await postJson("/retry", { name: names[k] })).json.id);

      return { st, calls: fake.calls(), turn: (await turnsOf(thread)).at(-1) };
    };
    const prompt = (k) => `[[script:legacy-${k}]] build ${k}`;
    const fix = await retry("fix");

    assert.equal(fix.calls.length, 2);
    assert.deepEqual(Object.keys(fix.calls[0].body), ["model", "max_tokens", "system", "messages", "stream"]);
    assert.deepEqual([fix.calls[0].body.model, fix.calls[0].body.max_tokens], [MODEL, 20000]);
    assert.match(fix.calls[0].body.system, new RegExp(`^You write Native Shell \\(nsh\\) modules: ONE TSX file, rendered as native UI \\(UIKit/AppKit\\)\\. You are writing module ${names.fix}\\.\\n`));
    assert.deepEqual(fix.calls[0].body.messages, [{ role: "user", content: prompt("fix") }]);
    assert.deepEqual(fix.calls[1].body.messages, [
      { role: "user", content: prompt("fix") },
      { role: "assistant", content: FAILING },
      { role: "user", content: `The nsh compiler rejected it:\n${NSH_ERR}\nReturn a corrected module only.` },
    ]);
    assert.equal(fix.st.result, `${names.fix}.tsx via ${MODEL}`);
    assert.deepEqual([fix.turn.name, fix.turn.label, fix.turn.caption, fix.turn.retries, fix.turn.v], [names.fix, MODEL, "Fixed it", 1, 2]);
    const trunc = await retry("trunc");

    assert.deepEqual(trunc.calls[1].body.messages, [
      { role: "user", content: prompt("trunc") },
      { role: "user", content: "Your reply was cut off at the token limit. Return the SAME module but shorter: fewer comments, tighter code, no extras." },
    ]);
    assert.deepEqual([trunc.turn.label, trunc.turn.caption, trunc.turn.v], [MODEL, "", 2]);
    const nomod = await retry("nomod");

    assert.deepEqual(nomod.calls[1].body.messages, [
      { role: "user", content: prompt("nomod") },
      { role: "user", content: "Your reply had no module. Reply with ONE fenced ```tsx block containing the complete module (it must include export default) and nothing else." },
    ]);
    const dead = await retry("dead");

    assert.equal(dead.calls.length, 1, "an API error is not retried by the generator");
    assert.equal(dead.st.result, `${names.dead}.tsx via mock-fallback`);
    assert.deepEqual(dead.st.events.map((e) => e.phase), ["coding", "fallback", "checking", "publishing", "done"]);
    assert.deepEqual([dead.turn.label, dead.turn.caption, dead.turn.retries], ["mock-fallback", "", 1]);
  } finally {
    await server.stop();
  }
});

test("MODEL-04 the OpenAI-compatible path: the classifier, a chat reply and a module over chat/completions with the key", async () => {
  const { server } = await broker("model-openai", { GEN_PROVIDER: "openai", GEN_BASE_URL: `${fake.url}/v1`, OPENROUTER_API_KEY: "test-key" });
  const OAI = "openai/gpt-4o-mini";

  try {
    assert.equal((await request("GET", "/threads")).json.model, OAI);
    const thread = await newThread();

    fake.script("oai-chat", [{ ...say("Hi from the fake."), classify: "CHAT" }]);
    fake.reset();
    const chat = await ask("[[script:oai-chat]] hello", thread);

    assert.equal(chat.detail, "replied");
    const [cls, reply] = fake.calls();

    assert.equal(cls.path, "/v1/chat/completions");
    assert.deepEqual(cls.headers, { "content-type": "application/json", authorization: "Bearer test-key" });
    assert.deepEqual(cls.body, {
      model: OAI,
      messages: [
        { role: "system", content: 'Classify the user message for a generative-UI chat app. Reply with exactly one word: UI or CHAT.\nUI = the user wants an interface, widget, app, card, or game built/rendered — including terse requests that name a thing to build ("pomodoro timer", "make a calculator").\nCHAT = greetings, questions, discussion, feedback, or anything that only wants a text answer.' },
        { role: "user", content: "[[script:oai-chat]] hello" },
      ],
      max_tokens: 8,
    });
    // BUG: chatReply's system prompt never reaches the OpenAI path (only
    // the Anthropic call passes `system`)
    assert.deepEqual(reply.body, { model: OAI, messages: [{ role: "user", content: "[[script:oai-chat]] hello" }], max_tokens: 1024 });
    assert.deepEqual((await turnsOf(thread)).map((t) => [t.role, t.text, t.label]), [["user", "[[script:oai-chat]] hello", ""], ["ai", "Hi from the fake.", OAI]]);
    fake.script("oai-ui", [{ classify: "UI", content: [{ type: "text", text: "```tsx\nexport default function R() { return <" }], stop_reason: "max_tokens" }, say(`A card\n\`\`\`tsx\n${CODE}\n\`\`\``)]);
    fake.reset();
    const ui = await ask("[[script:oai-ui]] a card", thread);
    const name = /^(R\d+)\.tsx via /.exec(ui.result)[1];

    assert.equal(ui.result, `${name}.tsx via ${OAI}`);
    const [, first, second] = fake.calls();

    assert.deepEqual(Object.keys(first.body), ["model", "messages", "temperature", "max_tokens"]);
    assert.deepEqual([first.body.temperature, first.body.max_tokens, first.body.messages[0].role], [0.4, 20000, "system"]);
    assert.match(first.body.messages[0].content, new RegExp(`You are writing module ${name}\\.`));
    assert.deepEqual(second.body.messages.slice(1), [
      { role: "user", content: "[[script:oai-ui]] a card" },
      { role: "user", content: "Your reply was cut off at the token limit. Return the SAME module but shorter: fewer comments, tighter code, no extras." },
    ]);
    const card = (await turnsOf(thread)).at(-1);

    assert.deepEqual([card.name, card.label, card.caption, card.v], [name, OAI, "A card", 1]);
    // an HTTP error is the llm-api error: no retry, the mock instead
    fake.script("oai-dead", [{ classify: "UI", status: 500, error: "down" }]);
    fake.reset();
    const dead = await ask("[[script:oai-dead]] another", thread);

    assert.match(dead.result, /^R\d+\.tsx via mock-fallback$/);
    assert.equal(fake.calls().length, 2);
  } finally {
    await server.stop();
  }
});

test("MODEL-05 the SDK retries 429 and 5xx twice; a stream that keeps failing turns streaming off and the job shows the API's error", async () => {
  const { server } = await broker("model-retry");

  try {
    const thread = await newThread();

    fake.reset();
    await ask("[[script:error-429]] hi", thread);
    assert.deepEqual(fake.calls().map((c) => c.stream), [true, true, true]);
    assert.equal((await turnsOf(thread)).at(-1).text, "After the retries.");
    fake.reset();
    const st = await ask("[[script:error-500]] hi", thread);
    const err = 'llm-api 500: 500 {"type":"error","error":{"type":"api_error","message":"upstream exploded"}}';

    // BUG: a 5xx on the first streamed round reads as "streaming
    // unavailable": streaming stays off for the process from then on
    assert.deepEqual(fake.calls().map((c) => c.stream), [true, true, true, false, false, false]);
    assert.deepEqual([st.phase, st.detail, st.result], ["done", `generator error — ${err}`, null]);
    assert.deepEqual(st.events.at(-1), { seq: st.seq, phase: "done", detail: `generator error — ${err}` });
    assert.deepEqual((await turnsOf(thread)).at(-1).text, "[[script:error-500]] hi", "the prompt stays, no reply");
    fake.reset();
    await ask("[[script:text]] again", thread);
    assert.deepEqual(fake.calls().map((c) => c.stream), [false]);
  } finally {
    await server.stop();
  }
});
