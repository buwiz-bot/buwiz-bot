// config.mjs — env-derived paths and constants shared by every module.
import { mkdirSync, readdirSync, rmSync, writeFileSync, readFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { execFile } from "node:child_process";

const SERVER_DIR = dirname(fileURLToPath(import.meta.url));
const APP_ROOT = join(SERVER_DIR, "..");
// Runtime assets resolved beside the server code — server/ in dev, the
// bundled Resources/server/ in the packaged app.
const serverFile = (rel) => join(SERVER_DIR, rel);
// Packaged mode: GROK_DATA_DIR points at a writable dir (~/Library/Application
// Support/Grok Shell) — generated/, widgets/, windows/, .stage all live there
// instead of inside the read-only .app bundle. widgets/ is seeded from the
// shipped copy on first launch (the launcher does the copy).
const DATA_DIR = process.env.GROK_DATA_DIR || APP_ROOT;
const GEN_DIR = join(DATA_DIR, "generated");
// Candidates are compiled+typechecked here first — OUTSIDE the app's import
// graph — so a bad module never reaches the dev server (no red overlay flash).
const STAGE_DIR = join(DATA_DIR, ".stage");
const PORT = Number(process.env.PORT || 8787);
// Providers: Claude goes through opencodex's Anthropic Messages endpoint by
// default (OPENCODEX_URL — it owns the real key; the client key is a dummy).
// GEN_PROVIDER=openai + GEN_BASE_URL/GEN_MODEL selects an OpenAI-compatible
// backend instead. Direct-to-Anthropic escape hatch: set ANTHROPIC_BASE_URL
// (+ ANTHROPIC_API_KEY) explicitly — never inferred, so a stray env key can
// no longer silently bypass the gateway.
const PROVIDER = process.env.GEN_PROVIDER || "anthropic";
const BASE_URL =
  process.env.GEN_BASE_URL || "https://openrouter.ai/api/v1";
const MODEL =
  process.env.GEN_MODEL ||
  (PROVIDER === "anthropic" ? "claude-haiku-5-5" : "openai/gpt-4o-mini");
// Output budget covers interleaved thinking (~5k) plus a real module — a
// polished game runs 400+ lines; 6k truncated mid-file and produced mocks.
const MAX_TOKENS = Number(process.env.GEN_MAX_TOKENS || 20000);
const MOCK = !!process.env.MOCK;
// nsh binary for stage typechecks and pop-outs: $NSH, else `nsh` on PATH.
// Needs the patched fork (Window.setFrame/setLevel, onContextMenu —
// upstream longbridge/nsh lacks them); put your build on PATH or point NSH
// (and NSH_HOST for the dev-host binary) at it.
const NSH = process.env.NSH || "nsh";
const NSH_HOST =
  process.env.NSH_HOST ||
  (NSH.includes("/") ? join(dirname(NSH), "nsh-macos-host") : "nsh-macos-host");
// Bundle id the packaged app and its pop-out windows share (nsh.json's "id").
const BUNDLE_ID = process.env.GROK_BUNDLE_ID || "dev.goldcoders.grokshell";
const OPENCODEX = process.env.OPENCODEX_URL || "http://localhost:8080";

const WIDGETS_DIR = join(DATA_DIR, "widgets");
const POP_W = process.env.POP_W || "360";
const POP_H = process.env.POP_H || "480";
const WINDOWS_DIR = join(DATA_DIR, "windows");
// Compiled module chunks the packaged app imports at runtime — generated
// code runs in the host's JS, but its own modules resolve through
// __nsh_import("<name>.js"), which the host serves from NSH_CHUNK_DIR (set
// per window below).
const CHUNK_DIR = join(DATA_DIR, "chunks");

const AVATAR_DIR = join(GEN_DIR, "avatars");
const ATTACH_DIR = join(GEN_DIR, "attachments");
// files a reply produced (Grok's file cards), one folder per turn
const OUT_DIR = join(GEN_DIR, "files");
const ASSETS_DIR = join(GEN_DIR, "assets");

const PLUGIN_CATALOG = (() => {
  try { return JSON.parse(readFileSync(join(APP_ROOT, "server/plugins/marketplace.json"), "utf8")); }
  catch { return { plugins: [] }; }
})();
// Grok Bot's plugin catalog snapshot (tools/sync-grokbot-catalog.mjs):
// {source, fetchedAt, plugins:[entry + key + install hints]}.
const GROKBOT_CATALOG = (() => {
  try { return JSON.parse(readFileSync(serverFile("plugins/grokbot-catalog.json"), "utf8")); }
  catch { return { plugins: [] }; }
})();
const PLUGINS_DIR = join(DATA_DIR, "plugins");
const git = (args) =>
  new Promise((ok, fail) =>
    execFile("git", args, { timeout: 120000 }, (err, out) => (err ? fail(err) : ok(out))),
  );

const outFile = (rel) => {
  const p = join(OUT_DIR, rel);
  return p.startsWith(OUT_DIR + "/") ? p : null;
};

mkdirSync(GEN_DIR, { recursive: true });
mkdirSync(STAGE_DIR, { recursive: true });
mkdirSync(AVATAR_DIR, { recursive: true });
mkdirSync(ATTACH_DIR, { recursive: true });
mkdirSync(OUT_DIR, { recursive: true });
// Stale stage files (crashed checks, old candidates) sit inside the stage
// tsconfig's include — one broken sibling fails every later check. Clear
// check entries, candidates and outputs left over from earlier runs.
for (const f of readdirSync(STAGE_DIR))
  if (f.endsWith(".tsx") || f.startsWith("out-"))
    rmSync(join(STAGE_DIR, f), { recursive: true, force: true });
// `nsh check` needs a tsconfig.json in the candidate's directory. It extends
// .nsh/tsconfig.json — the build step (which always runs first) regenerates
// .nsh/ and its types there — and scopes its include to the stage files
// themselves (a broken leftover would otherwise fail every later check).
writeFileSync(
  join(STAGE_DIR, "tsconfig.json"),
  JSON.stringify({
    extends: "./.nsh/tsconfig.json",
    include: ["*.ts", "*.tsx"],
  }) + "\n",
);

export {
  APP_ROOT, SERVER_DIR, serverFile, DATA_DIR, GEN_DIR, STAGE_DIR, PORT, PROVIDER, BASE_URL, MODEL,
  MAX_TOKENS, MOCK, NSH, NSH_HOST, BUNDLE_ID, OPENCODEX, WIDGETS_DIR, POP_W,
  POP_H, WINDOWS_DIR, CHUNK_DIR, AVATAR_DIR, ATTACH_DIR, OUT_DIR, ASSETS_DIR,
  PLUGIN_CATALOG, GROKBOT_CATALOG, PLUGINS_DIR, git, outFile,
};
