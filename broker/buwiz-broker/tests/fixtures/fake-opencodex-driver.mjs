// Serves the scripted fake opencodex (fake-opencodex.mjs) to a Rust test and
// reports what it was asked: the stand-in model behind agent_diff.rs and
// model_diff.rs. Plain Node, no packages; it listens on 127.0.0.1 only.
//
//   node fake-opencodex-driver.mjs <calls|all> <scripts JSON> [<data dir>]
//
// Prints one JSON line {url} when it is up. Then every "take" line on stdin
// is answered with one JSON line: the requests since the last take as
// {method, path, headers, body}, only the model calls (Messages and chat
// completions) with `calls`, every request with `all`. With a data dir it
// makes <dir>/widgets and writes the fake's 1x1 PNG as <dir>/src.png.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const [mode, raw, data] = process.argv.slice(2);
const { startFakeOpencodex, PNG_B64 } = await import(join(here, "fake-opencodex.mjs"));
const fake = await startFakeOpencodex();

for (const [name, steps] of Object.entries(JSON.parse(raw).scripts || {})) fake.script(name, steps);

if (data) {
  mkdirSync(join(data, "widgets"), { recursive: true });
  writeFileSync(join(data, "src.png"), Buffer.from(PNG_B64, "base64"));
}
const take = () =>
  fake.requests
    .splice(0)
    .filter((r) => mode === "all" || /\/(messages|chat\/completions)$/.test(r.path))
    .map(({ method, path, headers, body }) => ({ method, path, headers, body }));

process.stdout.write(`${JSON.stringify({ url: fake.url })}\n`);
for await (const line of createInterface({ input: process.stdin })) if (line.trim() === "take") process.stdout.write(`${JSON.stringify(take())}\n`);
await fake.close();
process.exit(0);
