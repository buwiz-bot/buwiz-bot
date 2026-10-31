# Transcend Rails Agent Controls (transcend-agent-governance) connector setup

Checked against the plugin repo and live endpoints on 2026-10-25. Transcend publishes no public docs for the Agent Governance gateway; items marked **(unconfirmed)** must be answered by Transcend.

Connector id: use the full id `transcend-agent-governance`. The entry in `needs-oauth-app.json` truncates the callback to `.../oauth/callback/transcend-agent-governan`; that is a truncation, so the callback to register is `http://127.0.0.1:8787/oauth/callback/transcend-agent-governance` (confirm in Buwiz Bot's plugin page).

## What it unlocks

Policy-enforced MCP tool access through your organization's Transcend "Agent Governance" gateway (Rails Agent Controls): the gateway aggregates MCP servers your admins assign, enforces policy on each call and writes an audit trail. First sign-in auto-registers a "connected agent" with **no** MCP servers; an administrator must assign servers/policy before any tools appear.

The pinned client `myelin_cursor_plugin` is a public PKCE client Transcend allowlisted for Cursor. Its redirect allowlist (per the plugin README) is `http://localhost:8787/callback`, Cursor deep-link and web callbacks; host must be `localhost`, not `127.0.0.1`. Buwiz Bot's callback is not on that list, so it will be rejected.

Endpoints (confirmed live for the plugin's default dev gateway; your organization's gateway host will differ):

| Item | Value |
|---|---|
| MCP server | `<GATEWAY_BASE_URL>/mcp/agent` (default `https://mcp.dev.myelin-internal.com/mcp/agent`) |
| Authorization server (issuer) | `https://api.dev.myelin-internal.com` (discovered from the gateway) |
| Authorize URL | `https://api.dev.myelin-internal.com/oauth/authorize` |
| Token URL | `https://api.dev.myelin-internal.com/oauth/token` |
| PKCE | S256 |
| Token endpoint client auth | `none`, `client_secret_basic`, `client_secret_post` |
| Grant types | `authorization_code`, `refresh_token`, `client_credentials` |
| Scopes supported | `mcp`, `openid`, `offline_access`, `agents.agent.start` |
| Registration endpoint | none; `client_id_metadata_document_supported: true` |

The `*.myelin-internal.com` default looks like Transcend's internal dev environment; do not point real work at it unless Transcend tells you to. A production host named in a Claude plugin PR (`https://mcp.withrails.ai/mcp/agent`) did not resolve in DNS on 2026-10-25. That PR also says Claude Code sign-in against this gateway is "not supported yet".

## Prerequisites

- An active Transcend Agent Governance organization. This is a Transcend enterprise product (Rails is described as beta in Transcend's beta terms). Pricing and onboarding: **(unconfirmed)**, contact Transcend.
- Your gateway base URL (HTTPS, scheme + host only, no path). Get it from your Agent Governance administrator or Transcend; self-hosted partners have their own `mcp.*` origin.
- A Transcend admin to register the OAuth client and assign MCP servers to your agent afterwards.

## Steps

No self-service developer console is documented. Ask Transcend (plugin author contact in the repo manifest: `support@transcend.io`; or your Agent Governance admin) to:

1. Register a **public** OAuth client for Buwiz Bot on your environment's authorization server: name `Buwiz Bot`, PKCE S256, token auth method `none`, scopes `mcp offline_access`, redirect URI `http://127.0.0.1:8787/oauth/callback/transcend-agent-governance`. Note their allowlist insists on `localhost` for Cursor; ask whether `127.0.0.1` is permitted **(unconfirmed)**.
2. Or, alternatively, use a Client ID Metadata Document: the server advertises support, so you could host a JSON document at an HTTPS URL listing that redirect URI and use the URL as Client ID. Needs Buwiz Bot to pass a URL through as `client_id` and Transcend to allow loopback redirects. **(unconfirmed)**
3. After first sign-in ask the admin to assign MCP servers/policy to the new connected agent (named like `Cursor - <user>` for Cursor; Buwiz Bot's name will differ).

## What to paste into Buwiz Bot

Open **Connect apps -> Transcend Rails Agent Controls** -> settings:

| Buwiz Bot field | Value |
|---|---|
| Gateway base URL (`GATEWAY_BASE_URL`) | your gateway origin, e.g. `https://mcp.<your-env>`; no `/mcp/...` suffix |
| Client ID | the id Transcend registers for Buwiz Bot (not `myelin_cursor_plugin`) |
| Client Secret | none (public client) |
| Callback URL (shown by Buwiz Bot, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/transcend-agent-governance` |

Then click **Authenticate**; pick your organization at consent.

## Scopes requested by the plugin

`mcp offline_access` (the server also advertises `openid` and `agents.agent.start`).

## Alternative: bound API key (operator fallback)

The plugin README describes a static `Authorization` header with a dashboard-minted **bound API key** for automation, preferring it over a ~1h machine JWT that cannot be refreshed. Limits: documented only as an operator fallback outside the plugin; where in the dashboard to mint it is **(unconfirmed)**; needs a custom-header option in Buwiz Bot; no per-user consent. Ask your admin.

## Verification

After sign-in ask Buwiz Bot: "List the tools available from the governed gateway." Tools are named `{slug}__{tool}`. An empty list means an admin has not assigned servers yet. Provider side: the dashboard shows a connected agent for your user; policy decisions are logged (for example `mcp.authz.allowed`).

## Troubleshooting

- **Redirect URI mismatch**: Buwiz Bot's callback is not on the client's allowlist; ask Transcend to add it (and check `127.0.0.1` vs `localhost`).
- **Discovery errors**: the gateway URL must be the origin only, not the dashboard origin; it must serve protected-resource metadata. A 401 with `TOKEN_INVALID` on the bare `/mcp/agent` URL is expected without a token.
- **Connected but no tools**: no MCP servers/policy assigned to your agent.
- **Wrong organization**: re-run Authenticate and choose the correct org at consent.
- **Auth failure after revoke or long idle**: re-authenticate; check `offline_access` was granted.

## Sources

- https://github.com/transcend-io/tools/tree/main/plugins/cursor/TranscendRailsAgentControls (README, mcp.json, plugin manifest)
- https://github.com/transcend-io/tools/blob/main/plugins/cursor/TEAM_MARKETPLACE.md
- https://github.com/transcend-io/tools/pull/674 (Claude plugin PR)
- https://mcp.dev.myelin-internal.com/.well-known/oauth-protected-resource/mcp/agent (live, 2026-10-25)
- https://api.dev.myelin-internal.com/.well-known/oauth-authorization-server (live, 2026-10-25)
