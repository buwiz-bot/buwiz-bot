// The tools that call the broker itself (launch_app, promote_app,
// demote_app, query_source) reach this broker's front door, PUBLIC_PORT
// (BUWIZ_PUBLIC_PORT behind buwiz-broker, else PORT), never a fixed 8787: a
// test or rehearsal broker must not touch the live one (plan D34). In this
// process on a temp data dir; a stub on TEST_PORT stands in for the front
// door and records what reaches it.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { after, before, test } from "node:test";
import { PORT, tempDir } from "./helpers.mjs";

process.env.BUWIZ_BROKER_DATA_DIR = tempDir("self-calls");
process.env.MOCK = "1";
// Node's own port is another one: the calls must name the front door's
process.env.PORT = String(PORT + 1);
process.env.BUWIZ_PUBLIC_PORT = String(PORT);

const { toolArtifact, toolQuerySource } = await import("../src/tools.mjs");

const seen = [];
let door;

before(async () => {
  door = createServer((req, res) => {
    let body = "";

    req.on("data", (c) => (body += c));
    req.on("end", () => {
      seen.push({ method: req.method, url: req.url, body });
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify({ ok: true, from: "front-door" }));
    });
  });
  await new Promise((resolve, reject) => door.once("error", reject).listen(PORT, "127.0.0.1", resolve));
});

after(() => new Promise((resolve) => door.close(resolve)));

test("SELF-01 launch_app, promote_app and demote_app POST to the front door's port", async () => {
  seen.length = 0;
  const launched = await toolArtifact({ name: "launch_app" }, { name: "R7", v: 2 });
  const promoted = await toolArtifact({ name: "promote_app" }, { name: "R7" });
  const demoted = await toolArtifact({ name: "demote_app" }, { name: "R7" });

  assert.equal(launched.is_error, undefined, launched.content);
  assert.equal(promoted.is_error, undefined, promoted.content);
  assert.equal(demoted.is_error, undefined, demoted.content);
  assert.deepEqual(seen, [
    { method: "POST", url: "/popout", body: JSON.stringify({ name: "R7", v: 2 }) },
    { method: "POST", url: "/publish", body: JSON.stringify({ name: "R7" }) },
    { method: "POST", url: "/unpublish", body: JSON.stringify({ name: "R7" }) },
  ]);
});

test("SELF-02 query_source reads /data/* from the front door's port", async () => {
  seen.length = 0;
  const reminders = await toolQuerySource({ source: "reminders" });
  const linear = await toolQuerySource({ source: "linear" });

  assert.equal(reminders.content, JSON.stringify({ ok: true, from: "front-door" }));
  assert.equal(linear.content, JSON.stringify({ ok: true, from: "front-door" }));
  assert.deepEqual(seen.map((r) => `${r.method} ${r.url}`), ["GET /data/reminders", "GET /data/linear/issues"]);
});
