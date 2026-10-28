// app-server.mjs — the "AI" behind the generative-UI chat.
//
//   node server/app-server.mjs            # uses OPENROUTER_API_KEY if set
//   MOCK=1 node server/app-server.mjs     # canned modules, no network needed
//
// POST http://127.0.0.1:8787/chat {"prompt": "…"}
//
// Flow per message: record the user turn → the model runs a tool loop
// (generate_app, generate_image, web_search, fetch_url, query_source) →
// each widget is a generated/r<N>.tsx file typechecked in server/.stage →
// generated/index.tsx regenerates → nsh's dev server hot-patches the app, so
// cards appear as real native UI. Plain questions answer in text — UI is one
// tool among several, not the only reply format.
//
// Layout: this file is bootstrap + HTTP dispatch. Subsystems live beside it:
//   config.mjs   env/paths/catalog            state.mjs  db/store/jobs/persist
//   model.mjs    provider routing/prompts     pipeline.mjs  nsh runs/versions
//   tools.mjs    tool schema + executors      agent.mjs    tool loop/generate
//   windows.mjs  popouts/slots/screenshots    avatars.mjs   avatar generation
//   groups.mjs   the sidebar's user-made groups
//   attachments.mjs  `@` tokens attached to chats   routines.mjs  timed agent work
//   routes/      the ~80 HTTP endpoints, grouped by prefix
//   v1/          the Mini App Protocol v1 broker (plugins, sessions, /v1/host/*)

import { createServer } from "node:http";
import { execFile } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  renameSync,
  statSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  BASE_URL,
  CHUNK_DIR,
  GEN_DIR,
  MOCK,
  MODEL,
  OUT_DIR,
  PORT,
  PROVIDER,
  WIDGETS_DIR,
} from "./config.mjs";
import { db, persist, writeIndex, writeShelf } from "./state.mjs";
import { emitChunk, headOf, verFile, verSlug } from "./pipeline.mjs";
import { ensureOfficeMcp, officeCaps, officePkgRoot } from "./office.mjs";
import miscRoute from "./routes/misc.mjs";
import chatRoute from "./routes/chat.mjs";
import botsRoute from "./routes/bots.mjs";
import filesRoute from "./routes/files.mjs";
import appsRoute from "./routes/apps.mjs";
import artifactsRoute from "./routes/artifacts.mjs";
import marketRoute from "./routes/market.mjs";
import dataRoute from "./routes/data.mjs";
import groupsRoute from "./routes/groups.mjs";
import routinesRoute from "./routes/routines.mjs";
import v1Route, { initHostToken } from "./v1/routes.mjs";
import { plugins as v1Plugins } from "./v1/broker.mjs";
import { migrateGroups } from "./groups.mjs";
import { startRoutines } from "./routines.mjs";
import { allowedOrigins, refusal } from "./guard.mjs";

const routes = [
  v1Route,
  miscRoute,
  chatRoute,
  botsRoute,
  filesRoute,
  appsRoute,
  artifactsRoute,
  groupsRoute,
  routinesRoute,
  marketRoute,
  dataRoute,
];

// No wildcard CORS: an origin is answered only when it is allowed (the
// server's own pages, plus GROK_ALLOWED_ORIGINS, comma-separated), and any
// other Origin is refused before a route runs (guard.mjs).
const ALLOWED = allowedOrigins(PORT, process.env.GROK_ALLOWED_ORIGINS);

const server = createServer(async (req, res) => {
  const no = refusal(req, ALLOWED);

  if (no) {
    res.statusCode = no.status;
    res.setHeader("content-type", "application/json");

    return res.end(JSON.stringify({ ok: false, error: no.error }));
  }

  if (req.headers.origin) {
    res.setHeader("Access-Control-Allow-Origin", req.headers.origin);
    res.setHeader("Access-Control-Allow-Headers", "content-type");
    res.setHeader("Vary", "Origin");
  }

  if (req.method === "OPTIONS") {
    res.statusCode = 204;

    return res.end();
  }

  for (const h of routes) {
    try {
      if (await h(req, res) || res.writableEnded || res.headersSent) return;
    } catch (e) {
      console.error(`route error ${req.method} ${req.url}:`, e);

      if (!res.headersSent) {
        res.statusCode = 500;
        res.end(JSON.stringify({ error: String(e?.message || e) }));
      }

      return;
    }
  }

  res.statusCode = 404;
  res.end("not found");
});

// Rox's host token for /v1/host/* (v1/routes.mjs), fresh each start
initHostToken();

persist(); // seed the DB on first boot / after a JSON→SQLite migration

if (migrateGroups()) console.log("bot sections + pinned Mini Apps → sidebar groups");

writeShelf(); // sync widgets/index.tsx with the DB

// BetterOffice MCP child (office_* tools) — connected in the background; a
// failed spawn just leaves the surface out of toolList().
ensureOfficeMcp(OUT_DIR).catch(() => {});

// The editor bundle is gitignored (dist/) — build it once when the engines
// are installed and dist/editor.js is missing; until then renderFile hands
// out the read-only /office/ viewer URL instead.
{
  const editorJs = join(dirname(fileURLToPath(import.meta.url)), "office", "editor", "dist", "editor.js");

  if (!existsSync(editorJs) && officeCaps().docx) {
    execFile("npx", ["esbuild", "editor.jsx", "--bundle", "--format=esm", "--splitting", "--outdir=dist",
      "--loader:.wasm=file", "--loader:.ttf=file", "--loader:.otf=file", "--external:*.png", "--minify"],
      { cwd: join(dirname(fileURLToPath(import.meta.url)), "office", "editor") }, (e) => {
      if (e) console.error(`  office editor build: ${String(e.message || e).slice(0, 160)}`);
      else {
        const ed = join(dirname(fileURLToPath(import.meta.url)), "office", "editor");
        execFile("node", [join(ed, "patch-dist.mjs")], () => {});
        const g = join(ed, "dist", "generated");

        for (const pkg of ["docx", "xlsx", "pptx"]) {
          const src = officePkgRoot(pkg);

          if (src) execFile("cp", ["-R", join(src, "dist", "generated") + "/", g + "/"], () => {});
        }
      }
    });
  }
}

// Materialise generated/index.tsx so a fresh clone typechecks before the
// first generation lands (generated/ is gitignored — nothing tracks it).
writeIndex();

// Migrate pre-versioning artifacts: the file as it stands becomes v1, the
// head pointer opens on it. (Cards rendered before `v` existed resolve the
// bare name — the head mirror — which is what they'd show anyway.)
for (const f of readdirSync(GEN_DIR)) {
  const m = f.match(/^(R\d+)\.tsx$/);

  if (!m || headOf(m[1])) continue;

  try {
    const dir = join(GEN_DIR, m[1]);
    mkdirSync(dir, { recursive: true });
    const v1 = join(dir, `${verSlug(m[1])}.v1.tsx`);

    if (!verFile(m[1], 1)) copyFileSync(join(GEN_DIR, f), v1);
    db.prepare(
      "INSERT OR IGNORE INTO versions(name,v,parent,prompt,created_at) VALUES(?,1,NULL,NULL,?)",
    ).run(m[1], statSync(join(GEN_DIR, f)).mtimeMs);
    db.prepare("INSERT OR IGNORE INTO heads(name,v) VALUES(?,1)").run(m[1]);
  } catch {}
}

// Pre-slug version files (v<n>.tsx) rename to <slug>.v<n>.tsx.
for (const d of readdirSync(GEN_DIR)) {
  if (!/^R\d+$/.test(d)) continue;

  try {
    for (const x of readdirSync(join(GEN_DIR, d))) {
      const m = x.match(/^v(\d+)\.tsx$/);

      if (m)
        renameSync(
          join(GEN_DIR, d, x),
          join(GEN_DIR, d, `${verSlug(d)}.v${m[1]}.tsx`),
        );
    }
  } catch {}
}

writeIndex();

// Backfill runtime chunks for modules that predate them (the packaged app
// imports these through NSH_CHUNK_DIR; without a chunk its card shows
// "module missing"). Version files get their own chunks — a card pins the
// exact version it rendered.
(async () => {
  for (const dir of [GEN_DIR, WIDGETS_DIR]) {
    if (!existsSync(dir)) continue;

    for (const f of readdirSync(dir)) {
      const m = f.match(/^(R\d+)\.tsx$/);

      if (!m) continue;
      const key = dir === WIDGETS_DIR ? `${m[1]}w` : m[1];

      if (!existsSync(join(CHUNK_DIR, `${key}.js`)))
        await emitChunk(key, join(dir, f));
    }
  }

  for (const d of readdirSync(GEN_DIR)) {
    if (!/^R\d+$/.test(d)) continue;

    try {
      for (const vf of readdirSync(join(GEN_DIR, d))) {
        const vm = vf.match(/\.v(\d+)\.tsx$/);

        if (!vm) continue;
        const key = `${d}v${vm[1]}`;

        if (!existsSync(join(CHUNK_DIR, `${key}.js`)))
          await emitChunk(key, join(GEN_DIR, d, vf));
      }
    } catch {}
  }
})().catch(() => {});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`app-server on http://127.0.0.1:${PORT}`);
  console.log(
    MOCK ||
    !(PROVIDER === "anthropic"
      ? process.env.ANTHROPIC_API_KEY
      : process.env.OPENROUTER_API_KEY)
      ? "mode: MOCK (set ANTHROPIC_API_KEY or OPENROUTER_API_KEY for real generation)"
      : `mode: ${MODEL} via ${PROVIDER === "anthropic" ? "anthropic" : BASE_URL}`,
  );
  // routines tick from here on (overdue ones at once)
  startRoutines();
  // v1 plugins the user has enabled: spawned, provisioned and verified
  v1Plugins.startAll().catch(() => {});
});
