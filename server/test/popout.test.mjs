// Pop-out windows (the Rust broker plan, item 6.3): POST /popout writes a
// mount wrapper for a module, builds it once with `nsh build -o` and
// starts `nsh-macos-host` on the bundle, one window per module (or pinned
// version); a second press raises the open window, a newer head replaces
// it, /app/version reopens it on the new head, and close_app ends it.
// AeroSpace floats a new window and pulls a raised one to the focused
// workspace. nsh, nsh-macos-host, osascript and aerospace are the PATH
// stubs: no window opens and no real window moves. The host stub stays
// alive until SIGTERM, as a window does.
//
//   POP-01 refusals: bad names, a bad body, no such module or version
//   POP-02 the head: the wrapper, one build, the host's arguments, the float
//   POP-03 open again: raised, not rebuilt
//   POP-04 sizes: popoutSize, a pinned version's badge, a caller's hints
//   POP-05 a closed window opens again from the cached bundle
//   POP-06 /app/version reopens the open window on the new head
//   POP-07 a failed build
//   POP-08 launch_app and close_app (the scripted model)
import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { startFakeOpencodex, say, use } from "./fixtures/fake-opencodex.mjs";
import { modelEnv, nodeIn, postJson, request, startServer, stubCalls, stubEnv, tempDir, waitFor } from "./helpers.mjs";

const dir = tempDir("popout");
const stubs = tempDir("popout-stubs");
const LOG = join(stubs, "stubs.log");
const WINDOWS = join(stubs, "aerospace-windows.txt");
const WIN = join(dir, "windows");
const CODE = {
  R1v1: 'import { Text } from "nsh";\n\nexport default function R() {\n  return <Text>one</Text>;\n}\n',
  R1v2: 'import { Text } from "nsh";\n\nexport const popoutWidth = 400;\nexport default function R() {\n  return <Text>two</Text>;\n}\n',
  R2: 'import { Text } from "nsh";\n\nexport const popoutSize = { w: 500, h: 420 };\nexport default function R() {\n  return <Text>timer</Text>;\n}\n',
  R3: 'import { Text } from "nsh";\n\nexport default function R() {\n  return <Text>plain</Text>;\n}\n',
  R9: 'import { Text } from "nsh";\n\n// nsh-stub:fail\nexport default function R() {\n  return <Text>broken</Text>;\n}\n',
};

let fake;
let server;

const calls = (bin) => stubCalls(LOG).filter((c) => c.bin === bin);
const hosts = () => calls("nsh-macos-host").filter((c) => c.argv);
const builds = () => calls("nsh").filter((c) => c.argv[0] === "build" && c.argv.includes("-o"));
const terms = () => calls("nsh-macos-host").filter((c) => c.signal === "SIGTERM").map((c) => c.pid);
const alive = (pid) => {
  try {
    process.kill(pid, 0);

    return true;
  } catch {
    return false;
  }
};
const pop = (body) => postJson("/popout", body);
const arg = (argv, flag) => argv[argv.indexOf(flag) + 1];
// the host logs itself once it runs, a moment after /popout answered
const hostOf = (pid) => waitFor(() => hosts().find((h) => h.pid === pid));

before(async () => {
  fake = await startFakeOpencodex();
  nodeIn(dir, `
    const { writeVersion } = await import("./src/pipeline.mjs");
    const { setAppCols } = await import("./src/appmeta.mjs");
    const { db } = await import("./src/state.mjs");
    const C = ${JSON.stringify(CODE)};
    writeVersion("R1", C.R1v1, null, "one");
    writeVersion("R1", C.R1v2, 1, "two");
    setAppCols("R1", { title: "Counter" });
    writeVersion("R2", C.R2, null, "timer");
    db.prepare("UPDATE versions SET label=? WHERE name=? AND v=?").run("first", "R2", 1);
    writeVersion("R3", C.R3, null, "plain");
    writeVersion("R9", C.R9, null, "broken");
    console.log("ok");
  `, stubEnv(LOG));
  writeFileSync(WINDOWS, "4242 widget · R1\n");
  server = await startServer(dir, { env: modelEnv(fake, LOG, { AEROSPACE_STUB_WINDOWS: WINDOWS, AEROSPACE_STUB_WORKSPACE: "3" }) });
});

after(async () => {
  await server?.stop();
  await fake?.close();
  // the hosts outlive the broker (detached, as a window does)
  for (const h of hosts()) if (alive(h.pid)) process.kill(h.pid, "SIGTERM");
});

test("POP-01 a bad name, a bad body, no such module or pinned version: no build, no window", async () => {
  for (const body of [{}, { name: "x" }, { name: "R1x" }, { name: "r1" }, { name: 7 }])
    assert.deepEqual((await pop(body)).json, { ok: false }, JSON.stringify(body));
  const bad = await request("POST", "/popout", { headers: { "content-type": "application/json" }, body: "{nope" });

  assert.equal(bad.status, 200);
  assert.deepEqual(bad.json, { ok: false });
  assert.equal(bad.headers["content-type"], "application/json");
  assert.deepEqual((await pop({ name: "R404" })).json, { ok: false, error: "no such module" });
  assert.deepEqual((await pop({ name: "R1", v: 9 })).json, { ok: false, error: "no such module" });
  assert.deepEqual(builds(), []);
  assert.deepEqual(hosts(), []);
});

test("POP-02 the head: its wrapper, built once, the host on the bundle at the module's width, floated by AeroSpace", async () => {
  const r = await pop({ name: "R1" });

  assert.equal(r.status, 200, r.text);
  assert.deepEqual(Object.keys(r.json), ["ok", "pid"]);
  assert.equal(r.json.ok, true);
  const wrapper = join(WIN, "pop-R1.tsx");
  const src = readFileSync(wrapper, "utf8");

  assert.match(src, /^import \{ mount, Text, View \} from "nsh";\n/);
  assert.match(src, /\nimport R, \* as mod from "\.\.\/generated\/R1";\n/);
  assert.match(src, /neither → 360pt wide, hugged height\./);
  assert.match(src, /\?\? 360\)\)\);\nconst capH = 675;\n/);
  assert.match(src, /Window\.setTitle\("widget · " \+ "R1"\);/);
  assert.match(src, /\nlet lastH = 0, pendingH = 0/);
  assert.match(src, /\{"v2" \? \(/);
  // the build directory is named for the wrapper's sha1: the same bytes
  // whichever broker wrote them
  const [b] = builds();

  assert.deepEqual(b.argv, ["build", wrapper, "-o", join(WIN, ".pop-R1-d3567deb6f")]);
  assert.ok(existsSync(join(WIN, ".pop-R1-d3567deb6f", "bundle.nshc")));
  const h = await hostOf(r.json.pid);

  assert.equal(hosts().length, 1);
  assert.deepEqual(h.argv, ["--bundle", join(WIN, ".pop-R1-d3567deb6f", "bundle.nshc"), "--name", "Counter · v2", "--app-id", "dev.goldcoders.grokshell.widget", "--window-size", "400x506"]);
  assert.ok(alive(h.pid));
  // the screen's height, asked once
  assert.equal(calls("osascript").filter((c) => c.argv.join(" ").includes("bounds of window of desktop")).length, 1);
  // AeroSpace: the window's id by its title, then float, move, focus
  const floated = await waitFor(() => calls("aerospace").some((c) => c.argv[0] === "focus"));

  assert.ok(floated, "the float poll focused the window");
  assert.deepEqual(calls("aerospace").map((c) => c.argv), [
    ["list-windows", "--all", "--format", "%{window-id} %{window-title}"],
    ["list-workspaces", "--focused"],
    ["layout", "floating", "--window-id", "4242"],
    ["move-node-to-workspace", "--window-id", "4242", "--focus-follows-window", "3"],
    ["focus", "--window-id", "4242"],
  ]);
});

test("POP-03 the same window again: raised to the focused workspace, not rebuilt or reopened", async () => {
  const before = stubCalls(LOG).length;
  const r = await pop({ name: "R1" });

  assert.deepEqual(r.json, { ok: true, already: true });
  await new Promise((r) => setTimeout(r, 300));
  // (the start's chunk backfill may still be building R9 with --out)
  assert.deepEqual(stubCalls(LOG).slice(before).filter((c) => c.bin !== "nsh" || c.argv.includes("-o")).map((c) => [c.bin, ...c.argv]), [
    ["aerospace", "list-windows", "--all", "--format", "%{window-id} %{window-title}"],
    ["aerospace", "list-workspaces", "--focused"],
    ["aerospace", "move-node-to-workspace", "--window-id", "4242", "--focus-follows-window", "3"],
    ["aerospace", "focus", "--window-id", "4242"],
  ]);
  assert.equal(hosts().length, 1);
});

test("POP-04 sizes: a popoutSize module pinned to a labelled version, and a caller's hints", async () => {
  const r2 = await pop({ name: "R2", v: 1, w: 300, h: 200 });

  assert.equal(r2.json.ok, true, r2.text);
  const src = readFileSync(join(WIN, "pop-R2-1.tsx"), "utf8");
  const slug = readdirSync(join(dir, "generated", "R2")).find((f) => f.endsWith(".v1.tsx")).slice(0, -4);

  assert.ok(src.includes(`import R, * as mod from "../generated/R2/${slug}";`));
  assert.ok(src.includes('Window.setTitle("widget · " + "R2@1");'));
  assert.ok(src.includes("?? 300)));\nconst capH = 675;"));
  assert.ok(src.includes("\nlet lastH = 420, pendingH = 0"));
  assert.ok(src.includes('{"v1 · first" ? ('));
  let h = await hostOf(r2.json.pid);

  assert.equal(arg(h.argv, "--name"), "R2 · v1");
  assert.equal(arg(h.argv, "--window-size"), "500x446");
  assert.match(arg(h.argv, "--bundle"), /\/windows\/\.pop-R2-1-[0-9a-f]{10}\/bundle\.nshc$/);
  // no declared size: the hints, clamped (width 240–720, height 120–¾ screen)
  const r3 = await pop({ name: "R3", w: 512.4, h: 1000 });

  assert.equal(r3.json.ok, true, r3.text);
  h = await hostOf(r3.json.pid);
  assert.equal(arg(h.argv, "--window-size"), "512x701");
  assert.equal(arg(h.argv, "--name"), "R3 · v1");
  const src3 = readFileSync(join(WIN, "pop-R3.tsx"), "utf8");

  assert.ok(src3.includes("?? 512.4)));\nconst capH = 675;"));
  assert.ok(src3.includes("\nlet lastH = 675, pendingH = 0"));
  assert.ok(src3.includes('{"v1" ? ('));
});

test("POP-05 a window that closed opens again from its cached bundle", async () => {
  const pid = hosts().find((h) => arg(h.argv, "--window-size") === "512x701").pid;

  process.kill(pid, "SIGTERM");
  assert.ok(await waitFor(() => !alive(pid)));
  // the broker sees the exit
  await new Promise((r) => setTimeout(r, 300));
  const n = builds().length;
  const r = await pop({ name: "R3", w: 512.4, h: 1000 });

  assert.equal(r.json.ok, true, r.text);
  assert.notEqual(r.json.pid, pid);
  assert.equal(builds().length, n, "no second build");
  assert.equal(arg((await hostOf(r.json.pid)).argv, "--window-size"), "512x701");
});

test("POP-06 /app/version: the open window is replaced by one on the new head, at its old hints", async () => {
  const old = hosts().find((h) => arg(h.argv, "--name") === "Counter · v2").pid;
  const n = hosts().length;
  const r = await postJson("/app/version", { name: "R1", v: 1 });

  assert.deepEqual(r.json, { ok: true, v: 1 });
  assert.ok(await waitFor(() => hosts().length > n), "a new window opened");
  assert.ok(await waitFor(() => terms().includes(old)), "the old window got SIGTERM");
  const h = hosts().at(-1);

  // v1 declares no width and the first press sent no hints: POP_W x POP_H
  assert.deepEqual([arg(h.argv, "--name"), arg(h.argv, "--window-size")], ["Counter · v1", "360x506"]);
  assert.match(readFileSync(join(WIN, "pop-R1.tsx"), "utf8"), /\{"v1" \? \(/);
  // a version the window already shows: nothing reopens
  assert.deepEqual((await postJson("/app/version", { name: "R1", v: 1 })).json, { ok: true, v: 1 });
  await new Promise((r) => setTimeout(r, 300));
  assert.equal(hosts().length, n + 1);
  // a head that moved under the window: the next press replaces it
  assert.deepEqual((await postJson("/app/version", { name: "R1", v: 2 })).json, { ok: true, v: 2 });
  assert.ok(await waitFor(() => hosts().length === n + 2));
  assert.equal(arg(hosts().at(-1).argv, "--name"), "Counter · v2");
});

test("POP-07 a wrapper that does not build: the build's last line, no window", async () => {
  const n = hosts().length;
  const r = await pop({ name: "R9" });

  assert.deepEqual(r.json, { ok: false, error: "build failed: error TS2304: Cannot find name 'nope'." });
  await new Promise((r) => setTimeout(r, 300));
  assert.equal(hosts().length, n);
});

test("POP-08 launch_app opens a window through /popout; close_app ends every window of the app", async () => {
  fake.script("pop-launch", [use("launch_app", { name: "R2" }), say("Opened.")]);
  fake.script("pop-close", [use("close_app", { name: "R2" }), say("Closed.")]);
  fake.script("pop-none", [use("close_app", { name: "R3x" }), say("No.")]);
  const thread = (await postJson("/thread/new", {})).json.id;
  const ask = async (prompt) => {
    fake.reset();
    const { id } = (await postJson("/chat", { prompt, thread })).json;

    assert.ok(await waitFor(async () => (await request("GET", `/status?id=${id}`)).json.done, 30000));

    return fake.calls().at(-1).body.messages.at(-1).content.find((b) => b.type === "tool_result");
  };
  const n = hosts().length;
  const opened = await ask("[[script:pop-launch]] open the timer");

  assert.equal(opened.content, "R2 opened in its own window.");
  assert.ok(!opened.is_error);
  assert.ok(await waitFor(() => hosts().length === n + 1));
  const r2 = hosts().filter((h) => arg(h.argv, "--name").startsWith("R2 ")).map((h) => h.pid).filter(alive);

  assert.equal(r2.length, 2, "R2 head and R2@1 are open");
  const closed = await ask("[[script:pop-close]] close it");

  assert.equal(closed.content, "closed 2 window(s).");
  assert.ok(await waitFor(() => r2.every((pid) => terms().includes(pid))));
  assert.equal((await ask("[[script:pop-none]] close nothing")).content, "bad artifact name R3x");
});
