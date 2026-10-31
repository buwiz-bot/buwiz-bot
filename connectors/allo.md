# Allo connector setup

Checked against Allo's help center, the plugin repo and live endpoints on 2026-10-25. Items marked **(unconfirmed)** could not be verified.

## What it unlocks

The Allo plugin connects Buwiz Bot to Allo's hosted MCP server: search calls, SMS and transcripts, team analytics, tags, send SMS, contacts/CRM, AI receptionist settings, power-dialer queue.

Allo's authorization server has **no dynamic client registration** and no self-service OAuth-app console that we could find. Allo issues static client ids to specific products (one for Claude, one for Cursor/Grok Bot: the catalog's `e3d79ef3-...` is the Cursor/Grok Bot one, registered for Cursor's callback, not Buwiz Bot's). The practical route for Buwiz Bot is an **API key** (below).

Endpoints (confirmed live):

| Item | Value |
|---|---|
| MCP server | `https://mcp.withallo.com/mcp` (POST only; GET returns 405) |
| Protected-resource metadata | `https://mcp.withallo.com/.well-known/oauth-protected-resource` |
| Authorization server (issuer) | `https://api.withallo.com` |
| Authorize URL | `https://api.withallo.com/v1/oauth/authorize` |
| Token URL | `https://api.withallo.com/v1/oauth/token` |
| Revocation | `https://api.withallo.com/v1/oauth/revoke` |
| PKCE | S256 |
| Token endpoint client auth | `none`, `client_secret_post` |
| Grant types | `authorization_code`, `refresh_token`, `client_credentials` |
| Registration endpoint | none; `client_id_metadata_document_supported: true` |

## Prerequisites

- An Allo workspace. Allo says every plan includes MCP access at no extra cost (https://www.withallo.com/mcp).
- To create an API key you must be a workspace **admin or owner** (the MCP help page says so; the API overview page says "Admin or Manager", so the two pages disagree: try it, and ask an owner if the button is missing).
- No approval wait for API keys.

## Steps

**A. API key (recommended, works today)**
1. In Allo open **Settings > API** (https://web.withallo.com/settings/api) and click **Create API Key**.
2. Choose scopes. For full MCP use, enable all scopes; for read-only use only the `READ` scopes.
3. Copy the key immediately; it is shown once.
4. In Buwiz Bot, set the connector's header to `Authorization: <key>` (raw key, as in Allo's MCP examples) or `Authorization: Api-Key <key>` (the REST API guide's form). **Do not use `Bearer`**: Allo's REST guide says a Bearer value is treated as a user session token and fails with 401. Which of raw/`Api-Key` the MCP endpoint prefers is **(unconfirmed)**; the MCP examples use the raw key.
5. Buwiz Bot needs a custom-header / API-key option for this connector. If its plugin page only offers Client ID, use route B or C.

**B. Ask Allo to register a client for Buwiz Bot**
Contact Allo support (help.withallo.com) and ask for a public PKCE client named `Buwiz Bot` with redirect URI `http://127.0.0.1:8787/oauth/callback/allo` (confirm in Buwiz Bot's plugin page). Approval time is unknown.

**C. Client ID Metadata Document.** The server advertises CIMD support: host a metadata JSON at an HTTPS URL with that redirect URI and use the URL as Client ID. Needs Buwiz Bot to pass a URL through as `client_id`. **(unconfirmed on both sides)**

## What to paste into Buwiz Bot

Open **Connect apps -> Allo** -> settings:

| Buwiz Bot field | Value |
|---|---|
| Client ID | id from Allo (route B) or your CIMD URL (route C); not used for the API-key route |
| Client Secret | leave empty (public client) |
| Callback URL (shown by Buwiz Bot, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/allo` |
| Authorization header (API-key route) | the key from step A.3, if Buwiz Bot offers the field |

## Scopes requested by the plugin

The catalog requests none. The server advertises 22: `CONVERSATIONS_READ`, `CONVERSATIONS_WRITE`, `CONTACTS_READ`, `CONTACTS_READ_WRITE`, `SMS_SEND`, `DIALING_QUEUE_READ_WRITE`, `PHONE_NUMBERS_READ`, `PHONE_NUMBERS_WRITE`, `AGENTS_READ`, `AGENTS_WRITE`, `TAGS_READ`, `TAGS_WRITE`, `USERS_READ`, `CRM_READ`, `CRM_WRITE`, `CRM_DELETE`, `SUMMARY_TEMPLATES_READ`, `SUMMARY_TEMPLATES_WRITE`, `NOTES_READ`, `NOTES_WRITE`, `THREADS_READ`, `THREADS_WRITE`. `SMS_SEND` sends real texts; grant only what you need.

## Alternative: API key limits

Keys carry the scopes you ticked, are tied to the workspace, and revoking one disconnects every client using it immediately. A key is a long-lived secret; there is no per-user consent screen. A 403 with `API_KEY_INSUFFICIENT_SCOPE` means the key lacks a scope; create a new key.

## Verification

Ask Buwiz Bot: "Show my missed calls from this week" (needs `CONVERSATIONS_READ`). Provider side: Settings > API lists the key as active.

## Troubleshooting

- **401**: wrong, revoked or `Bearer`-prefixed key. Check Settings > API for active keys.
- **403 / insufficient scope**: recreate the key with the missing scope.
- **OAuth "unknown client" / redirect mismatch**: the pinned Cursor id is not registered for Buwiz Bot's callback; use the API key or route B.
- **Cannot create a key**: you are not an admin/owner.

## Sources

- https://www.withallo.com/mcp
- https://help.withallo.com/en/integrations/mcp
- https://help.withallo.com/en/api-reference/guides/authentication
- https://github.com/themobilefirstco/grok-allo (README, mcp.json)
- https://api.withallo.com/.well-known/oauth-authorization-server (live, 2026-10-25)
- https://mcp.withallo.com/.well-known/oauth-protected-resource (live, 2026-10-25)
