# Box connector setup

Checked against Box's docs and live endpoints on 2026-10-25. Box's docs do not fully cover custom (non-listed) MCP clients; anything marked **(unconfirmed)** must be checked on screen.

## What it unlocks

Rox connects to Box's hosted MCP server at `https://mcp.box.com`. The assistant can search, read and manage Box content, and use Box AI (Q&A, summarization, extraction), limited by the scopes you grant and by what your Box user can already access.

Box has no OAuth dynamic client registration, so you create your own credentials and paste the **Client ID** and **Client Secret** into Rox.

Endpoints (confirmed live):

| Item | Value |
|---|---|
| MCP server | `https://mcp.box.com` (unauthenticated request returns 401 with `resource_metadata="https://mcp.box.com/.well-known/oauth-protected-resource"`) |
| Authorization server (issuer) | `https://api.box.com` (metadata at `https://api.box.com/.well-known/oauth-authorization-server`) |
| Authorize URL | `https://account.box.com/api/oauth2/authorize` |
| Token URL | `https://api.box.com/oauth2/token` |
| PKCE | S256 |
| Token endpoint client auth | `client_secret_basic`, `client_secret_post` (no `none`: a Client Secret is required) |
| Grant types | `authorization_code`, `refresh_token` |
| `registration_endpoint` | none |

## Prerequisites

- A Box account. Which Box plans can use the remote MCP server is **not stated** in Box's docs (**unconfirmed**).
- Box says an admin enables the MCP server: "An admin enables MCP in the Box Admin Console." If you are not the admin of your Box account, you need one.
- The `docgen.readwrite` scope requires an **Enterprise Advanced** license (Box docs). Skip that scope on other plans.
- No approval wait or fee for the credentials themselves is documented.

## Steps

Box documents two ways to get credentials. Route A is the one Box prefers; Route B is what Box's own Cursor plugin README uses.

### Route A: Admin Console integration credentials (preferred by Box, needs a Box admin)

1. Sign in to the Box **Admin Console** as an admin and click **Integrations** in the sidebar.
2. Search for **Box MCP server** (or filter by the MCP category), hover over it and click **Configure**. (GitHub's guide calls the same card **Custom Box MCP Server**.)
3. In **Additional Configuration**, click **+ Add Integration Credentials**, enter an integration name such as `Rox`, and click **Save**.
4. Expand the new entry and copy the **Client ID** and **Client Secret**.
5. Under **Redirect URIs**, enter `http://127.0.0.1:8787/oauth/callback/box` (confirm in Rox's plugin page). Box's docs only say "Enter the Redirect URI provided by the external MCP client"; whether this field accepts an `http://` loopback URI is **unconfirmed**. Box's own VS Code example registers `http://127.0.0.1:33418`, so loopback appears to work.
6. Tick the **Access Scopes** you need (see below) and click **Save**.

### Route B: your own OAuth app in the Developer Console

1. Go to **https://app.box.com/developers/console** and sign in.
2. Click **New App**, select **User** as the app type (Box's setup page; Box's Cursor README calls it "Custom App with OAuth 2.0 authentication"), then **Create**. The type cannot be changed later. Name it `Rox` **(name field unconfirmed)**.
3. On the **Configuration** tab, set **OAuth 2.0 Redirect URI** to `http://127.0.0.1:8787/oauth/callback/box`. Box requires an exact match and allows `http` only for localhost or loopback addresses; loopback may use any port, but scheme, domain, path and query must match.
4. Set the application scopes in the scopes section **(exact scope names to tick are unconfirmed; the MCP scopes below are Admin Console scopes)**.
5. Copy **Client ID** and **Client Secret** from the Configuration tab.
6. Whether `mcp.box.com` accepts a token from a Developer Console app unless the admin has also enabled the Box MCP server integration is **unconfirmed**. Box's `mcp-server-box-remote` README says you may "create an OAuth app with the proper redirect URI and scopes" in the Developer Console, and that this is needed "in certain flows". If the authorize page says the app is not authorized, an admin must approve it under **Admin Console -> Platform -> Platform Apps** (Box's admin-controls page names that location for unpublished platform apps; the approval step itself is **unconfirmed**).

## What to paste into Rox

Open **Connect apps -> Box** -> settings:

| Rox field | Value |
|---|---|
| Client ID | Client ID from Route A step 4 or Route B step 5 |
| Client Secret | Client Secret from the same place (required) |
| Callback URL (shown by Rox, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/box` (confirm in Rox's plugin page) |

Then click **Authenticate**. A browser window opens on `account.box.com/api/oauth2/authorize`; approve access.

## Scopes requested

The catalog pins no scopes. Box's docs give: `root_readwrite`, `ai.readwrite`, `docgen.readwrite`. The OAuth metadata does not publish `scopes_supported`. Scopes are the maximum; users can only reach content they already have permission to view or edit.

## Alternative: bearer token

The protected-resource metadata says `bearer_methods_supported: ["header"]`, and Box's docs mention an `authorization_token`. A Box Developer Token or other access token sent as `Authorization: Bearer <token>` may work **(unverified: not confirmed in Box's MCP docs)**. Developer Tokens are short-lived (about one hour; from general Box knowledge, not re-checked today), so this is only for a quick test and needs a Rox option for a custom header.

## Verification

1. After Authenticate, Rox should show Box as connected.
2. Ask Rox: "Call who_am_i on Box" (Box's own check for the MCP connection) and then "List the first 10 items in my Box root folder."
3. If tools are missing, an admin may have disabled them: Box omits admin-disabled tools from discovery.

## Troubleshooting

- **redirect_uri_mismatch**: the registered URI differs from what Rox sends. Re-enter `http://127.0.0.1:8787/oauth/callback/box` exactly; use `127.0.0.1`, not `localhost`, unless Rox's page shows `localhost`.
- **Redirect field rejects `http://`**: Route A's field may require https **(unconfirmed)**; try Route B, where Box states loopback http is allowed.
- **App not authorized / access denied for your enterprise**: ask the admin to enable the Box MCP server integration (Route A) or authorize the app (Route B).
- **Missing tools or 403**: admin tool settings, scopes, your Box permissions or plan entitlements (Box AI, `docgen.readwrite` needs Enterprise Advanced).
- **401 later**: re-authenticate; refresh tokens are supported (`refresh_token` grant).
- **Lost Client Secret**: regenerate it in the Admin Console entry or Developer Console and update Rox.

## Sources

- https://developer.box.com/guides/box-mcp/remote/
- https://developer.box.com/guides/box-mcp/setup
- https://developer.box.com/guides/box-mcp/integrations/github-copilot
- https://docs.box.com/en/box-mcp/admin-controls
- https://developer.box.com/guides/authentication/oauth2/oauth2-setup/
- https://github.com/box/box-for-ai/blob/main/.cursor-plugin/README.md
- https://github.com/box/mcp-server-box-remote/
- https://mcp.box.com/.well-known/oauth-protected-resource (live, 2026-10-25)
- https://api.box.com/.well-known/oauth-authorization-server (live, 2026-10-25)
