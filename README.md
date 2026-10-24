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

The status bar is the source of truth:

| it says | it means |
|---|---|
| `applied: N functions … remounts M — up T` | the edit spliced live into this process |
| `restart needed: the program was linked again` | the binary relinked but this process is orphaned — relaunch it (usually means the app started before the daemon's first link finished) |

## Keyboard

`⌘K` bot search / palette · `⌘1`–`⌘9` chat switching (pinned first, then
groups alphabetically, then unassigned, then chats) · `⌘⇧H` full↔rail
sidebar · `⌘R` fresh reset (clears overlays, drafts, scroll; refetches).

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

```sh
GPUI_AGENT=1 GPUI_AGENT_TOKEN=dev-secret ./target/debug/rox
# then: gpui-agent --addr 127.0.0.1:17421 hello | snapshot | click <id> |
# type <id> <text> | screenshot --out x.png | keybinding cmd+r | invoke …
```

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
nsh/           the UI engine (submodule; crates/nsh-platform-macos is the AppKit backend)
ra-jit/        the live-patching toolchain (submodule)
```
