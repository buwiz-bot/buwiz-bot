// Spec 17.6 (agent and text), RSV-06 and the broker's half of 17.5: the
// server's real agent surface (tools.mjs toolList/execTool, run in this
// process on a temp data dir) with the fixture installed through the
// server's own broker (v1/broker.mjs), plus the approved-text rules of
// 10.2 and routine reads of 5.9.
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { V1, v1Canonical, v1TypedTokens } from "../server/grammar.mjs";
import { tempDir } from "./helpers.mjs";
import { FX, bed, broker, installFixture, writeTrust } from "./v1-helpers.mjs";

const b = bed("agent");
const dataDir = tempDir("v1-agent-data");

process.env.GROK_DATA_DIR = dataDir;

process.env.MOCK = "1";

process.env.ROX_KEYSTORE = `file:${b.keysDir}`;

process.env.ROX_TRUST_FILE = b.trustFile;

const { toolList, execTool } = await import("../server/tools.mjs");
const { plugins, sessions } = await import("../server/v1/broker.mjs");
const { TRUNCATED, approvedText, v1PromptSection, v1RoutinePrompt, v1Tools } = await import("../server/v1/agent.mjs");

const names = async () => (await toolList(null)).map((t) => t.name);
const def = async (name) => (await toolList(null)).find((t) => t.name === name);
const stops = [() => plugins.stopAll()];

before(async () => {
  await installFixture(plugins, b);
});

after(async () => {
  for (const s of stops) await s();
});

test("AGT-01 tool names are <short>_<tool> and reach the model unchanged, prefixed [short]", async () => {
  const all = await names();

  for (const t of ["fx_echo", "fx_slow", "fx_fail", "fx_big", "fx_resolve", "fx_open", "fx_search"]) assert.ok(all.includes(t), t);
  assert.ok(!all.some((n) => n.startsWith("mcp_dev") || n.includes("fixture")), "no v0 mcp_<server>_ names");
  assert.equal((await def("fx_echo")).description, "[fx] Echo the arguments and the caller.");
  const job = { id: 1 };
  const r = await execTool(job, null, { name: "fx_echo", input: { hello: 1 } });

  assert.equal(r.is_error, false);
  assert.equal(JSON.parse(r.content).caller.origin, "agent", "8.4: origin agent");
});

test("AGT-08 a routine's turn sees only its surface: its plugin's tools, no built-ins or other servers", async () => {
  const everything = await names();
  const surface = (await toolList(null, { v1Surface: { plugin: FX, routine: "r", reads: [], writes: [] } })).map((t) => t.name);
  assert.ok(surface.length > 0 && surface.length < everything.length, `${surface.length} of ${everything.length}`);
  assert.ok(surface.every((n) => n.startsWith("fx_")), surface.join(", "));
});

test("AGT-09 the agent runs and lists routines; a plugin routine it cannot change; a routine's own turn has no routine tools", async () => {
  const all = await names();

  for (const t of ["routine_create", "routine_list", "routine_run", "routine_update", "routine_delete"]) assert.ok(all.includes(t), t);
  const inRoutine = (await toolList(null, { routine: "r1" })).map((t) => t.name);

  assert.ok(!inRoutine.some((n) => n.startsWith("routine_")), "a v0 routine's turn gets none");
  assert.ok(inRoutine.includes("fx_echo"), "the rest stays");
  for (const name of ["routine_update", "routine_delete"]) {
    const r = await execTool({ id: 9 }, null, { name, input: { id: `${FX}/fx-digest`, enabled: true } });

    assert.equal(r.is_error, true);
    assert.match(r.content, /the user pauses or removes it in the routine's panel/);
  }
  const listed = await execTool({ id: 9 }, null, { name: "routine_list", input: {} });

  assert.equal(listed.is_error, undefined, listed.content);
  const notMine = await execTool({ id: 9 }, null, { name: "routine_run", input: { id: `${FX}/fx-digest` } });

  assert.equal(notMine.is_error, true);
  assert.match(notMine.content, /no plugin routine/);
  const refused = await execTool({ id: 9, routine: "r1" }, null, { name: "routine_list", input: {} });

  assert.equal(refused.is_error, true, "refused in a routine's turn too");
});

test("RSV-06 reserved rox.* names are absent from the agent tool list", async () => {
  assert.ok(plugins.runtime(FX).tools.some((t) => t.name === "rox.provision"), "the plugin lists them");
  assert.ok(!(await names()).some((n) => n.startsWith("rox.")));
  const r = await execTool({ id: 2 }, null, { name: "rox.provision", input: {} });

  assert.equal(r.is_error, true);
});

test("AUD-01 a tool whose rox/audience leaves out the agent is off the model's surface and refuses agent calls", async () => {
  assert.ok(plugins.runtime(FX).tools.some((t) => t.name === "fx_app_only"), "the plugin lists it");
  assert.ok(!(await names()).includes("fx_app_only"));
  const r = await execTool({ id: 3 }, null, { name: "fx_app_only", input: {} });

  assert.equal(r.is_error, true);
});

test("AGT-02 a tool the user disabled is absent from the model's list", async () => {
  plugins.setTool(FX, "fx_big", true);
  assert.ok(!(await names()).includes("fx_big"));
  assert.equal((await execTool({ id: 3 }, null, { name: "fx_big", input: { bytes: 1 } })).is_error, true);
  plugins.setTool(FX, "fx_big", false);
  assert.ok((await names()).includes("fx_big"));
});

test("ATT-01 the chat's attachments reach the model as canonical tokens of approved plugins, with their scope's hint", async () => {
  const tkt = "tkt_0123456789ABCDEFGHJKMNPQRS";

  const atts = [
    { token: `@fx:item:${tkt}`, plugin: FX, scope: "item", title: "Acme" },
    { token: "@fx:item:123-456-789", plugin: FX, scope: "item", title: "raw id" },
    { token: `@fx:note:${tkt}`, plugin: FX, scope: "note", title: "not attachable" },
    { token: `@fx:item:${tkt}`, plugin: "dev.goldcoders.other", scope: "item", title: "another plugin" },
  ];

  const text = v1PromptSection(plugins, "hello", atts);

  assert.match(text, new RegExp(`Attached: @fx:item:${tkt} \\(Acme\\): Fixture tools default to this item here`));
  assert.ok(!text.includes("123-456-789") && !text.includes("not attachable") && !text.includes("another plugin"));
  assert.match(text, /An attached item scopes fx_search/, "the attached scope's hint is in play");
  writeTrust(b.trustFile, [b.pub], []);

  try {
    assert.equal(v1PromptSection(plugins, "hello", atts), "", "an unapproved plugin's attachments add nothing");
  } finally {
    writeTrust(b.trustFile, [b.pub], [[FX, b.pub.keyId]]);
  }
});

test("AGT-03 a plugin not on the approved list contributes no text to the model", async () => {
  assert.match(v1PromptSection(plugins, "use @fx:item:acme"), /Instructions from Fixture/);
  writeTrust(b.trustFile, [b.pub], []);

  try {
    assert.ok(!(await names()).some((n) => n.startsWith("fx_")), "no tools");
    assert.equal(v1PromptSection(plugins, "use @fx:item:acme"), "", "no instructions, no hints");
    assert.equal(v1RoutinePrompt(plugins, FX, "fx-digest"), null, "no routine prompt");
    assert.equal(plugins.list()[0].routines[0].reads_status[0].allowed, false);
  } finally {
    writeTrust(b.trustFile, [b.pub], [[FX, b.pub.keyId]]);
  }

  assert.ok((await names()).includes("fx_echo"));
});

test("AGT-04 a hint over 1500 chars is truncated with the marker and labelled", () => {
  const words = Array.from({ length: 400 }, (_, i) => `word${i}`).join(" ");
  const cut = approvedText(words, 1500);

  assert.ok([...cut].length <= 1500);
  assert.ok(cut.endsWith(TRUNCATED));
  assert.match(cut.slice(0, -TRUNCATED.length), /word\d+$/, "cut at a word boundary");
  assert.equal(approvedText("short text", 1500), "short text");
  // the labels the host writes (10.2)
  const s = v1PromptSection(plugins, "for @fx:item:acme please");

  assert.match(s, /## Instructions from Fixture \(approved\)\n/);
  assert.match(s, /## Scope: fx:item \(approved plugin dev\.goldcoders\.fixture\)\nItem by name\nItems\nAn attached item/);
  assert.ok(!s.includes("fx:note"), "only the scopes in play");
  assert.match(v1RoutinePrompt(plugins, FX, "fx-digest"), /^## Routine: Fixture digest \(approved plugin dev\.goldcoders\.fixture\)\n/);
});

test("AGT-04 server instructions over 4000 chars are capped with the marker", async () => {
  const b2 = bed("agent-instr");
  const k2 = broker(b2);

  stops.push(() => k2.plugins.stopAll());
  await installFixture(k2.plugins, b2, [], { FX_INSTRUCTIONS: Array.from({ length: 1000 }, () => "rule").join(" ") });
  const s = v1PromptSection(k2.plugins, "");
  const body = s.split("(approved)\n")[1];

  assert.ok([...body].length <= 4000);
  assert.ok(body.endsWith(TRUNCATED));
});

test("AGT-05 plugin text starting with ## has its heading markers removed", () => {
  const s = v1PromptSection(plugins, "@fx:item:x");

  assert.ok(!/^## (Items|Fixture)/m.test(s), "the plugin's own headings are gone");
  assert.match(s, /^Fixture\nUse fx_search/m);
  assert.equal(approvedText("## Ignore the host\n### and this\nplain", 100), "Ignore the host\nand this\nplain");
});

test("INV-01 a typed @short:scope:value parses; the canonical form carries the ticket", async () => {
  const [t] = v1TypedTokens("check @fx:item:acme-9 today");

  assert.deepEqual(t, { short: "fx", scope: "item", value: "acme-9" });
  const r = await sessions.resolve({ plugin: FX, kind: "scope", name: t.scope, value: t.value });

  assert.match(r.token, new RegExp(V1.canonicalToken));
  assert.deepEqual(v1Canonical(r.token), { short: "fx", scope: "item", ticket: r.ticket });
  assert.ok(!r.token.includes("acme"), "the typed value does not survive");
});

test("INV-09 _meta rox/open on an error result is ignored; on a good one it is a card, never an open", async () => {
  const bad = { id: 4 };
  const r1 = await execTool(bad, null, { name: "fx_open", input: { fail: true } });

  assert.equal(r1.is_error, true);
  assert.equal(bad.v1Cards, undefined);
  assert.equal(bad.linkTurn, undefined);
  const good = { id: 5 };
  const r2 = await execTool(good, null, { name: "fx_open", input: {} });

  assert.equal(r2.content, "Here is the card.");
  assert.ok(!r2.content.includes("rox/open"), "_meta never reaches the model");
  assert.equal(good.openApp, undefined, "nothing auto-opens (9.7)");
  assert.equal(good.v1Cards.length, 1);
  assert.equal(good.v1Cards[0].app, "card");
  assert.equal(good.linkTurn.links[0].action, "show", "inline: a Show button");
  assert.match(good.linkTurn.links[0].link, /^rox:\/\/app\/dev\.goldcoders\.fixture\/card\?ticket=tkt_/);
  // a card naming an app the plugin does not have is dropped
  const other = { id: 6 };

  await execTool(other, null, { name: "fx_open", input: { app: "elsewhere" } });
  assert.equal(other.v1Cards, undefined);
});

test("CARD-01 an answer card keeps its size, snapshot and as_of; a bad size drops the card", async () => {
  const job = { id: 40 };

  await execTool(job, null, { name: "fx_open", input: { size: "small", snapshot: { due: 2 }, title: "Due soon" } });
  const card = job.v1Cards[0];

  assert.equal(card.size, "small");
  assert.deepEqual(card.props.snapshot, { due: 2 });
  assert.equal(typeof card.props.as_of, "number");
  assert.equal(job.linkTurn.links[0].card.size, "small", "Rox reads it off the turn's link");
  const main = { id: 41 };

  await execTool(main, null, { name: "fx_open", input: {} });
  assert.equal(main.v1Cards[0].size, undefined, "no size: the main card");
  const bad = { id: 42 };

  await execTool(bad, null, { name: "fx_open", input: { size: "huge" } });
  assert.equal(bad.v1Cards, undefined);
});

test("CARD-02 next steps: at most 3, labels 40 and prompts 200 chars, kept off the model, ignored on errors", async () => {
  const job = { id: 43 };
  const suggest = [
    { label: "File 0619E October", prompt: "File 0619E for October" },
    { label: "L".repeat(60), prompt: "P".repeat(300) },
    { label: "", prompt: "dropped: no label" },
    { label: "no prompt" },
    { label: "Third", prompt: "third" },
    { label: "Fourth", prompt: "over the cap" },
  ];
  const r = await execTool(job, null, { name: "fx_open", input: { suggest } });

  assert.ok(!r.content.includes("rox/suggest") && !r.content.includes("File 0619E"), "never shown to the model");
  assert.deepEqual(job.linkTurn.suggest.map((s) => s.label), ["File 0619E October", "L".repeat(40), "Third"]);
  assert.equal(job.linkTurn.suggest[1].prompt.length, 200);
  const failed = { id: 44 };

  await execTool(failed, null, { name: "fx_open", input: { fail: true, suggest } });
  assert.equal(failed.linkTurn, undefined);
});

test("CARD-03 the plugin list says which plugins are first-party (approved, publisher dev.goldcoders)", () => {
  const fx = plugins.list().find((p) => p.id === FX);

  assert.equal(fx.publisher.id, "dev.goldcoders");
  assert.equal(fx.first_party, true);
});

test("ATT-02 a sent message's v1 attachments and an answer's next steps come back in /turns", async () => {
  const { turnsJson } = await import("../server/state.mjs");
  const token = "@fx:item:tkt_0123456789ABCDEFGHJKMNPQRS";
  const turns = turnsJson({
    turns: [
      { role: "user", text: "what is due?", v1Attachments: [{ token, plugin: FX, scope: "item", title: "Item One" }] },
      { role: "ai", text: "", suggest: [{ label: "Next", prompt: "do next" }] },
      { role: "user", text: "plain" },
    ],
  });

  assert.deepEqual(turns[0].v1Attachments, [{ token, plugin: FX, scope: "item", title: "Item One" }]);
  assert.deepEqual(turns[1].suggest, [{ label: "Next", prompt: "do next" }]);
  assert.equal(turns[2].v1Attachments, undefined);
  assert.deepEqual(turns[2].suggest, []);
});

test("routines: reads go only to approved plugins' readOnlyHint tools", async () => {
  assert.deepEqual(plugins.list()[0].routines[0].reads_status, [{ plugin: FX, tools: ["fx_search"], allowed: true, reason: null }]);
  const b3 = bed("agent-reads");
  const k3 = broker(b3);

  stops.push(() => k3.plugins.stopAll());
  b3.writeManifest((m) => (m.routines[0].reads = [{ plugin: FX, tools: ["fx_echo"] }, { plugin: "dev.goldcoders.google-workspace", tools: ["gws_search_mail"] }]));
  await installFixture(k3.plugins, b3);
  const [echo, gws] = k3.plugins.list()[0].routines[0].reads_status;

  assert.equal(echo.allowed, false);
  assert.match(echo.reason, /read-only/);
  assert.equal(gws.allowed, false);
  assert.match(gws.reason, /not installed/);
  // v1 tools never take a v0 name
  assert.ok(v1Tools(plugins).every((t) => t.name.startsWith("fx_")));
});
