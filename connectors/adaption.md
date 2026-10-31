# Adaption connector setup

Checked against Adaption's docs and live endpoints on 2026-10-25. Items marked **(unconfirmed)** could not be verified.

## What it unlocks

The Adaption plugin connects Buwiz Bot to Adaption's MCP server for dataset management and fine-tuning workflows (import datasets, run evaluations/AutoScientist runs, export, models). Write tools launch jobs that **spend credits**.

Adaption has **no dynamic client registration**. Its OAuth page says clients identify themselves either with a Client ID Metadata Document (an HTTPS URL as `client_id`; Claude and ChatGPT use this) or a pre-registered client ("Adaption's own plugins use fixed client IDs"). The catalog's pinned id `adaption-cursor-plugin` is the vendor's own and will not accept Buwiz Bot's callback. The docs do not say how a third party gets a pre-registered id.

Endpoints (confirmed live):

| Item | Value |
|---|---|
| MCP server | `https://api.prod.adaptionlabs.ai/api/v1/mcp` (the plugin appends `?source=cursor-plugin` and an `X-Adaption-Source` header; not needed for auth) |
| Authorization server (issuer) | `https://api.prod.adaptionlabs.ai` |
| Authorize URL | `https://api.prod.adaptionlabs.ai/api/v1/oauth/authorize` |
| Token URL | `https://api.prod.adaptionlabs.ai/api/v1/oauth/token` |
| Revocation | `https://api.prod.adaptionlabs.ai/api/v1/oauth/revoke` |
| PKCE | S256 only |
| Token endpoint client auth | `none` or `client_secret_post` |
| Grant types | `authorization_code`, `refresh_token` |
| Registration endpoint | none; `client_id_metadata_document_supported: true` |
| Redirect rule (docs) | exact match, except `http` loopback (`localhost`, `127.0.0.1`, `[::1]`) where the port is ignored |

## Prerequisites

- An Adaption account (https://adaptionlabs.ai).
- Credits for any job-launching tool; pricing and free allowance **(unconfirmed)**, see the Adaption site.
- For OAuth with Buwiz Bot: either a CIMD hosted at an HTTPS URL you control, or Adaption registering a client for you. No waiting time is published.

## Steps

**Easiest: use an API key instead (see below).** It needs no registered client.

**OAuth route (Client ID Metadata Document):**
1. Host a JSON metadata document at a public HTTPS URL you control (for example GitHub Pages). The Adaption docs say only that it "serves the client's metadata, including its redirect URIs"; the full field list is not given by Adaption. Use the standard MCP CIMD fields: `client_id` (the document's own URL), `client_name` (`Buwiz Bot`), `redirect_uris` (`["http://127.0.0.1:8787/oauth/callback/adaption"]`), `grant_types`, `response_types`, `token_endpoint_auth_method` (`none`). **(field set unconfirmed against Adaption)**
2. Paste that URL into Buwiz Bot's **Client ID** field. This works only if Buwiz Bot sends the URL as `client_id` unchanged. **(unconfirmed; check Buwiz Bot's plugin page)**

**OAuth route (pre-registered):** contact Adaption support (the docs site has a Support page) and ask for a public client named `Buwiz Bot` with redirect URI `http://127.0.0.1:8787/oauth/callback/adaption` and scopes `adaption-mcp:read adaption-mcp:write`.

## What to paste into Buwiz Bot

Open **Connect apps -> Adaption** -> settings:

| Buwiz Bot field | Value |
|---|---|
| Client ID | your CIMD URL, or the id Adaption issues |
| Client Secret | leave empty (public client) |
| Callback URL (shown by Buwiz Bot, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/adaption` (confirm in Buwiz Bot's plugin page) |

Then click **Authenticate**; in the browser choose the organization and click **Authorize** (labels from the plugin README).

## Scopes requested by the plugin

The catalog requests none explicitly. The server advertises:
- `adaption-mcp:read`: read datasets, evaluations, export links, models, AutoScientist runs.
- `adaption-mcp:write`: create datasets and launch jobs that spend credits; includes read.

## Alternative: API key (documented by the vendor)

The plugin README: create a key at https://adaptionlabs.ai/app/settings?tab=api_keys and configure the server as URL `https://api.prod.adaptionlabs.ai/api/v1/mcp` with header `Authorization: Bearer <key>`. Limits: it is a long-lived secret; Buwiz Bot needs a custom-header option for this connector (unconfirmed that Buwiz Bot has one). Not tied to a consent screen, so it carries your full account rights.

## Verification

Ask Buwiz Bot: "List my Adaption datasets" (read-only; needs only `adaption-mcp:read`). Do not start with a job-launching prompt.

## Troubleshooting

- **Unknown client / redirect error**: the client is not registered or the CIMD URL is unreachable/invalid. With a CIMD, loopback redirects ignore the port, but the host (`127.0.0.1`) and path must match.
- **401 "Provide token"** on the MCP URL: expected without a token; the server then advertises scopes `adaption-mcp:read adaption-mcp:write`.
- **Write tools fail with insufficient scope**: re-authorize with `adaption-mcp:write`.
- **Credits**: write jobs spend credits; check your balance in the Adaption app.

## Sources

- https://docs.adaptionlabs.ai/oauth (checked 2026-10-25)
- https://github.com/adaptionlabs/adaption-cursor-plugin (README, mcp.json)
- https://api.prod.adaptionlabs.ai/.well-known/oauth-authorization-server (live, 2026-10-25)
- https://api.prod.adaptionlabs.ai/.well-known/oauth-protected-resource/api/v1/mcp (live, 2026-10-25)
