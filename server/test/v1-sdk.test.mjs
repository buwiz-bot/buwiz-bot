// The Mini App developer kit (sdk/mini-app): the test runner passes the
// template and the fixture, fails the fixture when it breaks a rule on
// purpose, follows the README's sign-then-test flow, and the documented
// manifest limits are the broker's. Extension IDs SDK-01.. of
// `npm run conformance`. The runner runs as a child process, the way a
// developer runs it; it opens no port and uses only temp directories.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { generate } from "../server/v1/ed.mjs";
import { LIMITS } from "../server/v1/manifest.mjs";
import { ROOT, tempDir } from "./helpers.mjs";
import { FIXTURE } from "./v1-helpers.mjs";

const SDK = join(ROOT, "sdk", "mini-app");
const RUNNER = join(SDK, "bin", "rox-plugin-test.mjs");
const TEMPLATE = join(SDK, "template", "server.mjs");

// a test publisher key in a 0600 file, as --keygen writes it
function keyFile() {
  const file = join(tempDir("sdk-key"), "test.key");

  writeFileSync(file, `${generate().seed}\n`, { mode: 0o600 });

  return file;
}

const KEY = keyFile();

/** Runs the runner with `args` and --json: {code, res, stderr}. */
function runner(args) {
  return new Promise((resolve) => {
    execFile(process.execPath, ["--disable-warning=ExperimentalWarning", RUNNER, ...args, "--json"], { cwd: ROOT, timeout: 120_000 }, (e, stdout, stderr) => {
      let res = null;

      try {
        res = JSON.parse(stdout);
      } catch {}

      resolve({ code: e ? e.code : 0, res, stderr, stdout });
    });
  });
}

const failed = (res) => res.rows.filter((r) => r.status === "fail").map((r) => `${r.id} ${r.what}: ${r.note}`);
const ids = (res, status) => new Set(res.rows.filter((r) => r.status === status).map((r) => r.id));
const fixture = (...extra) => ["--command", process.execPath, "--args", FIXTURE, ...extra.flatMap((a) => ["--args", a]), "--manifest-env", "FX_MANIFEST", "--sign-with", KEY];

const CHECKED = ["MAN-01", "MAN-02", "MAN-10", "APP-01", "APP-02", "APP-03", "RSV-01", "RSV-02", "RSV-03", "RSV-04", "RSV-05", "LCY-01", "BRG-03", "BRG-04", "BRG-06", "INV-04", "INV-05"];

test("SDK-01 the template plugin passes the test runner", async () => {
  const { code, res, stderr } = await runner(["--command", process.execPath, "--args", TEMPLATE, "--sign-with", KEY]);

  assert.ok(res, stderr);
  assert.deepEqual(failed(res), []);
  assert.equal(code, 0);
  assert.equal(res.plugin.id, "dev.goldcoders.hello");

  for (const id of CHECKED) assert.ok(ids(res, "pass").has(id), `${id} was checked and passed`);
  assert.deepEqual([...ids(res, "skip")], [], "nothing skipped for the template");
});

test("SDK-02 the fixture plugin passes the test runner", async () => {
  const { code, res, stderr } = await runner(fixture());

  assert.ok(res, stderr);
  assert.deepEqual(failed(res), []);
  assert.equal(code, 0);

  for (const id of CHECKED) assert.ok(ids(res, "pass").has(id), `${id} was checked and passed`);
});

test("SDK-03 the runner fails a plugin that skips rox.provision (RSV-01)", async () => {
  const { code, res } = await runner(fixture("--misbehave=skip-provision"));

  assert.equal(code, 1);
  assert.equal(res.ok, false);
  assert.ok(ids(res, "fail").has("RSV-01"), failed(res).join("\n"));
  assert.ok(ids(res, "fail").has("LCY-01"), "the broker's gate refuses it too");
});

test("SDK-04 the runner fails a plugin whose session certificates do not verify (RSV-03)", async () => {
  const { code, res } = await runner(fixture("--misbehave=bad-cert"));

  assert.equal(code, 1);
  assert.ok(ids(res, "fail").has("RSV-03"), failed(res).join("\n"));
  assert.ok(ids(res, "fail").has("BRG-03"), "the broker refuses the open");
  assert.ok(!ids(res, "fail").has("RSV-01"), "provisioning itself is fine");
});

test("SDK-05 the runner fails a plugin that prints its key, and an app that imports outside 6.3", async () => {
  const leak = await runner(fixture("--misbehave=log-seed"));

  assert.equal(leak.code, 1);
  assert.ok(ids(leak.res, "fail").has("RSV-03"), failed(leak.res).join("\n"));
  // an app importing nsh/device, its fingerprint signed correctly
  const dir = tempDir("sdk-import");
  const card = join(dir, "card.tsx");

  writeFileSync(card, `import { windowSize } from "nsh/device";\nexport default function Card() { return windowSize(); }\n`);
  const bad = await runner([...fixture(), "--env", `FX_APP_CARD=${card}`]);

  assert.equal(bad.code, 1);
  assert.deepEqual(failed(bad.res).map((f) => f.split(":")[0]), ["APP-03 card"]);
  assert.match(failed(bad.res)[0], /unsupported_import: nsh\/device/);
});

test("SDK-06 the README flow: sign-manifest.mjs --key-file, then the runner with --manifest", async () => {
  const dir = tempDir("sdk-readme");
  const signed = join(dir, "signed.json");
  const tdir = join(SDK, "template");

  await new Promise((resolve, reject) =>
    execFile(process.execPath, [join(ROOT, "server", "v1", "sign-manifest.mjs"), join(tdir, "manifest.json"), "--key-file", KEY, "--app", `hello=${join(tdir, "hello.tsx")}`, "--out", signed], (e) => (e ? reject(e) : resolve())));
  const { code, res } = await runner(["--command", process.execPath, "--args", TEMPLATE, "--manifest", signed, "--key-file", KEY]);

  assert.deepEqual(failed(res), []);
  assert.equal(code, 0);
  // the same signed manifest under another key is refused (MAN-01)
  const other = keyFile();
  const refused = await runner(["--command", process.execPath, "--args", TEMPLATE, "--manifest", signed, "--key-file", other]);

  assert.equal(refused.code, 1);
  assert.ok(ids(refused.res, "fail").has("MAN-01"));
});

// "256 KiB" → 262144, "60 chars" → 60
const UNIT = { KiB: 1024, MiB: 1024 * 1024 };

function amount(s) {
  const [, num, u] = /^(\d+)\s*(KiB|MiB)?/.exec(s.trim()) || [];

  return Number(num) * (UNIT[u] || 1);
}

// docs/manifest.md's limits table: `| <field> | <limit> | \`LIMITS.<key>\` |`
function documentedLimits() {
  const md = readFileSync(join(SDK, "docs", "manifest.md"), "utf8");
  const rows = [...md.matchAll(/^\|[^|\n]*\|\s*([^|\n]+?)\s*\|\s*`LIMITS\.(\w+)`\s*\|$/gm)];

  return rows.map(([, limit, key]) => {
    const range = /^(\S+(?:\s(?:KiB|MiB))?) to (\S+(?:\s(?:KiB|MiB))?)/.exec(limit);

    return [key, range ? [amount(range[1]), amount(range[2])] : amount(limit)];
  });
}

test("SDK-07 every limit in docs/manifest.md equals server/v1/manifest.mjs LIMITS, and every limit is documented", () => {
  const rows = documentedLimits();

  assert.deepEqual(rows.map(([k]) => k).sort(), Object.keys(LIMITS).sort(), "the docs table lists each LIMITS key once");

  for (const [k, v] of rows) assert.deepEqual(v, LIMITS[k], `LIMITS.${k}`);
});
