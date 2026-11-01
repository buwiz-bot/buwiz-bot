# Buwiz Bot

A native macOS chat client — the desktop app formerly called Grok Shell,
ported from TSX to Rust. The UI is NSH (`nsh/`, AppKit backend); live
patching is ra-jit, from [blaze](https://github.com/indexable-inc/blaze)
(`blaze/`); the backend is the unchanged app server.

- Bundle id: `com.buwiz.bot` (`dev.goldcoders.rox` before the rename; see
  [Identity and data](#identity-and-data))
- App crate: `app/` (`rox`)

## Prerequisites

- macOS on Apple Silicon; Rust via `app/rust-toolchain.toml` (channel
  `1.99.0`, component `rust-src` — installed automatically by rustup).
- **ra-jit** ([blaze](https://github.com/indexable-inc/blaze)) on `PATH`:
  `cargo-ra-jit`, `ra-jit`, `ra-jit-rustc`, `ra-jit-rustc-first`. Install it
  from the `blaze/` submodule, which is on the fork's `rox` branch (blaze
  `main` plus Buwiz Bot's patches). Blaze builds with its own pinned nightly,
  whatever `app/rust-toolchain.toml` says:

  ```sh
  rustup toolchain install nightly-2026-10-25 --component rust-src
  cd blaze && cargo +nightly-2026-10-25 install --locked --path crates/ra-jit --force
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

```sh
open -a ~/Applications/Buwiz\ Bot.app  # the dev bundle (app/bundle-dev.sh) turns automation on
scripts/rox-agent snapshot            # click, type, screenshot, invoke …
```

The dev launcher makes a new random token at every launch and keeps it in
`$(getconf DARWIN_USER_TEMP_DIR)rox-gpui-agent-token`, readable only by you;
`scripts/rox-agent` passes it to `gpui-agent` through the environment, never
on a command line. To run a bare build with automation, do the same:
`GPUI_AGENT=1 GPUI_AGENT_ALLOW_RELEASE=1 GPUI_AGENT_TOKEN=<random>
GPUI_AGENT_ADDR=127.0.0.1:<port> ./target/debug/rox`.

`GPUI_AGENT_ALLOW_RELEASE=1`: the dev profile builds dependencies without
debug assertions, which gpui-agent's guard reads as a release build.

## Dev app & links

`app/bundle-dev.sh` creates `~/Applications/Buwiz Bot.app` and registers it
with LaunchServices. The bundle (`com.buwiz.bot`) declares the `buwiz://`
URL scheme. Its executable (`buwiz-bot`)
`exec`s `Contents/MacOS/buwiz-bot-bin` when there is one, else
`app/target/debug/rox`, with the
gpui-agent variables above and `ROX_FPS=0`, so it picks up each cargo rebuild
without re-bundling. Run the script again only if the bundle is deleted or
its Info.plist changes, or to repackage the broker: it also puts
`rox-broker` and the static files it serves into
`Contents/Resources/broker/`. At launch, when nothing answers on
`127.0.0.1:8787` and it is no test instance (`ROX_BACKEND`,
`ROX_BACKGROUND=1` or `ROX_DATA_DIR` set), Buwiz Bot starts that broker detached,
logging to `~/Library/Logs/Buwiz Bot/broker.log` (`app/src/broker.rs`).

The broker gets `NSH`, `NSH_HOST` and `NSH_CHUNK_DIR` (`<data>/chunks`)
from Buwiz Bot: `bundle-dev.sh` packages `nsh` and `nsh-macos-host` into
`Contents/Resources/broker/bin/`, from `NSH_BIN`/`NSH_HOST_BIN`, else the
nsh submodule's release build (`nsh/target/release`), else the old Grok Shell
app's `Grok Shell.app/Contents/Resources/server/`.

```sh
app/bundle-dev.sh                       # once
open -a ~/Applications/Buwiz\ Bot.app   # launch Buwiz Bot through the bundle
open "buwiz://chat/t1?m=10"             # thread t1, turn 10
```

### Identity and data

Buwiz Bot was Rox (`dev.goldcoders.rox`, `~/Applications/Rox.app`, whose
releases were withdrawn). What a Rox install kept is moved over once, and
nothing of Rox's is kept for compatibility:

- Data: `~/Library/Application Support/Buwiz Bot` (`ROX_DATA_DIR`
  overrides). On the first launch, when it is missing and Rox's `…/Rox`
  exists, Rox's folder is renamed to it (copied, then removed, when the
  rename fails) and `.migrated-from-rox` is written in it. When both exist
  neither is touched (`app/src/identity.rs`).
- Keychain: `com.buwiz.bot.site-login` and `com.buwiz.bot.plugin-key`. The
  secrets under Rox's `dev.goldcoders.rox.*` services are moved once (read,
  written to the new service, deleted from the old); a marker says it is
  done (`.keychain-moved-from-rox` in the data dir,
  `.plugin-keys-moved-from-rox` beside the broker's `app.db`).
- Logs: `~/Library/Logs/Buwiz Bot`; caches: `~/Library/Caches/Buwiz Bot`
  (the icon font Rox cached in `…/Rox` is used until then).
- `bundle-dev.sh` unregisters `~/Applications/Rox.app` from LaunchServices;
  delete it yourself.

### Switching the live broker to Buwiz Bot.app's

`scripts/broker-live use-rox` stops whatever broker holds port 8787 and
starts Buwiz Bot.app's own `rox-broker` with the same command line Buwiz Bot uses.
Buwiz Bot then finds it answering, and later launches start the same broker if
it is not running.

1. `app/bundle-dev.sh`, then `ls ~/Applications/Buwiz\ Bot.app/Contents/Resources/broker/{rox-broker,bin/nsh,bin/nsh-macos-host}`.
2. `scripts/broker-live backup`. Check that it prints a
   `backups/<time>/` dir holding `app.db`, `server/` and `rox-broker/`.
3. `scripts/broker-live use-rox`. Check that it ends with
   `Buwiz Bot.app's rox-broker on 8787`. If the broker doesn't come up, the
   script starts Node again and exits 1.
4. `scripts/broker-live status`. Check:
   - `health: 200` and `mode: rox`;
   - every `plugin:` line is running;
   - every `routine:` line ran again with no `error:`;
   - `keychain dialog: none`.
   Then build a widget in Buwiz Bot, and watch `~/Library/Logs/Buwiz\ Bot/broker.log`.

Rollback: `scripts/broker-live use-node` brings back the Node broker, and
`use-rust` brings back the rox-broker in `Grok Shell.app` with Node behind
it. Both need `Grok Shell.app`'s `Resources/server/` (`RES`). After either, run
`status` again and check that `mode:` reads `node` or `rust`, then make the
same checks as in step 4.

Links reach the running Buwiz Bot only when it was launched through the bundle. A
bare `./target/debug/rox` is a different app to macOS, so a link would start
a second copy. A link opened while Buwiz Bot is closed launches it and opens the
link. The app receives links with nsh's `on_open_url` (registered in the
`App` root, `src/lib.rs`), which hands `buwiz://chat/…` links to
`Store::open_link`.

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
  event). Buwiz Bot appends them to `~/Library/Logs/Buwiz Bot/jank.log`
  (`ROX_JANK_LOG` overrides) and to stderr.
- **Clear** — `⌘⇧K` (Window ▸ Clear Performance Monitor) forgets the
  readings and empties the jank log: reproduce, then read what that did.
- **`/perf`** — in Claude Code (launched from the `Projects` folder),
  `/perf` reads the jank log and reports causes worst first with the
  code each points at; `/perf clear` empties the log.

## Mermaid diagrams (Pele)

```` ```mermaid ```` fences render natively: Pele ported to Rust
([hexuria/pele-rs](https://github.com/hexuria/pele-rs), MIT) lays the
diagram out, and nsh's canvas draws the scene in Buwiz Bot's palette
(`app/src/mermaid/`) — no Node, no web view. A diagram Pele does not draw
shows Pele's message and line in its place.

## Smoke build without ra-jit

```sh
cd app && cargo build --target aarch64-apple-darwin
```

## Layout

```
app/           the Buwiz Bot application (dev.sh / ios.sh / ios-dev.sh inside)
server/        the broker Buwiz Bot talks to (Node): chats, models, routines, plugins, agent messaging
app/src/hot.rs the patchable UI root
app/src/live.rs the ra-jit bridge — store anchor; don't edit while live
app/src/store.rs the model, as anchored signals
app/src/mini.rs the mini-app host (scaled AppView per card)
app/src/mermaid/ native Mermaid via pele-rs and nsh's canvas
nsh/           the UI engine (submodule; crates/nsh-platform-macos is the AppKit backend)
blaze/         ra-jit, the live-patching toolchain (submodule)
```

## License

Buwiz Bot is free and open source under the [Apache License 2.0](LICENSE)
(`NOTICE` holds the copyright line). The bundled UI engine `nsh`
(longbridge/nsh, through the hexuria/nsh fork) and `blaze` (ra-jit) are
Apache-2.0 too. Third-party logos and plugin catalog entries under
`server/src/plugins/` name other companies' products; their marks belong to
their owners.
