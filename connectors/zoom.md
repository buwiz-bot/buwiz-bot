# Zoom connector setup

Checked against Zoom's developer docs and live endpoints on 2026-10-25. Anything marked **(unconfirmed)** must be checked on screen.

## What it unlocks

Buwiz Bot connects to Zoom's hosted MCP server at `https://mcp.zoom.us/mcp/zoom/streamable`: search meetings and recordings, pull summaries and transcripts, meeting assets, create/update/delete meetings, and work with Zoom Docs, Hubs and Workflows.

Zoom does not support Dynamic Client Registration or Client ID Metadata Documents ("Zoom remote MCP servers only support manual client registration through app creation"), so you create a Zoom Marketplace app and paste its Client ID into Buwiz Bot.

Endpoints (confirmed live via `https://zoom.us/.well-known/oauth-protected-resource/mcp/zoom/streamable` and `https://zoom.us/.well-known/oauth-authorization-server`):

| Item | Value |
|---|---|
| MCP server | `https://mcp.zoom.us/mcp/zoom/streamable` (401 `Access token is required`) |
| Authorization server (issuer) | `https://zoom.us` |
| Authorize URL | `https://zoom.us/oauth/authorize` |
| Token URL | `https://zoom.us/oauth/token` |
| PKCE | S256 (required by Zoom for MCP) |
| Token endpoint client auth (metadata) | `client_secret_basic` only; but Zoom's public-client mode (no secret, `client_id` + `code_verifier` in the body) also exists |
| Grants | `authorization_code`, `refresh_token`, `client_credentials`, `jwt-bearer` |
| `registration_endpoint` | none |

## Prerequisites

- A Zoom account with admin or developer authorizations ("Zoom account with admin or developer authorizations"), and licenses for the Zoom products you want to reach (Zoom's requirement: "Licenses for respective Zoom products"). Search over meetings and summaries works best with AI Companion features on **(licence specifics unconfirmed)**.
- Cost: creating a Marketplace app is free as far as Zoom's docs show; no fee or approval is documented for using your own app in your own account.
- Publishing and review: **not needed for personal use.** Zoom's docs: on the app's **Local Test** page you can "add the app for your own account" (**Add App Now**, then **Allow**), and share it with other users on your account with an Authorization URL. Only publishing to the Marketplace triggers app review. Note: adding scopes can be sent to Zoom's Security Review team, who "may reject the scope request" if it is not necessary (Zoom's create-app page; how often this applies to development apps is **unconfirmed**).
- App type: **General app** (Develop -> Build app). Not Server-to-Server OAuth.
- Development and production credentials are different sets. Use the **development** set for personal use; the exact tab/label that shows each is **(unconfirmed)**.

## Steps

1. Sign in to the Zoom App Marketplace (https://marketplace.zoom.us) and choose **Develop -> Build app**, then create a **General app**. Name it `Buwiz Bot`.
2. Choose how it is managed: **User-managed** (only your own data) or **Admin-managed** (account admins add it; can reach other users' data depending on scope). For personal use choose user-managed. The management option affects which scopes are available.
3. Go to **Basic Information -> OAuth Information**:
   - **OAuth Redirect URL (Required):** `http://127.0.0.1:8787/oauth/callback/zoom` (confirm in Buwiz Bot's plugin page).
   - **OAuth Allow Lists (Required):** add the same URL (an entry may be a full callback URL or a base URL; Zoom says so).
   - Strict Mode URL (optional): if enabled, matching is character by character; leave on or off but the URL must be identical.
4. **Use a public client.** Zoom's OAuth docs say loopback redirects are allowed only for PKCE-enabled/native clients, that you must use `127.0.0.1` ("Do not use `localhost`"), and that simple confidential clients must use HTTPS. Turn on the public client option for the app (the vendor's Cursor README calls it **Use Public Client OAuth**; the label is **unconfirmed** in Zoom's docs, which say "the public client option") and copy the **Public Client ID**. It has no secret.
5. Under **Scopes** choose **Add scopes** and add the granular scopes you want (list below).
6. Open **Local Test** and click **Add App Now**, then **Allow**, so your own account has the app installed (Zoom docs).
7. Copy the **Client ID** (or Public Client ID) from **Basic Information -> App Credentials**.
8. If you must use a confidential client instead: you need an HTTPS redirect URL that Zoom accepts, which Buwiz Bot's loopback callback does not provide.

## What to paste into Buwiz Bot

Open **Connect apps -> Zoom** -> settings:

| Buwiz Bot field | Value |
|---|---|
| Client ID | the Public Client ID (preferred), or the Client ID if using a secret |
| Client Secret | leave empty with a Public Client ID; otherwise the Client Secret |
| Callback URL (read-only) | should equal `http://127.0.0.1:8787/oauth/callback/zoom` |

Then click **Authenticate** and approve on zoom.us.

## Scopes requested

The catalog pins none. The server metadata `scopes_supported` is:

`meeting:read:search meeting:read:assets meeting:write:meeting meeting:update:meeting meeting:delete:meeting meeting:update:in_meeting_controls cloud_recording:read:content cloud_recording:read:list_user_recordings my_notes:read:content agentic_search:read:search agentic_search:read:ask docs:read:export docs:write:import docs:read:list_file_collaborators docs:write:collaborator docs:update:collaborator docs:delete:collaborator hub:read:content hub:write:content workflow:read:definition workflow:read:list_definitions workflow:write:definition`

Add only the scopes for tools you need; Zoom's per-tool scope lists are on its MCP server pages (`https://developers.zoom.us/docs/mcp/servers/`). The Cursor plugin README also names `ai_companion:read:search` for cross-Zoom search; that scope is not in the server's list **(unconfirmed which is current)**.

## Alternative: API key / token

None for this server. It answers "Access token is required"; static API keys are not offered. Server-to-Server OAuth tokens (client_credentials grant is advertised) are not documented for MCP **(unverified)** and would act as the app, not as you.

## Verification

1. Buwiz Bot shows Zoom connected.
2. Ask: "Search my Zoom meetings from last week" or "Get the summary of my most recent meeting."
3. In the Marketplace, check the app is installed under Manage -> Added Apps **(unconfirmed location)**.

## Troubleshooting

- **Invalid redirect / redirect URL not allowed**: the URL must be in both **OAuth Redirect URL** and **OAuth Allow Lists**, written with `127.0.0.1` not `localhost`; with Strict Mode it must be identical character by character.
- **Redirect rejected on a confidential client**: loopback is allowed only for public/PKCE clients; switch to the Public Client ID.
- **Token exchange 401 with Public Client ID**: Buwiz Bot must send `client_id` + `code_verifier` without an `Authorization` header; if it always sends Basic auth with an empty secret, this may fail (public-client behaviour per Zoom's docs; Buwiz Bot side **unconfirmed**).
- **Missing tools / insufficient scope**: add the scope to the app, then re-authorize.
- **App not installed**: finish Local Test -> Add App Now -> Allow.
- **License errors**: your Zoom user lacks the product licence for that tool.
- Some community pages cite `mcp-us.zoom.us` as a regional MCP host (**unverified**); the catalog URL is `mcp.zoom.us`.

## Sources

- https://developers.zoom.us/docs/mcp/servers/connect-to-zoom-mcp-servers/
- https://developers.zoom.us/docs/mcp/servers/
- https://developers.zoom.us/docs/integrations/create/
- https://developers.zoom.us/docs/integrations/oauth/
- https://developers.zoom.us/docs/build-flow/basic-info/oauth-info/
- https://developers.zoom.us/blog/public-pkce.md
- https://github.com/cursor/plugins/tree/main/third_party/zoom
- https://zoom.us/.well-known/oauth-protected-resource/mcp/zoom/streamable (live, 2026-10-25)
- https://zoom.us/.well-known/oauth-authorization-server (live, 2026-10-25)
