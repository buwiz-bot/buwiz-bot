# Rox security model

This model describes the `perf-parity` tree. Each control is labelled **[observed]** with a file:line reference or **[assumed]** when it depends on code outside this repository. Component and finding IDs are stable, so findings from future scans can be compared.

## Scope and evidence

Rox is a native macOS chat client connected to a separately running Grok app server. iOS is a compile-maintained simulator target, not a supported product target ([README](README.md)). The model covers every tracked path on `main`:

- `app/`: the application crate, its scripts and configuration.
- `nsh/` (UI/runtime submodule) and `blaze/` (ra-jit compiler/live-patching submodule): their security boundaries, without claiming an exhaustive audit of every dependency or platform path.
- `connectors/` (connector setup guides), `scripts/` (the nsh fork tool), `.devin/` (agent instructions for the live loop) and the root documents.

`app/target/` is build output and is not modelled.

Evidence: [README](README.md), [GOAL.md](GOAL.md), [app/Cargo.toml](app/Cargo.toml), [app/nsh.json](app/nsh.json), the app sources named below, the `gpui-agent` 0.1.0 crate (crates.io, checksum as in `app/Cargo.lock`), NSH's AppKit host, networking, QuickJS sandbox, capability and URL-opening code, and the [ra-jit security policy](blaze/SECURITY.md). The latter describes ra-jit only and is not a policy for the whole product.

The backend is a local `grok-shell` checkout that the README tells developers to run (`node server/app-server.mjs`, [README](README.md) "Prerequisites"). No backend source or deployment infrastructure is in this repository. Backend binding, authorization, credential storage and deployment controls are therefore unknown, not presumed safe.

## Owner-confirmed policy

- Trust the OS and the user's account. Generated content, remote services, and other local users are untrusted. Already-malicious code executing as the same account is outside the attacker model; a path that lets untrusted content acquire that account's privileges remains in scope.
- Protect all assets. Credentials, private chats/files, host integrity, and preventing unauthorized code execution are critical assets.
- The backend and infrastructure are outside this assessment, not accepted as secure.
- Record CWE, CVE, owning-team, and compliance-scope tags when supported by evidence. No team assignments or compliance obligations have been specified; do not invent them.
- No other risk acceptance or vulnerable-surface exclusion has been specified.

## Components and trust boundaries

A more specific row takes precedence over a containing directory. `app/src/store.rs` is split by function; the line ranges are those on this tree and should be updated when it moves. Where a weakness in a component is tracked in a private security advisory, this document says "see the private advisory" instead of describing it.

| ID | Component | Code covered | Responsibility and boundary |
|---|---|---|---|
| C1 | Native client | `app/src/{main,lib,hot,chat,composer,ctx,sidebar,pane,palette,botmodal,botdetails,models,settings,slash,theme,icons,statusfx,toast,zoom,dotgrid,history,versions,keys,mark}.rs`, `app/src/mark/`; `store.rs` outside the ranges below (threads, bots, turns, groups, drafts, palette, sending, selection, reactions); `app/nsh.json`, `app/Cargo.toml` | User actions, private conversation state, preferences, model selection and chat mutations. Backend/model data crosses into native presentation and reactive state. `keys.rs` holds the keymap and POSTs it to `/keymap`. `hot.rs` is the patchable UI root; its dev hook is C10. |
| C2 | Backend interface | `app/src/client.rs` | HTTP to `http://127.0.0.1:8787`. Route families used: `/chat`, `/turns`, `/thread(s)`, `/status`, `/prefs`, `/models`, `/catalog`, `/bot`, `/state`, `/app(s)`, `/module`, `/artifact`, `/file`, `/attach`, `/avatar`, `/copy`, `/popout`, `/retry`, `/group(s)`, `/routine(s)`, `/keymap`, `/mcp`, `/plugins`, `/skills`, and the card routes in C9/C12. `client::req` passes any path starting with `http` through unchanged (`client.rs:11-18`). Connector OAuth redirects to `http://127.0.0.1:8787/oauth/callback/<connector id>` on the backend ([connectors/README.md](connectors/README.md)). Paths interpolated without `client::urlenc`: `store.rs:745, 3134, 3312, 3468`, `files.rs:75-79`. Loopback is a network boundary, not an authentication credential. |
| C3 | Mini-app runtime | `app/src/{mini,widgets,appdetails}.rs`; `store.rs` `refresh_thread_apps` (3082), app details and icons (3266-3640), `mini_source`..`retry_card` (5728-5937), `mini_title`..`mini_close_top` (6656-6741); `nsh/crates/{nsh-js,nsh-engine,nsh-core,nsh-bundler,nsh-package,nsh-base}` | Fetches `/module?name=`, compiles server-provided TSX and runs it in its own AppView/QuickJS runtime. The shelf is in `widgets.rs`. Untrusted generated code crosses into native modules, network, WebView, clipboard and open-URL capabilities. |
| C4 | Content handling | `app/src/{files,md,avatar,mermaid}.rs`; `store.rs` drag/drop and avatar editing (4514-4664), `pick_attachments` (4947), `file_raw_url`..`copy_sel` (5295-5570), library items (6216-6373); NSH image, media, WebView and open-URL code | Private attachments, generated media, Markdown, Mermaid diagrams (rendered by the Pele helper, C10), library files and backend-rendered documents. Remote or generated bytes and URLs cross parser, renderer and external-navigation boundaries. |
| C5 | Connector management | `app/src/plugins.rs`, `app/src/cards/plugin_needs.rs`, `connectors/`; `store.rs` `open_mcp_details` (3353), `fetch_mcp`..`conn_items` (3688-4513), `detach_mcp` (6317) | MCP servers, plugin install/settings/uninstall, connector accounts, private skills and auth. Plugin setup values (client secrets, API keys) are POSTed as `values` to `/plugins/install` and `/plugins/settings` (`store.rs:4312-4326`); backend storage of them is unknown. |
| C6 | Platform | `nsh/crates/nsh-platform-*` and the platform runners, except C4's files and C10's `gpui_agent.rs` | AppKit/iOS hosting and networking. Other NSH backends are dependency code, not supported Rox targets. |
| C7 | Live build toolchain | `app/src/live.rs`, `app/{dev,ios,ios-dev,bundle-dev}.sh`, `app/rust-toolchain.toml`, `blaze/`, `.gitmodules`, `nsh-patches.toml`, `NSH-FORK.md`, `scripts/`, `.devin/` | Native builds, the compiler daemon, local/remote code caches, live binary patches, the dev app bundle (`bundle-dev.sh`, which declares the `rox://` scheme) and the nsh fork process. Patch material is executable code with the user's privileges, not a sandboxed widget. |
| C8 | Link router | `store.rs` `turn_link`, `open_link` (6566-6655), `link_token`, `mini_link` (6742-6770); `app/src/lib.rs:93-112` | Routes links from Markdown (`md.rs:78-80`), MCP resolve results (`slash.rs:441-444`), library items (`store.rs:6221`) and OS-level `rox://` opens from other apps and web pages (`lib.rs:101-112`). Routes: `rox://chat/<thread>[?m=<turn>]` opens a chat; `rox://chat/new?draft=<text>` starts one with a draft; `rox://plugin/<name>` opens a plugin page; `rox://app/<server>/<app>` opens a mini app with its link token; `rox://view?path=<file>` shows a local file in the viewer. Other URLs go to the OS through `open_url` (`store.rs:6652`). Weaknesses: see the private advisory. |
| C9 | Credential vault and user-form cards | `app/src/vault/`, `app/src/cards/{user_form,transport,save_login}.rs` | Saved site logins and the agent-requested forms that use them. Agent-supplied form specs choose the login offered (`user_form.rs:764-769`); answers go to the backend by REST (`transport.rs`), never through chat text. Weaknesses: see the private advisory. |
| C10 | Local listeners and dev hooks | `nsh/crates/nsh-platform-macos/src/gpui_agent.rs`, `app/pele/`, `app/src/hot.rs` dev hook (326-336, 643-683) | Loopback listeners reachable by other local users: gpui-agent (`127.0.0.1:17421` by default), the backend (8787, C2) and the Pele Mermaid helper (a random loopback port, `pele/serve.mjs:81-101`). gpui-agent clients get UI input and snapshots of displayed data. With `GPUI_AGENT=1`, `hot.rs` reads commands from `$TMPDIR/rox-agent-menu`. Weaknesses: see the private advisory. |
| C11 | Logs | `app/src/main.rs:12-70`, `log::` call sites | `env_logger` output, plus the jank log. |
| C12 | Agent cards | `app/src/cards/{mod,approval,choice,tests}.rs` | Interactive cards parsed from backend frames (`Card::from_wire`): approvals of tool runs and local-exec policy rules, and choices. Answers go to the `/ag-ui/…`, `/local-exec/…` and `/coworkers/…` routes (`transport.rs:116-144`). |

Flow: user → C1 → C2 → backend → models/connectors (external). Backend replies → C1/C4/C12 display, C9 forms, or the C3 compiler → mini-app VM → native capabilities. Links → C8 → in-app routes or the OS. Opt-in automation → C10 → native UI. Developer edits, build inputs and caches → C7 daemon → executable and live process patches.

## Entry points and reachable attackers

- Chat responses, generated widget source, attachment metadata/bytes, module names, URLs and backend JSON: influenced by a malicious model, generated content, connector, or compromised remote service.
- Agent-supplied card frames and form specs (C9, C12).
- Mini-app JS and native-module calls: generated code is untrusted even when delivered by the configured backend.
- `rox://` URLs opened by other apps and web pages, and links in generated content (C8).
- Loopback listeners (C10, and the backend on 8787): other local users can reach loopback ports. The backend's binding and authentication are unknown.
- MCP and plugin configuration: user input and backend catalog data reach privileged backend actions; plugin setup values carry credentials.
- Build workspaces, proc macros, build scripts, patch files, local caches and remote stores: executable-code supply chain. Other-user or remote-writer control matters; same-user compromise alone is outside the attacker model.

Configuration shipped in this repository (Cargo features, default policies, scripts, README instructions) counts as reachable, not as an opt-in to discount.

## Assets

| Class | Assets |
|---|---|
| A4 critical (owner-confirmed) | Saved site-login passwords and authenticator seeds; provider/connector credentials and plugin setup values; private prompts, chat histories, files, avatars and generated artifacts; host account integrity and freedom from unauthorized native code execution |
| A3 private state | Saved-login metadata, mini-app state, thread/model preferences, connector configuration and account identifiers, source text under live edit, trusted executable/cache integrity |
| A2 limited | File paths, metadata, frame timings, local process and build availability |
| A1 negligible | Public or cosmetic data |

## Existing controls and their limits

- **[observed]** Saved-login secrets are macOS Keychain generic passwords under service `dev.goldcoders.rox.site-login` (`vault/mod.rs:9-10, 39`). Metadata is in Rox's own `logins.db`, which has no password column (`vault/db.rs:1-2`). Every secret read needs device-owner authentication, and anything but `Verified` refuses (`vault/mod.rs:253-257`, `vault/touch_id.rs:36-46`). Login origins are matched by exact host, plus the port on loopback and IP hosts (`vault/origin.rs`). The log of an unsent card body redacts `values` (`cards/transport.rs:96-103`).
- **[observed]** gpui-agent is off unless `GPUI_AGENT` is set, serves a release build only with `GPUI_AGENT_ALLOW_RELEASE`, binds loopback by default, and requires a non-empty `GPUI_AGENT_TOKEN` unless `GPUI_AGENT_INSECURE_NO_TOKEN` is set (gpui-agent 0.1.0 `security.rs:33-46`). Requests are authorized by HMAC-SHA256 over a 32-byte per-connection nonce (`hmac_auth.rs:8-10`, `dispatch.rs:28-60`; `gpui_agent.rs:6-12`).
- **[observed]** The Pele helper is started by `mermaid.rs:90-116`, binds `127.0.0.1` on a random port, and answers only `POST /render` (`pele/serve.mjs:81-101`).
- **[observed]** The jank log (`~/Library/Logs/Rox/jank.log`, or `ROX_JANK_LOG`) is written only while the performance monitor runs (⌘⇧F, `keys.rs:220`; `main.rs:12-70`) and records frame timings. At `RUST_LOG=info`, a connector account identifier is logged (`cards/plugin_needs.rs:241`).
- **[observed]** Typed Rust UI and JSON parsing; `client::urlenc` encodes URI components where used (C2 lists where it is not).
- **[observed]** Mini apps get their own AppView/QuickJS runtime with heap, stack and time limits enforced in Rust (`nsh/crates/nsh-js/src/sandbox.rs`). `mini_bundle` selects policy by the module key, not the server-supplied title: generated widgets and other non-`mcp:` modules get a hardened deny-all declaration, while `mcp:` apps get only the app-server origin (`client::GEN`, `http://127.0.0.1:8787`) over HTTP on any path. NSH also applies that grant to WebSockets on the same origin. No other capabilities are granted, and an invalid app-server origin falls back to deny-all. `widget_policy_grants_nothing`, `mcp_apps_reach_only_the_app_server` and `a_widget_cannot_fetch_the_app_server_or_read_the_clipboard` cover the selection; the latter also proves plain `Bundle::source` retains the default-policy behavior. `Policy::default()` has `declared: None`, which `Policy::access` turns into `Capabilities::all()` (`nsh/crates/nsh-engine/src/lib.rs`); Rox avoids that for generated widgets.
- **[observed]** `check_url` is a deny-list: it refuses `javascript`, `vbscript`, `data`, `blob`, `file`, `about` and `asset`, whitespace/control characters, oversized URLs and host-less web addresses, and accepts every other scheme (`nsh/crates/nsh-core/src/open_url.rs:20-58`). An open-URL grant with no schemes allows every scheme (`nsh/crates/nsh-core/src/access.rs:376-387`), which is what `Capabilities::all()` grants. Mini apps receive no open-URL grant under either `widget_policy` or `mcp_app_policy`.
- **[observed]** NSH does not capability-gate `nsh/desktop` window/menu commands or `os.homedir()` (`nsh/crates/nsh-core/src/runtime/desktop.rs`, `nsh/crates/nsh-core/src/runtime/os.rs`). The mini-app test `known_gap_desktop_window_and_homedir_are_ungated` pins this behavior. Fixing it belongs in NSH, through an `Access` gate or a desktop-module filter for embedded/untrusted runtimes.
- **[assumed]** macOS HTTP uses NSURLSession with system proxy, cookie and transport policies. Separate VMs share that networking, so cross-surface data isolation is not assumed.
- **[observed]** The ra-jit policy says ra-jit "is a prototype and has not had a security review" (`blaze/SECURITY.md:3`). Builds and the daemon run with the user's privileges. On Unix the socket directory must be owned by the user with mode 0700, the socket is 0600, and other-user peers are refused (`blaze/crates/ra-jit/src/socket.rs`); the policy says this refusal "is not covered by a test" (`blaze/SECURITY.md:26-27`). Local cache checksums detect damage, not tampering. Remote pack hashes do not authenticate the bucket writer; signed manifests are enforced only with `RA_JIT_REMOTE_TRUST`, which is not configured in this repository. ra-jit is neither a sandbox nor a borrow/move checker.
- **[observed]** `ra-jit-live` does not apply those checks itself: `edit()` connects with a plain `UnixStream::connect` (`blaze/crates/ra-jit-live/src/lib.rs:190-205`), and the patch reader creates its directory 0700 but reads `patch` without checking an existing directory's owner or mode (`blaze/crates/ra-jit-live/src/macos.rs:109, 419-468`). The privacy of the `RA_JIT_HOME` / `XDG_RUNTIME_DIR` root is the only control.

## Finding prioritization

Inputs: the **scanner severity** is the severity a security scan reports on a finding. The **verified preconditions** are those the scan confirmed in source; anything it marks unconfirmed is unverified.

Likelihood, by the least-privileged attacker that can trigger it:

| Level | Attacker and interaction |
|---|---|
| L4 | Remote party, no user action |
| L3 | Untrusted content or a remote service, with ordinary use (opening a chat, an attachment, a link) |
| L2 | Another local user, or a precondition not met by shipped configuration |
| L1 | Same user only, or an already-compromised account (outside the attacker model) |

Impact is the highest asset class (A1–A4) reached.

Rules:

1. Start from the scanner severity. Unchanged findings keep it, with a reason naming their likelihood and impact levels.
2. Lower a finding only when the owner policy places it there: L1 findings, or findings reaching only assets the owner excluded.
3. Raise a finding only when it is L3 or L4 with every precondition verified and it reaches A4. A finding with any unverified precondition is never raised, whatever it reaches. Owner-confirmed asset classes are the facts that permit the rare increase, so the two rules do not conflict.
4. No team or compliance scope is assigned without evidence. Dismissed, resolved and duplicate findings are not reopened by reranking.

## Known findings

| ID | Finding | Component | Likelihood / impact | Priority |
|---|---|---|---|---|
| RX-001 | Generated mini-app widgets run with broad default capabilities — fixed: non-`mcp:` modules deny all; `mcp:` apps reach only the app-server origin over HTTP and its NSH-authorized WebSocket | C3 | L3 (needs the model to follow injected instructions; unverified) / A4 | Medium |
| RX-002 | `svg_view` interpolates the attachment URL into HTML unescaped, under the backend's origin | C4 | L3 (outside control of the attachment URL unverified) / A4 | Fixed in 7925692 (escaped, server files only, no base URL) |
| RX-003 | `ra_jit_live::edit` connects to the daemon socket without a peer check | C7 | L2 (shared machine, other-user-writable socket root) / A3 | Low |
| RX-004 | See the private advisory | C9 | See the private advisory | See the private advisory |
| RX-005 | The dev launcher and README published a fixed gpui-agent token; any local user could drive the dev bundle's Rox ([GHSA-pwr9-c68j-6jr4](https://github.com/hexuria/rox/security/advisories/GHSA-pwr9-c68j-6jr4)) | C10 | L2 (another local user) / A4 | Fixed in 5e7b0a2 (a new random token per launch, in a 0600 file) |
| RX-006 | `open_link` let links from content read any local file (`rox://view`) and open any scheme the deny-list allowed ([GHSA-xpgx-g4x9-7f5g](https://github.com/hexuria/rox/security/advisories/GHSA-xpgx-g4x9-7f5g)) | C8 | L3 (untrusted content, one click) / A4 | Fixed in 1e7f231, 369882d (previews only; other schemes and OS-created chats ask) |
| RX-007 | Embedded mini apps can invoke ungated `nsh/desktop` window/menu commands and read `os.homedir()`; fix in NSH with an `Access` gate or desktop-module filter for untrusted runtimes (`known_gap_desktop_window_and_homedir_are_ungated`) | C3 | L3 (model-generated code; precondition unverified) / A2 | Low |

## Open questions and validation limits

- Backend: storage of plugin setup values and card answers, binding, authentication and deployment controls.
- Per-widget storage identity, and which navigation paths call `check_url`.
- Windows: `blaze/SECURITY.md` says the directory and peer checks are not implemented, but `socket.rs` (lines 226-240, 298-299) and `socket/windows.rs` (lines 125-266) implement owner/ACL and peer checks. Which one is current?
- Who can write the code caches, and which remote signing keys are intended.
- Owners and compliance frameworks.

This model comes from static inspection, not macOS/iOS runtime or exploit validation. Unverified preconditions stay uncertainties, not accepted risks. Update this model when trust boundaries, data handling, runtime capabilities, backend integration or owner risk decisions change. Keep component and finding IDs stable.
