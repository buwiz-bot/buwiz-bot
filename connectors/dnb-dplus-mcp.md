# D&B Commercial Graph (Direct+) connector setup

Checked 2026-10-25 against live endpoints, D&B's public pages and the plugin repo. D&B's real documentation (`directplus.documentation.dnb.com`) is behind a D&B login (redirects to `sso.dnb.com`), so anything about how to obtain credentials is **(unconfirmed)**. This is an enterprise product, not a self-service developer sign-up.

## What it unlocks

Rox connects to the D&B Direct+ MCP server at `https://plus.dnb.com/v2/mcp`: entity resolution and D-U-N-S matching, and D&B data blocks (financial strength, third-party risk, ownership, inquiry, principals and contacts, payment, sales and marketing insights).

D&B has no dynamic client registration. The catalog's `CLIENT_ID` / `CLIENT_SECRET` are placeholders for env vars `DNB_DIRECT_PLUS_MCP_CLIENT_ID` and `DNB_DIRECT_PLUS_MCP_CLIENT_SECRET`, i.e. **credentials issued to you by D&B**, not a shared vendor app.

Endpoints (confirmed live via `https://plus.dnb.com/.well-known/oauth-protected-resource/v2/mcp` and `https://plus.dnb.com/.well-known/oauth-authorization-server/v2/mcp`):

| Item | Value |
|---|---|
| MCP server | `https://plus.dnb.com/v2/mcp` (POST without a token: 401, `resource_metadata` header present) |
| Authorization server (issuer) | `https://plus.dnb.com/v2/mcp` |
| Authorize URL | `https://plus.dnb.com/v2/mcp/authorize` |
| Token URL | `https://plus.dnb.com/v2/mcp/token` |
| PKCE | `S256`, `plain` |
| Token endpoint client auth | `client_secret_post` only (confidential client; secret required) |
| Grants | `authorization_code`, `refresh_token` |
| `scopes_supported` | `offline_access`, `email`, `profile`, `mcp`, `openid` |
| `registration_endpoint` | none |

Note: a bare GET on the MCP URL returns 405 JSON-RPC "Method not allowed" (normal for POST-only streamable HTTP).

## Prerequisites

- A D&B relationship with Direct+ / MCP entitlement. The unauthenticated POST response carries the D&B error "You are not currently authorised to access this product. Please contact your D&B account representative" (code 00004), which points to account-level entitlement.
- Request access: the "Get Access" form at https://www.dnb.com/en-us/solutions/artificial-intelligence/connectors/dnb-mcp.html (form is shown on that page; the page also lists phone (855)-540-1971 without saying what it is for), or email **CursorLeads@dnb.com** (listed in the plugin README and plugin.json). US customer service per the README: 1.800.234.3867 or https://support.dnb.com.
- Price, trial and approval time: not published (**unconfirmed**). Expect a sales conversation.

## Steps

1. Request MCP access from D&B (form or email above) and say you are connecting a custom desktop MCP client (Rox), not Cursor.
2. Ask D&B for a **Client ID and Client Secret for the Commercial Graph MCP server** (OAuth 2.x, authorization code with PKCE, confidential client).
3. Ask D&B to register this redirect URI on that client: `http://127.0.0.1:8787/oauth/callback/dnb-dplus-mcp` (confirm in Rox's plugin page). Whether D&B accepts a loopback `http://` redirect, or only lets D&B staff set redirect URIs, is **(unconfirmed)**. The Cursor flow uses Cursor's own callback, so credentials issued for Cursor may not include yours.
4. Paste the credentials into Rox (below).

## What to paste into Rox

Open **Connect apps -> D&B Commercial Graph** (plugin `dnb-dplus-mcp`) -> settings:

| Rox field | Value |
|---|---|
| Client ID | the Client ID D&B issued |
| Client Secret | the Client Secret D&B issued |
| Callback URL (read-only) | should equal `http://127.0.0.1:8787/oauth/callback/dnb-dplus-mcp` |

Then click **Authenticate**. What login page opens (D&B SSO / Direct+ user) is **(unconfirmed)**.

## Scopes requested

The catalog pins none. The server advertises `mcp`, `openid`, `email`, `profile`, `offline_access`; `offline_access` gives a refresh token.

## Alternative: API key

D&B's connectors page is reported (search snippet only, **unverified**) to say that "a single API key connects your MCP-compatible runtime to the D&B Commercial Graph". The plugin README documents only OAuth. If D&B gives you a key or bearer token, Rox would need a custom-header option; ask D&B whether the MCP server accepts `Authorization: Bearer` with it.

## Verification

1. Rox shows the connector connected.
2. Ask: "Look up the D-U-N-S number for Dun & Bradstreet and summarise its ownership." Any data-block tool call proves entitlement.
3. On D&B's side, ask your account rep whether the MCP client shows usage.

## Troubleshooting

- **Redirect URI mismatch / invalid_redirect_uri**: D&B has not registered Rox's callback. Send them the exact URL.
- **Error 00004 "not currently authorised to access this product"**: your account lacks the MCP entitlement; contact your D&B representative.
- **invalid_client**: the token endpoint accepts only `client_secret_post`; wrong or missing secret.
- **Docs unreachable**: `directplus.documentation.dnb.com` needs a D&B login.

## Sources

- https://github.com/dnb-public/dnb-dplus-plugin-cursor (README, mcp.json, plugin.json)
- https://www.dnb.com/en-us/solutions/artificial-intelligence/connectors/dnb-mcp.html
- https://directplus.documentation.dnb.com/html/guides/MCP/AgentReadyData.html (login required; not read)
- https://plus.dnb.com/.well-known/oauth-protected-resource/v2/mcp (live, 2026-10-25)
- https://plus.dnb.com/.well-known/oauth-authorization-server/v2/mcp (live, 2026-10-25)
