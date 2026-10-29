// The tool loop (Rust broker plan, phase 5 item 5.4) as Node runs it, with
// MOCK off against the scripted fake opencodex (fixtures/fake-opencodex.mjs)
// and nsh at the PATH stub: replies, tool rounds and what the model is sent
// back, the generate_app placeholder and card, revise, the eighth round,
// pause_turn, the web-search and streaming fallbacks, API errors, deferred
// apps, a routine's surface, /status and a bot's tools. The streaming and
// server-search switches are process-wide and sticky, so the cases that
// turn them off run on brokers of their own.
import assert from "node:assert/strict";
import { existsSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { say, startFakeOpencodex, use } from "./fixtures/fake-opencodex.mjs";
import { FIXTURE, dbRows, modelEnv, nodeIn, postJson, request, startServer, stubCalls, tempDir, waitFor } from "./helpers.mjs";

const MODEL = "claude-haiku-5-5";
const CODE = 'import { Text } from "nsh";\n\nexport default function R() {\n  return <Text>hi</Text>;\n}\n';
const PHRASES = {
  thinking: ["Thinking it through", "Mulling it over", "Putting it together"],
  working: ["Getting it done", "Making progress", "Working on it", "Handling it", "Chipping away"],
};

const dir = tempDir("agent-loop");
const log = join(dir, "stubs.log");

let fake;
let server;
// Node's own tool definitions (the P7 contract), read from its modules
let DEFS;

before(async () => {
  fake = await startFakeOpencodex();
  DEFS = JSON.parse(nodeIn(tempDir("agent-loop-defs"), `
    const { TOOLS, CLIENT_SEARCH } = await import("./src/tools.mjs");
    const { ROUTINE_TOOLS } = await import("./src/routines.mjs");
    const { BOT_TOOLS } = await import("./src/botctx.mjs");
    const { AGENT_TOOLS } = await import("./src/agents-tools.mjs");
    console.log(JSON.stringify({ TOOLS, CLIENT_SEARCH, ROUTINE_TOOLS, BOT_TOOLS, AGENT_TOOLS }));
  `));
  server = await startServer(dir, { env: modelEnv(fake, log) });
});

after(async () => {
  await server?.stop();
  await own?.stop();
  await fake?.close();
});

const status = (id) => waitFor(async () => {
  const j = (await request("GET", `/status?id=${id}`)).json;

  return j.done && j;
}, 30000);
const send = async (prompt, thread) => {
  const r = await postJson("/chat", { prompt, ...(thread ? { thread } : {}) });

  assert.equal(r.status, 200, r.text);

  return r.json.id;
};
// a /chat that runs to its end: its last /status
const ask = async (prompt, thread) => status(await send(prompt, thread));
const turnsOf = async (thread) => (await request("GET", `/turns?thread=${thread}`)).json.turns;
const newThread = async () => (await postJson("/thread/new", {})).json.id;
const names = (call) => call.body.tools.map((t) => t.name);
const last = (call) => call.body.messages.at(-1);

test("LOOP-01 a text reply: one streamed call with the system prompt and tools; the reply lands as the thread's answer", async () => {
  const thread = await newThread();

  fake.reset();
  const st = await ask("[[script:text]] hello there", thread);
  const [call] = fake.calls();

  assert.equal(fake.calls().length, 1);
  assert.deepEqual(Object.keys(call.body), ["model", "max_tokens", "system", "tools", "messages", "stream"]);
  assert.deepEqual([call.body.model, call.body.max_tokens, call.body.stream], [MODEL, 20000, true]);
  assert.match(call.body.system, /^You are the assistant inside a generative-UI chat app: a chat where you answer in text OR native UI, whichever fits the request\.\n/);
  assert.deepEqual(call.body.tools[0], { type: "web_search_20250305", name: "web_search", max_uses: 3 });
  // Node's TOOLS then ROUTINE_TOOLS, byte for byte
  const n = DEFS.TOOLS.length;

  assert.equal(JSON.stringify(call.body.tools.slice(1, 1 + n)), JSON.stringify(DEFS.TOOLS));
  assert.equal(JSON.stringify(call.body.tools.slice(1 + n, 1 + n + DEFS.ROUTINE_TOOLS.length)), JSON.stringify(DEFS.ROUTINE_TOOLS));
  // BUG: the new prompt goes twice — once as the history's last turn (the
  // user turn is pushed before agentTurn reads the history) and once as
  // the message itself
  assert.deepEqual(call.body.messages, [{ role: "user", content: "[[script:text]] hello there" }, { role: "user", content: "[[script:text]] hello there" }]);
  assert.equal(call.body.tool_choice, undefined);
  assert.deepEqual([st.detail, st.result], ["replied", "reply"]);
  assert.deepEqual((await turnsOf(thread)).map((t) => [t.role, t.text, t.label]), [["user", "[[script:text]] hello there", ""], ["ai", "Hello from the fake.", MODEL]]);
  assert.equal((await request("GET", "/threads")).json.list.find((t) => t.id === thread).title, "[[script:text]] hello there");
});

test("LOOP-02 a tool round then the answer: the streamed message is sent back whole (thinking, tool inputs from input_json_delta, an empty input as {}), then the results", async () => {
  const thread = await newThread();

  fake.script("think-tools", [
    {
      content: [
        { type: "thinking", thinking: "Let me think.", signature: "sig-1" },
        { type: "tool_use", id: "toolu_a", name: "react", input: { emoji: "👍" }, chunks: 4 },
        { type: "tool_use", id: "toolu_b", name: "skill_list", input: {} },
      ],
      stop_reason: "tool_use",
    },
    say("Done reacting."),
  ]);
  fake.reset();
  await ask("[[script:think-tools]] react please", thread);
  const [, second] = fake.calls();

  assert.equal(fake.calls().length, 2);
  assert.deepEqual(second.body.messages.slice(-2), [
    {
      role: "assistant",
      content: [
        { type: "thinking", thinking: "Let me think.", signature: "sig-1" },
        { type: "tool_use", id: "toolu_a", name: "react", input: { emoji: "👍" } },
        { type: "tool_use", id: "toolu_b", name: "skill_list", input: {} },
      ],
    },
    {
      role: "user",
      content: [
        { type: "tool_result", tool_use_id: "toolu_a", content: "reacted 👍 to turn 0" },
        { type: "tool_result", tool_use_id: "toolu_b", content: "no private skills yet" },
      ],
    },
  ]);
  const turns = await turnsOf(thread);

  assert.deepEqual(turns.map((t) => [t.role, t.text, t.reaction]), [["user", "[[script:think-tools]] react please", "👍"], ["ai", "Done reacting.", ""]]);
});

test("LOOP-03 a note written before the tools lands at once as its own message", async () => {
  const thread = await newThread();

  fake.reset();
  await ask("[[script:note-then-tool]] check it", thread);
  assert.deepEqual((await turnsOf(thread)).map((t) => [t.role, t.text, t.label, t.reaction]), [
    ["user", "[[script:note-then-tool]] check it", "", "👀"],
    ["ai", "Checking that for you.", MODEL, ""],
    ["ai", "All checked.", MODEL, ""],
  ]);
  assert.deepEqual(fake.calls()[1].body.messages.at(-2).content[0], { type: "text", text: "Checking that for you." });
});

let card; // LOOP-04's card, revised in LOOP-06
let cardThread;

test("LOOP-04 generate_app: a placeholder titled and sized from the streamed input, then the card with its version; nsh builds and checks it", async () => {
  cardThread = await newThread();
  const input = { title: "Tiny Tip", desc: "Splits a bill", layout: { width: 320, height: 200 }, slug: "tiny-tip", description: "Splits a restaurant bill.", code: CODE };

  // BUG: nothing in Node makes <data>/chunks, so on a data dir without it
  // every chunk copy fails silently (emitChunk); made here to see the copy
  assert.ok(!existsSync(join(dir, "chunks")));
  mkdirSync(join(dir, "chunks"));
  fake.script("gen", [use("generate_app", input, { chunks: 8, delay: 120 }), say("Built it.")]);
  fake.reset();
  const n = stubCalls(log).length;
  const job = await send("[[script:gen]] build a tip calculator", cardThread);
  const placeholder = await waitFor(async () => {
    const t = (await turnsOf(cardThread)).find((x) => x.building && x.art?.w);

    return t;
  }, 15000);

  assert.ok(placeholder, "a building placeholder while the input streams");
  assert.deepEqual({ role: placeholder.role, name: placeholder.name, label: placeholder.label, art: placeholder.art, building: placeholder.building }, {
    role: "ai", name: "", label: MODEL, art: { title: "Tiny Tip", desc: "Splits a bill", w: 320, h: 200 }, building: true,
  });
  const st = await status(job);
  const turns = await turnsOf(cardThread);

  card = turns.find((t) => t.name);
  assert.deepEqual([st.detail, st.result], ["1 card(s)", "cards"]);
  assert.equal(card.id, placeholder.id, "the placeholder becomes the card");
  assert.deepEqual({ name: card.name, v: card.v, label: card.label, caption: card.caption, art: card.art, building: card.building },
    { name: card.name, v: 1, label: MODEL, caption: "", art: { title: "Tiny Tip", desc: "Splits a bill", w: 320, h: 200 }, building: undefined });
  assert.deepEqual(turns.map((t) => t.role), ["user", "ai", "ai"]);
  assert.equal(turns[2].text, "Built it.");
  assert.deepEqual(last(fake.calls()[1]).content, [{ type: "tool_result", tool_use_id: "toolu_generate_app", content: `mounted as ${card.name} v1 — the card is live in the conversation. Library now launches v1; earlier versions stay on the tree.` }]);
  assert.deepEqual(st.events.map((e) => e.phase), ["thinking", "coding", "coding", "checking", "working", "streaming", "done"]);
  const stage = join(dir, ".stage");
  const nsh = stubCalls(log).slice(n).filter((c) => c.bin === "nsh").map((c) => c.argv);

  assert.deepEqual(nsh.slice(0, 2), [
    ["build", join(stage, `check-${card.name}.tsx`), "--out", join(stage, `out-${card.name}`)],
    ["check", join(stage, `check-${card.name}.tsx`)],
  ]);
  assert.deepEqual(nsh.slice(2).sort(), [
    ["build", join(dir, "generated", card.name, "tiny-tip.v1.tsx"), "--out", join(stage, `chunk-${card.name}v1`)],
    ["build", join(dir, "generated", `${card.name}.tsx`), "--out", join(stage, `chunk-${card.name}`)],
  ].sort());
  assert.ok(await waitFor(() => existsSync(join(dir, "chunks", `${card.name}v1.js`)) && existsSync(join(dir, "chunks", `${card.name}.js`))), "both chunks copied");
  assert.deepEqual(dbRows(dir, "SELECT title, file, description FROM apps WHERE name=?", card.name), [{ title: "Tiny Tip", file: "tiny-tip.tsx", description: "Splits a restaurant bill." }]);
  // BUG: toolGenerateApp records `job.prompt`, which no job carries, so a
  // tool-made version's prompt is the caption or NULL
  assert.deepEqual(dbRows(dir, "SELECT v, parent, prompt FROM versions WHERE name=?", card.name), [{ v: 1, parent: null, prompt: null }]);
});

test("LOOP-05 an nsh rejection drops the placeholder and answers is_error; the name is used up", async () => {
  const thread = await newThread();

  fake.script("gen-bad", [
    use("generate_app", { title: "Bad", code: `${CODE}// nsh-stub:fail\n` }, { id: "toolu_bad" }),
    use("generate_app", { title: "Good", code: CODE }, { id: "toolu_good" }),
    say("Second time lucky."),
  ]);
  fake.reset();
  await ask("[[script:gen-bad]] build it", thread);
  const [, second, third] = fake.calls();
  const bad = /^nsh check rejected (R\d+):/.exec(last(second).content[0].content)?.[1];

  assert.deepEqual(last(second).content, [{
    type: "tool_result", tool_use_id: "toolu_bad", is_error: true,
    content: `nsh check rejected ${bad}:\nerror TS2304: Cannot find name 'nope'.\n\nFix the module and call generate_app again with the corrected code.`,
  }]);
  const good = `R${Number(bad.slice(1)) + 1}`;

  assert.match(last(third).content[0].content, new RegExp(`^mounted as ${good} v1 `));
  const turns = await turnsOf(thread);

  assert.deepEqual(turns.map((t) => [t.role, t.name, t.building]), [["user", "", undefined], ["ai", good, undefined], ["ai", "", undefined]]);
  assert.ok(!existsSync(join(dir, "generated", `${bad}.tsx`)));
});

test("LOOP-06 revise_app covers the card it revises while the code streams; the new version lands as a new card", async () => {
  const code = CODE.replace("hi", "hello");

  fake.script("revise", [use("revise_app", { name: card.name, code, description: "Says hello." }, { chunks: 8, delay: 120 }), say("Revised.")]);
  fake.reset();
  const job = await send("[[script:revise]] say hello instead", cardThread);
  const covered = await waitFor(async () => (await turnsOf(cardThread)).find((t) => t.id === card.id && t.revising), 15000);

  assert.ok(covered, "the card shows revising");
  await status(job);
  const turns = await turnsOf(cardThread);
  const cards = turns.filter((t) => t.name === card.name);

  assert.deepEqual(cards.map((t) => [t.id === card.id, t.v, t.revising]), [[true, 1, undefined], [false, 2, undefined]]);
  assert.deepEqual(last(fake.calls()[1]).content[0].content, `mounted as ${card.name} v2 — the card is live in the conversation. Library now launches v2; earlier versions stay on the tree.`);
  assert.deepEqual(dbRows(dir, "SELECT v, parent FROM versions WHERE name=? ORDER BY v", card.name), [{ v: 1, parent: null }, { v: 2, parent: 1 }]);
  assert.equal(dbRows(dir, "SELECT description FROM apps WHERE name=?", card.name)[0].description, "Says hello.");
});

test("LOOP-07 the eighth round sends tool_choice none", async () => {
  const thread = await newThread();

  fake.script("rounds", [{ ...use("react", { emoji: "🔁" }), times: 7 }, say("Finally.")]);
  fake.reset();
  await ask("[[script:rounds]] keep going", thread);
  const calls = fake.calls();

  assert.equal(calls.length, 8);
  assert.deepEqual(calls.map((c) => c.body.tool_choice), [...Array(7).fill(undefined), { type: "none" }]);
  assert.ok(calls.every((c) => c.body.tools.length > 0));
  assert.equal((await turnsOf(thread)).at(-1).text, "Finally.");
});

test("LOOP-08 pause_turn: the same conversation goes back with the paused message, and the answer follows", async () => {
  const thread = await newThread();

  fake.reset();
  const st = await ask("[[script:pause]] look it up", thread);
  const [first, second] = fake.calls();

  assert.equal(fake.calls().length, 2);
  assert.deepEqual(second.body.messages, [
    ...first.body.messages,
    { role: "assistant", content: [{ type: "server_tool_use", id: "srvtoolu_1", name: "web_search", input: { query: "rox" } }] },
  ]);
  assert.ok(st.events.some((e) => e.phase === "searching" && e.detail === "Digging around"));
  assert.equal((await turnsOf(thread)).at(-1).text, "Found it after the pause.");
});

test("LOOP-12 an app over 80 tools waits behind app_tools_load; loading it puts its tools in the next call", async () => {
  const add = await postJson("/mcp/servers", { id: "many", name: "Many", command: process.execPath, args: [FIXTURE], env: { FIXTURE_TOOLS: "85" } });

  assert.equal(add.json.tools, 87, add.text);
  const thread = await newThread();

  fake.script("defer", [use("app_tools_load", { app: "many" }), say("Loaded.")]);
  fake.reset();

  try {
    const st = await ask("[[script:defer]] use Many", thread);
    const [first, second] = fake.calls();

    assert.ok(!names(first).some((n) => n.startsWith("mcp_many_")));
    assert.deepEqual(first.body.tools.find((t) => t.name === "app_tools_load"), {
      name: "app_tools_load",
      description: "Load a connected app's tools so you can call them (they become callable on your next step and stay loaded for this chat). Load one only when the request needs that app. Apps: many (Many, 87 tools, e.g. echo, secret, tool_1, tool_2, tool_3, tool_4)",
      input_schema: { type: "object", properties: { app: { type: "string", enum: ["many"], description: "The app id" } }, required: ["app"] },
    });
    const loaded = ["mcp_many_echo", "mcp_many_secret", ...Array.from({ length: 85 }, (_, k) => `mcp_many_tool_${k + 1}`)];

    assert.deepEqual(last(second).content, [{ type: "tool_result", tool_use_id: "toolu_app_tools_load", content: `Loaded 87 Many tools; call them from your next step:\n${loaded.join("\n")}` }]);
    assert.deepEqual(names(second).filter((n) => n.startsWith("mcp_many_")), loaded);
    assert.ok(!names(second).includes("app_tools_load"));
    assert.deepEqual(second.body.tools.find((t) => t.name === "mcp_many_echo"), { name: "mcp_many_echo", description: "[many] Echo the arguments", input_schema: { type: "object" } });
    assert.ok(st.events.some((e) => e.phase === "connecting" && e.detail === "Working in Many"));
  } finally {
    await postJson("/mcp/server/state", { id: "many", enabled: false });
  }
});

test("LOOP-13 a routine's turn has no routine tools; the chat shows its note and the model reads its prompt", async () => {
  const thread = await newThread();

  fake.script("mk-routine", [use("routine_create", { name: "Daily check", every_s: 3600, prompt: "[[script:routine-run]] report" }), say("Set up.")]);
  fake.script("routine-run", [say("Routine report.")]);
  await ask("[[script:mk-routine]] check daily", thread);
  const r = (await request("GET", "/routines")).json.routines.find((x) => x.name === "Daily check");

  assert.ok(r, "the agent made the routine");
  fake.reset();
  const run = await postJson("/routine/run", { id: r.id });

  assert.equal(run.status, 200, run.text);
  assert.equal(run.json.ok, true);
  const [call] = fake.calls();

  assert.ok(!names(call).some((n) => n.startsWith("routine_")), names(call).join(" "));
  assert.ok(names(call).includes("generate_app"));
  assert.deepEqual(call.body.messages.slice(-2), [
    { role: "user", content: "Routine · Daily check" },
    { role: "user", content: `(Routine "Daily check" (${r.id}) is running on its own, every 3600s — the user isn't waiting on this message. Do what it asks and report briefly.)\n\n[[script:routine-run]] report` },
  ]);
  assert.deepEqual((await turnsOf(thread)).slice(-2).map((t) => [t.role, t.text, t.label]), [["user", "Routine · Daily check", ""], ["ai", "Routine report.", MODEL]]);
});

test("LOOP-14 /status walks thinking, the tool's activity, a working phrase, typing, done; phrases come from their pools", async () => {
  const thread = await newThread();

  fake.reset();
  const st = await ask("[[script:tool-then-text]] react", thread);

  assert.deepEqual(st.events.map((e) => e.seq), [1, 2, 3, 4, 5, 6]);
  assert.deepEqual(st.events.map((e) => e.phase), ["thinking", "working", "working", "working", "streaming", "done"]);
  const [thinking, starts, runs, working, typing, done] = st.events.map((e) => e.detail);

  assert.ok(PHRASES.thinking.includes(thinking), thinking);
  assert.deepEqual([starts, runs], ["Getting it done", "Getting it done"]);
  assert.ok(PHRASES.working.includes(working), working);
  assert.deepEqual([typing, done], ["Typing...", "replied"]);
  assert.deepEqual([st.phase, st.detail, st.done, st.result, st.open], ["done", "replied", true, "reply", null]);
});

test("LOOP-15 a bot's thread gets BOT_TOOLS and AGENT_TOOLS after ROUTINE_TOOLS, byte for byte, and its section in the system prompt", async () => {
  const bot = (await postJson("/bot/new", { name: "Ada" })).json.bot;
  const plain = await newThread();

  fake.reset();
  await ask("[[script:text]] hi Ada", bot.threadId);
  await ask("[[script:text]] hi nobody", plain);
  const [mine, theirs] = fake.calls();
  const at = 1 + DEFS.TOOLS.length + DEFS.ROUTINE_TOOLS.length;

  assert.equal(JSON.stringify(mine.body.tools.slice(at, at + DEFS.BOT_TOOLS.length)), JSON.stringify(DEFS.BOT_TOOLS));
  assert.equal(JSON.stringify(mine.body.tools.slice(at + DEFS.BOT_TOOLS.length, at + DEFS.BOT_TOOLS.length + DEFS.AGENT_TOOLS.length)), JSON.stringify(DEFS.AGENT_TOOLS));
  assert.ok(!names(theirs).some((n) => DEFS.BOT_TOOLS.some((t) => t.name === n) || DEFS.AGENT_TOOLS.some((t) => t.name === n)));
  assert.ok(mine.body.system.endsWith(
    "\n\n## Who you are\nYou are Ada. Ada is a personal assistant bot in Grok Shell.\n\nInstructions:\nStay in character as Ada. Be helpful, concise and friendly; ask one short clarifying question when a request is ambiguous.\n\nYour profile and memories are yours to maintain: memory_add / memory_list / memory_remove, bot_profile_update, integrations_list.",
  ));
  assert.equal(mine.body.system.slice(0, theirs.body.system.length), theirs.body.system);
  assert.equal((await turnsOf(bot.threadId)).at(-1).label, "Ada");
});

// the switches below are sticky: brokers of their own
let own = null;

async function ownBroker(label) {
  await server?.stop();
  server = null;
  await own?.stop();
  const d = tempDir(label);

  own = await startServer(d, { env: modelEnv(fake, join(d, "stubs.log")) });
}

test("LOOP-09 a refused server web search turns on the client search tool, for good", async () => {
  await ownBroker("agent-loop-search");
  const serverTool = (req) => req.body.tools.some((t) => t.type === "web_search_20250305");

  fake.script("no-server-search", [(req) => (serverTool(req) ? { status: 400, error: "tool type web_search_20250305 is not supported" } : say("Client search it is."))]);
  fake.reset();
  await ask("[[script:no-server-search]] search", await newThread());
  const [first, second] = fake.calls();

  assert.equal(fake.calls().length, 2);
  assert.ok(serverTool(first));
  assert.ok(!serverTool(second));
  assert.equal(second.body.tools[0].name, "generate_app");
  assert.equal(JSON.stringify(second.body.tools.at(-1)), JSON.stringify(DEFS.CLIENT_SEARCH));
  assert.equal(second.stream, true);
  fake.reset();
  await ask("[[script:text]] again", await newThread());
  assert.equal(JSON.stringify(fake.calls()[0].body.tools.at(-1)), JSON.stringify(DEFS.CLIENT_SEARCH));
});

let streamThread;

test("LOOP-10 a stream that dies before its first event falls back to a plain create, for good", async () => {
  await ownBroker("agent-loop-stream");
  streamThread = await newThread();
  fake.reset();
  await ask("[[script:die]] answer", streamThread);
  assert.deepEqual(fake.calls().map((c) => c.stream), [true, false]);
  assert.equal(fake.calls()[1].body.stream, undefined);
  assert.equal((await turnsOf(streamThread)).at(-1).text, "Answered without streaming.");
  fake.reset();
  await ask("[[script:text]] again", streamThread);
  assert.deepEqual(fake.calls().map((c) => c.stream), [false]);
});

test("LOOP-11 an API error ends the job with the error as its detail; the prompt stays, no reply", async () => {
  fake.script("too-long", [{ status: 400, error: "prompt is too long" }]);
  fake.reset();
  const st = await ask("[[script:too-long]] a lot", streamThread);
  const err = 'generator error — llm-api 400: 400 {"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long"}}';

  assert.equal(fake.calls().length, 1, "a 400 is not retried");
  assert.deepEqual([st.phase, st.detail, st.done, st.result], ["done", err, true, null]);
  assert.deepEqual(st.events.map((e) => e.phase), ["thinking", "done"]);
  assert.deepEqual((await turnsOf(streamThread)).slice(-1).map((t) => [t.role, t.text]), [["user", "[[script:too-long]] a lot"]]);
});
