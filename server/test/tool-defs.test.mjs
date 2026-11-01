// The tool loop's contract with buwiz-broker (Rust broker plan, D35): the
// tool schemas and the agent's system prompt Node sends, as Rust reads them
// from broker/buwiz-broker/src/hub/tools/defs.json. This fails when one side
// drifts. `BUWIZ_DEFS_WRITE=1 node --test test/tool-defs.test.mjs` rewrites
// the file from Node's modules.
import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { ROOT, STUB_BIN, env, nodeIn, tempDir } from "./helpers.mjs";

const FILE = join(ROOT, "..", "broker", "buwiz-broker", "src", "hub", "tools", "defs.json");
// what follows the skill sections in the agent's system prompt
const CONTRACT = "\n\nWhen you DO write a widget module, this contract applies:\n";

/** Node's definitions, from its own modules on an empty data dir (no
 *  skills, no Linear key: the stubs stand in for `secret`). */
function nodeDefs() {
  const dir = tempDir("tool-defs");

  return JSON.parse(nodeIn(dir, `
    const { TOOLS, CLIENT_SEARCH, agentSystem } = await import("./src/tools.mjs");
    const { ROUTINE_TOOLS } = await import("./src/routines.mjs");
    const { BOT_TOOLS } = await import("./src/botctx.mjs");
    const { AGENT_TOOLS } = await import("./src/agents-tools.mjs");
    const { contractFor } = await import("./src/model.mjs");
    const system = agentSystem(null);
    const tail = ${JSON.stringify(CONTRACT)} + contractFor("__NAME__");
    if (!system.endsWith(tail)) throw new Error("the system prompt no longer ends with the contract");
    console.log(JSON.stringify({ TOOLS, CLIENT_SEARCH, ROUTINE_TOOLS, BOT_TOOLS, AGENT_TOOLS, SYSTEM: system.slice(0, -tail.length), CONTRACT: ${JSON.stringify(CONTRACT)} }));
  `, { PATH: `${STUB_BIN}:${process.env.PATH}`, LINEAR_API_KEY: "", BUWIZ_STUB_LOG: join(dir, "stubs.log") }));
}

test("DEFS-01 Rust's defs.json is Node's tool schemas and system prompt, byte for byte", () => {
  const text = `${JSON.stringify(nodeDefs(), null, 2)}\n`;

  if (env("BUWIZ_DEFS_WRITE") === "1") writeFileSync(FILE, text);
  assert.equal(readFileSync(FILE, "utf8"), text, "defs.json drifted from Node: rewrite it with BUWIZ_DEFS_WRITE=1 and port the change");
});
