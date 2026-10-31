# Slack connector setup

Checked against Slack's docs and the live endpoints on 2026-10-25. Items marked **(unconfirmed)** could not be verified from official docs. Slack's rules for MCP and for localhost redirects interact in a way that is not fully documented, so expect to test once.

## What it unlocks

The Slack plugin connects Buwiz Bot to Slack's hosted MCP server at `https://mcp.slack.com/mcp` (Streamable HTTP, JSON-RPC 2.0; no SSE). The assistant can search messages, files, users and channels, read channels and threads, send and schedule messages, add reactions, create and edit canvases and lists, upload files, and read profiles. All calls use a **user token**, so they act as you.

Slack has no dynamic client registration. Slack's rule: "MCP clients must be backed by a registered Slack app with a fixed app ID". You create your own Slack app and paste its Client ID (and Client Secret if the app is confidential) into Buwiz Bot.

Probed live on 2026-10-25:

| Item | Value |
|---|---|
| MCP server | `https://mcp.slack.com/mcp` (401 with `WWW-Authenticate: Bearer resource_metadata="https://mcp.slack.com/.well-known/oauth-protected-resource"`) |
| Authorization server (issuer) | `https://mcp.slack.com` |
| Authorize URL | `https://slack.com/oauth/v2_user/authorize` |
| Token URL | `https://slack.com/api/oauth.v2.user.access` |
| PKCE | S256 |
| Token endpoint client auth | `client_secret_post` only (advertised) |
| Grant types | `authorization_code`, `refresh_token`, `urn:ietf:params:oauth:grant-type:jwt-bearer` |
| `registration_endpoint` | none (Slack docs: Dynamic Client Registration not supported) |

## Prerequisites

- A Slack account in a workspace where you can create apps, and a workspace admin who will approve the app. Slack docs: "Workspace admins can approve and manage all MCP client integrations"; the plugin README says the admin "must approve MCP integration before you can authenticate". If you own the workspace, that is you.
- The app must be **internal** (built for your own workspace) or directory-published. Slack docs: "Only apps published in the Slack Marketplace and internal apps can use MCP at this time; unlisted apps are prohibited." Do not distribute your app publicly or via an unlisted link.
- Cost: creating a Slack app is free. Whether MCP works on the Free plan is not stated in Slack's docs. **(unconfirmed)** No review wait is needed for an internal app.
- If your app has an IP allowlist configured, Buwiz Bot's machine IP must be on it.

## Steps

1. Go to **https://api.slack.com/apps** and sign in. Click **Create New App** and choose **From scratch** (or **From a manifest**). Name it `Buwiz Bot` and pick your workspace.
2. Open **OAuth & Permissions** in the left sidebar.
3. Under **Redirect URLs** add the Buwiz Bot callback and click **Save URLs**:
   - Slack requires HTTPS for ordinary (server) redirects: "The `redirect_uri` must use HTTPS". Matching is by prefix: the sent `redirect_uri` must "match or be a subdirectory of" a registered URL (scheme, host and port must match).
   - Buwiz Bot's callback is plain-http loopback: `http://127.0.0.1:8787/oauth/callback/slack` (confirm in Buwiz Bot's plugin page). Slack's PKCE docs say redirects to `localhost` (their example: `http://localhost:8080/auth`) are accepted as "desktop redirects" **only if the app has opted into PKCE**. Slack does not mention `127.0.0.1`. **(unconfirmed)** Try `http://127.0.0.1:8787/oauth/callback/slack`; if Slack rejects it, register `http://localhost:8787/oauth/callback/slack` and see whether Buwiz Bot's plugin page can use `localhost`.
   - Without PKCE enabled, a loopback URL is treated as a server redirect and needs HTTPS, so it will not work.
4. Enable PKCE: on **OAuth & Permissions** (for standard apps; "Published App Settings" for directory apps) turn on the PKCE setting, or set `pkce_enabled: true` in the manifest. **(exact toggle label unconfirmed)** This is one-way: it marks the app as a public client and cannot be undone without contacting Slack support. PKCE token exchanges omit `client_secret`; refresh tokens of PKCE apps expire after 30 days (instead of never).
5. Under **User Token Scopes** add the scopes you want (see "Scopes requested"). Do not add bot scopes: desktop redirects "are not allowed to request bot scopes".
6. Enable MCP for the app. Slack's developer guide: open the **Agents** sidebar section and turn on "Slack Model Context Protocol (MCP) Server". A third-party guide calls it **Agents & AI Apps** with a **Model Context Protocol** toggle. If the toggle is off, OAuth can succeed while tools come back empty or 403.
7. Install the app to your workspace (**Install App** / **Install to Workspace**). An admin may need to approve it.
8. From **Basic Information -> App Credentials** copy the **Client ID**. The **Client Secret** is shown there as well; with PKCE on, Buwiz Bot should not need it. **(unconfirmed, see below)**

Open question to test: Slack's MCP metadata advertises only `client_secret_post` at the token endpoint, while PKCE apps are public and send no secret. Slack's own docs say "Slack uses confidential OAuth" for MCP and separately that "PKCE support is now available" for desktop clients, without joining the two. Whether `oauth.v2.user.access` accepts a PKCE exchange with no secret is **(unconfirmed)**. Practical approach: enable PKCE, leave Buwiz Bot's Client Secret blank first; if the token step fails with an invalid-client style error, paste the Client Secret too.

## What to paste into Buwiz Bot

Open **Connect apps -> Slack** -> settings:

| Buwiz Bot field | Value |
|---|---|
| Client ID | App's Client ID (Basic Information) |
| Client Secret | Leave blank first (PKCE app); if token exchange fails, paste the app's Client Secret |
| Callback URL (shown by Buwiz Bot, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/slack` |

Then click **Authenticate**. Each user authorizes through Slack's user-OAuth consent screen. The catalog's built-in Slack entry pins another vendor's Client ID (`1601185624273.8899143856786`, callback port 3118); that app cannot use Buwiz Bot's callback, so replace it with yours.

## Scopes requested

The plugin sends none by default (the catalog entry only pins a client ID). Slack's MCP server advertises these user scopes in `scopes_supported`:

`canvases:read canvases:write channels:history channels:read channels:write chat:write emoji:read files:read files:write groups:history groups:read groups:write im:history im:read im:write lists:read lists:write mpim:history mpim:read mpim:write reactions:read reactions:write search:read.files search:read.im search:read.mpim search:read.private search:read.public search:read.users users:read users:read.email`

Per-tool needs (from Slack's docs): search needs the `search:read.*` set; read channel/thread needs the `*:history` scopes; send/schedule needs `chat:write`; canvases need `canvases:read` and `canvases:write`; lists need `lists:read`/`lists:write`. Add only what you need to the app; a scope missing from the app fails at consent. `chat:write`, `*:write` and `files:write` let the assistant post and change things as you.

## Alternative: token option

Slack's docs say you can call `https://mcp.slack.com/mcp` with `Authorization: Bearer <user token>` (a user token from your own app, `xoxp-...`). The server metadata lists `header` as a supported bearer method. Limits: you must obtain the token yourself via the app's install flow; it must belong to an app with MCP enabled and sufficient user scopes; tokens from PKCE apps expire. Whether Buwiz Bot lets you set a custom header for this connector is **(unconfirmed)**; use OAuth if not.

## Verification

1. After Authenticate, Buwiz Bot should show Slack as connected.
2. Ask Buwiz Bot: "Search Slack for messages about 'launch' in the last week" or "List my Slack channels".
3. In Slack, check **Apps** in your workspace or ask the admin to see the app under approved apps / MCP clients.

## Troubleshooting

- **`bad_redirect_uri` / "redirect_uri did not match":** the URL Buwiz Bot sends is not registered exactly (scheme, host, port) or is not a subpath of a registered URL. The same `redirect_uri` must be sent at authorize and token steps. Check `127.0.0.1` vs `localhost`, and that PKCE is enabled so loopback is accepted.
- **Redirect URL rejected as not HTTPS:** PKCE is not enabled on the app, or Slack does not accept `127.0.0.1`; enable PKCE and try `localhost`.
- **Admin approval required / app not approved:** a workspace admin must approve the app and MCP.
- **OAuth works but tools are empty or 403:** the MCP toggle in the app (Agents section) is off, or the app is distributed/unlisted (prohibited).
- **`invalid_scope` or missing tool:** add the matching user scope in the app and re-install.
- **Token exchange fails with no secret:** add the Client Secret in Buwiz Bot (see open question above).
- **Re-auth every 30 days:** expected with PKCE (refresh tokens expire after 30 days).
- **IP blocked:** the app has an IP allowlist; add Buwiz Bot's IP.

## Sources

- https://docs.slack.dev/ai/slack-mcp-server/ (eligibility, auth, scopes per tool)
- https://docs.slack.dev/ai/slack-mcp-server/developing (app setup, Agents section)
- https://docs.slack.dev/authentication/installing-with-oauth (HTTPS and redirect matching)
- https://docs.slack.dev/authentication/using-pkce (PKCE, localhost desktop redirects)
- https://docs.slack.dev/changelog/2026/03/30/pkce
- https://mcp.slack.com/.well-known/oauth-authorization-server and /.well-known/oauth-protected-resource (live, 2026-10-25)
- https://github.com/slackapi/slack-mcp-plugin (plugin README and .mcp.json)
- https://docs.litellm.ai/docs/mcp_servers/slack (third party: "Agents & AI Apps" toggle label)
