# Rox

A native macOS chat client — the Grok Shell desktop app ported from TSX to
Rust. The UI is NSH (`nsh/`, AppKit backend); live patching is ra-jit
(`ra-jit/`); the backend is the unchanged Grok app server.

- Bundle id: `dev.goldcoders.rox`
- App crate: `app/` (`rox`)

## Prerequisites

- macOS on Apple Silicon; Rust via `app/rust-toolchain.toml` (channel
  `1.99.0`, component `rust-src` — installed automatically by rustup).
- **ra-jit** on `PATH`: `cargo-ra-jit`, `ra-jit`, `ra-jit-rustc`,
  `ra-jit-rustc-first`. The live splice needs the fixes in
  [indexable-inc/ra-jit#237](https://github.com/indexable-inc/ra-jit/pull/237);
  until that lands, install from the patched branch:

  ```sh
  cargo install --path crates/ra-jit --force   # in a checkout of the PR head
  ```

- `cargo-watch` (`cargo install cargo-watch`).
- The backend: `grok-shell/server/app-server.mjs` listening on
  `http://127.0.0.1:8787` — start it the usual way (`node
  server/app-server.mjs` in the grok-shell checkout).

## Getting started

```sh
cd app
./dev.sh
```

`dev.sh` warms the ra-jit daemon (`cargo ra-jit build`), starts `cargo
watch -w src -x 'ra-jit build'`, then `cargo ra-jit run` opens the window.
The first cold load of a workspace takes a few minutes; after that the
daemon is warm.

## Live editing

Every save of a `src/*.rs` file is answered with a binary patch the
running app applies in-process — typically `applied: N functions` in the
status bar, under a millisecond, with all state (threads, transcript,
draft, scroll) intact.

Rules for patchable code (`src/hot.rs` and everything it pulls in — see
`.devin/skills/rox-live/SKILL.md` for the full list):

- Persistent state goes through `crate::live::store()` or
  `crate::live::keep("name", init)`. No `static`, `thread_local!` or
  `#[no_mangle]` data in patched modules.
- Don't edit `src/live.rs` while the app is live — it owns the store
  anchor. Type-layout changes (`Store` fields, public structs) need a
  restart.
- Plain `Hsla` literals in `const`s only — const eval of `to_hsla()` /
  `clamp` isn't in ra-jit's evaluator, and a function whose consts can't
  be evaluated goes to cargo, not a patch.

The live pill (bottom left, drag it anywhere, ⌘⇧R hides it) is the source
of truth: a status dot, ↻ and the remount count; hover it for ra-jit's last
report (`edit N applied: M functions …`, or why it needs a restart).

| dot | it means |
|---|---|
| green | live — the last edit spliced into this process |
| amber | restart needed (e.g. the program was linked again, or a type layout changed) — relaunch |
| red | the patch was declined; the app runs on as it was |
| grey | ra-jit's watcher did not start |

## Keyboard

`⌘K` bot search / palette · `⌘1`–`⌘9` chat switching (pinned first, then
groups alphabetically, then unassigned, then chats) · `⌘⇧H` full↔rail
sidebar · `⌘R` fresh reset (clears overlays, drafts, scroll; refetches) ·
`⌘⇧F` performance monitor · `⌘⇧K` clear it (and the jank log) · `⌘⇧R`
live pill.

## iOS simulator (compiles, not a supported target yet)

```sh
cd app
./ios-dev.sh            # or ./ios.sh for a one-shot launch
```

Same live loop for `aarch64-apple-ios-sim`: the watcher answers saves with
`cargo ra-jit build --target aarch64-apple-ios-sim`, patched live into the
running sim app. iOS is kept compiling but is not built or tested as part
of the desktop port.

## Driving the UI programmatically (gpui-agent)

The app embeds a gpui-agent host. Launch with the control plane on:

`GPUI_AGENT_ALLOW_RELEASE=1` is required: the dev profile builds
dependencies without debug assertions (see Build profile), which
gpui-agent's guard reads as a release build.

```sh
GPUI_AGENT=1 GPUI_AGENT_ALLOW_RELEASE=1 GPUI_AGENT_TOKEN=dev-secret ./target/debug/rox
# then: gpui-agent --addr 127.0.0.1:17421 hello | snapshot | click <id> |
# type <id> <text> | screenshot --out x.png | keybinding cmd+r | invoke …
```

## Build profile

`app/Cargo.toml` builds every dependency optimized even in dev builds:

```toml
[profile.dev.package."*"]
opt-level = 3
debug-assertions = false
```

Only the `rox` crate itself stays a debug build, so ra-jit keeps patching
it. Why:

- nsh's own workspace optimizes its hot crates in dev (QuickJS, taffy,
  objc2, syntect…), but Cargo applies profiles only from the root
  manifest — `app/` is its own workspace, so without this block every
  mini app's interpreter and every layout ran unoptimized.
- `debug-assertions = false` turns off objc2's per-message signature
  verification, which took ~65% of the main thread while switching chats
  (profiled: main thread 99% busy → 16%).

Costs: the first build after a clean rebuilds all dependencies optimized
(~8–9 min); after that only `rox` recompiles (~1–2 min, seconds for small
edits). Dependencies' internal `debug_assert!`s are off — when debugging
nsh itself, delete the block. For a fair comparison with other apps, use
`cargo build --release`.

## Performance tools

- **Monitor** — `⌘⇧F` (Window ▸ Performance Monitor) shows nsh's HUD:
  FPS (frames that changed; an idle app reads 0), main-thread cost per
  frame and its `drop` share, engine commit costs, CPU/GPU/memory, and a
  trace of the last 120 frames. Drag it to move it. `ROX_FPS=1` opens
  with it on.
- **Jank log** — while the monitor runs, every frame over budget and every
  engine wake-up over 16 ms logs one `nsh::jank` line naming what it did
  (apply / tick / input time, the mount items by kind, the triggering
  event). Rox appends them to `~/Library/Logs/Rox/jank.log`
  (`ROX_JANK_LOG` overrides) and to stderr.
- **Clear** — `⌘⇧K` (Window ▸ Clear Performance Monitor) forgets the
  readings and empties the jank log: reproduce, then read what that did.
- **`/perf`** — in Claude Code (launched from the `Projects` folder),
  `/perf` reads the jank log and reports causes worst first with the
  code each points at; `/perf clear` empties the log.

## Mermaid diagrams (Pele)

```` ```mermaid ```` fences render natively: `app/pele/serve.mjs` runs
[Pele](https://pele.run) (`pele.js`, vendored, MIT) behind a localhost
port, resolves its CSS variables to Rox's palette and rasterizes with
`@resvg/resvg-js` (nsh's SVG renderer draws no `<text>`), and Rox shows
the PNG as an image — no WKWebView per diagram. Rox starts the helper on
the first diagram and it exits with Rox.

- One-time setup: `cd app/pele && npm install` (pinned in
  `package.json`; `node_modules/` is ignored).
- Node is found at `/opt/homebrew/bin/node`, `/usr/local/bin/node` or on
  `PATH`; `ROX_NODE` overrides. `ROX_PELE_DIR` points at the helper's
  folder (a packaged app must ship `pele/` and set it).
- If the helper can't start or a diagram doesn't parse, the block falls
  back to mermaid.js in a web view (the server's `/mermaid` page). The
  full-screen lightbox still uses that page.

## Smoke build without ra-jit

```sh
cd app && cargo build --target aarch64-apple-darwin
```

## Layout

```
GOAL.md        the port spec and acceptance checks this was built against
app/           the Rox application (dev.sh / ios.sh / ios-dev.sh inside)
app/src/hot.rs the patchable UI root
app/src/live.rs the ra-jit bridge — store anchor; don't edit while live
app/src/store.rs the model, as anchored signals
app/src/mini.rs the mini-app host (scaled AppView per card)
app/src/mermaid.rs + app/pele/ native Mermaid via the Pele helper
nsh/           the UI engine (submodule; crates/nsh-platform-macos is the AppKit backend)
ra-jit/        the live-patching toolchain (submodule)
```
