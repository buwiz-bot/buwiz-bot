# Bitrise connector setup

Checked against Bitrise's docs, the `bitrise-io/bitrise-mcp` repo and live endpoints on 2026-10-25. Anything marked **(unconfirmed)** could not be verified.

## What it unlocks

Buwiz Bot connects to Bitrise's hosted MCP server to manage mobile CI/CD: apps, builds, artifacts, workspaces, team members.

| Item | Value |
|---|---|
| MCP server | `https://mcp.bitrise.io` |
| Authorization server (issuer) | `https://oauth.bitrise.io` |
| Authorize URL | `https://oauth.bitrise.io/oauth2/authorize` (hands off to a WorkOS sign-in at `app.bitrise.io/workos/connect`) |
| Token URL | `https://oauth.bitrise.io/oauth2/token` |
| Device authorization URL | `https://oauth.bitrise.io/oauth2/device_authorization` |
| PKCE | S256 |
| Token endpoint client auth | `none`, `client_secret_post`, `client_secret_basic` |
| Grant types | `authorization_code`, `refresh_token`, `urn:ietf:params:oauth:grant-type:device_code` |
| Scopes (server) | `openid profile email offline_access` |
| Registration endpoint (DCR) | none |
| CIMD | **supported** (`client_id_metadata_document_supported: true`) |

Probed at `https://mcp.bitrise.io/.well-known/oauth-protected-resource` and `https://oauth.bitrise.io/.well-known/oauth-authorization-server`.

## Does Buwiz Bot need its own app?

Bitrise has **no developer page for creating an OAuth app** that I could find, and the catalog's "no dynamic client registration" is true. But the catalog entry pins no client ID at all: Bitrise's own docs show clients connecting with just the URL ("The server tells your client it's OAuth-protected, and your client handles authorization from there"). That works through CIMD. Verified 2026-10-25 by a read-only GET: Claude Code's CIMD client with redirect `http://localhost:33333/callback` is accepted and redirected to the WorkOS login.

Two practical routes:

1. **Personal access token (works today, documented, see below).** No app needed.
2. **OAuth via CIMD:** host a small JSON file and use its URL as Buwiz Bot's Client ID (needs Buwiz Bot's Client ID field to accept an https URL **(unconfirmed)**).

## Prerequisites

- A Bitrise account (sign-up at bitrise.io). Plan requirements and price for MCP use: not stated in the MCP docs checked **(unconfirmed)**.
- No approval or waiting time documented.

## Steps, route 2 (OAuth with your own CIMD client)

1. Host this at an https URL with a non-root path, e.g. `https://YOURNAME.github.io/buwiz-bot/bitrise-client.json` (the `client_id` must equal that URL):

   ```json
   {
     "client_id": "https://YOURNAME.github.io/buwiz-bot/bitrise-client.json",
     "client_name": "Buwiz Bot",
     "client_uri": "https://YOURNAME.github.io/buwiz-bot/",
     "redirect_uris": ["http://127.0.0.1/oauth/callback/bitrise"],
     "grant_types": ["authorization_code", "refresh_token"],
     "response_types": ["code"],
     "token_endpoint_auth_method": "none"
   }
   ```

   Callback URL to confirm in Buwiz Bot's plugin page (Buwiz Bot's exact string is not final): `http://127.0.0.1:8787/oauth/callback/bitrise`. Whether Bitrise matches a portless loopback entry against port 8787, and whether it needs the exact port listed, was tested only for `localhost` with Claude Code's document: **(unconfirmed for this path)**. If it fails, list `http://127.0.0.1:8787/oauth/callback/bitrise` explicitly.
2. Paste the URL into Buwiz Bot as Client ID, leave Client Secret empty, click Authenticate, sign in to Bitrise, approve the consent screen (it "names the application that's asking for access", so it will say Buwiz Bot).

App name if asked: `Buwiz Bot`. Client type: public.

## What to paste into Buwiz Bot

Open **Connect apps -> Bitrise -> settings**:

| Buwiz Bot field | Value |
|---|---|
| Client ID | https URL of your CIMD JSON (route 2) |
| Client Secret | empty |
| Callback URL (read-only) | should equal `http://127.0.0.1:8787/oauth/callback/bitrise` |

## Scopes requested

None pinned in the catalog. Server scopes: `openid profile email offline_access`; request `offline_access` to get a refresh token **(unconfirmed that Bitrise requires it)**.

## Alternative: Personal Access Token (documented)

Bitrise's docs: for clients without MCP OAuth, send the PAT as `Authorization: Bearer <PAT>` to `https://mcp.bitrise.io` (for example `claude mcp add --transport http bitrise https://mcp.bitrise.io -H "Authorization: Bearer YOUR_BITRISE_PAT"`).

Create the token (official path): Bitrise -> **Bitrise CI** -> profile image (upper right) -> **Account settings** -> **Security** tab -> **Personal access tokens** -> **Create token** -> enter a **Name**, choose expiry (6 months, 3 months, 1 month, 2 weeks, 1 day, 1 hour, or never) -> **Next** -> copy it (shown once).

Limits: the token acts as you with your workspace access; scopes or workspace restriction are not documented **(unconfirmed)**; it expires per your choice and does not refresh. Buwiz Bot needs a plugin setting for a custom `Authorization` header to use it **(unconfirmed that Buwiz Bot has one)**; otherwise use route 2.

## Verification

Ask Buwiz Bot: "List my Bitrise apps" then "Show my last failed build and why it failed." On Bitrise, Account settings -> Security lists tokens you created.

## Troubleshooting

- 401 with a PAT: token expired, revoked, or mistyped; create a new one.
- Authorize error `invalid_client` / redirect mismatch (route 2): `client_id` in the JSON differs from the hosted URL, file not publicly reachable, or the callback path/port is not listed.
- Sign-in loop: re-authenticate; Bitrise's docs suggest deleting and re-adding the server if OAuth state is stale.
- Consent screen shows an unexpected app name: check `client_name` in your JSON.

## Sources

- https://github.com/bitrise-io/bitrise-mcp (README)
- https://docs.bitrise.io/en/bitrise-platform/ai/bitrise-mcp
- https://docs.bitrise.io/en/bitrise-platform/ai/bitrise-mcp/installing-the-bitrise-mcp-server/install-claude
- https://docs.bitrise.io/en/bitrise-platform/ai/onboarding-for-agents
- https://docs.bitrise.io/en/bitrise-platform/accounts/personal-access-tokens
- https://oauth.bitrise.io/.well-known/oauth-authorization-server and https://mcp.bitrise.io/.well-known/oauth-protected-resource (live)
