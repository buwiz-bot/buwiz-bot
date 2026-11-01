// The line token rox-broker hands Node never reaches what Node starts
// (v1/line-env.mjs): stdio MCP servers and other children inherit Node's
// environment.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { test } from "node:test";
import { ROOT } from "./helpers.mjs";

test("the line's address and token are read once and kept from every child", () => {
  const script = `
    const { LINE } = await import(${JSON.stringify(join(ROOT, "src", "v1", "line-env.mjs"))});
    const { execFileSync } = await import("node:child_process");
    const child = execFileSync(process.execPath, ["-e", "process.stdout.write(JSON.stringify([process.env.BUWIZ_V1_LINE || '', process.env.BUWIZ_V1_LINE_TOKEN || '']))"], { encoding: "utf8" });
    process.stdout.write(JSON.stringify({ LINE, here: [process.env.BUWIZ_V1_LINE || "", process.env.BUWIZ_V1_LINE_TOKEN || ""], child: JSON.parse(child) }));
  `;
  const out = JSON.parse(execFileSync(process.execPath, ["--input-type=module", "-e", script], {
    env: { ...process.env, BUWIZ_V1_LINE: "http://127.0.0.1:1", BUWIZ_V1_LINE_TOKEN: "secret-line-token" },
    encoding: "utf8",
  }));

  assert.deepEqual(out.LINE, { base: "http://127.0.0.1:1", token: "secret-line-token", hosts: ["v1"] });
  assert.deepEqual(out.here, ["", ""], "gone from Node's own environment");
  assert.deepEqual(out.child, ["", ""], "and from a child's");
});

test("a line handed over by its old names is read and kept from every child too", () => {
  const script = `
    const { LINE } = await import(${JSON.stringify(join(ROOT, "src", "v1", "line-env.mjs"))});
    const names = ["BUWIZ_V1_LINE", "BUWIZ_V1_LINE_TOKEN", "ROX_V1_LINE", "ROX_V1_LINE_TOKEN"];
    process.stdout.write(JSON.stringify({ LINE, here: names.filter((n) => n in process.env) }));
  `;
  const out = JSON.parse(execFileSync(process.execPath, ["--input-type=module", "-e", script], {
    env: { ...process.env, ROX_V1_LINE: "http://127.0.0.1:1", ROX_V1_LINE_TOKEN: "secret-line-token", ROX_LINE_HOSTS: "v1,mcp" },
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  }));

  assert.deepEqual(out.LINE, { base: "http://127.0.0.1:1", token: "secret-line-token", hosts: ["v1", "mcp"] });
  assert.deepEqual(out.here, [], "neither name stays in Node's environment");
});
