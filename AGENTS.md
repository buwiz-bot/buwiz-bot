# Buwiz Bot

A native chat app in Rust on Native Shell (nsh). `app/` is the app,
`server/` the broker it talks to (Node), `nsh/` the nsh fork it builds on
(a submodule), `connectors/` and `blaze/` (ra-jit, a submodule) its
helpers. `README.md` explains the build and the live loop.

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

- Build with `cd app && cargo build -p rox`. Format by hand: rustfmt on the
  app crate rewrites dozens of untouched files.
- Install a build into the dev bundle by removing the old binary first
  (`rm` then `cp` to `~/Applications/Rox.app/Contents/MacOS/rox-bin`): a
  copy over it keeps the old code signature and macOS kills the app at
  launch.
- Drive the running app with `gpui-agent` (`README.md`), not OS clicks or
  keystrokes. Buwiz Bot's dev hook (`$(getconf DARWIN_USER_TEMP_DIR)rox-agent-menu`,
  read by `app/src/hot.rs`) runs commands: `cmd <command-id>`.
- Performance: the monitor (`dev.perf`) logs over-budget frames to
  `~/Library/Logs/Rox/jank.log`; the `/perf` skill reads it.
- A build that ignores a source change came from a rustc-wrapper cache:
  rebuild with `RUSTC_WRAPPER=` set empty, and check the version a binary
  reports before you install it.

## Working on the broker (`server/`)

- The broker holds chats, model calls, routines, v1 plugin hosting and
  agent messaging. Code is in `server/src/`, tests in `server/test/`.
- Test with `cd server && npm test` (after `npm ci`). Each test starts its
  own broker on `TEST_PORT` (8797 by default) with a temp data dir; pick
  another port when one is taken.
- Package it into an app bundle with
  `server/tools/package-server.sh <Resources dir>`.
- A plugin's `BUWIZ_BROKER_DIR` is this `server/` directory (BIR's manifest
  signing and broker tests read `src/v1/` from it).
