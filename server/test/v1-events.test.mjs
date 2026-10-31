// Rox reloads a plugin's cards on its `plugin` events (spec 13.7), so the
// broker announces one only when the plugin's state changes: never for a
// session opening, calling or closing (EVT-01). On Node's plugin host and,
// with BUWIZ_BROKER_CMD, through the line to rox-broker, whose session
// answers carry the plugin's runtime snapshot.
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { FX, bed, broker, envelope, installFixture, openApp } from "./v1-helpers.mjs";

const b = bed("events");
const { plugins, sessions } = broker(b);
let m;

before(async () => {
  await installFixture(plugins, b);
  m = plugins.manifestOf(plugins.rowOf(FX));
});

after(() => plugins.stopAll());

test("EVT-01 sessions opening, calling and closing announce no plugin event; a state change does", async () => {
  const seen = [];
  const on = (e) => seen.push(e);

  plugins.events.on("plugin", on);

  try {
    for (let i = 0; i < 3; i++) {
      const open = await openApp(sessions, m, "board");

      assert.equal(open.res.ok, true, JSON.stringify(open.res));
      await sessions.call(envelope(open, "fx_slow", { ms: 1 }));
      assert.deepEqual(await sessions.close({ open_id: open.open_id }), { ok: true });
    }

    assert.deepEqual(seen, [], "no event while the plugin runs on");
    await plugins.disable(FX);
    await plugins.enable(FX);
    assert.ok(seen.some((e) => e.id === FX && e.state === "running"), JSON.stringify(seen));
  } finally {
    plugins.events.off("plugin", on);
  }
});
