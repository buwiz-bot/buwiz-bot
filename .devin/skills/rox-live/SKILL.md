---
name: rox-live
description: Work on the rox NSH+ra-jit app — edit patchable src/ code, run the macOS/iOS-simulator dev loops, or verify a live splice landed with state intact. Use when touching app/, running dev.sh/ios.sh, debugging a patch that didn't apply, or checking whether an edit spliced or relinked.
---

# rox — live Rust UI with NSH + ra-jit

`app/` is the Rox chat shell (Grok Shell ported to Rust): `src/hot.rs` and
the modules it pulls in are the patchable area; `src/live.rs` is the
bridge (never edit while live — it is the dependency the store anchor
lives in); `src/store.rs` holds the model as anchored signals.

## Rules for patchable code

- Persistent state goes through `crate::live::store()` (the one model) or
  `crate::live::keep("name", init)` for per-signal persistence. No
  `static`, `thread_local!`, or `#[no_mangle]` data in patched modules —
  each patch re-emits them (the "empty board" bug), and thread-locals
  cannot splice on the iOS simulator at all.
- Type-layout changes (`Store` fields, public structs) need a restart —
  the daemon answers `Restart`, not a patch.
- Prefer `#[cfg]`-gated constants over `cfg!` in macro-expanded code.
- Colors: plain `Hsla` literals in `const`s — `to_hsla()`/`clamp`/`max` in
  const eval are not in ra-jit's evaluator, and a function whose consts
  cannot be evaluated goes to cargo, not a patch.

## Dev loops

- `./dev.sh` — macOS: daemon build + `cargo watch`, then `cargo ra-jit run`.
  Each save of `src/*.rs` answers with a patch; the status bar reports it.
- `./ios.sh` / `./ios-dev.sh` — iOS simulator, same live loop for
  `aarch64-apple-ios-sim`.
- Plain `cargo build --target aarch64-apple-darwin` works for a smoke test.

## Verifying a patch landed

1. Status bar: `applied: N functions`, remount count up.
2. Uptime does not reset; the draft/thread/scroll are intact.
3. `~/.cache/ra-jit/workers/<hash>/log` shows
   `spliced: N functions redirected` for the Darwin worker (and the
   iOS-sim worker for ios-dev).
4. Screenshot and check both the visual change and preserved state.
