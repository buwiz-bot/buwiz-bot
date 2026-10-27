# Rox

A native chat app in Rust on Native Shell (nsh). `app/` is the app,
`nsh/` the nsh fork it builds on (a submodule), `connectors/` and `blaze/`
(ra-jit, a submodule) its helpers. `README.md` explains the build and the live loop.

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
  keystrokes. Rox's dev hook (`$(getconf DARWIN_USER_TEMP_DIR)rox-agent-menu`,
  read by `app/src/hot.rs`) runs commands: `cmd <command-id>`.
- Performance: the monitor (`dev.perf`) logs over-budget frames to
  `~/Library/Logs/Rox/jank.log`; the `/perf` skill reads it.
