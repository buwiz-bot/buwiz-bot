# MongoDB Atlas connector setup

Checked against MongoDB's docs, the `mongodb-js` wrapper package source and live endpoints on 2026-10-25. Anything marked **(unconfirmed)** could not be verified.

## What it unlocks

Rox connects to the Atlas Managed MCP Server: query and aggregate data, manage collections and indexes, and manage Atlas resources (clusters, projects, database users, network access). It works only with Atlas-hosted clusters.

| Item | Value |
|---|---|
| MCP server | `https://mcp.mongodb.com/` |
| Authorization server (issuer) | `https://authorize.mongodb.com` |
| Authorize URL | `https://cloud.mongodb.com/oauth/authorize` |
| Token URL | `https://authorize.mongodb.com/tokens` |
| PKCE | S256 |
| Token endpoint client auth | `none`, `client_secret_basic`, `client_secret_post`, `private_key_jwt` |
| Grant types | `authorization_code`, `refresh_token`, `client_credentials`, token-exchange |
| Registration endpoint (DCR) | none |
| CIMD | metadata says `client_id_metadata_document_supported: true`; MongoDB's docs say user OAuth works "only with AI clients that MongoDB registers in advance" |
| Scopes | `scopes_supported: []` |

Probed: `https://mcp.mongodb.com/.well-known/oauth-protected-resource` and `https://authorize.mongodb.com/.well-known/oauth-authorization-server`. The unauthenticated MCP URL returns 401 with a `resource_metadata` pointer.

## Does Rox need its own app? There is no self-service app; use an MCP configuration

MongoDB documents two access models:

1. **User-delegated OAuth (App Connections):** only for clients MongoDB pre-registers. The documented list is ChatGPT, Claude Code, Claude Desktop and Web, Codex, Cursor, Devin AI, fx, Grok Build. **Rox is not on it.** The pinned `anysphere-cursor` is Cursor's client; the authorize endpoint did not reject it or a CIMD client for a loopback redirect, but only because it redirects to the Atlas login first and checks the client afterwards, so acceptance is **(unconfirmed)**. MongoDB's partner path ("Integrate Your App with Atlas App Connections") is for "approved design partners": submit the Become a Partner form at `https://www.mongodb.com/company/partners/` with "Partner API" in the More Information field. That guide covers the Atlas Admin API; whether the same client also reaches `mcp.mongodb.com` is **(unconfirmed)**. Partner redirect URIs may be http on loopback (`localhost`, `127.0.0.1`, `::1`, any port).
2. **Programmatic access (MCP configuration):** "does not require MongoDB to register the agent in advance". This is the route you can do today. Atlas creates a pair of service accounts; you get a Client ID and Client Secret and authenticate with the OAuth **client-credentials** grant. Needs Rox to support client credentials, or a bearer token you refresh by hand (see "What to paste").

## Prerequisites

- An Atlas account and an organization with at least one project/cluster. Cost: the managed MCP server's price is not stated in the docs checked; clusters cost what they cost **(unconfirmed whether a free tier works)**.
- To create an MCP configuration you need the **Organization Owner** (org-level roles) or **Project Owner** (project-level roles) role.
- Atlas CLI (`atlas`) or the Atlas Admin API; the docs show no point-and-click UI for MCP configurations **(unconfirmed)**.
- No approval wait for route 2. Route 1 for Rox needs MongoDB's partner approval; timing unknown.

## Steps (route 2, MCP configuration with the Atlas CLI)

From MongoDB's "Get Started" page:

1. `atlas auth login -P mcp`, choose **UserAccount**, finish the browser code login.
2. `export ORG_ID="<your org id>"` (or `export GROUP_ID="<your project id>"` for a project-level config).
3. Create the configuration. Org-level example, with the least roles you need instead of `ORG_OWNER`, and read-only if you do not need writes (Atlas preselects read-only):

   ```
   {"mcpConfigName": "rox", "roles": ["ORG_OWNER"], "ipAccessList": [{"ipAddress": "<YOUR_IP>"}]}
   ```

   `atlas api remoteMcpConfigurations createOrgMcpConfig --orgId "$ORG_ID" --file /tmp/mcp-config.json -P mcp -o '{{.mcpConfigId}} {{.clientId}}'` (project level: `createGroupMcpConfig --groupId`, role such as `GROUP_OWNER`). The output gives `mcpConfigId` and the **Client ID**.
4. Create the secret: `atlas api remoteMcpConfigurations createOrgMcpSecret --orgId "$ORG_ID" --mcpConfigId "$MCP_CONFIG_ID" --file /tmp/mcp-secret.json -P mcp -o '{{.secret}}'` with `{"secretExpiresAfterHours": 8760}` (the docs' example value). "You can only retrieve the secret once."
5. Keep Client ID and Secret. These are different from ordinary Atlas API service-account keys.

App name: `Rox` (use as `mcpConfigName`). The callback URL is not used in route 2. If you later get a Rox-specific OAuth client from MongoDB (route 1): public client, PKCE S256, redirect `http://127.0.0.1:8787/oauth/callback/mongodb-atlas` (**confirm in Rox's plugin page**; Rox's exact string is not final).

## What to paste into Rox

Open **Connect apps -> MongoDB Atlas -> settings**:

| Rox field | Value |
|---|---|
| Client ID | MCP configuration Client ID |
| Client Secret | MCP configuration secret |
| Grant / flow | client credentials, if Rox offers it **(unconfirmed)** |
| Token URL (if asked) | `https://cloud.mongodb.com/api/oauth/token` (what MongoDB's wrapper uses, HTTP Basic with `grant_type=client_credentials`) or `https://authorize.mongodb.com/tokens` (advertised) **(unconfirmed which is canonical)** |

If Rox only has the authorization-code flow, MongoDB cannot be connected from Rox until MongoDB registers a client for it.

## Scopes requested

None pinned; none advertised. Permissions come from the MCP configuration's Atlas roles (route 2) or the authorizing user's roles plus the org's AI-client mode (route 1).

## Alternative: service-account credentials

This is route 2 above. Bearer-token form: the client-credentials response gives an `access_token` that is sent as a Bearer to `https://mcp.mongodb.com`. Lifetime comes from `expires_in`; the wrapper assumes 3600 s when absent **(actual value unconfirmed)**, so a pasted static token will expire quickly. MongoDB's official bridge is `npx -y mongodb-atlas-mcp-remote@latest` with env `MDB_MCP_API_CLIENT_ID` and `MDB_MCP_API_CLIENT_SECRET` (a local stdio proxy, Node 20.19+/22.13+/24+). Limits: the MCP configuration has its own roles, optional IP access list and read-only flag; secrets expire per `secretExpiresAfterHours`.

For route 1, an Organization Owner must first enable AI client access (Atlas -> Organization Settings -> **App Connections** -> toggle **Allow users to connect AI clients to**, choose Read or Read and write); access ends after 7 days idle or 30 days.

## Verification

Ask Rox: "List my Atlas projects" then "List the databases in project <name>". In Atlas, check the MCP configuration and the audit log (events record the service account).

## Troubleshooting

- Token request 401: wrong Client ID/Secret, expired secret, or you used ordinary Atlas API keys instead of the MCP configuration's.
- 403 from tools: the configuration's roles do not include the permission; MongoDB notes tools are not hidden by permissions.
- Request blocked: the configuration's IP access list does not include your current IP.
- Org with AI client access disabled (route 1): ask an Organization Owner to enable it; only affects user-delegated access.
- Redirect mismatch (route 1): the client is not registered for Rox's callback; use route 2.
- 429: see MongoDB's rate limits page.

## Sources

- https://www.mongodb.com/docs/mcp-server/remote-mcp/access-models.md
- https://www.mongodb.com/docs/mcp-server/get-started.md
- https://www.mongodb.com/docs/mcp-server/remote-mcp/manage-ai-client-access.md
- https://www.mongodb.com/docs/atlas/app-connections.md and https://www.mongodb.com/docs/atlas/app-connections/partner-integration-guide.md
- https://www.npmjs.com/package/mongodb-atlas-mcp-remote (v1.0.0, source read 2026-10-25)
- https://authorize.mongodb.com/.well-known/oauth-authorization-server and https://mcp.mongodb.com/.well-known/oauth-protected-resource (live)
