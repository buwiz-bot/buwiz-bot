# Tasks: Buwiz Bot onto the plug

One line per task. Format: `ID  repo  what  -> check  [helper: yes/no]`. A helper is a Devin session on the machine the
check needs. Tasks inside a stage run in order unless a line says it can run in parallel. Stage order and done lists are
in `PLAN.md`. Read `GOAL.md` rules first.

Repos, short names: `NSH` = `hexuria/nsh`; `PLUG` = `hexuria/nsh-gpui-agent`; `BOT` = `buwiz-bot/buwiz-bot`;
`SDK` = `buwiz-app/buwiz-sdk`; `BOOKS` = `buwiz-app/books`; `GWS` = `buwiz-app/google-workspace`;
`MINI-RS` = `buwiz-app/mini-rs`; `EBIR-MINI` = `buwiz-app/ebirforms`; `TSX` = `buwiz-app/mini-tsx`.

## Stage 0: probe (lead session)

- [ ] S0.1  all  `git fetch origin` in every repo in PLAN "Where things stand"; record the tips  -> table matches PLAN  [helper: no]
- [ ] S0.2  NSH  branch `feat/bot-agent-plug` from `origin/buwiz-next` (local; push only as `try/*` if a helper needs it)  -> `git log --oneline origin/buwiz-next..feat/bot-agent-plug` empty  [helper: no]
- [ ] S0.3  NSH  cherry-pick the Bot's non-agent commits (from `data/buwiz-only-commits.tsv`, not the agent group)  -> each conflict recorded with its file  [helper: no]
- [ ] S0.4  BOT  scratch (uncommitted): submodule to the S0.3 candidate; Cargo changes from PLAN Stage 3 step 1  -> `scripts/wait-build cargo check -p buwiz-bot` on macOS; every error becomes a gap-table row  [helper: yes, macOS]
- [ ] S0.5  PLUG  scratch branch `try/bot-agent-plug` pointing NSH entries at the S0.3 candidate  -> `cargo check -p nsh-gpui-agent-macos` on macOS  [helper: yes, macOS]
- [ ] S0.6  NSH candidate  `cargo check -p nsh-platform-linux --features dev` and `-p nsh-platform-windows --features dev`  -> both compile, output kept  [helper: yes, Linux and Windows]
- [ ] S0.7  BOT  `cargo check --no-default-features --features dev-agent` on Linux  -> result recorded (settles an inferred item in PLAN Q1)  [helper: yes, Linux]
- [ ] S0.8  BOT  read `scripts/test-agent`, `scripts/buwiz-drive` and the test-instance flow; install `gpui-agent` `=0.4.0` CLI in a helper; run `hello` on the current Bot  -> output kept (settles PLAN Q7)  [helper: yes, macOS]
- [ ] S0.9  NSH  read the 14 workflows on `origin/buwiz-next`; say which compile `nsh-platform-macos`  -> list in the report  [helper: no]
- [ ] S0.10 all  compile-check the Bot with and without the 14 upstream commits (D3)  -> result per choice  [helper: yes, macOS]
- [ ] S0.11 all  write go or stop, and the layout recommendation (D1, D2), with the compile output  -> report sent  [helper: no]

## Stage 1: the NSH line for the Bot (NSH)

- [ ] S1.1  NSH  make the branch final: base per D2; commits = the Bot's non-agent topics (D1 applied)  -> `git diff --stat origin/<base>..feat/bot-agent-plug` lists only those  [helper: no]
- [ ] S1.2  NSH  add each door the plug needs from the Stage 0 list, one door per commit  -> each commit compiles on macOS  [helper: yes, macOS]
- [ ] S1.3  NSH  `cargo +nightly fmt --all`; crate tests and clippy for each crate touched (`AGENTS.md` "Verification")  -> green, output kept  [helper: yes, macOS]
- [ ] S1.4  NSH  `cargo metadata --locked` (the Bot's `nsh-lock` job)  -> passes  [helper: no]
- [ ] S1.5  NSH  compile `nsh-platform-macos --features dev` on macOS; `nsh-platform-linux --features dev` on Linux; `nsh-platform-windows --features dev` on Windows  -> each passes, output kept  [helper: yes, all three]
- [ ] S1.6  NSH  draft pull request `feat/bot-agent-plug` into the base (not merged)  -> open; body has `Feature: bot-agent-plug — tracking <org/repo#n>`  [helper: no]
- [ ] S1.7  NSH  confirm `buwiz`, `buwiz-next` tips are unchanged  -> shas recorded  [helper: no]

## Stage 2: the plug on the new line (PLUG)

- [ ] S2.1  PLUG  `Cargo.toml` NSH entries (all `nsh-*`) to the S1.6 commit, one commit  -> `cargo tree -i nsh-testing --depth 1` one entry; `cargo check --locked` on ubuntu, macos, windows  [helper: yes, Linux and Windows]
- [ ] S2.2  PLUG  README "Which NSH to use": the new branch and commit  -> the sha in README equals the Cargo entries  [helper: no]
- [ ] S2.3  PLUG  release build of `nsh-gpui-agent-macos` without `dev` (D10): feature or documented switch  -> `cargo tree -p nsh-gpui-agent-macos -e features --no-default-features` shows no `dev` from NSH  [helper: yes, macOS]
- [ ] S2.4  PLUG  draft pull request; the plug's `ci.yml` green on its three jobs  -> the three jobs green, links kept  [helper: no]

## Stage 3: the Bot (BOT)

- [ ] S3.1  BOT  `app/Cargo.toml`: drop `features = ["gpui-agent"]` on `nsh-platform-macos`; add the plug's crate as optional Git dependency under `dev-agent` with `default-features = false`  -> `cargo check -p buwiz-bot` macOS  [helper: yes, macOS]
- [ ] S3.2  BOT  `app/Cargo.toml` `[patch."https://github.com/hexuria/nsh"]`: every `nsh-*` crate the plug reaches  -> `cargo tree -p buwiz-bot -i nsh-testing --depth 1` one entry  [helper: yes, macOS]
- [ ] S3.3  BOT  `app/src/agent_hub/mod.rs`: `AppViewAgentExt`, `ui_tree::UiNode`, `UiAct`, `SCREEN_CHANGED` from the plug  -> compiles; `cfg` gates kept  [helper: yes, macOS]
- [ ] S3.4  BOT  `app/src/agent_hub/filter.rs` line 26: `ui_tree::UiNode` from the plug  -> compiles; `cargo test -p buwiz-bot` the `filter` tests pass  [helper: yes, macOS]
- [ ] S3.5  BOT  `app/src/mini_v1/attribution.rs` lines 274 to 279: `on_input` and `Input` from the plug  -> compiles; provenance check in S3.9  [helper: yes, macOS]
- [ ] S3.6  BOT  submodule `nsh` to the S1.6 commit; `.gitmodules` branch to `feat/bot-agent-plug`  -> `git submodule status` shows the sha  [helper: no]
- [ ] S3.7  BOT  `scripts/wait-build cargo test --locked -p buwiz-bot -p buwiz-office-kit -p buwiz-word -p buwiz-spreadsheet` (the `app` job) and `clippy` (default features)  -> green, output kept  [helper: yes, macOS]
- [ ] S3.8  BOT  release: `scripts/release/build.sh` (`--no-default-features`); keep its three string checks; add a check for the plug's listener and dev symbols  -> build passes; the checks reject a binary with the symbols (test it once with a dev binary)  [helper: yes, macOS]
- [ ] S3.9  BOT  test instance (`scripts/buwiz-test`, own data dir, port, token): `scripts/test-agent <dir> snapshot`, `click <id>`, `type`, `screenshot --out x.png`, on a chat and on a mini app screen  -> output kept  [helper: yes, macOS]
- [ ] S3.10 BOT  provenance on the test instance: `GPUI_AGENT_ACTOR=agent`; an agent press reaches the mini app as `origin: agent`; a person's press does not  -> output kept; PLAN row 7 answered  [helper: yes, macOS]
- [ ] S3.11 BOT  second window: open a second mini app screen; check whether snapshot and click reach it  -> output kept; PLAN row 9 answered  [helper: yes, macOS]
- [ ] S3.12 BOT  decide D4 with S3.9 to S3.11 output; if (b) or (c), enable `gpui-agent-server` on the plug path and rerun S3.9  -> decision recorded in the report  [helper: no]
- [ ] S3.13 BOT  Linux and Windows non-agent build (D5)  -> `cargo check` passes on each, output kept  [helper: yes, Linux and Windows]
- [ ] S3.14 BOT  docs: `README.md` "Driving the UI programmatically", `AGENTS.md` lines 30 to 65, `NSH-FORK.md`, `nsh-patches.toml` (mark moved topics with the plug's PR)  -> `git grep -n 'buwiz' -- '*.md'` shows no stale NSH branch claim for the Bot  [helper: no]
- [ ] S3.15 BOT  draft pull request: code and submodule pointer in one commit; CI (`ci.yml`) green  -> open, links kept  [helper: no]

## Stage 4: buwiz-sdk (SDK)

- [ ] S4.1  SDK  `crates/app/Cargo.toml` `nsh-core` and `nsh-engine` to the S1.6 commit  -> `cargo check -p buwiz-app` on macOS and Linux  [helper: yes, macOS and Linux]
- [ ] S4.2  SDK  `cargo test` for the workspace on macOS and Linux (no CI exists; local only)  -> green, output kept  [helper: yes, macOS and Linux]
- [ ] S4.3  SDK  draft pull request; name the commit that everyone pins (D7)  -> open; sha recorded  [helper: no]

## Stage 5: the mini apps (in this order)

- [ ] S5.1  BOOKS  `crates/books-screens` NSH pins to S1.6 commit; `Cargo.toml` SDK pins to the S4.3 commit (both entries)  -> `cargo test` locally on macOS  [helper: yes, macOS]
- [ ] S5.2  BOOKS  draft pull request  -> open; sha recorded  [helper: no]
- [ ] S5.3  GWS  `crates/gws-screens` NSH pins to S1.6 commit; `nsh-testing` `agent` feature (`Cargo.toml` line 26)  -> `cargo check --workspace --locked` on macOS  [helper: yes, macOS]
- [ ] S5.4  GWS  `tests/bot_filter/mod.rs`: replace `nsh_testing::agent::SemanticNode` with the plug's `nsh-agent-actions::SemanticNode` (PLAN row 11)  -> `cargo test --workspace --locked` on Linux and macOS  [helper: yes, Linux and macOS]
- [ ] S5.5  GWS  SDK pins to the S4.3 commit; draft pull request; CI (`ci.yml`) green on `ubuntu-latest` and `macos-15`  -> green, links kept  [helper: no]
- [ ] S5.6  MINI-RS  `crates/hello-screens` NSH pins; SDK pins  -> `cargo test` locally  [helper: yes, macOS]
- [ ] S5.7  MINI-RS  draft pull request  -> open  [helper: no]
- [ ] S5.8  EBIR-MINI  SDK pin (`buwiz-plugin`) to the S4.3 commit  -> `cargo check` locally  [helper: yes, macOS]
- [ ] S5.9  EBIR-MINI  draft pull request  -> open  [helper: no]
- [ ] S5.10 TSX  `package.json` `@buwiz/sdk` to the S4.3 commit  -> `npm ci` and the repo's own checks  [helper: yes, macOS]
- [ ] S5.11 TSX  draft pull request  -> open  [helper: no]
- [ ] S5.12 BOT  `app/Cargo.toml`: `buwiz-app`, `books-screens`, `books-mini-app`, `gws-screens` to the S4.3 and S5 commits; `buwiz-app` equal to Books' (D8)  -> `cargo check -p buwiz-bot`; `git grep -n 'rev =' origin/HEAD -- app/Cargo.toml` shows the agreed revs  [helper: yes, macOS]

## Stage 6: retire the old line (owner approval first)

- [ ] S6.1  all  search the Bot, SDK and the mini apps for the NSH branch `buwiz`  -> no match outside history and notes  [helper: no]
- [ ] S6.2  NSH  propose the dated tag for `buwiz`'s tip (D6); do not create it  -> the proposal is in the report  [helper: no]
- [ ] S6.3  all  after written approval only: create the tag; confirm the `buwiz` tip sha is unchanged  -> `git rev-parse origin/buwiz` equals the start-of-round sha  [helper: no]
- [ ] S6.4  NSH  update `NSH-FORK.md` for the new line (the rename is D6, not done here)  -> draft pull request, not merged  [helper: no]
- [ ] S6.5  all  final report to the owner (GOAL "Done when")  -> sent  [helper: no]
