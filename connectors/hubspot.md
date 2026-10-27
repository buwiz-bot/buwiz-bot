# HubSpot connector setup

Checked against HubSpot's docs and the live endpoints on 2026-10-25. HubSpot's MCP auth feature is labelled Beta by at least one third-party guide and its menu names differ between sources; anything marked **(unconfirmed)** must be checked on screen.

## What it unlocks

The HubSpot plugin connects Rox to HubSpot's remote CRM MCP server at `https://mcp.hubspot.com`. The assistant can search and update contacts, companies, deals, tickets and other CRM objects, plus activities (calls, emails, notes). Users, teams and marketing content are read-only; custom Sensitive Data properties are not reachable. Every action runs as the signed-in HubSpot user and respects that user's HubSpot permissions.

HubSpot has no OAuth dynamic client registration (the metadata has no `registration_endpoint`), so you create your own **MCP connector** (HubSpot's name for an MCP auth app) and paste its Client ID and Client secret into Rox. It is a **confidential client**: the token endpoint accepts only `client_secret_post`, so Rox needs both values.

Probed live on 2026-10-25:

| Item | Value |
|---|---|
| MCP server | `https://mcp.hubspot.com` (401 with `WWW-Authenticate: Bearer resource_metadata="https://mcp.hubspot.com/.well-known/oauth-protected-resource"`) |
| Authorization server (issuer) | `https://mcp.hubspot.com` |
| Authorize URL | `https://mcp.hubspot.com/oauth/authorize/user` |
| Token URL | `https://mcp.hubspot.com/oauth/v3/token` |
| PKCE | S256 (HubSpot docs: "PKCE is required") |
| Token endpoint client auth | `client_secret_post` only |
| Grant types | `authorization_code`, `refresh_token`, `client_credentials`, `urn:ietf:params:oauth:grant-type:jwt-bearer` |
| `scopes_supported` | empty (scopes are not declared up front) |
| `registration_endpoint` | none |

## Prerequisites

- A HubSpot account in which you can open **Development** in the main navigation bar. HubSpot's docs say "In the main navigation bar of your HubSpot account, click **Development**" and do not name a required role or plan. A third-party guide says to use a HubSpot **developer account** (developers.hubspot.com) and warns against Legacy Apps and Projects. Whether a plain CRM account can create the connector is **(unconfirmed)**; if the Development menu is missing, sign up for a free developer account at developers.hubspot.com and try there, or ask a super admin.
- The account must be on HubSpot's new Developer Platform (HubSpot docs: "you must be on the new HubSpot Developer Platform").
- Cost: HubSpot's MCP page states none for creating the connector. Some tools need paid hubs (AEO tools: Marketing Hub Professional or Enterprise; Quotes beta: Revenue Hub Professional or Enterprise). Free-tier accounts cannot create custom pipelines through it.
- Account admin connects first: per HubSpot, "the account admin must connect first, and then other users in the account can connect". Treat as likely, exact wording **(unconfirmed)**.
- No approval wait is documented.

## Steps

1. Sign in to HubSpot and click **Development** in the main navigation bar. Direct link: `https://app.hubspot.com/l/mcp-auth-apps/`.
2. In the left sidebar open **MCP Connectors** (HubSpot's current doc). Third-party guides call it **MCP Auth Apps** and the button **Create MCP auth app**; plugin README says the same. Use whichever label you see.
3. Click **Create MCP connector** (upper right).
4. In the dialog enter:
   - **Name:** `Rox`
   - **Description:** optional.
   - **Redirect URL:** `http://127.0.0.1:8787/oauth/callback/hubspot` (confirm in Rox's plugin page).
   - **Icon:** optional.
   HubSpot lets you list several redirect URLs and uses the first as the default. HubSpot's docs do not say whether `http` or `127.0.0.1` is accepted; their only loopback example is `http://localhost:6274/oauth/callback/debug`, and the Cursor plugin registers `http://localhost:8787/callback`. If `127.0.0.1` is rejected, add `http://localhost:8787/oauth/callback/hubspot` instead and check whether Rox's plugin page can use it. **(unconfirmed)**
5. Click **Create**.
6. On the connector's details page copy the **Client ID** and **Client secret**. One third-party guide says the secret is shown once and regenerating it breaks existing connections; copy it immediately. **(unconfirmed in HubSpot docs)** To change the redirect URL later, click **Edit info** (upper right).
7. There is no scope field when creating the connector. Scopes come from the tools and the permissions each user grants at the consent screen. A third-party guide (Scalekit) shows a Scopes section in the app; HubSpot's own page says "You don't define scopes when creating the connector". Follow HubSpot; if you do see a Scopes field, add the CRM read/write scopes you want (for example `crm.objects.contacts.read`, `crm.objects.contacts.write`, `crm.objects.companies.read`, `crm.objects.deals.read`).

## What to paste into Rox

Open **Connect apps -> HubSpot** -> settings:

| Rox field | Value |
|---|---|
| HubSpot Client ID | Client ID from the connector's details page |
| HubSpot Client Secret | Client secret from the same page |
| Callback URL (shown by Rox, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/hubspot` |

Then click **Authenticate**. HubSpot shows three steps: select the account, grant permissions, authorize.

Leaving Client ID blank: the catalog marks both fields optional and says blank means "use Grok's HubSpot app where it is available". That is the vendor's own pre-registered app, tied to its own callback URLs, and the catalog's server entry carries no pinned client ID for Rox. Do not rely on blank; enter your own values. **(whether Rox ships any default app is unconfirmed)**

## Scopes requested

The plugin sends none. `scopes_supported` is empty on both the resource and the authorization server. HubSpot derives the consent screen from the MCP tools and your HubSpot permissions. If HubSpot adds tools later, reinstall/re-authenticate to grant the new scopes.

## Alternative: API key / token

None for this server. HubSpot's private-app access tokens and the old API keys are for the REST API (`api.hubapi.com`); HubSpot's MCP docs describe only OAuth with a Client ID and Client secret for `mcp.hubspot.com`. Whether the server accepts a private-app token as `Authorization: Bearer` was not found in any source. **(unverified, do not assume)**

## Verification

1. After Authenticate, Rox's plugin page should show HubSpot as connected.
2. Ask Rox: "Run get_user_details on HubSpot" (HubSpot's own verify step; it returns your user, account and tool access), then "List my 5 most recent HubSpot contacts".
3. In HubSpot, open the connector under Development and check it lists the install (**(unconfirmed)**).

## Troubleshooting

- **Redirect URL mismatch / invalid redirect_uri:** the URL in Rox must match one on the connector exactly (scheme, host, port, path). Try `localhost` vs `127.0.0.1`; add both.
- **401 / invalid_client after consent:** wrong or regenerated secret. The token endpoint only takes `client_secret_post`, so a blank secret fails.
- **Cursor-style "does not support dynamic client registration":** a third party reports this when a client has no Client ID set. Enter your own ID and secret.
- **Missing tools or permission errors:** access follows your HubSpot user permissions; Sensitive Data blocks activity and conversation data; some tools need paid hubs.
- **Newly added scopes not applied:** disconnect and re-authenticate.
- **Token expired:** use the refresh token; if that is also invalid, re-run Authenticate. Refresh-token rotation behavior is **(unconfirmed)**; HubSpot's page says the server will move toward OAuth 2.1 including single-use refresh tokens.
- **Another HubSpot account:** HubSpot docs do not say whether a connector made in one account can be authorized against others. **(unconfirmed)**

## Sources

- https://developers.hubspot.com/mcp
- https://developers.hubspot.com/docs/apps/developer-platform/build-apps/integrate-with-the-remote-hubspot-mcp-server
- https://mcp.hubspot.com/.well-known/oauth-protected-resource (live, 2026-10-25)
- https://mcp.hubspot.com/.well-known/oauth-authorization-server (live, 2026-10-25)
- https://github.com/cursor/plugins/tree/main/third_party/hubspot (plugin README)
- https://docs.scalekit.com/agentkit/connectors/hubspotmcp/ (third party: "MCP Auth Apps", Beta, developer account, secret shown once)
- https://www.nango.dev/docs/api-integrations/hubspot-mcp/how-to-register-your-own-hubspot-mcp-api-oauth-app.md (third party)
