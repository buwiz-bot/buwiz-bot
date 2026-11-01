// The Node side of tests/agent_diff.rs: runs agent.mjs's `generate` (the
// tool loop, as /chat runs it) on each scenario's prompt, in a thread of
// its own on data dir argv[2], against the scripted fake opencodex
// (server/test/fixtures/fake-opencodex.mjs), and records the model calls
// each sent, the job's phases and the thread's turns. Then it keeps the
// fake up for the Rust side: every "take" line on stdin answers with the
// requests since the last one.
//
//   node agent-diff.mjs <data dir> <scenarios JSON>
//
// Prints one JSON line {url, results: [{calls, phases, detail, turns}]}
// after Node's run, then one JSON line of requests per "take". Nothing
// listens beyond 127.0.0.1 and nothing else is fetched.
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const server = join(here, "..", "..", "..", "server");
const [data, raw] = process.argv.slice(2);
const scenarios = JSON.parse(raw);
const { startFakeOpencodex } = await import(join(server, "test", "fixtures", "fake-opencodex.mjs"));
const fake = await startFakeOpencodex();

for (const [name, steps] of Object.entries(scenarios.scripts || {})) fake.script(name, steps);
mkdirSync(join(data, "widgets"), { recursive: true });
for (const k of ["MOCK", "ANTHROPIC_BASE_URL", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "GEN_PROVIDER", "GEN_BASE_URL", "GEN_MODEL", "GEN_MAX_TOKENS", "OPENROUTER_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY", "LINEAR_API_KEY"]) delete process.env[k];
const bin = join(server, "test", "fixtures", "bin");

Object.assign(process.env, { BUWIZ_BROKER_DATA_DIR: data, OPENCODEX_URL: fake.url, PATH: `${bin}:${process.env.PATH}`, NSH: join(bin, "nsh"), BUWIZ_STUB_LOG: join(data, "stubs.log") });
console.error = () => {};
console.log = () => {};

const state = await import(join(server, "src", "state.mjs"));
const { generate, artifactBlocks } = await import(join(server, "src", "agent.mjs"));
const { choiceFor } = await import(join(server, "src", "model.mjs"));
const out = (v) => process.stdout.write(JSON.stringify(v) + "\n");
const calls = () => fake.requests.splice(0).filter((r) => /\/(messages|chat\/completions)$/.test(r.path)).map(({ method, path, headers, body }) => ({ method, path, headers, body }));
const turnsOf = (t) => t.turns.map((u) => [u.role, u.text ?? null, u.label ?? null, u.name ?? null, u.v ?? null, u.reaction ?? null]);

const results = [];

for (const s of scenarios.runs) {
  const t = { id: `t${state.nextThreadSeq()}`, uid: state.uuidv7(), title: "New chat", turns: [] };

  state.store.list.unshift(t);
  const job = { id: state.allocJobId(), thread: t.id, seq: 0, phase: "queued", detail: "", done: false, result: null, extra: {} };

  job.ctx = await artifactBlocks(undefined);
  await state.modelCtx.run(choiceFor(t.id), () => generate(job, s.prompt)).catch((e) => {
    job.done = true;
    state.setJob(job, "done", `generator error — ${e.message}`);
  });
  results.push({ calls: calls(), phases: (job.events || []).map((e) => e.phase), detail: job.detail, turns: turnsOf(t) });
}

// the scripts start over for the Rust side
fake.reset();
out({ url: fake.url, results });
for await (const line of createInterface({ input: process.stdin })) if (line.trim() === "take") out(calls());
