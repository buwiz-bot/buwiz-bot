// The environment variables' Buwiz names, the old Rox and Grok names still
// read (src/env.mjs; broker/buwiz-protocol/src/env.rs keeps the same table and
// checks the two match).
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { both, env, NAMES, oldName, withOld } from "../src/env.mjs";
import { ROOT } from "./helpers.mjs";

test("the new name wins", () => {
  assert.equal(env("BUWIZ_BROKER_DATA_DIR", { BUWIZ_BROKER_DATA_DIR: "/new", GROK_DATA_DIR: "/old" }), "/new");
  // set but empty still counts as set, unless the old name has a value
  assert.equal(env("BUWIZ_ALLOWED_ORIGINS", { BUWIZ_ALLOWED_ORIGINS: "" }), "");
  assert.equal(env("BUWIZ_ALLOWED_ORIGINS", { BUWIZ_ALLOWED_ORIGINS: "", GROK_ALLOWED_ORIGINS: "" }), "");
  assert.equal(env("BUWIZ_ALLOWED_ORIGINS", { BUWIZ_ALLOWED_ORIGINS: "", GROK_ALLOWED_ORIGINS: "http://a" }), "http://a");
});

test("neither name is undefined; a name with no old one reads only itself", () => {
  assert.equal(env("BUWIZ_PUBLIC_PORT", { PORT: "1" }), undefined);
  assert.equal(oldName("BUWIZ_TRUST_FILE"), undefined);
  assert.equal(env("BUWIZ_TRUST_FILE", { ROX_TRUST_FILE: "x" }), undefined);
});

test("an old name is still read", () => {
  const script = `
    const { env } = await import(${JSON.stringify(join(ROOT, "src", "env.mjs"))});
    process.stdout.write(JSON.stringify([env("BUWIZ_BUNDLE_ID"), env("BUWIZ_BUNDLE_ID"), env("BUWIZ_NODE_ROLE")]));
  `;
  const r = execFileSync(process.execPath, ["--input-type=module", "-e", script], {
    env: { ...process.env, GROK_BUNDLE_ID: "dev.example", BUWIZ_BUNDLE_ID: undefined, BUWIZ_NODE_ROLE: undefined, ROX_NODE_ROLE: undefined },
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  assert.deepEqual(JSON.parse(r), ["dev.example", "dev.example", null]);
});

test("the warning is said once per old name", () => {
  const r = JSON.parse(execFileSync(process.execPath, ["--input-type=module", "-e", `
    const { env } = await import(${JSON.stringify(join(ROOT, "src", "env.mjs"))});
    const said = [];
    process.stderr.write = (s) => (said.push(String(s)), true);
    env("BUWIZ_BUNDLE_ID"); env("BUWIZ_BUNDLE_ID");
    process.stdout.write(JSON.stringify(said));
  `], { env: { ...process.env, GROK_BUNDLE_ID: "dev.example" }, encoding: "utf8" }));
  assert.deepEqual(r, ["GROK_BUNDLE_ID is an old name; use BUWIZ_BUNDLE_ID\n"]);
});

test("a child's environment gets both names", () => {
  assert.deepEqual(withOld("BUWIZ_BROKER_DATA_DIR"), ["BUWIZ_BROKER_DATA_DIR", "GROK_DATA_DIR"]);
  assert.deepEqual(both("BUWIZ_LINE_TOKEN", "t"), { BUWIZ_LINE_TOKEN: "t", ROX_LINE_TOKEN: "t" });
  assert.deepEqual(both("BUWIZ_TRUST_FILE", "f"), { BUWIZ_TRUST_FILE: "f" });
});

test("each name is in the table once", () => {
  const seen = new Set();
  for (const [name, old] of NAMES) {
    assert.match(name, /^BUWIZ_/);
    assert.match(old, /^(ROX|GROK)_/);
    assert.ok(!seen.has(name) && !seen.has(old), `${name} / ${old} twice`);
    seen.add(name).add(old);
  }
  // the Rust table (broker/buwiz-protocol/src/env.rs) has the same pairs, in order
  const rust = [...readFileSync(join(ROOT, "..", "broker", "buwiz-protocol", "src", "env.rs"), "utf8").split("pub const NAMES")[1].split("];")[0].matchAll(/\("(\w+)", "(\w+)"\)/g)].map((m) => [m[1], m[2]]);
  assert.deepEqual(rust, NAMES);
});
