# Salesforce connectors setup (salesforce and salesforce-headless-360)

Checked on 2026-10-25 against live endpoints, the Cursor plugin READMEs (which quote Salesforce's setup in detail) and third-party write-ups. Salesforce's own docs pages (`developer.salesforce.com/docs/platform/hosted-mcp-servers/...`) returned HTTP 403 to automated fetches, so UI labels below come from the plugin READMEs and third parties. Items marked **(unconfirmed)** must be checked on screen.

This file covers both connector ids: `salesforce` and `salesforce-headless-360`. The second plugin name has a short stub file pointing here.

## What it unlocks

Salesforce Hosted MCP is Salesforce's first-party MCP service. Every call runs as the signed-in Salesforce user, with that user's object permissions, field-level security and sharing rules.

| Connector id | Plugin | What it does | MCP URL (per account) |
|---|---|---|---|
| `salesforce` | Salesforce | SOQL/SOSL, object schemas, relationships, create/update/delete records. Servers: `sobject-all`, `sobject-reads` (read only), `sobject-mutations` (reads, create, update), `sobject-deletes`, or your own custom server | `${SALESFORCE_MCP_URL}` |
| `salesforce-headless-360` | Salesforce (Headless 360, Beta) | Four tools (`discover`, `describe`, `dispatch`, `dispatch_readonly`): records, users, permission sets, Apex triggers, platform events/CDC, named credentials, Commerce orders. `dispatch` can change org configuration | `${SALESFORCE_MCP_URL}` |

There is no shared app to borrow: you create your own **External Client App** (Connected Apps are not supported) and give Rox its Consumer Key (and Consumer Secret for headless-360).

Probed live on 2026-10-25:

| Item | Value |
|---|---|
| Resource metadata | `https://api.salesforce.com/.well-known/oauth-protected-resource/platform/mcp/v1/platform/sobject-all` (and `.../platform/headless-360`) |
| Unauthenticated MCP call | 401 `{"errors":[{"message":"JWT Token is required"}]}` (no `WWW-Authenticate` header returned) |
| Authorization server, production/Developer | `https://login.salesforce.com` |
| Authorization server, sandbox/scratch | `https://test.salesforce.com` (resource metadata for `/sandbox/` URLs points here) |
| Authorize URL | `https://login.salesforce.com/services/oauth2/authorize` (sandbox: `https://test.salesforce.com/...`) |
| Token URL | `https://login.salesforce.com/services/oauth2/token` (sandbox: `https://test.salesforce.com/...`) |
| PKCE | S256 |
| `scopes_supported` (resource) | `mcp_api`, `refresh_token` |
| Token endpoint client auth | `client_secret_post`, `client_secret_basic`, `private_key_jwt`, `tls_client_auth` |
| Grant types | `authorization_code`, `refresh_token` |
| `registration_endpoint` | `https://login.salesforce.com/services/oauth2/register` is advertised in `/.well-known/openid-configuration` |

About that `registration_endpoint`: Salesforce's login hosts advertise one, but no Salesforce doc found says hosted MCP clients can self-register with it, and both Cursor plugins require a pre-created External Client App and its Consumer Key. I did not POST to it. Treat dynamic registration as **(unconfirmed, not usable by Rox's catalog flow)**; create the External Client App.

## Prerequisites

- A Salesforce org where you are a system administrator. Hosted MCP runs on Enterprise, Performance, Unlimited and Developer editions (third-party; **unconfirmed** by Salesforce docs). A free **Developer Edition** org works: sign up at https://developer.salesforce.com/signup.
- GA date per a third party: 2026-04-29. Cost: Salesforce is reported to say the servers are "intended only for customers with Flex Credits"; metering is planned from November 2026 (agent registration, calls drawing on Flex Credits, 30 days' notice, rate unpublished), and sandboxes, scratch orgs and Developer Edition orgs are reported exempt. Each tool call counts toward the org's daily API quota. All **(third-party, unverified)**. Use a Developer Edition or sandbox to avoid cost.
- Headless 360 (Beta) needs API version 67.0 or later (plugin README) and is a Beta; whether a beta enrollment is required is **(unconfirmed)**.
- New External Client Apps can take up to 30 minutes to propagate (until then you get `invalid_client_id`). Activating an MCP server can take up to 2 minutes.

## Steps

Callback URLs (confirm in Rox's plugin page):

- `salesforce`: `http://127.0.0.1:8787/oauth/callback/salesforce`
- `salesforce-headless-360`: `http://127.0.0.1:8787/oauth/callback/salesforce-headless-360`

Salesforce accepts plain-http loopback callbacks: the headless-360 plugin README lists both `http://localhost:8787/callback` and `http://127.0.0.1:8787/callback`. Add the `localhost` form too if Rox's page uses it.

1. In Setup (Quick Find: **MCP Servers**) open **MCP Servers** (under **API Catalog**). Servers are off by default. Activate the one you want: `sobject-all` (or a narrower `sobject-reads` / `sobject-mutations` / `sobject-deletes`) for `salesforce`, `headless-360` for `salesforce-headless-360`. Copy its **Server URL**. If you cannot find it, one third party also mentions **Setup -> User Interface -> Enable MCP Service** **(unconfirmed)**.
2. Setup -> **External Client App Manager** -> **New External Client App**. Fill in the basics. App name `Rox`, contact email yours.
3. Expand **API (Enable OAuth Settings)** and check **Enable OAuth**.
4. **Callback URL:** the callback(s) above, one per line.
5. **OAuth Scopes**, select exactly two (the picker shows descriptions, not values):
   - **Access Salesforce hosted MCP servers** (`mcp_api`)
   - **Perform requests at any time** (`refresh_token`, `offline_access`)
   Do not add **Full access** (`full`). Do not add `api`: a third party notes that tokens with `api` are no longer limited to MCP.
6. **Security** section:
   - Select **Issue JSON Web Token (JWT)-based access tokens for named users**. Required: without it tools fail with `JWT Token is required` / `Invalid token` even after a successful login.
   - **Require Proof Key for Code Exchange (PKCE)** on.
   - For `salesforce` (no secret field in the plugin): leave **Require Secret for Web Server Flow** off, so Rox acts as a public client with PKCE.
   - For `salesforce-headless-360`: keep **Require Secret for Web Server Flow** on; Rox needs the Consumer Secret.
   - Do not enable **JWT Bearer Flow** (different feature, needs a certificate).
7. Save. Open the app -> **Settings** -> **Consumer Key and Secret** (a third party calls the button **Manage Consumer Details**) and copy the **Consumer Key**; for headless-360 also the **Consumer Secret**. Wait up to 30 minutes.
8. Optional hardening (third-party advice): require a permission set so only chosen users can authorize; set refresh tokens to expire (they never expire by default). Under policies, who may authorize (admin-approved users vs all users) is **(exact labels unconfirmed)**; if sign-in says the app is not approved, change the policy.

One app or two: the `salesforce` plugin sends no secret, so it needs an app with the secret requirement off; `headless-360` needs the secret on (per the plugin README). Create two apps (`Rox` and `Rox Headless 360`) to follow both READMEs. Using one app with the secret requirement off for both is probably workable (Rox would send the secret to a server that does not require it) but is **(unconfirmed)**.

## What to paste into Rox

**Connect apps -> Salesforce** -> settings:

| Rox field | Value |
|---|---|
| Salesforce Consumer Key | Consumer Key of the app |
| Salesforce MCP server URL | Server URL from Setup -> MCP Servers (see below) |
| Callback URL (read-only, shown by Rox) | should equal `http://127.0.0.1:8787/oauth/callback/salesforce` |

**Connect apps -> Salesforce (Headless 360, Beta)** -> settings:

| Rox field | Value |
|---|---|
| Salesforce Consumer Key | Consumer Key of the app |
| Salesforce Consumer Secret | Consumer Secret of the same app |
| Salesforce MCP server URL | `https://api.salesforce.com/platform/mcp/v1/platform/headless-360` (production/Developer) or `https://api.salesforce.com/platform/mcp/v1/sandbox/platform/headless-360` (sandbox/scratch) |
| Callback URL (read-only) | should equal `http://127.0.0.1:8787/oauth/callback/salesforce-headless-360` |

Then click **Authenticate** and sign in to the Salesforce org.

Where to find `SALESFORCE_MCP_URL`: Setup -> MCP Servers -> your activated server -> **Server URL**. The host is always `api.salesforce.com`; the org is identified by who signs in, not by the URL. Patterns:

| Org type | Standard server | Custom server |
|---|---|---|
| Production, Developer, Enterprise | `https://api.salesforce.com/platform/mcp/v1/platform/sobject-all` | `https://api.salesforce.com/platform/mcp/v1/custom/<name>` |
| Sandbox or scratch | `https://api.salesforce.com/platform/mcp/v1/sandbox/platform/sobject-all` | `https://api.salesforce.com/platform/mcp/v1/sandbox/custom/<name>` |

## Scopes requested

`mcp_api refresh_token` for both connectors (catalog `auth.scopes`; the server's metadata lists exactly these two). `refresh_token` is what keeps Rox signed in; without it you re-authenticate whenever the access token expires. You cannot restrict an app to one MCP server; pick the narrowest server URL instead.

## Alternative: API key / token

None documented. The server answers unauthenticated calls with "JWT Token is required", and the plugin READMEs say tokens must be JWT-based access tokens issued through the External Client App's OAuth flow. Whether a session ID or other token is accepted as `Authorization: Bearer` is **(unverified; not found in any source)**. Use OAuth.

## Verification

1. After Authenticate, Rox shows the connector as connected.
2. Ask Rox: "Using Salesforce, describe the Account object" and "List 5 most recently created Accounts". For headless-360: "Use Salesforce discover to find operations for creating a user" (read-only; do not run `dispatch` yet).
3. In Salesforce: Setup -> OAuth Usage / connected sessions (or your user's **Connected Apps OAuth Usage**) should show the app. **(path unconfirmed)**

## Troubleshooting

- **`invalid_client_id`:** the app has not propagated; wait up to 30 minutes, do not recreate it.
- **`redirect_uri_mismatch`:** the callback in the app is not exactly Rox's. Add both `127.0.0.1` and `localhost` forms, no trailing slash.
- **`invalid_scope`:** the app lacks **Access Salesforce hosted MCP servers** or **Perform requests at any time**.
- **`invalid_client` / invalid client credentials (headless-360):** secret does not match the key; copy both again from the same app. For `salesforce`, this can mean the app still requires a secret; turn **Require Secret for Web Server Flow** off.
- **`JWT Token is required` or `Invalid token` after login:** **Issue JSON Web Token (JWT)-based access tokens for named users** is off.
- **404 from the server after sign-in:** the MCP server is not activated in Setup, or the URL's org type (production vs `/sandbox/`) does not match the org you logged into (sandbox login uses `test.salesforce.com`).
- **Login page for a different org / My Domain:** sign in with the right org; if Salesforce redirects to a My Domain URL, that is fine.
- **Access denied for objects/fields:** the signed-in user's profile/permission sets and field-level security apply.
- **`dispatch` approvals (headless-360):** configure Rox to ask before running `dispatch`; try configuration changes in a sandbox first.

## Sources

- https://github.com/cursor/plugins/tree/main/third_party/salesforce (README: External Client App steps, URLs, troubleshooting)
- https://github.com/cursor/plugins/tree/main/third_party/salesforce-headless-360 (README: admin setup, secret, callback URLs)
- https://developer.salesforce.com/docs/platform/hosted-mcp-servers/guide/create-external-client-app.html (official; not fetchable by automation, 403)
- https://developer.salesforce.com/docs/platform/hosted-mcp-servers/guide/headless-360-mcp.html (official; 403)
- https://developer.salesforce.com/docs/platform/hosted-mcp-servers/guide/cursor.html (official; 403)
- https://api.salesforce.com/.well-known/oauth-protected-resource/platform/mcp/v1/platform/sobject-all (live, 2026-10-25)
- https://login.salesforce.com/.well-known/openid-configuration and https://test.salesforce.com/.well-known/openid-configuration (live, 2026-10-25)
- https://coworker.ai/blog/salesforce-mcp (third party: GA date, editions, Flex Credits, metering)
- https://www.jitendrazaa.com/blog/salesforce/salesforce-mcp-server-for-claude-code-mcp-clients-setup/ (third party: setup, PKCE, Developer Edition)
