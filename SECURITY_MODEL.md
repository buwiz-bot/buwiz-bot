# Threat model: Buwiz (Buwiz Bot + buwiz.com)

This model covers one system split across two repositories: `buwiz-bot/buwiz-bot` (desktop chat app, local broker, bridge) and `buggyDcode/buwiz-com` (the public Buwiz server).

## System overview and trust boundaries

- **buwiz.com server (`buwiz-cloud`, the Gatekeeper):** one Rust/axum binary serving `buwiz.com`, `ebirforms.`, `account.` and `api.` on AWS EC2 (one t4g.micro, `ap-southeast-1`), bound to 127.0.0.1:8080 behind a Cloudflare Tunnel; Postgres on Neon; claim documents in Cloudflare R2, encrypted with `DOCUMENT_KEY`. Settings come from SSM `/buwiz/*`.
- **The Sender (`buwiz-sender`):** a private process on 127.0.0.1:3009 holding BIR's passphrases, the SFTP login and the HPKE private key; the Gatekeeper reaches it with `SENDER_SECRET`. The BIR relay code (`src/bir/`, IAF encryption, SFTP upload) is in a private repository and is not scanned.
- **Buwiz Bot:** a macOS app (`app/`, NSH UI) and its broker (`broker/buwiz-broker`) on `127.0.0.1:8787`. The broker holds chats, model calls, routines, v1 plugins/mini apps, MCP hosting and agent messaging; secrets live in the macOS Keychain (`helper/keystore.rs`). Agent "computers" run in Docker, EC2 or E2B (`broker/buwiz-computer`).
- **How they connect:** the eBIRForms mini app inside Buwiz Bot signs in to `api.buwiz.com` through the device flow (client `buwiz-ebirforms`) and files returns through the relay; `buwiz.com` serves Bot's download (`latest.json` from R2) published by `buwiz-bot/.github/workflows/release.yml`.

Boundaries: internet → Cloudflare → Gatekeeper; Gatekeeper → Sender (shared secret); Gatekeeper → Postgres / R2; web pages and other local origins → Bot broker (Origin guard, `guard.rs`); mini apps / plugins / MCP servers → broker host APIs; untrusted content → model → tools / computers; outside coding agent → `/v1/bridge/*` (endpoint token).

## Components

| Component | Code |
|---|---|
| **Accounts** | `buwiz-com/src/account/` (sign-in, TIN claims, staff, roles, OAuth device flow, agent OAuth, `/mcp`), `src/ocr/`, `migrations/` |
| **Filing Relay** | `buwiz-com/src/filing/` (devices, tickets, envelope, submissions, gate, trust), `src/sender/`, `src/bin/`, `src/relay/` |
| **Public Sites** | `buwiz-com/src/sites/`, `bot-site/`, `src/uptime/` |
| **Bot App** | `buwiz-bot/app/` (incl. `mini_v1/`, `vault/`, `office/`), `buwiz-bot/office/` |
| **Bot Broker** | `buwiz-bot/broker/buwiz-broker`, `buwiz-mcp`, `buwiz-protocol`, `bridge/`, `connectors/` |
| **Agent Computers** | `buwiz-bot/broker/buwiz-computer/` |
| **Infra & Release** | `buwiz-com/deploy/ec2/`, `Dockerfile`, `.github/`; `buwiz-bot/.github/workflows/release.yml`, `scripts/release/` |

## Entry points and who can reach them

- `account.buwiz.com`, `api.buwiz.com`, `ebirforms.buwiz.com`, `buwiz.com`: anyone on the internet. **Accounts are open to anyone with an email address** (6-digit emailed code; no passwords).
- `api.buwiz.com`: app tokens (device flow, 15-min access, 90-day rotating refresh), device-key–signed v2 requests, 5-minute tickets; `/mcp` for AI agents with `mcp`-scope tokens (DCR + PKCE S256, read-only).
- Staff pages (`/staff/*`): Support and Admin roles; `ADMIN_EMAILS` break-glass Admins.
- Sender `/internal/*`: only the Gatekeeper, via the private address and `SENDER_SECRET`.
- Bot broker on 127.0.0.1:8787: the app, mini apps and plugins, the bridge; browsers are refused by Origin (`Origin: null` never allowed).
- Model-driven tools, routines and agent computers: driven by whatever content enters a chat.

## Assets **[owner: all of these matter, equally]**

- TIN claim documents and IDs (R2 objects, `DOCUMENT_KEY`).
- The Sender's BIR passphrases, SFTP login and HPKE key; `TICKET_SIGNING_KEY`, `RECEIPT_SIGNING_KEY`.
- Integrity of filings: nobody files under a TIN they do not own, test/unverified/paused TINs never send.
- Staff/Admin powers on `account.buwiz.com` (approve claims, activate TINs, ban, change roles).
- Users' local Bot data: chats, model API keys, connector and mini app account tokens, computer SSH keys and provider credentials (Keychain).

## Attacker profiles

- **[owner] In scope:** anonymous internet users and anyone with a self-made buwiz.com account; a signed-in user going after another account's TINs, documents or submissions; a non-Admin staff member escalating; a malicious AI agent client registering through DCR; web pages the Bot user visits (reaching 127.0.0.1:8787); mini apps and plugins the user did not install themselves; untrusted content (web pages, files, messages) fed to the model and steering tools or computers.
- **[owner] Out of scope / accepted risk:** attackers already running code as the same macOS user; plugins or MCP servers the user installed themselves acting maliciously; prompt injection that arrives through the model's own output.

## Existing controls

- Origin allow-list on the broker (`broker/buwiz-broker/src/guard.rs`), JSON-only bodies; `/mcp` refuses foreign origins (DNS rebinding).
- Role checks repeated inside each staff action's transaction; own-account restrictions; audit tables `account_audit`, `claim_audit`.
- Device-key request signing, single-return tickets, HPKE-sealed XML only the Sender can open; return/file-name match check; signed receipts and a published trust list.
- `test_only` TINs refused at the send gate and by the Sender; bans answer `403 account_banned` on every API call.
- Documents encrypted before upload and deleted when a claim is decided; plugin keys and tokens in the Keychain via `/usr/bin/security` stdin.
- Production listens only on loopback behind Cloudflare Tunnel; instance role limited to SSM `/buwiz/*` reads and the deploy bucket.

## Out of scope **[owner]**

- Test, e2e and dev-only code and modes: `e2e/`, `tests/`, `*_tests.rs`, `test-support/`, `BIR_RELAY_MODE=dry_run` and `lab`, dev scripts (`scripts/buwiz-test`, `scripts/v6.sh`, `smoke.sh`).
- The private BIR relay code (not in these repositories). The `nsh/` and `blaze/` submodules except where Bot's own use of them is exploitable.

## Open questions

- No separate infrastructure repository; Cloudflare, Neon and R2 configuration are not in code, so `deploy/ec2/` is the only infrastructure covered.
- The eBIRForms desktop app (`buwiz-desktop`) and Buwiz Books clients are not in these repositories.
