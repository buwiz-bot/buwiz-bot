# Buwiz Bot

A native macOS chat client written in Rust. The UI is NSH (`nsh/`, AppKit backend); live
patching is ra-jit, from [blaze](https://github.com/indexable-inc/blaze)
(`blaze/`); the backend is the Rust broker in `broker/`.

- Bundle id: `com.buwiz.bot` (see [Identity and data](#identity-and-data))
- App crate: `app/` (`buwiz-bot`)

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
- The backend: the Rust broker (`broker/`, `buwiz-broker`) listening on
  `http://127.0.0.1:8787` — `cd broker && cargo build -p buwiz-broker`, then
  `target/debug/buwiz-broker --data <dir> --assets <dir>` (`--assets` is the
  `server` folder of a bundle's `Contents/Resources/broker/`;
  `app/bundle-dev.sh` makes one). With `--data`, start the app with
  `BUWIZ_HOST_TOKEN_FILE=<dir>/host-token` so it sends that broker's host
  token: the locked routes (`/v1/host/*`, `/bot/*`, `/skills`, `/mcp/servers`,
  ...) answer 401 without it. Its tests: `cd broker && cargo test
  --locked`, and the HTTP tests, plain `node` scripts that drive a built
  broker: `broker/buwiz-broker/tests/http/run.sh <buwiz-broker binary>`; only
  the office group needs `npm ci` in `office/` (`--core` leaves it out).

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
`.devin/skills/buwiz-live/SKILL.md` for the full list):

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
scripts/buwiz-drive snapshot          # click, type, screenshot, invoke …
```

The dev launcher makes a new random token at every launch and keeps it in
`$(getconf DARWIN_USER_TEMP_DIR)buwiz-gpui-agent-token`, readable only by you;
`scripts/buwiz-drive` passes it to `gpui-agent` through the environment, never
on a command line. To run a bare build with automation, do the same:
`GPUI_AGENT=1 GPUI_AGENT_ALLOW_RELEASE=1 GPUI_AGENT_TOKEN=<random>
GPUI_AGENT_ADDR=127.0.0.1:<port> ./target/debug/buwiz-bot`.

`GPUI_AGENT_ALLOW_RELEASE=1`: the dev profile builds dependencies without
debug assertions, which gpui-agent's guard reads as a release build.

The server is the `dev-agent` feature, on by default for dev and test builds.
A release build has no server: `scripts/release/build.sh` builds with
`--no-default-features`, and `--release` with the feature on doesn't compile.

The channel's clicks count as yours, so a test Bot behaves as if you pressed.
Set `GPUI_AGENT_ACTOR=agent` to drive it as the AI: writes then reach the mini
app as `origin: agent` until a real press in that surface, and a tool kept for
apps refuses them. Reads still work.

### The model's view of a screen (Agent Hub)

`ui_look` lets the broker's model see open mini app screens. It is off for every
Bot: turn on **Mini app screens** in that Bot's `/tools`. The Bot keeps a connection
to the broker (`/v1/host/ui/stream`, `/v1/host/ui/reply`) and opens no port.

The model gets each screen's name, which is on top, button labels and field names.
Page text, field values, and the words on a row of data stay in the Bot
(`app/src/agent_hub/filter.rs`). For fake data in a test Bot only,
`BUWIZ_HUB_FULL_TEXT=1` adds page text. A release build doesn't have this setting.

`ui_act` lets it press a named button and fill a named field on the screen on top.
It is off for every Bot too, with its own switch: **Act on mini app screens** in
`/tools`, which works only with **Mini app screens** on. Its input goes out as the
agent's, so what it starts reaches the mini app as `origin: agent`. Each action reads
the screen again first, and refuses a screen that changed since the model looked, a
covered screen, and a node with no name. The Bot holds back any write whose tool's
`buwiz/audience` doesn't name `agent`, unsent, and says why in the app and the chat.
A write that is open to agents still answers to that mini app's own switch for the
Bot: off refuses, Ask asks, and Allow there covers that one call. It also needs a
`ui_act` in flight on that mini app, so a test Bot driven with
`GPUI_AGENT_ACTOR=agent` gets no writes.
Keyboard shortcuts, presses at a point, the Bot's own window and the pasteboard are
out of its reach (`app/src/agent_hub/press.rs`).

## Dev app & links

`app/bundle-dev.sh` creates `~/Applications/Buwiz Bot.app` and registers it
with LaunchServices. The bundle (`com.buwiz.bot`) declares the `buwiz://`
URL scheme. Its executable (`buwiz-bot`)
`exec`s `Contents/MacOS/buwiz-bot-bin` when there is one, else
`app/target/debug/buwiz-bot`, with the
gpui-agent variables above and `BUWIZ_FPS=0`, so it picks up each cargo rebuild
without re-bundling. Run the script again only if the bundle is deleted or
its Info.plist changes, or to repackage the broker: it also puts
`buwiz-broker` and the static files it serves into
`Contents/Resources/broker/`. At launch, when nothing answers on
`127.0.0.1:8787` and it is no test instance (`BUWIZ_BACKEND`,
`BUWIZ_BACKGROUND=1` or `BUWIZ_DATA_DIR` set), Buwiz Bot starts that broker detached,
logging to `~/Library/Logs/Buwiz Bot/broker.log` (`app/src/broker.rs`).

### Environment variables

Every variable has a Buwiz name (`BUWIZ_DATA_DIR`, `BUWIZ_BACKEND`, …). A
variable of an earlier name (any `ROX_*`, or `GROK_DATA_DIR`,
`GROK_ALLOWED_ORIGINS`, `GROK_BUNDLE_ID`) is not read: the app and the broker
refuse to start while one is set, and say which. A test instance is one that
sets `BUWIZ_BACKEND`, `BUWIZ_BACKGROUND=1` or `BUWIZ_DATA_DIR`.

The broker gets `NSH`, `NSH_HOST` and `NSH_CHUNK_DIR` (`<data>/chunks`)
from Buwiz Bot: `bundle-dev.sh` packages `nsh` and `nsh-macos-host` into
`Contents/Resources/broker/bin/`, from `NSH_BIN`/`NSH_HOST_BIN`, else the
nsh submodule's release build (`nsh/target/release`).

```sh
app/bundle-dev.sh                       # once
open -a ~/Applications/Buwiz\ Bot.app   # launch Buwiz Bot through the bundle
open "buwiz://chat/t1?m=10"             # thread t1, turn 10
```

### Identity and data

- Data: `~/Library/Application Support/Buwiz Bot` (`BUWIZ_DATA_DIR`
  overrides). On the first launch, when it is missing and an earlier
  install's folder exists, that folder is renamed to it (copied, then
  removed, when the rename fails) and a marker is written in it. When both
  exist neither is touched (`app/src/identity.rs`).
- Keychain: `com.buwiz.bot.site-login` and `com.buwiz.bot.plugin-key`. Secrets
  an earlier install kept under other service names are moved once (read,
  written to the new service, deleted from the old); a marker says it is
  done (in the data dir, and beside the broker's `app.db`).
- Broker data: `~/Library/Application Support/Buwiz Broker`. An earlier
  folder of the broker's moves there once, by a rename, just before a broker
  starts on it: Buwiz Bot moves it before it starts its bundled broker,
  and a `buwiz-broker` started on the owner's folder (`scripts/broker-live`)
  moves it after binding its port. First the database is backed up
  (`VACUUM INTO`) to `backups/<UTC time>-pre-move/app.db` in the old folder,
  then the folder is renamed. Right after, in one transaction, every text
  value in the database holding a path under the old folder (plugin commands
  and env, icons, attachments, MCP servers' env and auth) is rewritten to the
  new one; if that fails the folder is renamed back. The old name is then
  left as a symlink to `Buwiz Broker`, so paths stored outside the database
  keep resolving, and a marker is written in the new folder. Nothing moves
  while a broker answers on 8787, when `Buwiz Broker` exists already (the
  old folder is then left alone), or for any other data dir (tests,
  benches). A rename that fails is not replaced by a copy: the old folder
  stays in use and the move is tried at the next start. Until it moves, the
  host token, `scripts/broker-live` and `scripts/live-copy` read the old
  folder (`broker/buwiz-protocol/src/broker_dir.rs`).
- Logs: `~/Library/Logs/Buwiz Bot`; caches: `~/Library/Caches/Buwiz Bot`.
- Crates and binaries: the app crate is `buwiz-bot`, the broker
  `broker/buwiz-broker`, its MCP client `broker/buwiz-mcp` and the protocol
  pieces `broker/buwiz-protocol`. The broker's subcommands are
  `buwiz-broker <subcommand>`.

### Switching the live broker to Buwiz Bot.app's

`scripts/broker-live use-bundled` stops whatever broker holds port 8787 and
starts Buwiz Bot.app's own `buwiz-broker` with the same command line Buwiz Bot uses.
Buwiz Bot then finds it answering, and later launches start the same broker if
it is not running.

1. `app/bundle-dev.sh`, then `ls ~/Applications/Buwiz\ Bot.app/Contents/Resources/broker/{buwiz-broker,bin/nsh,bin/nsh-macos-host}`.
2. `scripts/broker-live backup`. Check that it prints a
   `backups/<time>/` dir holding `app.db` and `buwiz-broker/`.
3. `scripts/broker-live use-bundled`. Check that it ends with
   `Buwiz Bot.app's buwiz-broker on 8787`. If the broker doesn't come up, the
   script stops the broker it started, says so and exits 1; nothing holds the port then.
4. `scripts/broker-live status`. Check:
   - `health: 200` and `mode: bundled`;
   - every `plugin:` line is running;
   - every `routine:` line ran again with no `error:`;
   - `keychain dialog: none`.
   Then build a widget in Buwiz Bot, and watch `~/Library/Logs/Buwiz\ Bot/broker.log`.

Rollback: the script keeps no other broker to go back to. Restore the
database and the bundle from the `backups/<time>/` folder `backup` made, and
run `use-bundled` again.

Links reach the running Buwiz Bot only when it was launched through the bundle. A
bare `./target/debug/buwiz-bot` is a different app to macOS, so a link would start
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

Only the `buwiz-bot` crate itself stays a debug build, so ra-jit keeps patching
it. Why:

- nsh's own workspace optimizes its hot crates in dev (QuickJS, taffy,
  objc2, syntect…), but Cargo applies profiles only from the root
  manifest — `app/` is its own workspace, so without this block every
  mini app's interpreter and every layout ran unoptimized.
- `debug-assertions = false` turns off objc2's per-message signature
  verification, which took ~65% of the main thread while switching chats
  (profiled: main thread 99% busy → 16%).

Costs: the first build after a clean rebuilds all dependencies optimized
(~8–9 min); after that only `buwiz-bot` recompiles (~1–2 min, seconds for small
edits). Dependencies' internal `debug_assert!`s are off — when debugging
nsh itself, delete the block. For a fair comparison with other apps, use
`cargo build --release`.

## Performance tools

- **Monitor** — `⌘⇧F` (Window ▸ Performance Monitor) shows nsh's HUD:
  FPS (frames that changed; an idle app reads 0), main-thread cost per
  frame and its `drop` share, engine commit costs, CPU/GPU/memory, and a
  trace of the last 120 frames. Drag it to move it. `BUWIZ_FPS=1` opens
  with it on.
- **Jank log** — while the monitor runs, every frame over budget and every
  engine wake-up over 16 ms logs one `nsh::jank` line naming what it did
  (apply / tick / input time, the mount items by kind, the triggering
  event). Buwiz Bot appends them to `~/Library/Logs/Buwiz Bot/jank.log`
  (`BUWIZ_JANK_LOG` overrides) and to stderr.
- **Clear** — `⌘⇧K` (Window ▸ Clear Performance Monitor) forgets the
  readings and empties the jank log: reproduce, then read what that did.
- **`/perf`** — in Claude Code (launched from the `Projects` folder),
  `/perf` reads the jank log and reports causes worst first with the
  code each points at; `/perf clear` empties the log.

## Mermaid diagrams (Pele)

```` ```mermaid ```` fences render natively: Pele, in Rust
([hexuria/pele-rs](https://github.com/hexuria/pele-rs), MIT) lays the
diagram out, and nsh's canvas draws the scene in Buwiz Bot's palette
(`app/src/mermaid/`) — no web view. A diagram Pele does not draw
shows Pele's message and line in its place.

## Smoke build without ra-jit

```sh
cd app && cargo build --target aarch64-apple-darwin
```

## Layout

```
app/           the Buwiz Bot application (dev.sh / ios.sh / ios-dev.sh inside)
broker/        the broker Buwiz Bot talks to (Rust workspace: buwiz-broker, buwiz-protocol, buwiz-mcp, buwiz-computer, ...): chats, models, routines, plugins, agent messaging; its HTTP tests (node scripts) are in broker/buwiz-broker/tests/http/
bridge/        buwiz-bridge, the MCP server an outside coding agent uses to talk with a Buwiz Bot agent (npm ci here)
office/        the office editor page and viewer, seeds, and the BetterOffice packages and fonts (npm ci here, then npm run build:office-editor)
broker/buwiz-broker/assets/  what the Rust broker compiles in or ships: templates, plugins, trust.json
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
Apache-2.0 too. The plugin catalog is fetched at runtime from
[buwiz-app/catalog](https://github.com/buwiz-app/catalog); the plugins it
lists carry their own licenses. They speak mini app protocol 1.0
([the spec](https://github.com/buwiz-app/buwiz-sdk/blob/main/docs/protocol-1.0.md)). See `NOTICE` for third-party code in this
repository.
