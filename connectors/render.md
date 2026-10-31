# Render connector setup

Checked against Render's docs, the `render-oss/render-cursor-plugin` repo and live endpoints on 2026-10-25. Anything marked **(unconfirmed)** could not be verified.

## What it unlocks

Buwiz Bot connects to Render's hosted MCP server: list and create services and databases, deploy, read logs and metrics, run read-only SQL queries. It can also change environment variables and trigger deploys (Render's docs flag these as potentially destructive).

| Item | Value |
|---|---|
| MCP server | `https://mcp.render.com/mcp` |
| Authorization server (issuer) | `https://api.render.com` |
| Authorize URL | `https://api.render.com/v1/oauth/authorize` |
| Token URL | `https://api.render.com/v1/oauth/token` |
| PKCE | S256 |
| Token endpoint client auth | `none` only (public clients) |
| Grant types | `authorization_code`, `refresh_token` |
| Registration endpoint (DCR) | none |
| CIMD | not advertised; a CIMD URL as `client_id` returns `invalid_client: client is invalid or inactive` |

Probed: `https://mcp.render.com/.well-known/oauth-protected-resource/mcp` and `https://api.render.com/.well-known/oauth-authorization-server`. The unauthenticated MCP URL returns 401 with `WWW-Authenticate: Bearer ... resource_metadata=...`.

## Does Buwiz Bot need its own app? Use the API key instead

The pinned client ID `cursor` is Render's pre-registered app for Cursor. Live probe: with `client_id=cursor` and `redirect_uri=http://127.0.0.1:8787/...` the authorize endpoint answers `400 redirect_uri is not registered for this client`.

I found **no Render page for creating your own OAuth application** (Render's docs list only OAuth sign-in for its official plugins and Claude Desktop connector, and API keys for everything else). Whether Render will register a client for Buwiz Bot on request is **(unconfirmed)**; ask Render support if you want OAuth.

**What does work, documented by Render:** a Render **API key** sent as `Authorization: Bearer <key>` to `https://mcp.render.com/mcp`. Render's MCP page shows exactly this for Claude Code (`--header "Authorization: Bearer <YOUR_API_KEY>"`), Cursor (`"headers": {"Authorization": "Bearer <YOUR_API_KEY>"}`) and Codex (`http_headers`), "for non-interactive environments".

## Prerequisites

- A Render account. Creating an API key is free; the services you create through MCP are billed per Render's normal pricing.
- No approval or waiting time.

## Steps (API key)

1. Open `https://dashboard.render.com/u/settings?add-api-key` (Account Settings in the dashboard; the docs link to it directly). Create an API key. Exact button and field labels: **(unconfirmed, the docs only show screenshots)**. Name it `Buwiz Bot` if a name is asked.
2. Copy it immediately: "An API key is displayed in full only when it's created."
3. Put it in Buwiz Bot (below).
4. Connect, then tell Buwiz Bot which workspace to use: "Set my Render workspace to [WORKSPACE_NAME]". Every MCP action is scoped to the selected workspace.

If you prefer OAuth and Render gives you a client: public client, PKCE S256, redirect `http://127.0.0.1:8787/oauth/callback/render` (**confirm in Buwiz Bot's plugin page**; Buwiz Bot's exact string is not final), app name `Buwiz Bot`.

## What to paste into Buwiz Bot

Open **Connect apps -> Render -> settings**:

| Buwiz Bot field | Value |
|---|---|
| Header name | `Authorization` **(unconfirmed that Buwiz Bot has a header field)** |
| Header value / API key | `Bearer <your Render API key>` |
| Client ID / Secret | leave empty |

If Buwiz Bot's Render page only offers Client ID / Authenticate, API-key mode is not available there yet and the OAuth route needs a Render-issued client.

## Scopes requested

None. The catalog pins no scopes and the server metadata lists none. An API key has no scopes.

## Alternative: API key (limits)

- One key grants access to **all workspaces you belong to** (Render API docs). Render's MCP page says "Authenticating the MCP server grants access to your workspaces and services." It cannot be narrowed to one workspace or made read-only; the workspace choice is a prompt, not a permission.
- Keys do not expire by default **(unconfirmed)**; revoke a leaked key in Account Settings and create a new one.
- Test outside Buwiz Bot: `curl -H "Authorization: Bearer $KEY" "https://api.render.com/v1/services?limit=3"` should return 200.

## Verification

Ask Buwiz Bot: "Set my Render workspace to <name>, then list my services." Or run the curl above.

## Troubleshooting

- 401 from `https://mcp.render.com/mcp`: key missing, mistyped, revoked, or sent without the `Bearer ` prefix.
- OAuth `redirect_uri is not registered for this client`: you are using the pinned `cursor` client; use the API key.
- OAuth `client is invalid or inactive`: the client ID is unknown to Render.
- "No workspace selected": tell Buwiz Bot which workspace to use.
- MCP cannot do something (image-backed services, IP allowlists, scaling changes): Render lists these as MCP limitations.

## Sources

- https://render.com/docs/mcp-server (and .md version)
- https://render.com/docs/api and https://api-docs.render.com/reference/authentication
- https://github.com/render-oss/render-cursor-plugin
- https://api.render.com/.well-known/oauth-authorization-server and https://mcp.render.com/.well-known/oauth-protected-resource/mcp (live)
