// The Node side of tests/model_diff.rs: runs model.mjs (and avatars.mjs'
// image calls) on the scenarios given in argv against the scripted fake
// opencodex (server/test/fixtures/fake-opencodex.mjs), and records what
// each sent. Then it keeps the fake up for the Rust side: every "take"
// line on stdin answers with the requests since the last one.
//
//   node model-diff.mjs <anthropic|openai> <data dir> <scenarios JSON>
//
// Prints one JSON line {url, results: [{requests, result}]} after Node's
// run, then one JSON line of requests per "take". Nothing listens beyond
// 127.0.0.1 and nothing else is fetched.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const server = join(here, "..", "..", "..", "server");
const [mode, data, raw] = process.argv.slice(2);
const scenarios = JSON.parse(raw);
const { startFakeOpencodex, PNG_B64 } = await import(join(server, "test", "fixtures", "fake-opencodex.mjs"));
const fake = await startFakeOpencodex();

for (const [name, steps] of Object.entries(scenarios.scripts || {})) fake.script(name, steps);
mkdirSync(join(data, "widgets"), { recursive: true });
writeFileSync(join(data, "src.png"), Buffer.from(PNG_B64, "base64"));
for (const k of ["MOCK", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "GEN_PROVIDER", "GEN_BASE_URL", "GEN_MODEL", "GEN_MAX_TOKENS", "OPENROUTER_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY", "LINEAR_API_KEY"]) delete process.env[k];
Object.assign(process.env, {
  GROK_DATA_DIR: data,
  OPENCODEX_URL: fake.url,
  PATH: `${join(server, "test", "fixtures", "bin")}:${process.env.PATH}`,
  ...(mode === "openai" ? { GEN_PROVIDER: "openai", GEN_BASE_URL: `${fake.url}/v1`, OPENROUTER_API_KEY: "test-key" } : { ANTHROPIC_BASE_URL: `${fake.url}/direct` }),
});
// quiet: the classifier and the SVG fallback log
console.error = () => {};
console.log = () => {};

const model = await import(join(server, "src", "model.mjs"));
const { modelCtx } = await import(join(server, "src", "state.mjs"));
const avatars = await import(join(server, "src", "avatars.mjs"));
const out = (v) => process.stdout.write(JSON.stringify(v) + "\n");
const take = () => fake.requests.splice(0).map(({ method, path, headers, body }) => ({ method, path, headers, body }));
const llm = (e) => `llm-api ${e.status ?? ""}: ${e.message}`;

async function run(s) {
  if (s.linear) process.env.LINEAR_API_KEY = "lin-test";
  const inCtx = (f) => modelCtx.run(s.ctx ?? undefined, f);

  switch (s.kind) {
    case "ask":
      return inCtx(() => model.askModel(null, s.prompt, s.prevError, s.prevCode, s.name)).catch((e) => ({ error: e.message }));
    case "classify":
      return inCtx(() => model.classifyIntent(s.prompt));
    case "chat":
      return inCtx(() => model.chatReply(s.prompt, { turns: s.turns }, null));
    case "create":
      // the SDK throws its non-streaming refusal synchronously
      return inCtx(async () => model.anthropic.messages.create(s.body)).catch((e) => ({ error: llm(e) }));
    case "stream":
      return inCtx(async () => {
        try {
          return await model.anthropic.messages.stream(s.body).finalMessage();
        } catch (e) {
          return { error: llm(e) };
        }
      });
    case "imageModels":
      return Object.fromEntries(await avatars.imageModels());
    case "generate":
      return inCtx(() => avatars.generateImage(s.prompt, join(data, "node-gen.png"), s.opts)).catch((e) => ({ error: e.message }));
    case "edit":
      return avatars.editImage(join(data, "src.png"), s.prompt, join(data, "node-edit.png"), { size: s.size }).catch((e) => ({ error: e.message }));
    default:
      throw new Error(`no scenario kind ${s.kind}`);
  }
}

const results = [];

for (const s of scenarios.list) {
  const result = await run(s);

  results.push({ requests: take(), result: result === undefined ? null : result });
}

out({ url: fake.url, results });
fake.reset();
const lines = createInterface({ input: process.stdin });

for await (const line of lines) if (line.trim() === "take") out(take());
await fake.close();
process.exit(0);
