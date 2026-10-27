# Databricks Genie One connector setup

Checked against Databricks' docs and the plugin repo on 2026-10-25. Items marked **(unconfirmed)** could not be verified. The MCP URL is per workspace, so there is no public endpoint to probe.

## What it unlocks

The plugin connects Rox to the Genie One MCP server (`system.ai.genie_one_mcp`) on your Databricks workspace, so Rox can ask Genie plain-language questions about data. Databricks states it is generally available and governed through Unity Gateway; Unity Catalog permissions are enforced on every request, so Rox only sees data your signed-in account can read.

Databricks has **no dynamic client registration** for these endpoints. A Databricks **account admin** creates a custom OAuth app connection, and you paste its Client ID (and secret if you made one) into Rox.

| Item | Value |
|---|---|
| MCP server | `https://<workspace-hostname>/ai-gateway/mcp-services/system.ai.genie_one_mcp` |
| Authorization server | your workspace (Databricks OAuth); URLs not probeable without a tenant |
| Scopes | `ai-gateway`, `offline_access` |
| Old endpoint | `/api/2.0/mcp/genie` uses scope `genie` and is deprecated; Databricks says it sunsets 2026-10-31. Do not use it. |

## Prerequisites

- A Databricks workspace with Genie One / Unity Gateway available to you. Plan or enablement requirements are not stated on the Genie One page **(unconfirmed)**; a workspace admin may need to enable Managed MCP Servers, and some `system.ai.*` servers need an account admin to enable the Unity Gateway beta.
- A Databricks **account admin** to create the app connection (the Databricks docs require the account console).
- Your workspace hostname, for example `dbc-a1b2c3d4-e5f6.cloud.databricks.com`: copy it from your browser address bar when logged in to the workspace, without `https://`, path or port. Per the docs, the Unity Gateway > MCPs page in the workspace lists your MCP URLs.
- Cost: Databricks usage (compute/SQL warehouse for Genie answers) bills to your account **(unconfirmed amount)**; no extra fee for the OAuth app.
- Waiting: Databricks says changes to an app connection can take about 30 minutes to take effect.

## Steps

1. As an account admin open the **account console** (https://accounts.cloud.databricks.com, or the Azure/GCP equivalent) and click the **Settings** icon in the sidebar.
2. On the **App connections** tab click **Add connection**.
3. Fill in:
   - **Name:** `Rox`
   - **Redirect URLs:** `http://127.0.0.1:8787/oauth/callback/databricks-genie-one` (confirm in Rox's plugin page). The plugin's own README registers `http://127.0.0.1:8787/callback` for Cursor, which shows Databricks accepts `http` on `127.0.0.1`; Rox's path differs, so enter Rox's exact string.
   - **Access scopes:** the console doc lists **SQL** and **All APIs**, and says `openid`, `email`, `profile`, `offline_access` are automatic. Genie One needs `ai-gateway`; Databricks' ChatGPT instructions say to create the client "with that URL and the `ai-gateway` scope". Whether the console offers `ai-gateway` as a choice is **(unconfirmed)**; look for it in the scopes list. If it is not there, create the app through the account API instead (below).
   - **Access token TTL / Refresh token TTL:** defaults 60 and 10080 minutes are fine.
   - **Client secret:** untick **Generate a client secret** for a public client (Cursor's README says a secret is optional), or keep it for a confidential client.
4. Save. In the **Connection created** dialog copy the **Client ID** and, if generated, the **Client secret**; the secret cannot be retrieved later.
5. If `ai-gateway` is not offered in the UI: the account API `POST /api/2.0/accounts/{account_id}/oauth2/custom-app-integrations` accepts `name`, `redirect_urls`, `confidential`, `scopes`. The API reference lists only `all-apis, sql, offline_access, openid, profile, email` as example values, so even the API may not accept `ai-gateway` **(unconfirmed)**. In that case ask Databricks support, or use `all-apis` as a broader fallback (more access than needed; check it works with Genie One).

## What to paste into Rox

Open **Connect apps -> Databricks Genie One** -> settings:

| Rox field | Value |
|---|---|
| Workspace hostname (`DATABRICKS_HOST`) | bare hostname, e.g. `dbc-a1b2c3d4-e5f6.cloud.databricks.com` |
| Client ID | from step 4 |
| Client secret | from step 4, or empty for a public client |
| Callback URL (shown by Rox, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/databricks-genie-one` |

Then click **Authenticate** and sign in to Databricks.

## Scopes requested by the plugin

`ai-gateway offline_access`

## Alternative: personal access token

Databricks' Unity Gateway docs show a Replit example sending `Authorization: Bearer <databricks-pat>` to a managed MCP URL. The Genie One page does not mention PATs, so whether Genie One accepts one is **(unconfirmed)**. It also needs a custom-header option in Rox. Service-principal OAuth is documented for automation. Prefer user OAuth: PATs carry your full workspace rights.

## Verification

Ask Rox: "Ask Genie: what tables can I query?" or a simple question about a dataset you know. Provider side: the app connection appears under account console > Settings > App connections.

## Troubleshooting

- **Redirect mismatch**: the URL in App connections must be byte-identical to Rox's callback; fix it and wait up to 30 minutes.
- **Invalid scope**: the app connection lacks `ai-gateway`; see step 3/5.
- **404 on the MCP URL**: wrong hostname (include no `https://`), or Genie One / Unity Gateway is not enabled for the workspace.
- **Empty or denied answers**: Unity Catalog permissions; your account cannot read that data.
- **IP access lists enabled**: Databricks notes client IPs may need allowing.
- **Using the old `/api/2.0/mcp/genie` URL**: deprecated and scheduled for sunset on 2026-10-31.

## Sources

- https://github.com/cursor/plugins/tree/main/third_party/databricks-genie-one (README)
- https://docs.databricks.com/aws/en/agents/mcp-tools/genie-mcp
- https://docs.databricks.com/aws/en/integrations/enable-disable-oauth
- https://docs.databricks.com/aws/en/ai-gateway/coding-agent-mcp-skills
- https://docs.databricks.com/api/account/customappintegration/create
