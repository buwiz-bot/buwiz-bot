# Adapter (Mind) connector setup

Checked against Adapter's live endpoints and the plugin repo on 2026-10-25. Adapter publishes no developer documentation for third-party OAuth clients that we could find; anything marked **(unconfirmed)** must be checked with Adapter.

## What it unlocks

The Adapter plugin connects Rox to Adapter "Mind", a memory/knowledge layer for your Adapter workspace: recall context before working, save decisions and results back (`ask`, `search_knowledge`, `remember` and similar tools, per the plugin README).

Adapter has **no OAuth dynamic client registration** (no `registration_endpoint`). The catalog pins the vendor's own client id `cursor`; that id is registered for Cursor's callback only and will not accept Rox's callback (a live GET to the authorize endpoint with an unregistered client answers `unknown client_id or unregistered redirect_uri`). So you need Adapter to register a client for Rox, or a Client ID Metadata Document route (below).

Endpoints (confirmed live):

| Item | Value |
|---|---|
| MCP server | `https://api.adapter.com/v1/mcp/mind/http` |
| Protected-resource metadata | `https://api.adapter.com/.well-known/oauth-protected-resource/v1/mcp/mind/http` |
| Authorization server (issuer) | `https://api.adapter.com` |
| Authorize URL | `https://api.adapter.com/v1/oauth2/authorize` (also requires a `resource` parameter) |
| Token URL | `https://api.adapter.com/v1/oauth2/token` |
| PKCE | S256 |
| Token endpoint client auth | `none`, `client_secret_basic`, `client_secret_post` |
| Grant types | `authorization_code`, `refresh_token`, token-exchange |
| Registration endpoint | none; `client_id_metadata_document_supported: true` |

## Prerequisites

- An Adapter account at https://adapter.com (sign-in is in the browser; the README says there is no API key).
- A way to get a client registered. Adapter has no self-service developer console that we could find. **(unconfirmed)**
- Cost, plan limits and approval time: not published. **(unconfirmed)**

## Steps

There is no confirmed self-service path. Two routes, in order of preference:

**Route A: ask Adapter to register a client (works with any Rox build).**
1. Contact Adapter (use the support/contact option in your Adapter account or on adapter.com; no support address is published in the sources we found).
2. Ask for a **public** OAuth client (PKCE, token auth method `none`) for the MCP resource `https://api.adapter.com/v1/mcp/mind/http`, with:
   - App name: `Rox`
   - Redirect URI: `http://127.0.0.1:8787/oauth/callback/adapter` (confirm in Rox's plugin page)
   - Scopes: `mcp:read mcp:write offline_access`
3. Ask whether `http://127.0.0.1` loopback redirects are accepted. Adaption's server (a different vendor) ignores the port for loopback; Adapter's behavior is **(unconfirmed)**.

**Route B: Client ID Metadata Document (CIMD).** The server advertises `client_id_metadata_document_supported: true`. In CIMD the `client_id` is an HTTPS URL of a JSON document you host that lists your redirect URIs. This only works if Rox accepts a URL as Client ID and Adapter accepts a loopback `http` redirect in the document. Both **(unconfirmed)**; the required document fields are not published by Adapter (see the MCP spec for the standard format).

## What to paste into Rox

Open **Connect apps -> Adapter** -> settings:

| Rox field | Value |
|---|---|
| Client ID | the id Adapter issues (Route A), or your CIMD URL (Route B) |
| Client Secret | leave empty for a public client |
| Callback URL (shown by Rox, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/adapter` |

Then click **Authenticate**. The browser opens Adapter's consent page.

## Scopes requested by the plugin

`mcp:read mcp:write offline_access`

The server also advertises `mcp:feedback`, which the plugin does not request. `offline_access` gives Rox a refresh token.

## Alternative: API key / token

None offered. The plugin README states "there is no API key to manage". The OpenAPI spec at `https://api.adapter.com/openapi.json` lists only HTTP basic/bearer schemes with no documented key-issuing endpoint. **(unconfirmed that no token route exists)**

## Verification

1. After Authenticate, Rox should show Adapter as connected.
2. Ask Rox: "Search my Adapter knowledge for <a topic you know you saved>" (read-only), then "Remember: Rox connector test" and check it appears in Adapter.

## Troubleshooting

- **`unknown client_id or unregistered redirect_uri`** at the authorize step: the client id is not registered for Rox's callback, which is what happens with the pinned id `cursor`. Get a registered client (Route A).
- **`validation_error` mentioning `resource`**: the client must send the RFC 8707 `resource` parameter set to the MCP URL; Adapter's authorize endpoint requires it. This is a Rox-side requirement.
- **Redirect mismatch**: use `127.0.0.1`, not `localhost`, and copy the callback exactly as Rox shows it.
- **401 after sign-in**: the token has expired and no refresh token was issued; check that `offline_access` was granted.

## Sources

- https://github.com/Adapter-Group/cursor-connector (README, mcp.json)
- https://api.adapter.com/.well-known/oauth-authorization-server (live, 2026-10-25)
- https://api.adapter.com/.well-known/oauth-protected-resource/v1/mcp/mind/http (live, 2026-10-25)
- https://api.adapter.com/openapi.json (live, 2026-10-25)
