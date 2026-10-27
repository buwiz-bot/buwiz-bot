# OneSignal connector setup

Checked against OneSignal's docs and live endpoints on 2026-10-25. Items marked **(unconfirmed)** could not be verified.

## Result: no app needed

OneSignal's authorization server **supports OAuth dynamic client registration**. Its metadata at `https://api.onesignal.com/.well-known/oauth-authorization-server` lists `registration_endpoint: https://api.onesignal.com/oauth/register`, and OneSignal's docs say "OneSignal registers your client automatically, so filling these fields [Client ID / Client Secret] causes the connection to fail." The catalog's pinned id `1fa917d8-...` is the vendor's own for Cursor and is not needed.

`needs-oauth-app.json` lists OneSignal as needing an app ("no OAuth metadata found"). Likely cause: the path-specific protected-resource document `https://api.onesignal.com/.well-known/oauth-protected-resource/mcp/oauth` reports `"resource":"https://api.onesignal.com"` (not the full MCP URL) and the unauthenticated MCP response carries no `WWW-Authenticate` header, so a strict discovery client may reject it. The root document `/.well-known/oauth-protected-resource` and the authorization-server metadata are fine. Rox should leave the Client ID empty and let registration run; if Rox's discovery rejects the resource mismatch, that is a Rox-side fix, not something you can solve at OneSignal.

## What it unlocks

The plugin connects Rox to OneSignal's hosted MCP server: apps, messages, segments, templates, Journeys, delivery debugging and (with approval) sends. OneSignal describes it as in **open beta**: "app access may require enablement before non-utility tools are available."

| Item | Value |
|---|---|
| MCP server | `https://api.onesignal.com/mcp/oauth` |
| Authorization server (issuer) | `https://api.onesignal.com` |
| Authorize URL | `https://dashboard.onesignal.com/oauth/authorize` |
| Token URL | `https://api.onesignal.com/oauth/token` |
| Revocation | `https://api.onesignal.com/oauth/revoke` |
| Registration endpoint | `https://api.onesignal.com/oauth/register` (not called by us; read-only checks only) |
| PKCE | S256 |
| Token endpoint client auth | `client_secret_post`, `none` |
| Grant types | `authorization_code`, `refresh_token` |

## Prerequisites

- A OneSignal account with access to the apps you want to use. Free to use per OneSignal's docs; tool calls count against normal API usage limits.
- Possibly beta enablement on your apps for non-utility tools (see above; how to request it is **(unconfirmed)**, ask OneSignal support).

## Steps

1. Nothing to create at OneSignal.
2. In Rox open **Connect apps -> OneSignal** and click **Authenticate** with Client ID and Client Secret **empty**.
3. Sign in to OneSignal in the browser and approve access for your account.

Whether OneSignal's registration accepts the redirect URI `http://127.0.0.1:8787/oauth/callback/onesignal` (an `http` loopback address) is **(unconfirmed)**: we did not POST to the registration endpoint. If registration is rejected, report it to OneSignal.

## What to paste into Rox

| Rox field | Value |
|---|---|
| Client ID | leave empty |
| Client Secret | leave empty |
| Callback URL (shown by Rox, read-only) | `http://127.0.0.1:8787/oauth/callback/onesignal` (confirm in Rox's plugin page); registered automatically |

## Scopes requested by the plugin

None declared; the server metadata publishes no `scopes_supported`. The grant follows the permissions of the signed-in OneSignal user.

## Alternative: API key

None for this endpoint. OneSignal's MCP docs say all supported connections use OneSignal OAuth and there is "no API key to copy, store, or rotate". (A different OneSignal REST API key exists for the REST API, but it is not documented for this MCP URL.)

## Verification

Ask Rox: "Run onesignal_health" or "Show onesignal_config" (read-only utility tools the plugin recommends first). Provider side: Settings > Account settings > Connected apps (`dashboard.onesignal.com/settings/connected-apps`) lists the client; revoking there revokes its tokens.

## Troubleshooting

- **Connection fails right after you filled Client ID/Secret**: clear both fields; OneSignal's docs say filling them breaks the connection.
- **Registration rejected / redirect not allowed**: loopback `http` redirect may not be allowed for dynamically registered clients **(unconfirmed)**; contact OneSignal.
- **Tools appear but writes are missing**: the beta may need enablement on your app.
- **401 "include an Authorization header with a valid API key"** when calling the URL by hand: expected without a token.

## Sources

- https://documentation.onesignal.com/docs/en/model-context-protocol
- https://github.com/OneSignal/onesignal-cursor-plugin (README, setup skill)
- https://api.onesignal.com/.well-known/oauth-authorization-server (live, 2026-10-25)
- https://api.onesignal.com/.well-known/oauth-protected-resource and `/mcp/oauth` variant (live, 2026-10-25)
