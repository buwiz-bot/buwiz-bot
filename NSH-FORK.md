# The nsh fork

Rox builds on its own copy of Native Shell. **`rox` = upstream `main` + our
topics**, nothing else:

| Where | What | Rule |
|---|---|---|
| `longbridge/nsh` `main` | upstream | we change it only through PRs |
| `hexuria/nsh` `main` | mirror of upstream `main` | only ever fast-forwarded (`scripts/nsh-fork sync`) |
| `hexuria/nsh` `pr/<topic>` | one feature or one fix, cut from `main` | follows upstream's `AGENTS.md`; becomes one upstream PR |
| `hexuria/nsh` `rox` | `main` + the active topics of `nsh-patches.toml` | rebuilt from scratch, never merged into |
| `rox/nsh` (submodule) | a commit of `rox` | Rox's `Cargo.toml` takes nsh's crates from it by path |
| tags `rox-YYYY-MM-DD` | every `rox` tip before a rebuild | keep old Rox commits buildable |

`nsh-patches.toml` is the registry: every topic Rox carries, its upstream PR
and its status. `scripts/nsh-fork status` checks it against git and GitHub.
`hexuria/native-shell` is the fork's old name (a redirect): use
`hexuria/nsh`.

## Guardrails

- Commit nsh changes on a topic branch. `main` holds upstream's commits
  only, and `rox` only what a rebuild puts there.
- Change `rox` with `scripts/nsh-fork rebuild --push`: it tags the old tip
  before it moves the branch, so every commit Rox recorded still fetches.
- One topic is one upstream PR: one area, its tests, upstream's commit
  style (`macos: Do the thing`), formatted with `cargo +nightly fmt --all`,
  passing the crate's tests. Write it as if it goes upstream tomorrow, even
  a fix we expect to carry: carried patches stay small that way.
- Keep the fork's diff small: when upstream declines a feature, propose the
  hook it needs instead (a callback, an extension point) and move the
  feature itself into Rox.
- Rox depends on nsh by path through the submodule. Publish nothing of the
  fork to crates.io: renaming every crate (and the paths `nsh-macros`
  generates) would fork the ecosystem for an app nobody else builds on.

## Changing nsh

1. Start from a fresh `main`: `scripts/nsh-fork sync`, then in `rox/nsh`
   `git switch -c pr/<topic> main` (or a worktree).
2. Make the change with its test; read upstream's `AGENTS.md` and the
   `docs/dev/` page of the area first. Run the crate's tests, clippy and
   `cargo +nightly fmt --all`.
3. Add a `[[topic]]` to `nsh-patches.toml` (status `local`), push the
   branch to `origin`.
4. Rebuild `rox` and update Rox (below). Done when Rox builds and runs on
   the new `rox` and `scripts/nsh-fork status` flags nothing for the topic.

## Upstreaming a topic

1. `scripts/nsh-fork rebase pr/<topic> --push` so it applies to today's
   upstream.
2. `gh pr create --repo longbridge/nsh --head hexuria:pr/<topic>` with
   upstream's PR template (`## Public API` for new API, before/after
   numbers for hot paths). A topic that depends on another goes up after it.
3. Set `pr` and `status = "open"` in the registry.
4. Answer reviews on the topic branch, then rebuild `rox`.
5. Merged: set `status = "merged"`. The next rebuild leaves it out (upstream
   `main` has it). Declined: `status = "rox-only"` with the reason in
   `note`, and look for the hook upstream would take instead.

## Keeping up with upstream

1. `scripts/nsh-fork sync`, then `scripts/nsh-fork status`.
2. Topics flagged `rebase`: `scripts/nsh-fork rebase <topic> --push`; fix
   what breaks on the topic itself.
3. `scripts/nsh-fork rebuild`: builds `rox-new` in `/tmp/rox-nsh-fork`. A
   conflict stops it there; resolve it once (git `rerere` replays the
   resolution in later rebuilds) and run it again.
4. Check `rox-new`: point the submodule at it, build Rox, launch it, then
   `scripts/nsh-fork rebuild --push`.
5. Update Rox. Done when Rox is committed on the new `rox` commit and runs.

## Updating Rox

1. In `rox/nsh`: `git fetch origin && git switch rox && git reset --hard origin/rox`.
2. Fix Rox for upstream's changes (`cd app && cargo build`), launch it and
   smoke-test chats, the palette, Settings, the file viewer and a live mini
   app.
3. Commit in Rox: the code and the `nsh` submodule pointer together.

Upstream moves fast and breaks before its first compatible release: 0.3
moved `nsh_core::ui` to `nsh_core::element` / `nsh_core::prelude`, `input`
to `nsh_base`, and every `nsh/<x>` submodule into `nsh` (no aliases). Rox
rewrites old imports in the mini apps it compiles (`mini::current_homes`);
the server's prompts and the MCP mini apps should import from the new homes.

## Why it is set up this way

- **Rebuilt, not merged.** `git log main..rox` is exactly what we carry.
  Merging upstream into `rox` buries the patches in merge history and
  keeps duplicates around once upstream takes a change in another form.
- **The registry, not patch matching.** Upstream squash-merges PRs, so git
  cannot tell that a topic landed; the PR number in the registry can.
- **Tags before every rebuild.** Rox's history records submodule commits;
  a rebuilt `rox` would orphan them without a tag.
