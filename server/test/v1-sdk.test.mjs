// The Mini App developer kit (sdk/mini-app): the test runner passes the
// template and the fixture, fails the fixture when it breaks a rule on
// purpose, follows the README's sign-then-test flow, and the documented
// manifest limits are the broker's. Extension IDs SDK-01.. of
// `npm run conformance`. The runner runs as a child process, the way a
// developer runs it; it opens no port and uses only temp directories.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { generate, pubOf } from "../src/v1/ed.mjs";
import { LIMITS } from "../src/v1/manifest.mjs";
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

/** Runs the runner with `args` and --json: {code, res, stderr}. `env`
 *  adds to the environment (ROX_TRUST_FILE); without it the runner sees
 *  none of the test run's own. */
function runner(args, env = {}) {
  const { ROX_TRUST_FILE, ...rest } = process.env;

  void ROX_TRUST_FILE;

  return new Promise((resolve) => {
    execFile(process.execPath, ["--disable-warning=ExperimentalWarning", RUNNER, ...args, "--json"], { cwd: ROOT, timeout: 120_000, env: { ...rest, ...env } }, (e, stdout, stderr) => {
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
    execFile(process.execPath, [join(ROOT, "src", "v1", "sign-manifest.mjs"), join(tdir, "manifest.json"), "--key-file", KEY, "--app", `hello=${join(tdir, "hello.tsx")}`, "--out", signed], (e) => (e ? reject(e) : resolve())));
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

test("SDK-07 every limit in docs/manifest.md equals src/v1/manifest.mjs LIMITS, and every limit is documented", () => {
  const rows = documentedLimits();

  assert.deepEqual(rows.map(([k]) => k).sort(), Object.keys(LIMITS).sort(), "the docs table lists each LIMITS key once");

  for (const [k, v] of rows) assert.deepEqual(v, LIMITS[k], `LIMITS.${k}`);
});

test("SDK-08 with ROX_TRUST_FILE the runner checks that store as a broker does: key id and approved list", async () => {
  const dir = tempDir("sdk-trust");
  const pub = pubOf(readFileSync(KEY, "utf8").trim());
  const store = (approved) => {
    const file = join(dir, `trust-${approved.length}.json`);

    writeFileSync(file, JSON.stringify({ v: 1, publishers: { "test-sdk-08": { publisher: "dev.goldcoders", alg: "ed25519", public_key: pub } }, approved_plugins: approved }));

    return file;
  };
  const args = ["--command", process.execPath, "--args", TEMPLATE, "--sign-with", KEY, "--key-id", "test-sdk-08"];
  // without it: the run's own trust, which approves the plugin for the run
  const own = await runner(args);

  assert.equal(own.code, 0);
  assert.deepEqual(own.res.trust, { store: null, key_id: "test-sdk-08", key_from: "the --sign-with key", approved: true });
  // a store that trusts the key but does not approve the plugin: refused as a broker refuses it
  const unapproved = await runner(args, { ROX_TRUST_FILE: store([]) });

  assert.equal(unapproved.code, 1);
  assert.match(unapproved.res.rows.find((r) => r.id === "MAN-01" && r.status === "fail").note, /not on the approved list for key test-sdk-08/);
  assert.equal(unapproved.res.trust.approved, false);
  // approved under that key id: it passes, and the report names the store
  const file = store([{ id: "dev.goldcoders.hello", publisher_key_id: "test-sdk-08" }]);
  const approved = await runner(args, { ROX_TRUST_FILE: file });

  assert.deepEqual(failed(approved.res), []);
  assert.deepEqual(approved.res.trust, { store: file, key_id: "test-sdk-08", key_from: "ROX_TRUST_FILE", approved: true });
  // another key id than the store knows: not a trusted publisher key
  const wrongKey = await runner(["--command", process.execPath, "--args", TEMPLATE, "--sign-with", KEY, "--key-id", "test-other"], { ROX_TRUST_FILE: file });

  assert.match(wrongKey.res.rows.find((r) => r.id === "MAN-01").note, /test-other is not a trusted publisher key/);
  assert.equal(wrongKey.res.trust.key_from, null);
});

test("SDK-09 the v1 command-line tools run when started through a symlinked folder", async () => {
  // on macOS anything under $TMPDIR is such a path (/var -> /private/var)
  const link = join(mkdtempSync(join(tmpdir(), "sdk-link-")), "server");

  symlinkSync(ROOT, link);

  for (const tool of ["src/v1/sign-manifest.mjs", "src/v1/publisher-keygen.mjs", "sdk/mini-app/bin/rox-plugin-test.mjs"]) {
    const { code, stderr } = await new Promise((resolve) =>
      execFile(process.execPath, ["--disable-warning=ExperimentalWarning", join(link, tool)], (e, stdout, err) => resolve({ code: e ? e.code : 0, stderr: err })));

    assert.notEqual(code, 0, `${tool} ran nothing through ${link}`);
    assert.match(stderr, /usage/, tool);
  }
});

test("SDK-10 BRG-03 paces an app's calls to the broker's 20 a second: 25 tools all answer", async () => {
  const dir = tempDir("sdk-many");
  const tools = Array.from({ length: 25 }, (_, i) => `many_t${i}`);

  copyFileSync(join(SDK, "template", "hello.tsx"), join(dir, "many.tsx"));
  writeFileSync(join(dir, "manifest.json"), JSON.stringify({
    protocol: "1.0.0", id: "dev.goldcoders.many", short: "many", version: "0.1.0", title: "Many",
    publisher: { id: "dev.goldcoders", key_id: "test-sdk-10", name: "Goldcoders" },
    apps: [{
      id: "many", title: "Many", description: "", entry: "rox-app://many", mime: "text/x-nsh-tsx", sdk: "1",
      fingerprint: `sha256:${"0".repeat(64)}`, placement: ["full"], permissions: { tools },
    }],
  }));
  writeFileSync(join(dir, "server.mjs"), `
    import { ok, startPlugin, ticketStore } from ${JSON.stringify(join(SDK, "lib", "rox-plugin.mjs"))};
    await startPlugin({
      name: "many", manifestFile: process.env.ROX_PLUGIN_MANIFEST || ${JSON.stringify(join(dir, "manifest.json"))},
      apps: { many: ${JSON.stringify(join(dir, "many.tsx"))} }, tickets: ticketStore(), ticketTitle: (n) => n,
      tools: ${JSON.stringify(tools)}.map((name) => ({ name, description: name, inputSchema: { type: "object" }, annotations: { readOnlyHint: true }, run: () => ok({ name }) })),
    });
  `);
  const { code, res, stderr } = await runner(["--command", process.execPath, "--args", join(dir, "server.mjs"), "--sign-with", KEY]);

  assert.ok(res, stderr);
  const calls = res.rows.filter((r) => r.id === "BRG-03" && / reaches the plugin/.test(r.what));

  assert.equal(calls.length, 25);
  assert.deepEqual(calls.filter((r) => r.status !== "pass").map((r) => `${r.what}: ${r.note}`), []);
  assert.deepEqual(failed(res), []);
  assert.equal(code, 0);
});

test("SDK-11 BRG-03's pacer holds when the broker counts a call after the gate let it go (a slow first call)", async () => {
  const { pacer } = await import(RUNNER);
  const pace = pacer(3, 300);
  const counted = [];
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  // the broker counts each call this long after it leaves the gate
  for (const lag of [150, 0, 0, 0, 0, 0, 0])
    await pace(async () => {
      await sleep(lag);
      counted.push(Date.now());
    });

  const crowded = counted.filter((t) => counted.filter((u) => u <= t && t - u < 300).length > 3).map((t) => t - counted[0]);

  assert.deepEqual(crowded, [], `more than 3 calls in 300 ms as the broker counts: ${counted.map((t) => t - counted[0])}`);
});

test("SDK-12 INV-04 asks an attachable scope to resolve only a ticket its own completion offered, and skips when it offered none", async () => {
  const dir = tempDir("sdk-scope");

  copyFileSync(join(SDK, "template", "hello.tsx"), join(dir, "quiet.tsx"));
  writeFileSync(join(dir, "manifest.json"), JSON.stringify({
    protocol: "1.0.0", id: "dev.goldcoders.quiet", short: "quiet", version: "0.1.0", title: "Quiet",
    publisher: { id: "dev.goldcoders", key_id: "test-sdk-12", name: "Goldcoders" },
    apps: [{
      id: "quiet", title: "Quiet", description: "", entry: "rox-app://quiet", mime: "text/x-nsh-tsx", sdk: "1",
      fingerprint: `sha256:${"0".repeat(64)}`, placement: ["full"], permissions: { tools: [] },
    }],
    commands: [{ name: "quiet", args: "", description: "Open Quiet", resolve: "quiet_resolve" }],
    scopes: [{ name: "handoff", description: "A hand-off", resolve: "quiet_resolve", attach: true }],
  }));
  // the command's open always carries a ticket; the scope has nothing to
  // offer and refuses any ticket it did not offer itself
  writeFileSync(join(dir, "server.mjs"), `
    import { ok, startPlugin, ticketStore } from ${JSON.stringify(join(SDK, "lib", "rox-plugin.mjs"))};
    const tickets = ticketStore();
    const offered = new Set();
    function resolve(args) {
      if (args.kind === "command") return ok({ result: "open", app: "quiet", props: { ticket: tickets.issue("home") }, title: "Quiet", placement: "full" });
      if (args.complete) return ok({ result: "choices", choices: [] });
      if (!offered.has(args.value)) return ok({ result: "error", message: "That is not a hand-off." });
      return ok({ result: "token", ticket: args.value, title: "Hand-off" });
    }
    await startPlugin({
      name: "quiet", manifestFile: process.env.ROX_PLUGIN_MANIFEST || ${JSON.stringify(join(dir, "manifest.json"))},
      apps: { quiet: ${JSON.stringify(join(dir, "quiet.tsx"))} }, tickets, ticketTitle: (n) => n,
      tools: [{ name: "quiet_resolve", description: "Resolve /quiet and @quiet:handoff.", inputSchema: { type: "object" }, annotations: { readOnlyHint: true }, run: resolve }],
    });
  `);
  const { code, res, stderr } = await runner(["--command", process.execPath, "--args", join(dir, "server.mjs"), "--sign-with", KEY]);

  assert.ok(res, stderr);
  const chosen = res.rows.filter((r) => r.id === "INV-04" && /a chosen completion/.test(r.what));

  assert.deepEqual(chosen.map((r) => r.status), ["skip"]);
  assert.deepEqual(failed(res), []);
  assert.equal(code, 0);
});

test("SDK-13 INV-10 checks a starter flow's prompt answer: a command's, non-blank, at most 4000 characters, from a 1.1 plugin", async () => {
  const dir = tempDir("sdk-prompt");
  const manifest = (protocol) => ({
    protocol, id: "dev.goldcoders.starter", short: "starter", version: "0.1.0", title: "Starter",
    publisher: { id: "dev.goldcoders", key_id: "test-sdk-13", name: "Goldcoders" },
    apps: [{
      id: "starter", title: "Starter", description: "", entry: "rox-app://starter", mime: "text/x-nsh-tsx", sdk: "1",
      fingerprint: `sha256:${"0".repeat(64)}`, placement: ["full"], permissions: { tools: [] },
    }],
    commands: [{ name: "draft", args: "", description: "Start a draft", resolve: "starter_resolve" }],
    scopes: [{ name: "note", description: "A note", resolve: "starter_resolve" }],
  });

  copyFileSync(join(SDK, "template", "hello.tsx"), join(dir, "starter.tsx"));
  writeFileSync(join(dir, "manifest-1.1.json"), JSON.stringify(manifest("1.1.0")));
  writeFileSync(join(dir, "manifest-1.0.json"), JSON.stringify(manifest("1.0.0")));
  // a bare /draft is a starter flow: text for the composer. MODE=bad
  // answers it over the cap, and answers prompt for the scope too; SRC
  // serves another manifest
  writeFileSync(join(dir, "server.mjs"), `
    import { ok, startPlugin } from ${JSON.stringify(join(SDK, "lib", "rox-plugin.mjs"))};
    const bad = process.env.MODE === "bad";
    function resolve(args) {
      if (args.kind === "scope") return ok(bad ? { result: "prompt", text: "Note this" } : args.complete ? { result: "choices", choices: [] } : { result: "error", message: "No such note." });
      if (args.value === "") return ok({ result: "prompt", text: bad ? "x".repeat(4001) : "Draft a note to the team about:\\n- " });
      return ok({ result: "open", app: "starter", title: "Starter", placement: "full" });
    }
    await startPlugin({
      name: "starter", manifestFile: process.env.ROX_PLUGIN_MANIFEST || process.env.SRC || ${JSON.stringify(join(dir, "manifest-1.1.json"))},
      apps: { starter: ${JSON.stringify(join(dir, "starter.tsx"))} },
      tools: [{ name: "starter_resolve", description: "Resolve /draft and @starter:note.", inputSchema: { type: "object" }, annotations: { readOnlyHint: true }, run: resolve }],
    });
  `);
  const server = ["--command", process.execPath, "--args", join(dir, "server.mjs"), "--sign-with", KEY];
  const prompts = (res) => res.rows.filter((r) => r.id === "INV-10").map((r) => [r.status, r.note]);

  // a 1.1 plugin: the bare command's prompt passes
  const good = await runner(server);

  assert.ok(good.res, good.stderr);
  assert.deepEqual(failed(good.res), []);
  assert.deepEqual(prompts(good.res), [["pass", "34 characters"]]);
  assert.equal(good.code, 0);

  // over the cap, and a scope's prompt (complete and not): INV-10 fails
  const bad = await runner([...server, "--env", "MODE=bad"]);

  assert.ok(bad.res, bad.stderr);
  assert.deepEqual(prompts(bad.res), [
    ["fail", "a prompt's text is 4001 characters; at most 4000"],
    ["fail", "a prompt result answers a command, not a scope"],
    ["fail", "a prompt result answers a command, not a scope"],
  ]);
  assert.deepEqual(failed(bad.res).filter((f) => !f.startsWith("INV-10")), [], "the other checks pass: the shape is 9.2's");
  assert.equal(bad.code, 1);

  // a 1.0 plugin cannot answer prompt: a 1.0 host would not read it
  const old = await runner([...server, "--env", `SRC=${join(dir, "manifest-1.0.json")}`]);

  assert.ok(old.res, old.stderr);
  assert.deepEqual(prompts(old.res), [["fail", "the manifest declares protocol 1.0.0; a prompt answer needs 1.1.0 or later"]]);
});
