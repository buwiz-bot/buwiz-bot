// The packaged server is one bundled file (tools/package-server.sh), so a
// module's "am I the main script" check (`process.argv[1] ===
// fileURLToPath(import.meta.url)`) is true inside it: a CLI module the
// server imports would run its CLI instead of the server. The server's
// import graph must reach none.
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { test } from "node:test";
import { ROOT } from "./helpers.mjs";

const MAIN_CHECK = /process\.argv\[1\]\s*===\s*fileURLToPath\(import\.meta\.url\)/;

function graph(entry) {
  const seen = new Set();
  const todo = [entry];

  while (todo.length) {
    const file = todo.pop();

    if (seen.has(file) || !existsSync(file)) continue;
    seen.add(file);

    for (const m of readFileSync(file, "utf8").matchAll(/(?:import|export)[^"'`]*?from\s*["'](\.{1,2}\/[^"']+)["']|import\(\s*["'](\.{1,2}\/[^"']+)["']\s*\)/g))
      todo.push(resolve(dirname(file), m[1] || m[2]));
  }

  return seen;
}

test("PKG-01 the server's import graph reaches no module that runs as a CLI", () => {
  const files = graph(join(ROOT, "server", "app-server.mjs"));
  const clis = [...files].filter((f) => MAIN_CHECK.test(readFileSync(f, "utf8")));

  assert.ok(files.has(join(ROOT, "server", "v1", "sessions.mjs")), "the walk reaches the v1 modules");
  assert.deepEqual(clis, []);
});
