# Plan: Buwiz Bot moves onto the plug

Branch `feat/bot-agent-plug` of `buwiz-bot/buwiz-bot`. This branch carries the handover only. Nothing here is merged
anywhere.

Facts were verified on 2026-11-24 with `git fetch origin` in each repository. Each fact names its evidence. Anything
marked **inferred** was not checked by reading or compiling, and Stage 0 must check it.

## Where things stand (verified 2026-11-24)

| Repository (local path) | origin tip | What it pins today | Evidence |
|---|---|---|---|
| `buwiz-bot/buwiz-bot` (`/Volumes/goldcoders/features/bot-agent-plug/buwiz-bot`, worktree) | `e3f0b1eb` | NSH submodule `nsh` at `3fcb4e58`, branch `buwiz` (`.gitmodules`) | `git ls-tree origin/HEAD nsh` |
| `hexuria/nsh`, branch `buwiz` | `70d8ff26b` (2026-11-21) | the old line; Bot pins `3fcb4e58` on its history | `git rev-parse origin/buwiz` |
| `hexuria/nsh`, branch `buwiz-next` | `ca39c0e91` | eBIRForms' pin (`NSH_PINNED_REF`) | `buwiz-forms` `.github/workflows/nsh.yml` line 74 on `feat/nsh-plug` |
| `hexuria/nsh`, branch `gpui-agent` | `365864ba8` | upstream v0.6.1 plus the doors | `SOCKET.md` on the branch |
| `hexuria/nsh`, `main` | `4a2a112a8` | upstream mirror; does not contain v0.6.1 | `git merge-base --is-ancestor 2f51d8611 origin/main` is false |
| `hexuria/nsh-gpui-agent` (the plug), `main` | `66ec14b` | NSH `365864ba8` on every `nsh-*` entry; `gpui-agent =0.4.0` | `Cargo.toml` lines 21 to 45 |
| `buwiz-app/buwiz-sdk` | `19ff3df` | `nsh-core`, `nsh-engine` at `3fcb4e58` | `crates/app/Cargo.toml` lines 18, 27 |
| `buwiz-app/books` | `4d9944e` | SDK `bf188594` (buwiz-plugin and buwiz-app); NSH `3fcb4e58` in `crates/books-screens` | `Cargo.toml` lines 24 to 25; `crates/books-screens/Cargo.toml` lines 12, 17 |
| `buwiz-app/google-workspace` | `652527b` | SDK `f63038d`; NSH `3fcb4e58` incl. `nsh-testing` with `features = ["agent"]` | `Cargo.toml` lines 14 to 15; `crates/gws-screens/Cargo.toml` lines 13 to 26 |
| `buwiz-app/mini-rs` | `ee19cdd` | SDK `f63038d`; NSH `3fcb4e58` in `crates/hello-screens` | `Cargo.toml` lines 14 to 15; `crates/hello-screens/Cargo.toml` lines 12, 16 |
| `buwiz-app/ebirforms` (mini app) | `90eb514` | SDK `f63038d` (`buwiz-plugin` only); no NSH | `Cargo.toml` line 29 |
| `buwiz-app/mini-tsx` | `aedfdf4` | SDK `f63038d` via `package.json` (`@buwiz/sdk`); no NSH | `package.json` line 16 |
| `buwiz-app/buwiz-agent` | `3c829c4` | message types only; no NSH, no gpui | `crates/agent/Cargo.toml` (serde, ts-rs only) |

Correction to the brief: the plug's `main` is `66ec14b` (a merge of PR #10). The brief's `a6bb130` is an ancestor of it
(verified). The brief said the Bot's `[patch]` table names `buwiz-agent`, `buwiz-app`, `books-screens`. It does not.
The table (`app/Cargo.toml` lines 125 to 128) names `nsh-core`, `nsh-macros` and `nsh-engine` only. Those three
are the only patched crates. The other pins are plain Git dependencies (`app/Cargo.toml` lines 30, 46, 50, 53, 58).

## Decisions already made (owner, final: do not reopen)

1. The Bot moves to the plug. Target: nothing the owner ships uses `origin/buwiz` any more. Then that line can retire
   to a dated tag.
2. This is its own round, in its own session. The eBIRForms desktop app (`hexuria/buwiz-forms`) is not part of it
   and must not be changed by it.
3. The plan starts with a Stage 0 probe that can stop and report if the move turns out not to be small.
4. One pull request at a time per repository. Merging is the owner's call. Nothing is merged by a session.
5. `buwiz` stays exactly as it is until the Bot has left it. No rename of `buwiz-next` in this round, unless
   Stage 6 proposes it for the owner to approve.
6. Devin sessions are free helper machines for macOS, Linux and Windows. They do the per-system fixes and the real
   test drives. The lead session does the hard parts.
7. Worktrees follow `/Volumes/goldcoders/orchestrator/ops/WORKTREES.md`. For each further repo, `ws add bot-agent-plug
   <org/repo>`. The same branch name `feat/bot-agent-plug` everywhere. `FEATURE.md`. A `feature:bot-agent-plug`
   label. One tracking issue. Pull request bodies start `Feature: bot-agent-plug — tracking <org/repo#n>`.

## Question 1: how the Bot uses the NSH agent today (on the `buwiz` line)

Every agent touch point in the Bot at `origin/HEAD` (`e3f0b1eb`), with its evidence:

- **Cargo features.** `app/Cargo.toml` lines 73 to 81: `default = ["dev-agent"]`, `dev-agent =
  ["nsh-platform-macos/gpui-agent-server"]`. The macOS-only path dependency `nsh-platform-macos` (`../nsh/crates/
  nsh-platform-macos`, `default-features = false`, `features = ["gpui-agent"]`). The `dev-agent` feature itself is
  not limited to macOS (inferred from the `[features]` block; Stage 0 checks `cargo check --no-default-features
  --features dev-agent` on Linux). The `nsh-platform-macos` line (line 81) sits in `[target.'cfg(target_os = "macos")'.dependencies]`.
- **Release gate.** `app/src/lib.rs` lines 13 to 16: `compile_error!` when `dev-agent` is on in a build without
  `debug_assertions`. `scripts/release/build.sh` lines 108 to 109 build with `--no-default-features`. Lines 113 to 122
  refuse a binary that holds `gpui-agent: serving`, `BUWIZ_HUB_POSTED_ALLOW`, `GPUI_AGENT_ACTOR` or
  `BUWIZ_HUB_FULL_TEXT`.
- **Agent Hub (macOS).** `app/src/lib.rs` line 19 `pub mod agent_hub`. `app/src/agent_hub/mod.rs`:
  - line 63: `use nsh_platform_macos::{AppView, SCREEN_CHANGED, UiAct, ui_tree::UiNode as Seen};`
  - line 141 `opened(..., view: Weak<AppView>)`. The caller is `app/src/mini_v1/native.rs` line 545
    (`Rc::downgrade(view)`).
  - line 184 `view.ui_tree().nodes` (the tree of a mini app's screen).
  - lines 276 to 280 `UiAct::Press`, `UiAct::Type(text)`, `UiAct::SetValue(value)`.
  - line 358 `error.starts_with(SCREEN_CHANGED)`.
  - line 451 `view.act(&plan.node, plan.act, move || unmoved(open, rev), move |done| finish(...))`. `finish` takes
    `done: Result<(), String>` (line 398).
  - lines 175 and 533 `view.view()` and `.window()`: the `NSView` of the surface.
  - Its header (lines 1 to 50) says the Hub "reads nsh's tree in the process, so a shipped Bot opens no listener".
- **Agent Hub filter.** `app/src/agent_hub/filter.rs` line 26 `use nsh_platform_macos::ui_tree::UiNode as Seen`. Fields
  read: `visible`, `role`, `name`, `value`, `states`, `children`, `enabled`, `focused` (lines 139 to 150, 233 to 237,
  255 to 264). `filter.rs` line 35 onward has `#[cfg(feature = "dev-agent")]`.
- **Agent Hub card.** `app/src/agent_hub/card.rs` lines 31 to 41 import AppKit and objc2 re-exports through
  `nsh_platform_macos`. These are not agent symbols, but the Bot relies on the NSH crate's re-exports
  (`nsh-platform-macos` lib: `pub use objc2`, `objc2_app_kit`, `objc2_foundation` at `lib.rs` lines 88 to 90 on
  `buwiz`). The plug does not re-export them (inferred from its `lib.rs`).
- **Input provenance.** `app/src/mini_v1/attribution.rs` line 274 onward: `nsh_platform_macos::on_input(mtm, ...)`
  (line 276) and the variants `Input::Agent` (line 278) and `Input::Person { surface }` (line 279). `attribution::watch`
  is called from `app/src/main.rs` (macOS entry point, about line 216). The variant `Input::Agent` tells the Bot an
  automation acts, so its presses count as the agent's.
- **Development hooks.** `app/src/hot.rs` lines 337 to 340: `#[cfg(feature = "dev-agent")]`, `GPUI_AGENT=1` reads
  `buwiz-agent-menu` from the temp dir. `app/src/identity.rs` lines 304 to 318 and `app/src/broker.rs` lines 330 to
  333: environment names (`GPUI_AGENT_TOKEN_FILE`, `GPUI_AGENT_ACTOR` and others). These are not NSH symbols.
- **Linux and Windows.** `app/src/lib.rs` lines 20 to 22: `agent_hub_linux.rs` on Linux and Windows. It uses only
  `buwiz_agent` types (`app/src/agent_hub_linux.rs` line 9). The comment above the Linux entry in `app/src/main.rs`
  (about line 245) says "No Agent Hub, zoom or key host here yet." **So the agent is used on macOS only.**
- **Tests.** `cfg(test)` modules in `app/src/agent_hub/*.rs`; `app/tests/windows_portability.rs` (calls `agent_hub::start`,
  `opened`, `show_card`, `hide_card`); `app/tests/card_scroll.rs` (`agent_hub::card::allow_on`); `app/src/identity.rs`
  tests (lines 310 to 379); `app/src/hot.rs` line 908.
- **Scripts that drive the app through the agent.** `scripts/buwiz-drive` (reads `buwiz-gpui-agent-token`, sets
  `GPUI_AGENT_ADDR`, default `127.0.0.1:17421`, runs `gpui-agent`); `scripts/test-agent` (instance dir, token, addr
  file, runs `gpui-agent`); `scripts/buwiz-test` (lines 93 and 120 to 121: `GPUI_AGENT=1`, `GPUI_AGENT_ALLOW_RELEASE=1`,
  `GPUI_AGENT_ADDR`, `GPUI_AGENT_TOKEN_FILE`); `scripts/broker-live` (unsets the `GPUI_AGENT*` variables); `scripts/wait-build`
  (serializes cargo, not agent); `app/bundle-dev.sh` (token file for the dev bundle).

Which NSH symbols the Bot uses (the input to Question 2): `AppView` (type, `Weak`), `AppView::ui_tree`, `AppView::act`,
`AppView::view`, `ui_tree::UiNode` (and `.nodes`, `UiTree`), `UiAct` (three variants), `SCREEN_CHANGED`, `on_input`
and `Input` (two variants), features `gpui-agent` and `gpui-agent-server` of `nsh-platform-macos`.

## Question 2: where each agent symbol lives, and the gap table

Checked with `git -C /Volumes/goldcoders/hexuria/nsh grep -n <symbol> <branch> -- crates/...` on each branch.
"Present" means the name is found in that crate. Behavior was not compared. Nothing was compiled, so every
"gap" below is a hypothesis for Stage 0 to confirm or reject by compiling.

| # | The Bot needs (today, on `buwiz`) | `buwiz` | `buwiz-next` | `gpui-agent` | The plug (`main`) | Gap |
|---|---|---|---|---|---|---|
| 1 | Feature `gpui-agent` (and `gpui-agent-server` via `dev-agent`) on `nsh-platform-macos` | `crates/nsh-platform-macos/Cargo.toml` lines 55, 58 | absent | absent | its own features on `nsh-gpui-agent-macos` (`gpui-agent`, `gpui-agent-server`, `dev-agent`; default on) | The Bot's `Cargo.toml` must change. The NSH feature does not exist on the new lines. |
| 2 | `AppView::ui_tree()` | `crates/nsh-platform-macos/src/app.rs` line 445 | absent | absent | trait `AppViewAgentExt::ui_tree` (`lib.rs` lines 180 to 200), implemented for `nsh_platform_macos::AppView` | The Bot must import the trait. Not compiled. |
| 3 | `AppView::act(target, UiAct, still, done)` | `app.rs` line 457 | absent | absent | trait `AppViewAgentExt::act` (`lib.rs` lines 187 to 212) | Same call shape (`&str`, `UiAct`, `Fn() -> bool`, `FnOnce(Result<(), String>)`). Not compiled. |
| 4 | `ui_tree::UiNode`, `UiTree` (the gpui-agent tree) | `lib.rs` line 81 re-export of `gpui_agent::tree` | absent | absent | re-export `pub use ::gpui_agent::tree as ui_tree` (`lib.rs` line 55) on gpui-agent `=0.4.0` | The `buwiz` line uses gpui-agent `=0.3.0` (`Cargo.toml` line 57); the plug uses `=0.4.0`. Tree shape not compared (inferred to be the same: the plug README says 0.4.0 "changed the client's recipes, not what a host implements"). |
| 5 | `UiAct` (`Press`, `Type`, `SetValue`) | `gpui_agent.rs` line 510; `lib.rs` line 95 | absent | absent | `pub use crate::gpui_agent::{SCREEN_CHANGED, UiAct}` (`lib.rs` line 53) | Path only. |
| 6 | `SCREEN_CHANGED` | `gpui_agent.rs` line 565 | absent | absent | `lib.rs` line 53 | Path only. |
| 7 | `on_input`, `Input::{Agent, Person{surface}}` (provenance) | `provenance.rs` lines 29 and 60; `lib.rs` line 104 | absent | absent | own module `provenance` (`lib.rs` line 42), `pub use provenance::{Input, on_input}` (line 44); its own thread-local state (`provenance.rs` lines 46 to 56) | **Real gap.** The plug's provenance state is separate from NSH's. The plug's `act` calls its own `agent_acts()` (`gpui_agent.rs` lines 265, 286). Whether the Bot's `attribution` hears both the agent's presses and the person's, with the plug only, is **not known**. Needs a run on a test instance (`GPUI_AGENT_ACTOR=agent`). |
| 8 | `AppView` type, `AppView::view()`, `.window()` on the view | `app.rs` line 358 (type); line 468 (`view`) | type line 358; `view` line 441 | type line 349; `view` line 432 | n/a | None (present). |
| 9 | Legacy listener (`gpui-agent-server`): serves the window on `GPUI_AGENT_ADDR` when `GPUI_AGENT=1` | `gpui_agent.rs` lines 126 to 253 (feature-gated) | absent | absent | two listeners: the strict adapter via `nsh_gpui_agent_macos::install()` (plug README, "An app with its own Rust host"), and the legacy server behind `gpui-agent-server` (plug `Cargo.toml`) | **Behavior change.** Plug README, "Keybindings and app windows": the legacy in-process server cannot reach a second app window, and does not advertise `press` or `scroll` (clients use `click`, `type`, `invoke`). The Bot's scripts use `snapshot`, `click`, `type`, `screenshot`, `invoke`. Whether the Bot's workflows survive the strict adapter is **not known**. |
| 10 | Release build holds no development code | `build.sh` lines 108 to 122 check two strings | n/a | n/a | the plug pins `nsh-platform-macos` with features `["dev"]` (plug `Cargo.toml` line 44); the README says this "compiles upstream development support into such a build" | **Release risk.** The release gate must add a plug symbol check and a `dev`-free build of the plug's macOS crate. |
| 11 | nsh-testing items used by google-workspace tests: `Model`, `Scene`, `SemanticNode` (`crates/gws-screens/tests/bot_filter/mod.rs` lines 12 to 13) | `nsh-testing` `agent` feature exists | `Model`, `Scene` present; `SemanticNode` **absent** | same as `buwiz-next` | the plug has its own `SemanticNode` (`nsh-agent-actions/src/lib.rs` line 97), a different type | **Gap** for google-workspace's test. The test copies the Bot's filter and reads `nsh_testing::agent::SemanticNode`. |
| 12 | Non-agent AppView API the Bot calls, by name in `app/src`: `set_error_handler` (7 hits), `set_key_precedence` (3), `AppOptions` `activate` (2), `data_dir` (64), `fps` (2) | `set_error_handler`, `set_key_precedence`, `activate`, `data_dir`, `fps` present in `nsh-platform-macos` | present (same five names) | `set_error_handler`, `set_key_precedence`, `activate` absent; `data_dir`, `fps` present | n/a | Names exist on `buwiz-next`. Behavior differs by commit (the next section). Not compared. |
| 13 | Agent code on Linux and Windows | none (no NSH agent symbol in `agent_hub_linux.rs`) | n/a | n/a | n/a | None for the Bot. |

Summary of the gap table: no NSH agent symbol the Bot uses exists on `buwiz-next` or `gpui-agent`. The plug
supplies each one by name (rows 2 to 7, 9 and 11), and the Bot needs a Cargo and import change for them. Two rows
need a run before anyone can say they work: row 7 (provenance) and row 9 (server behavior). Row 10 is a release risk
that the plug creates.

## Question 3: can the Bot sit on `buwiz-next` as it is?

**No, not as it is.** Evidence:

- `buwiz-next` is based on upstream v0.6.1 (`2f51d8611` is an ancestor of `buwiz-next`, verified). `buwiz` carries
  14 commits with upstream squash subjects numbered `#2330` to `#2356` (verified: `#2330` is on `origin/main`).
  These are missing on `buwiz-next`. List: `data/buwiz-only-commits.tsv` (the lines ending in `(#NNNN)`).
- `git cherry -v origin/buwiz-next origin/buwiz` lists 209 `buwiz` commits with no patch-id match on `buwiz-next`
  (saved as `data/buwiz-only-commits.tsv`). By subject, about 120 are agent work (heuristic: subjects containing
  agent, synth, provenance, ui_tree, hub, socket, door, strict). About 89 are not. That grep is a heuristic. The
  list is the input for Stage 0, not a verdict.
- The Bot's non-agent topics (`nsh-patches.toml`) with commits that have no match on `buwiz-next` (patch-id, with
  `origin/main` as the limit; a heuristic because some topics were squash-merged upstream):
  - `pr/macos-shown-scale`: `88b940945` "macos: Draw views at the scale they are shown at".
  - `pr/macos-pause-embedded`: `aae18fbe` "macos: Pause an embedded app's frames and timers while it is unseen".
  - `pr/macos-draw-ahead`: `ad469b46` "macos: Draw content about to scroll into view ahead of time".
  - `pr/macos-hosted-keys`: `407a4aed` "macos: Let a hosted app view take the keys it binds ahead of its host, or
    after it" (2 commits missing).
  - `pr/macos-pace-mounts`: `804ed65c` "macos: Spread a batch that makes hundreds of views out of sight over frames".
  - `pr/keymap-claim-menu-keys`: `f7afafd8` "Keymaps: bind a menu's reserved keystroke; read a key event as the
    monitor does".
  - `pr/macos-launch-without-activation`: 1 commit missing.
  - `pr/perf-jank-log` (3), `pr/macos-jank-paint-time` (4), `pr/macos-jank-commit-time` (4): the `/perf` skill reads
    their log. Missing.
  - `pr/core-item-offset` (2), `pr/appview-error-hook` (1): missing by patch. The Bot's `set_error_handler` name
    exists on `buwiz-next` (row 12); `core-item-offset` is not a name the Bot calls (not checked further).
  - `pr/linux-appview-error-handler` and `buwiz-windows-error-handler`: their branches are not on `origin` (checked
    with `git branch -r`). Their Windows commit is in the Bot's pin (`3fcb4e58` is merge PR #26 of that branch).
  - `buwiz-port/upstream-0.6`, `-0.6-drawing`, `-macos`: "buwiz-only" integration fixes. Not checked.
  - `feat/agent-hub`: 129 commits missing on `buwiz-next`. This is agent work (the Hub's in-process tree and action
    code), and the plug replaces it. Not a Bot topic to carry.
- The Bot's pin `3fcb4e58` is not on `buwiz-next` (verified: `git merge-base --is-ancestor 3fcb4e58 origin/buwiz-next`
  is false). The Bot's baseline is that commit, not `buwiz`'s tip.

**Recommendation (for Stage 0 to confirm by compiling):** a new branch on `hexuria/nsh`, `feat/bot-agent-plug`, cut from
`buwiz-next` at `ca39c0e91`. Cherry-pick the Bot's non-agent commits from the list above, and decide the 14 upstream
commits by compiling the Bot with and without them. Do not add to `buwiz-next`: eBIRForms pins that commit, and adding
commits to the line changes what its next pin would bring. Options are in "Open decisions" (D1, D2).

## Question 4: the ripple, in order, with every pin that moves

Order matters. An SDK commit must build on the new NSH line before a mini app pins it, and the Bot pins the same
`buwiz-app` rev as Books (the rule in `app/Cargo.toml` line 44 onward, "Books pins buwiz-app too: bump both together").

| Step | Repository | Pin today | What moves and why |
|---|---|---|---|
| 1 | `hexuria/nsh` (new branch) | n/a | Creates the NSH commit the rest pins. |
| 2 | `hexuria/nsh-gpui-agent` | NSH `365864ba8` (all `nsh-*`); `gpui-agent =0.4.0` | The NSH rev moves to step 1's commit (`Cargo.toml` lines 28 to 45). README "Which NSH to use" follows. |
| 3 | `buwiz-app/buwiz-sdk` | `nsh-core`, `nsh-engine` at `3fcb4e58` (`crates/app/Cargo.toml` lines 18, 27) | Move to step 1's commit. The SDK has no CI, so its check is local. |
| 4 | `buwiz-app/buwiz-sdk` (rev for everyone) | HEAD `19ff3df` | Choose one SDK commit after step 3 (Open decision D7). |
| 5 | `buwiz-bot/buwiz-bot` | submodule `nsh` at `3fcb4e58` (branch `buwiz`); `[patch]` covers 3 crates; `buwiz-app` `369935a8`; `books-screens` and `books-mini-app` `8f71afff`; `gws-screens` `924b529f` | Submodule to step 1's commit, and `.gitmodules` branch to `feat/bot-agent-plug`. `[patch]` extended to every `nsh-*` crate the plug uses (row 1 and row 9 in the table). `nsh-platform-macos` feature list changes (row 1). Plug added (row 2 to 7). `buwiz-app` to step 4's rev. `books-screens`, `books-mini-app` to Books' commit at that same rev (drift today, see D8). `gws-screens` to google-workspace's commit (drift today, D8). |
| 6 | `buwiz-app/books` | SDK `bf188594` (both entries); NSH `3fcb4e58` (`crates/books-screens`) | NSH to step 1; SDK to step 4's rev. Books' own build is checked locally (no CI). |
| 7 | `buwiz-app/google-workspace` | SDK `f63038d`; NSH `3fcb4e58`, `nsh-testing` `features = ["agent"]` | NSH to step 1; SDK to step 4's rev. The test that needs `nsh_testing::agent::SemanticNode` (row 11) changes to the plug's type. CI runs on `ubuntu-latest` and `macos-15`. |
| 8 | `buwiz-app/mini-rs` | SDK `f63038d`; NSH `3fcb4e58` (`crates/hello-screens`) | NSH to step 1; SDK to step 4's rev. |
| 9 | `buwiz-app/ebirforms` (mini app) | SDK `f63038d` (`buwiz-plugin` only) | SDK to step 4's rev. No NSH pin. |
| 10 | `buwiz-app/mini-tsx` | SDK `f63038d` (`package.json`, `@buwiz/sdk`) | SDK to step 4's rev. No NSH pin. |
| 11 | `buwiz-bot/buwiz-bot` | `buwiz-agent` `0610474f` (ancestor of `3c829c4`, verified) | Not moved in this round. The brief asked for it to be confirmed as not touched by the move, and it is not. |

Pins that drift today, before this round touches them: the Bot's `buwiz-app` (`369935a8`) and Books' (`bf188594`) differ,
so the rule in the Bot's `Cargo.toml` is already broken. The Bot's `books-screens` (`8f71afff`) and Books' HEAD (`4d9944e`)
differ. The Bot's `gws-screens` (`924b529f`) and google-workspace's HEAD (`652527b`) differ. Each is recorded as D8.

## Question 5: CI in each repository

| Repository | Workflow files on `origin/HEAD` | What the job does | Builds the agent feature? |
|---|---|---|---|
| `buwiz-bot/buwiz-bot` | `.github/workflows/ci.yml`, `release.yml` | `ci.yml`: `nsh-lock` (ubuntu-latest, `cargo metadata --locked` in `nsh`); `app` (macos-15, `cargo test --locked -p buwiz-bot ...`, default features); `clippy` (macos-15, `--all-targets -D warnings`, default features); `broker` (macos-15). `release.yml` (ubuntu-24.04 and a matrix) runs `scripts/release/build.sh` (`--no-default-features`). | **Yes, on macOS**, through the default features (`dev-agent`) in `app` and `clippy`. Not on Linux or Windows. |
| `buwiz-app/google-workspace` | `.github/workflows/ci.yml` | `test`: matrix `ubuntu-latest`, `macos-15`; `cargo test --workspace --locked`; `cargo test -p buwiz-gws --features stub`; `cargo clippy --workspace --all-targets -D warnings` | **Yes, on both**: `gws-screens` (in the workspace) enables `nsh-testing` `agent`. |
| `buwiz-app/buwiz-sdk`, `books`, `mini-rs`, `ebirforms`, `mini-tsx`, `buwiz-agent` | none found (`git ls-tree -r --name-only origin/HEAD` lists no workflow) | n/a | No. Their checks are local only. |
| `hexuria/nsh`, branch `buwiz-next` | 14 files, including `core.yml`, `linux.yml`, `windows.yml`, `gpui.yml`, `rust-app.yml`, `release.yml` | Not read in this probe. | Not known (Stage 0 reads them). |
| `hexuria/nsh-gpui-agent`, `main` | `.github/workflows/ci.yml`: `ubuntu-24.04` (`cargo test -p nsh-agent-actions -p nsh-gpui-agent`, `-p nsh-agent-web`, `-p nsh-gpui-agent-linux`, clippy); `macos-15` (`-p nsh-gpui-agent-macos`, clippy, and `cargo check -p nsh-gpui-agent --features ios`); `windows-2025` (`-p nsh-gpui-agent-windows`) | Each platform crate is tested on its own system. | Yes, the plug's own crates on their systems. Not the Bot's build. |
| `hexuria/buwiz-forms`, branch `feat/nsh-plug` (out of scope; for reference) | `.github/workflows/nsh.yml` | Three NSH jobs: `nsh-macos` (line 83), `nsh-linux` (230), `nsh-windows` (417). Each has "The agent feature builds, with one NSH in the host" (lines 190, 361, 536). | Yes, on all three. This is the model to copy for the Bot's Stage 3. |

## Question 6: platforms, and what the plug changes

- **The Bot's agent runs on macOS only.** Evidence: the NSH dependency is macOS-only (`app/Cargo.toml` line 81, inside
  `[target.'cfg(target_os = "macos")'.dependencies]`); `app/src/lib.rs` lines 18 and 19 are the macOS Hub; Linux and
  Windows use `agent_hub_linux.rs` with no NSH symbol; the comment above the Linux entry in `app/src/main.rs` (about
  line 245) says "No Agent Hub".
- **The Windows entry point still calls `agent_hub::start()`** (`app/src/main.rs` line 312). That starts the Linux/
  Windows Hub, which does not use the NSH agent. Not an NSH dependency.
- **What the plug adds for the Bot.** On macOS: a new listener and in-process API (rows 2 to 9 above). On Linux, Windows,
  iOS, Android and web: nothing the Bot uses. Inferred: the Bot builds on Linux and Windows with the new NSH line (its
  non-agent code), so the NSH line must compile there too. D5 decides whether the Bot must keep building on those.
- **The Bot's scripts depend on the server.** `buwiz-drive`, `test-agent` and `buwiz-test` talk to `GPUI_AGENT_ADDR`.
  Under the plug, the listener is `nsh_gpui_agent_macos::install()`, which needs `GPUI_AGENT=1`, a loopback address and a
  token (plug README). The legacy server is behind `gpui-agent-server`. Neither has been run against the Bot.
- **The plug's README rule for second windows.** The Bot has several surfaces (a full-screen mini app and inline cards,
  `agent_hub/mod.rs` lines 96 to 104: each open holds a `Weak<AppView>`). The legacy listener serves the first surface only (plug
  README). Whether the Bot's drive scripts need a second window is **not known** (Stage 3 checks it on a test instance
  that opens a second mini app screen).

## Question 7: gpui-agent versions

- `buwiz` (NSH): `gpui-agent = "=0.3.0"` (`origin/buwiz` `Cargo.toml` line 57). `nsh-gpui-agent` 0.6.1 (line 56).
- The plug: `gpui-agent = "=0.4.0"` (plug `Cargo.toml` line 21). Plug README "gpui-agent version": 0.4.0 "changed the
  client's recipes, not what a host implements".
- `gpui-agent` branch of NSH: no root `gpui-agent` line was found (`git grep` on `origin/gpui-agent -- Cargo.toml`
  returned nothing). Where it is pinned on that line is not checked.
- The Bot: does not pin the `gpui-agent` CLI. The scripts call `gpui-agent` from `PATH` (`scripts/buwiz-drive`
  line 11). The README and AGENTS.md give no install line. So the CLI version a developer runs is not controlled.
- `buwiz-agent`: no gpui dependency. Its `PROTOCOL_VERSION = 1` is its own wire (`crates/agent/src/ids.rs` line 17, with
  the comment "nothing to do with gpui-agent's 2"). **Inferred**: the Bot's Hub assumes gpui-agent wire `2`, from
  that comment. Not tested.
- The plan's gate: run the Bot's three scripts with the `gpui-agent` `0.4.0` CLI (`cargo install gpui-agent-cli
  --version =0.4.0 --locked`, plug README) and keep the output. Then decide the pinned version (D9).

## Stages

Stages run in order. A stage is done only when its done list is ticked and its output is in the report. Each stage
opens its pull requests one at a time per repository and stops for the owner's merge before a dependent stage pins
the result. Draft pull requests are fine for proof.

### Stage 0: probe (lead session; can stop the round)

Work:

1. Fetch every repository in "Where things stand". Record the tips in the report.
2. On `hexuria/nsh`, make the candidate branch `feat/bot-agent-plug` from `origin/buwiz-next` (a local branch; push
   only as a `try/*` branch if a helper machine needs it).
3. Cherry-pick the Bot's non-agent commits from `data/buwiz-only-commits.tsv` onto the candidate. Record each conflict.
   Do not pick the agent commits (the ones in the 120-line heuristic group), except to read them.
4. In the Bot worktree (scratch, not committed): point `nsh` at the candidate, change the Bot's dependency on NSH to
   the plug, extend the `[patch]` table to every `nsh-*` crate the plug uses, and run `cargo check -p buwiz-bot` on
   macOS through `scripts/wait-build`. Record every error. Each error is a row of the gap table.
5. In the plug (scratch branch `try/bot-agent-plug` if needed), point its NSH entries at the candidate and run
   `cargo check -p nsh-gpui-agent-macos`. Record the result.
6. Ask Devin helper machines (Linux and Windows) to run `cargo check` on the candidate NSH's `nsh-platform-linux`
   and `nsh-platform-windows` with `dev`. Record their output.
7. Confirm inferred items: `dev-agent` on Linux (`cargo check --no-default-features --features dev-agent` on the
   Bot), the `gpui-agent` CLI output of `hello` (0.4.0), and whether `buwiz-next`'s own workflows
   (`.github/workflows/*.yml`) compile `nsh-platform-macos` on macOS.

Done when:

- [ ] The gap table is confirmed or corrected by compiler output, and each row says "compiles", "error: <text>" or
      "not compiled".
- [ ] The branch-layout decision (D1, D2) is recommended with evidence, after compile.
- [ ] Go or stop is written. A stop names the door list that would be needed, and why the move is not small.

### Stage 1: the NSH line for the Bot (hexuria/nsh)

Work: decide D1 and D2. Make the branch final. Add the Bot's non-agent commits, and any door the plug needs (one
door per commit, in NSH's commit style). Run NSH's own checks for each crate touched (`cargo +nightly fmt --all`,
`cargo test`, clippy), and `cargo metadata --locked` (the Bot's `nsh-lock` job checks it). Open one draft pull request
from `feat/bot-agent-plug` into `buwiz-next`. Do not merge.

Proof: compile `nsh-platform-macos` (with `dev`), `nsh-platform-linux` (`dev`), `nsh-platform-windows` (`dev`) on
macOS, Linux and Windows machines. Keep the output.

Done when:

- [ ] Branch exists with only the decided commits. `buwiz` and `buwiz-next` are unchanged (their tip shas recorded).
- [ ] `cargo metadata --locked` passes.
- [ ] Compiled on all three systems, output kept.
- [ ] Draft pull request open, not merged. The 14 upstream commits (D3) are either in it or listed as left out, with a
      compile result for each choice.

### Stage 2: the plug on the new NSH line (hexuria/nsh-gpui-agent)

Work: move the plug's NSH entries to the Stage 1 commit (one commit, all `nsh-*` entries). Update the README "Which NSH to
use" section. Make the macOS crate usable without NSH's `dev` in a release build (D10): either a feature that the Bot
turns off for release, or a doc that the Bot's release build is checked by Stage 3. Add a test or check on the plug's
CI that the NSH commit is the one the README names.

Proof: the plug's CI (`ci.yml`) on its three systems, with the new pin. A local `cargo tree` check on the Bot for one
NSH (Stage 3 uses it).

Done when:

- [ ] Plug pull request open with the new pin. CI green on ubuntu-24.04, macos-15, windows-2025 (its `ci.yml`).
- [ ] README names the new NSH branch and commit.
- [ ] `cargo tree -p nsh-gpui-agent-macos -i nsh-testing --depth 1` shows one entry.

### Stage 3: the Bot (buwiz-bot/buwiz-bot)

Work:

1. Cargo: remove `features = ["gpui-agent"]` from the `nsh-platform-macos` path dependency (the feature does not exist on
   the new line). Add the plug's `nsh-gpui-agent-macos` as a Git dependency at the Stage 2 commit, optional, under
   `dev-agent`, with `default-features = false` for the release build. Extend the `[patch."https://github.com/hexuria/nsh"]`
   table to every `nsh-*` crate the plug reaches (at least `nsh-testing`, `nsh-dev`, `nsh-base`, `nsh-core`, `nsh-macros`,
   `nsh-engine`, and `nsh-platform-macos` if the plug pulls it from Git). Re-check with `cargo tree`.
2. Code: import the plug's `AppViewAgentExt` where `view.ui_tree()` and `view.act()` are called
   (`agent_hub/mod.rs`). Import `ui_tree::UiNode`, `UiAct`, `SCREEN_CHANGED` from the plug (`mod.rs` line 63,
   `filter.rs` line 26). Import `on_input` and `Input` from the plug (`mini_v1/attribution.rs` line 276).
   Every import stays behind `cfg(feature = "dev-agent")` where the release build needs it gone.
3. Submodule: `nsh` to the Stage 1 commit, `.gitmodules` branch to `feat/bot-agent-plug`.
4. Behavior: decide D4 (listener) and run the Bot's three scripts on a test instance.
5. Docs: `README.md` section "Driving the UI programmatically" and `AGENTS.md` lines 30 to 65 name the new listener and
   the gpui-agent CLI version (D9). `NSH-FORK.md` and `nsh-patches.toml`: entries for the topics that moved to the plug
   are marked with the plug's pull request.

Proof, on macOS (a Devin macOS helper for the long runs):

- `scripts/wait-build cargo test --locked -p buwiz-bot ...` (the `app` job) and the `clippy` job, with default features.
- `cargo tree -p buwiz-bot -i nsh-testing --depth 1` (one NSH).
- Release: `scripts/release/build.sh` (`--no-default-features`), then the three string checks of lines 113 to 122 **and** a
  new check for the plug's listener and development symbols. Keep the output.
- Test instance (`scripts/buwiz-test`): `scripts/test-agent <dir> snapshot`, `click <id>`, `screenshot --out ...`,
  on a chat and on one mini app screen (Books or hello-mini-app) that takes a press and a type. Keep the output.
- Provenance: with `GPUI_AGENT_ACTOR=agent` on the test instance, a press through the agent reaches the mini app as
  `origin: agent`, and a press by the person does not. Keep the output (row 7).

Done when:

- [ ] `cargo tree` shows one NSH. `cargo test`, `clippy` and the release check pass, with output kept.
- [ ] The test instance is driven through the agent (snapshot, click, type, screenshot), output kept.
- [ ] Provenance check (row 7) reported, pass or fail, with the output.
- [ ] Bot pull request open with the submodule pointer and the Cargo changes together (NSH-FORK.md "Updating Buwiz
      Bot": code and submodule in one commit). CI green on `ci.yml`.

### Stage 4: buwiz-sdk

Work: `crates/app/Cargo.toml` `nsh-core` and `nsh-engine` to the Stage 1 commit. Build `buwiz-app` on macOS and Linux
(Devin helpers). Pull request. Then D7 chooses the one SDK commit everyone pins.

Done when:

- [ ] `cargo check` and `cargo test` of the SDK pass on macOS and Linux, output kept.
- [ ] Pull request open, not merged. The chosen SDK commit is named in the report.

### Stage 5: the mini apps

Work, in this order: books, google-workspace, mini-rs, ebirforms, mini-tsx. Each: NSH pins to the Stage 1 commit (where
present); SDK pins to the Stage 4 commit; google-workspace's test that uses `nsh_testing::agent::SemanticNode` moves to the
plug's type (row 11). The Bot's pins of `books-screens`, `books-mini-app` and `gws-screens` move to the same commits as
their repositories, and the Bot's `buwiz-app` rev equals Books' (D8).

Proof: each repo's own build and tests locally; google-workspace's CI (`ubuntu-latest`, `macos-15`).

Done when:

- [ ] Each pull request open, not merged, with its checks output kept.
- [ ] The Bot's pins match the mini apps' pins (checked with `git grep -n "rev ="` on each).

### Stage 6: retire the old line (owner approval first)

Work: the Bot and the mini apps no longer use `buwiz`. Propose, do not do: a dated tag on `buwiz`'s tip (the convention in
`NSH-FORK.md`, `buwiz-YYYY-MM-DD`) and the rename question (D6). Update `NSH-FORK.md` for the new line.

Done when:

- [ ] `grep` for `hexuria/nsh` branch `buwiz` in the Bot and in the SDK and the mini apps finds nothing.
- [ ] The owner has approved the tag in writing. The tag is created only after that approval.
- [ ] `buwiz` tip sha is the same as at the start of the round.

## Decisions for the candidate lines (recommended, to be confirmed in Stage 0)

These are not owner decisions. They are the recommendation a session makes unless Stage 0 shows otherwise.

- The Bot's NSH is a new branch, not `buwiz-next` and not `buwiz` (D1).
- The base is `buwiz-next` at `ca39c0e91`, because the plug's pinned commit `365864ba8` is an ancestor of it (verified).
- The Bot's non-agent topics are the list in Question 3.

## Open decisions for the owner

Each has options and a recommendation. Stage 0 may change a recommendation with evidence. It must not change a decision
above.

- **D1. Where the Bot's NSH lives.** (a) New branch `feat/bot-agent-plug` on `hexuria/nsh`, cut from `buwiz-next`, with
  the Bot's topics. (b) Add the Bot's topics to `buwiz-next`. (c) Keep the Bot on `buwiz`. **Recommend (a).** (b) changes
  the line eBIRForms' next pin would bring. (c) is excluded by the decisions above.
- **D2. Base of that branch.** (a) `buwiz-next` at `ca39c0e91` (contains the plug's `365864ba8`). (b) `gpui-agent` plus
  topics. **Recommend (a)**: it has the same doors plus the non-agent symbols the Bot uses (Question 3, row 12).
- **D3. The 14 upstream PRs on `buwiz` (`#2330` to `#2356`).** (a) Carry them in the branch. (b) Leave them out. **Recommend**:
  decide by compiling the Bot both ways in Stage 0. If the Bot compiles and its tests pass without them, leave them out.
- **D4. Which listener the Bot uses in dev.** (a) The plug's strict adapter via `install()` (the plug's own listener;
  smaller; no `press`/`scroll` gap in its hello; the plug README says it serves the window). (b) Keep the legacy
  in-process server by enabling `gpui-agent-server` on the plug (keeps the Bot's behavior exactly; keeps the legacy code the
  plug says it moved "unchanged"). (c) Both, selectable by env. **Recommend (a)**, proven by the Stage 3 scripts. If the
  scripts need the second window or `press`/`scroll`, take (b) for that step and report it.
- **D5. Must the Bot keep building on Linux and Windows?** The Bot's CI is macOS only (`ci.yml`), but the Bot has Linux and
  Windows entry points. (a) Yes: compile the non-agent Bot on Linux and Windows helpers in Stage 1 and 3. (b) No: macOS only.
  **Recommend (a)**, cheap (no agent code there).
- **D6. Rename and retirement.** After the Bot leaves, (a) tag `buwiz` as `buwiz-YYYY-MM-DD` (NSH-FORK.md practice), and keep
  `buwiz-next` as its name; (b) also rename `buwiz-next` to `buwiz`. **Recommend (a)**. The rename touches every pin that
  says `buwiz`, including eBIRForms. Only the owner decides it, and not in this round.
- **D7. One SDK commit for everyone.** Today the SDK revs are `369935a8` (Bot), `bf188594` (Books), `f63038d` (google-workspace,
  mini-rs, ebirforms, mini-tsx). **Recommend**: the SDK commit that builds on the new NSH line (Stage 4), used by all of them.
- **D8. Drift that exists today.** The Bot's `buwiz-app` differs from Books'; the Bot's `books-screens` and `gws-screens` are
  behind their repos' HEADs. **Recommend**: bring them to the same commits in Stage 5, because the Bot's own rule (Books and
  the Bot pin the same `buwiz-app` rev) is already broken. The owner confirms, since a newer Books or google-workspace may
  change the Bot's screens.
- **D9. The gpui-agent CLI version the Bot's scripts use.** (a) Pin `=0.4.0` in the Bot's docs and scripts check. (b) Keep
  `=0.3.0` and test both. **Recommend (a)** after a Stage 3 run shows the three scripts work with 0.4.0.
- **D10. Dev code in the plug's release build.** The plug pins `nsh-platform-macos` with `dev`. (a) The plug's macOS crate
  takes a feature that the Bot leaves off for release, and CI checks the release binary. (b) Accept `dev` in release and say so.
  **Recommend (a)**: the owner's rule is that a shipped Bot has no test or agent code (`build.sh` lines 108 to 122).
- **D11. `buwiz-agent` pin.** The Bot pins `0610474f`; its origin tip is `3c829c4`. **Recommend**: leave it. This round does
  not move it (Question 4, row 11).

## Words used here

- **Line**: a named NSH branch (`buwiz`, `buwiz-next`, `gpui-agent`, the new `feat/bot-agent-plug`).
- **Door**: a small change in NSH that makes something public. No logic (`SOCKET.md` on `gpui-agent`).
- **Plug**: `hexuria/nsh-gpui-agent`, all the agent logic outside NSH.
- **Bot**: `buwiz-bot/buwiz-bot`, the desktop app.
- **Ripple**: the SDK, the mini apps and the pins that follow the Bot.
- **Verified**: checked in the repository or its remote with a command or a file path. **Inferred**: not checked.
