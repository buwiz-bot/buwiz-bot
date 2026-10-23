# GOAL — finish the Grok Shell → Rust port

You are completing a port of the Grok Shell desktop app from TSX to native
Rust. Work until the acceptance checks at the bottom pass.

## Lay of the land

| What | Where |
|---|---|
| The Rust app being built | `/Volumes/goldcoders/Projects/rox/app` (`rox` / `dev.goldcoders.rox`) |
| The TSX app being ported (source of truth) | `/Volumes/goldcoders/Projects/grok-shell` |
| The UI engine (submodule, AppKit backend) | `/Volumes/goldcoders/Projects/rox/nsh` |
| Live patching (submodule) | `/Volumes/goldcoders/Projects/rox/ra-jit` |
| The backend server — **unchanged** | `grok-shell/server/app-server.mjs` at `http://127.0.0.1:8787` |
| Live-edit rules — **read before editing `src/`** | `rox/.devin/skills/rox-live/SKILL.md` |

The scaffold already runs: sidebar, transcript (`virtual_list`), composer,
model/effort picker, palette, detail pane, file viewer, `TextView`
markdown — all as `Signal`s in one anchored `Store` (`src/live.rs` +
`src/store.rs`), talking to the server over `src/client.rs`.

## Hard requirements

### 1. On-demand TSX compiled as mini apps

Grok Shell's generated widgets (`generated/*.tsx`, served to the TSX app
as runtime chunks) must **compile on demand and render as live mini
apps** — inline inside turns and in the widgets shelf, plus popout
windows. This is the headline feature; do not settle for placeholders.

- Compile: `nsh-bundler` (`compile_module` / the bundler pipeline) turns
  the TSX source into an nsh `Bundle`; or reuse the server's compiled
  chunk output. Pick whichever keeps imports of `nsh:*` modules working.
- Mount inline: register a native component (`native("rox/mini-app")`)
  whose macOS view is an `AppView` driven by `AppSource::Bundle` — it
  embeds in the Rust tree with no layering problems, like `web_view`.
  Each mini app gets its own engine+QuickJS; the Rust root stays pure.
- Popouts: a separate `AppView` in its own `NSWindow` for "Open window" /
  "Pin to desktop".
- Preserve the widget behaviors end to end: `/popout`, `/retry`, `/pin`,
  `/reuse`, `/unpublish`, `/widget-meta` (rename/tags/archive), version
  labels, and the card's action bar (view code, copy source, history).

### 2. Avatar animations — 100% native Rust

No WebView, no JS. Port the entire blob/mark animation stack to Rust and
drive it on `canvas()` with the engine's animator:

- `app/mark/motion.mjs` → `MarkEngine` (`setBody`/`setState`/`setGaze`/
  `poke`/`advance`/`frame`) + `MarkDirector` (state→clip table `yl`,
  morph table `vx`, transitions, holds, `next` chains, resting mode).
- `app/mark/markatlas.mjs` + `atlas.data.ts`/`rest.data.ts` → atlas
  decode and the baked data (port as generated Rust constants or decode
  at startup; do not re-bake).
- `app/mark/markpaint.ts` → the rasterizer behind `canvas()`.
- `app/blob.tsx` → `BotBlob`/`BotAvatar`/`SleepingBlob`/`CroppedImage`:
  30 fps clock (~33 ms), gaze following the pointer, pokes on press,
  compact/emphasized variants, edit-dim, rest frames while idle.
- `app/statusfx.tsx` `drawFx` → the 13 morph overlays (dots, orbit,
  radar, progress, gather, wave, send, receive, dock, ball, pencil,
  bang, standby) with their exact params, plus `ShimmerText`.
- Wire `state` to the server run/agent status everywhere an avatar
  appears (sidebar agents, header, composer, runs).

### 3. Feature + UI/UX parity (desktop)

Port everything the TSX app does, matching its metrics and behaviors —
the leaf-file port spec in this repo's history/`.devin` notes has the
exact numbers (radii, sizes, colors, keymaps, endpoint payloads):

- Transcript: streaming markdown, date separators, reactions, replies,
  turn context menus, scroll-follow while streaming.
- Composer: paste/drop attachments, `@`-agent mentions, reply banner,
  send/stop, draft restore.
- Files: `FileTile`/`FileCard`/`AudioCard`/`MediaGallery`, the full file
  viewer (code + gutter + highlight, JSON tree, sheets with cell
  selection and ⌘C TSV copy, office/pdf/audio/image via WebView where
  the TSX used one), lightbox with zoom/filmstrip/mermaid.
- Plugins overlay: market, connected, plugin, MCP, and skill pages with
  search dropdown, install/uninstall, tool toggles, OAuth retry.
- Bots: creation modal with live blob preview and shape/color grids,
  `/bot/new`.
- Chrome: palette with fuzzy search + ⌘1-9 hints + full keyboard nav,
  native-feeling context menus (incl. the widgets `Menu.popup` items),
  hidden title bar + drag regions, theme toggle honoring the `tc()`
  light remap and OnFill rule, account popover, agent presence/status
  surfaces, popout windows.
- The 97-icon set: port `ICONS` path data to canvas drawing (drop the
  glyph fallbacks).
- Keep the ra-jit live-edit loop working: new persistent state goes in
  the store/anchors, per `rox-live`.

### 4. Desktop only

macOS/AppKit only for now. Keep `cfg(target_os = "ios")` code compiling,
but do not build or test iOS targets.

## Out of scope

- Changing `server/app-server.mjs` or its endpoints.
- New features not in grok-shell.
- iOS, Android, web.

## Acceptance checks

1. `cargo build --target aarch64-apple-darwin` clean; the app sends a
   message and streams a reply end to end against the live server.
2. Asking for a widget produces a turn card whose **mini app renders
   and interacts inline**, and `/popout` opens it in its own window.
3. Avatars visibly animate through run states (think/orbit/progress at
   least), track the pointer, and rest when idle — all native drawing.
4. Every overlay listed above opens and matches the TSX app's layout in
   side-by-side screenshots (capture both apps on the same thread).
5. `./dev.sh` still splices an edit live with state preserved
   (`applied: N functions`, uptime uninterrupted).

Work module by module, keep the tree compiling, commit each working
piece, and verify visuals with `screencapture` against the running TSX
app.
