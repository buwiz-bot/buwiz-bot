// pipeline.mjs — the build side: `nsh check` in .stage, import fixing,
// runtime chunk emission, and the generated/<name>/v<k>.tsx version tree.
import { execFile } from "node:child_process";
import {
  copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, renameSync,
  rmSync, unlinkSync, writeFileSync,
} from "node:fs";
import { basename, dirname, join } from "node:path";
import { createHash } from "node:crypto";
import { CHUNK_DIR, GEN_DIR, NSH, STAGE_DIR, WIDGETS_DIR } from "./config.mjs";
import { db, slugFile, widgetRow, writeIndex, writeShelf } from "./state.mjs";

export function runNsh(args) {
  return new Promise((resolve) => {
    execFile(NSH, args, { timeout: 90000 }, (err, stdout, stderr) =>
      resolve(err ? String(stdout) + String(stderr) : null),
    );
  });
}

// nsh names the model is allowed to use without thinking about imports —
// inject the ones it forgets instead of failing the module.
const CORE_NAMES = [
  "batch", "cleanup", "computed", "context", "effect", "Input", "mount",
  "Scroll", "signal", "split", "Text", "untracked", "View",
];
const MODULE_NAMES = {
  Canvas: "nsh/canvas",
  Image: "nsh/image",
  List: "nsh/list",
  Video: "nsh/video",
  WebView: "nsh/webview",
  Button: "nsh/ui/mobile",
  Card: "nsh/ui/mobile",
  Keyboard: "nsh/ui/mobile",
  KeyboardAvoidingView: "nsh/ui/mobile",
  SafeAreaView: "nsh/ui/mobile",
  StatusBar: "nsh/ui/mobile",
  TextField: "nsh/ui/mobile",
};

export function fixImports(code) {
  // Drop imports nsh can't resolve (react, react-native, …).
  code = code.replace(
    /^[ \t]*import[^\n]*from\s*["'](react|react-native|solid-js|preact|vue|svelte)[^\n]*\n/gm,
    "",
  );
  const imported = new Set();
  for (const m of code.matchAll(/import\s*\{([^}]*)\}\s*from\s*["'][^"']+["']/g))
    for (const s of m[1].split(","))
      imported.add(s.trim().split(/\s+as\s+/)[0].trim());
  const missing = (n) => new RegExp(`\\b${n}\\b`).test(code) && !imported.has(n);
  const core = CORE_NAMES.filter(missing);
  const byMod = new Map();
  for (const [n, mod] of Object.entries(MODULE_NAMES))
    if (missing(n)) (byMod.get(mod) ?? byMod.set(mod, []).get(mod)).push(n);

  if (
    core.length &&
    /import\s*\{[^}]*\}\s*from\s*["']nsh["']/.test(code)
  )
    code = code.replace(
      /import\s*\{([^}]*)\}\s*from\s*["']nsh["']/,
      (m, names) =>
        `import { ${[...names.split(",").map((s) => s.trim()), ...core].join(
          ", ",
        )} } from "nsh"`,
    );
  else if (core.length)
    code = `import { ${core.join(", ")} } from "nsh";\n` + code;

  const extra = [...byMod].map(
    ([mod, names]) => `import { ${names.join(", ")} } from "${mod}";`,
  );
  if (extra.length) code = extra.join("\n") + "\n" + code;
  return code;
}

// Compiles one module file into a runtime-loadable chunk
// (CHUNK_DIR/<name>.js). `nsh build` on the module alone produces a bundle
// whose namespace's `default` is the component — the chunk shape
// __nsh_import resolves. Best-effort: a failure leaves the card on its
// "module missing" path, same as before chunks existed.
export async function emitChunk(name, srcFile) {
  const out = join(STAGE_DIR, `chunk-${name}`);
  rmSync(out, { recursive: true, force: true });
  const err = await runNsh(["build", srcFile, "--out", out]);
  if (err)
    return console.log(
      `chunk ${name}: ${String(err).trim().split("\n").pop()}`,
    );
  try {
    copyFileSync(join(out, "bundle.js"), join(CHUNK_DIR, `${name}.js`));
  } catch {}
}

// workspace files (OUT_DIR recursively) as {name, rel} for the @ picker.
export function walkFiles(dir, base = dir, out = []) {
  try {
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      const p = join(dir, e.name);
      if (e.isDirectory()) walkFiles(p, base, out);
      else out.push({ name: e.name, rel: p.slice(base.length + 1) });
    }
  } catch {}
  return out;
}

export const headOf = (name) =>
  db.prepare("SELECT v FROM heads WHERE name=?").get(name)?.v ?? null;
// Version files are named <slug>.v<n>.tsx inside generated/<name>/ — slug
// is the artifact's file label (or title slug, else the R-name). Renaming
// the artifact's file name or title batch-renames every version's file.
export const verSlug = (name) => {
  const meta =
    db.prepare("SELECT file,title FROM apps WHERE name=?").get(name) || {};
  // apps.file/title slugs carry their ".tsx" — the version file appends
  // ".v<n>" before it.
  return (meta.file || slugFile(meta.title) || `${name}.tsx`).replace(
    /\.tsx$/i,
    "",
  );
};
export const verFile = (name, v) => {
  const dir = join(GEN_DIR, name);
  try {
    const f = readdirSync(dir).find((x) =>
      new RegExp(`\\.v${v}\\.tsx$`).test(x),
    );
    return f ? join(dir, f) : null;
  } catch {
    return null;
  }
};
export const versionsOf = (name) =>
  db
    .prepare(
      "SELECT v,parent,created_at,label FROM versions WHERE name=? ORDER BY v",
    )
    .all(name);
// Chunk ids must be flat <stem>.js (nsh-js validates) — "R6@v3" ships as
// "R6v3.js". R-names are digits-only so the mapping can't collide.
export const chunkKey = (key) => key.replace(/^R(\d+)@v(\d+)$/, "R$1v$2");

// A generated artifact's immutable version files live under
// generated/<name>/v<k>.tsx; generated/<name>.tsx mirrors the head so every
// consumer that names the bare artifact (baked index, /module, /popout)
// sees current. A revise off a non-head version parents the new version
// there — a branch, never a rewrite.
export function writeVersion(name, code, parent, prompt, label) {
  const dir = join(GEN_DIR, name);
  mkdirSync(dir, { recursive: true });
  const v =
    (db.prepare("SELECT MAX(v) AS m FROM versions WHERE name=?").get(name)
      ?.m || 0) + 1;
  const vFile = join(dir, `${verSlug(name)}.v${v}.tsx`);
  writeFileSync(vFile, code);
  db.prepare(
    "INSERT INTO versions(name,v,parent,prompt,created_at,label) VALUES(?,?,?,?,?,?)",
  ).run(
    name, v, parent ?? null,
    String(prompt || "").slice(0, 500) || null, Date.now(),
    String(label || "").trim().slice(0, 40) || null,
  );
  db.prepare("INSERT OR REPLACE INTO heads(name,v) VALUES(?,?)").run(name, v);
  writeFileSync(join(GEN_DIR, `${name}.tsx`), code);
  // A published Mini App is the same artifact — its file mirrors head too.
  if (widgetRow(name)) writeFileSync(join(WIDGETS_DIR, `${name}.tsx`), code);
  emitChunk(chunkKey(`${name}@v${v}`), vFile);
  emitChunk(name, join(GEN_DIR, `${name}.tsx`));
  return v;
}

// The current pointer moves; nothing is rewritten — later versions stay on
// the tree as siblings of whatever branches off the reverted one.
export function setHead(name, v) {
  const file = verFile(name, v);
  if (!file || !existsSync(file)) return false;
  db.prepare("INSERT OR REPLACE INTO heads(name,v) VALUES(?,?)").run(name, v);
  const code = readFileSync(file, "utf8");
  writeFileSync(join(GEN_DIR, `${name}.tsx`), code);
  if (widgetRow(name)) writeFileSync(join(WIDGETS_DIR, `${name}.tsx`), code);
  emitChunk(name, join(GEN_DIR, `${name}.tsx`));
  writeIndex();
  writeShelf();
  return true;
}

// Compile + typecheck the staged candidate; the app on the device is untouched
// until this passes. Entry/output are per-module so concurrent chats don't
// clobber each other's validation.
export async function checkCandidate(name) {
  const entry = join(STAGE_DIR, `check-${name}.tsx`);
  const mod = join(STAGE_DIR, `${name}.tsx`);
  writeFileSync(
    entry,
    `import { mount } from "nsh";\nimport R from "./${name}";\nmount(R);\n`,
  );
  const buildErr = await runNsh([
    "build",
    entry,
    "--out",
    join(STAGE_DIR, `out-${name}`),
  ]);
  const err = buildErr ?? (await runNsh(["check", entry]));
  // The stage dir's tsconfig includes every sibling — a broken leftover
  // rejects the NEXT module's check too. Stage files are intermediates only
  // (the real one lands in generated/), so remove the pair as soon as the
  // verdict is in.
  try {
    rmSync(entry);
    rmSync(mod);
  } catch {}
  return err;
}

export function errorCard(note) {
  return `import { Text, View } from "nsh";

export default function R() {
  return (
    <View class="gap-2 rounded-xl border border-red-200 bg-red-50 p-3">
      <Text class="text-sm font-semibold text-red-700">generation failed</Text>
      <Text class="text-xs text-red-600">{${JSON.stringify(
        note.slice(0, 600),
      )}}</Text>
    </View>
  );
}
`;
}

// ————— tool-call agent —————
// One /chat turn = a model loop with real tools. Widgets are one tool among
// several: the model answers plainly when no UI is needed, and can search,
// read pages, query local data, or generate images — then optionally build a
// widget from what it found. Module names are server-assigned; modules write
// the literal "__NAME__" where their /state key goes and it's substituted on
// write (the contract below tells the model that).


export function mockModule(prompt) {
  const p = prompt.toLowerCase();
  if (/counter|count|stepper/.test(p))
    return `import { signal, Text, View } from "nsh";

export default function R() {
  const n = signal(0);
  return (
    <View class="items-center gap-3 py-2">
      <Text class="text-5xl font-bold text-zinc-900">{n()}</Text>
      <View class="flex-row gap-3">
        <View onPress={() => n.update((v) => v - 1)} activeOpacity={0.6} class="rounded-lg bg-zinc-200 px-5 py-2"><Text class="text-lg">−</Text></View>
        <View onPress={() => n.update((v) => v + 1)} activeOpacity={0.6} class="rounded-lg bg-blue-600 px-5 py-2"><Text class="text-lg font-semibold text-white">+</Text></View>
      </View>
    </View>
  );
}
`;
  if (/chart|graph|bar|week/.test(p))
    return `import { Text, View } from "nsh";

const days = ["M", "T", "W", "T", "F", "S", "S"];
const vals = [42, 68, 35, 90, 74, 20, 55];

export default function R() {
  return (
    <View class="gap-2 py-2">
      <Text class="text-sm font-semibold text-zinc-800">Sessions this week</Text>
      <View class="h-32 flex-row items-end gap-2">
        <Each of={vals}>{(v, i) => (
          <View class="flex-1 items-center gap-1">
            <View class="w-full rounded-t-md bg-blue-500" style={{ height: v() }} />
            <Text class="text-[10px] text-zinc-500">{days[i()]}</Text>
          </View>
        )}
        </Each>
      </View>
    </View>
  );
}
`;
  if (/form|book|sign|input/.test(p))
    return `import { Input, signal, Text, View } from "nsh";

export default function R() {
  const name = signal("");
  const when = signal("");
  const sent = signal(false);
  return (
    <View class="gap-2 py-1">
      <Text class="text-sm font-semibold text-zinc-800">Book a table</Text>
      <Input value={name()} onChangeText={(t) => name.set(t)} placeholder="Name" class="rounded-lg border border-zinc-300 px-3 py-2 text-sm" />
      <Input value={when()} onChangeText={(t) => when.set(t)} placeholder="Friday, 8 pm" class="rounded-lg border border-zinc-300 px-3 py-2 text-sm" />
      <View onPress={() => sent.set(true)} activeOpacity={0.7} class="items-center rounded-lg bg-blue-600 py-2">
        <Text class="text-sm font-semibold text-white">Reserve</Text>
      </View>
      <If when={sent()}><Text class="text-xs text-green-600">Reserved for {name() || "—"}, {when() || "—"} ✓</Text></If>
    </View>
  );
}
`;
  if (/todo|task|check|list/.test(p))
    return `import { signal, Text, View } from "nsh";

const items0 = ["Ship nsh demo", "Write docs", "Test on iOS"];
const done = signal([false, false, false]);

export default function R() {
  return (
    <View class="gap-1 py-1">
      <Each of={items0}>{(t, i) => (
        <View onPress={() => done.update((d) => d.map((v, j) => (j === i() ? !v : v)))} activeOpacity={0.6} class="flex-row items-center gap-2 py-1">
          <View classList={{ "bg-blue-600 border-blue-600": done()[i()] }} class="h-4 w-4 rounded border border-zinc-400" />
          <Text classList={{ "line-through text-zinc-400": done()[i()] }} class="text-sm text-zinc-800">{t()}</Text>
        </View>
      )}
      </Each>
    </View>
  );
}
`;
  if (/stock|nvda|aapl|ticker|price/.test(p))
    return `import { Text, View } from "nsh";

export default function R() {
  return (
    <View class="gap-1 py-1">
      <View class="flex-row items-baseline justify-between">
        <Text class="text-base font-bold text-zinc-900">NVDA</Text>
        <Text class="text-base font-semibold text-green-600">▲ 8.6%</Text>
      </View>
      <Text class="text-3xl font-bold text-zinc-900">$186.42</Text>
      <Text class="text-xs text-zinc-500">NVIDIA Corp · close Fri · mock data</Text>
    </View>
  );
}
`;
  return `import { signal, Text, View } from "nsh";

export default function R() {
  const liked = signal(false);
  return (
    <View class="gap-2 py-1">
      <Text class="text-sm text-zinc-700">{${JSON.stringify(
        `You asked for: ${prompt}`,
      )}}</Text>
      <View onPress={() => liked.update((v) => !v)} activeOpacity={0.6} class="self-start rounded-lg bg-zinc-100 px-3 py-1.5">
        <Text class="text-sm">{liked() ? "♥ liked" : "♡ like"}</Text>
      </View>
    </View>
  );
}
`;
}
