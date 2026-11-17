# Buwiz Bot

A native chat app in Rust on Native Shell (nsh). `app/` is the app,
`broker/` the broker it talks to (Rust), `nsh/` the nsh fork it builds on
(a submodule), `connectors/` and `blaze/` (ra-jit, a submodule) its
helpers. `office/` holds the office editor and viewer pages, seeds and the
BetterOffice packages (`npm ci` there; `npm run build:office-editor` builds the
editor the bundles ship); `broker/buwiz-broker/assets/` the templates, bundled
plugins and trust list; `bridge/` is buwiz-bridge, the MCP server an outside
coding agent runs to talk with a Buwiz Bot agent (`npm ci` there). `README.md`
explains the build and the live loop.

## Private files

This repo is public. Plans, specs, briefs and status notes go under
`drafts/`, which git ignores; the private stow repo tracks them in place
(`stow add <path>`, then `stow push`; it lives in `.stow/`). Keep machine
paths, client data and secrets out of tracked files.

## Changing nsh

Anything under `nsh/` goes through the fork's process in `NSH-FORK.md`: a
topic branch, the registry `nsh-patches.toml`, `scripts/nsh-fork`. Read it
before the first edit there. Commits on `nsh/`'s `rox` branch or `main`
are lost at the next rebuild.

## Working on the app

- Build with `cd app && cargo build -p buwiz-bot`. Format by hand: rustfmt on the
  app crate rewrites dozens of untouched files.
- Install a build into the dev bundle by removing the old binary first
  (`rm` then `cp` to `~/Applications/Buwiz Bot.app/Contents/MacOS/buwiz-bot-bin`): a
  copy over it keeps the old code signature and macOS kills the app at
  launch.
- Drive the running app with `gpui-agent` (`README.md`), not OS clicks or
  keystrokes. Buwiz Bot's dev hook (`$(getconf DARWIN_USER_TEMP_DIR)buwiz-agent-menu`,
  or its old name `rox-agent-menu`, read by `app/src/hot.rs`) runs
  commands: `cmd <command-id>`.
- Start a test instance with `scripts/buwiz-test` (`scripts/test-rox` is
  its old name). Environment variables have Buwiz names (`BUWIZ_*`); the
  old `ROX_*`/`GROK_*` names still work, with a warning
  (`broker/buwiz-protocol/src/env.rs` has the table).
- Point `gpui-agent` at your own instance by address: `--addr
  127.0.0.1:<your agent port>` (or `GPUI_AGENT_ADDR`), never `--connect`
  (by bundle id or `last`). `--connect` finds whichever Buwiz Bot is
  running, which can be the owner's own app or another agent's test bench.
  A test instance gets its own broker port, agent port and data dir.
- Avatar work: `app/tools/mark-sheet` compiles `app/src/mark` in place, so
  its spin sheets are the real engine's, its `cargo test` runs the engine's
  tests in seconds, and `time` and `dump` compare builds.
- Performance: the monitor (`dev.perf`) logs over-budget frames to
  `~/Library/Logs/Buwiz Bot/jank.log`; the `/perf` skill reads it.
- A build that ignores a source change came from a rustc-wrapper cache:
  rebuild with `RUSTC_WRAPPER=` set empty, and check the version a binary
  reports before you install it.

## Working on the broker (`broker/`)

- The broker holds chats, model calls, routines, v1 plugin hosting and
  agent messaging. It is the Rust workspace in `broker/` (`buwiz-broker` is
  the binary); build and test with `cd broker && cargo test --locked`. Its
  hub, plugin host and MCP host are always on: `--no-node`,
  `BUWIZ_BROKER_HUB`, `BUWIZ_BROKER_V1_HOST` and `BUWIZ_BROKER_MCP_HOST` are
  accepted and do nothing.
- The broker's HTTP tests are in `broker/buwiz-broker/tests/http/` (plain
  `node`). Run them with `broker/buwiz-broker/tests/http/run.sh
  <built buwiz-broker>`; each test starts the broker on `TEST_PORT` (8797 by
  default) with a temp data dir, from a clean environment. Only the office
  group (`office.test.mjs`) needs packages: `npm ci` in `office/` first, or
  `run.sh <bin> --core` runs everything else without it (`--office` runs only
  the office group). CI's broker job runs them all. The plugin host's own are
  `v1-plugins`, `v1-sessions` and `v1-names`, which speak to `/v1/host/*` as
  the app does. The fake model, stub commands and plugin and MCP fixtures
  they and the Rust tests start are under `broker/buwiz-broker/tests/`.
- `bridge/buwiz-bridge.mjs` talks to the broker's `/v1/bridge/*` routes; its
  setup is in the file's header, its tests are `cd bridge && npm ci && npm test`.
- The owner's broker data is `~/Library/Application Support/Buwiz Broker`
  (`Grok Shell` before; it moves once, `broker/buwiz-protocol/src/broker_dir.rs`).
  Never point a test or bench at it: give it its own data dir.
