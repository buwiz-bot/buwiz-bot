# Google Cloud BigQuery connector setup

Checked against Google's docs and live endpoints on 2026-10-25. Items marked **(unconfirmed)** could not be verified.

This plugin uses a Google OAuth client you create yourself, exactly like the other Google plugins. **Read [google.md](google.md) first** for creating the Cloud project, the consent screen (Audience, test users, Testing vs. In production, the 7-day refresh-token expiry) and the **Desktop app** OAuth client. This file covers only what is specific to BigQuery.

## What it unlocks

Google's hosted BigQuery MCP server. Tools (from a live unauthenticated `tools/list`, 2026-10-25): `list_dataset_ids`, `get_dataset_info`, `list_table_ids`, `get_table_info`, `execute_sql_readonly`, `execute_sql` (the only one that can change data), `get_query_results`, `get_job`, `cancel_job`. Limits from Google's page: results capped at 3,000 rows; queries running over 3 minutes are cancelled by default; `execute_sql_readonly` allows only SELECT (no DML, DDL, Python UDFs, GQL); neither SQL tool queries Google Drive external tables.

| Item | Value |
|---|---|
| MCP server | `https://bigquery.googleapis.com/mcp` (Streamable HTTP; GET returns 405, use POST) |
| Protected-resource metadata | `https://bigquery.googleapis.com/.well-known/oauth-protected-resource/mcp` (live) |
| Authorization server | `https://accounts.google.com/` |
| Authorize URL | `https://accounts.google.com/o/oauth2/v2/auth` |
| Token URL | `https://oauth2.googleapis.com/token` |
| Registration endpoint | none (no dynamic client registration) |
| PKCE | `S256` and `plain` advertised |
| Token endpoint client auth | `client_secret_post`, `client_secret_basic` (a Desktop client has a secret; Google treats it as non-confidential) |
| `scopes_supported` | `https://www.googleapis.com/auth/bigquery` only |

## Prerequisites

- A Google account and a Google Cloud project (the one from google.md is fine).
- **BigQuery API enabled** in that project. Google's page says "The BigQuery remote MCP server is enabled when you enable the BigQuery API" (`bigquery.googleapis.com`); there is no separate `bigquerymcp` service in the docs (unlike the Workspace servers in google.md). New projects have the BigQuery API on already. Enabling an API needs `serviceusage.services.enable` (Owner or Service Usage Admin).
- IAM roles for the signed-in Google account, on the project that will run queries: **MCP Tool User** (`roles/mcp.toolUser`), **BigQuery Job User** (`roles/bigquery.jobUser`), **BigQuery Data Viewer** (`roles/bigquery.dataViewer`). Data Viewer can also be granted on the specific dataset instead of the whole project. `execute_sql` (writes) needs more, for example Data Editor **(unconfirmed: the page lists only the three roles above)**. Google recommends a separate identity for agents.
- Cost: see "Cost of queries" below. Creating the OAuth client is free. No approval wait for personal use (Testing mode).

### Cost of queries (read this)

- The MCP server itself: Google's page describes no MCP-specific price and says the server has no quotas of its own beyond the underlying BigQuery API quotas.
- Queries are billed by BigQuery. Default is **on-demand: you pay per TiB of data scanned, and the first 1 TiB of query data processed per month is free** (BigQuery pricing page). Capacity (slot-hour) pricing is the alternative if the project has reservations. Per-TiB price depends on region; read https://cloud.google.com/bigquery/pricing for your region rather than trusting a number here.
- The tool argument `projectId` is documented as "Project that will be used for query execution and billing". The assistant chooses it per call, so any project your account can run jobs in can be billed. Tell Rox which project to use.
- Storage and egress of anything you create are charged separately.
- **No billing account needed to try it:** Google says the steps work without billing through the BigQuery sandbox. Sandbox limits (sandbox page): 10 GiB lifetime storage, the 1 TiB/month free query allowance, tables/views/partitions expire after 60 days, no DML, no streaming inserts. If the project has a billing account, the sandbox is not used.
- Protect yourself: ask for a dry run first (`execute_sql_readonly` has a `dryRun` argument that returns bytes that would be processed), set a per-project or per-user query quota (**IAM & Admin -> Quotas**, "Query usage per day"), or set a budget alert (**Billing -> Budgets & alerts**). Exact quota names are **(unconfirmed)**; search the Quotas page for "Query usage".

## Steps

1. Do google.md steps 1, 3 and 4 (project, consent screen with test user, **Desktop app** client named `Rox desktop`). Skip the Workspace API list in step 2.
2. Enable the BigQuery API: **APIs & Services -> Library -> BigQuery API -> Enable**, or:
   ```
   gcloud services enable bigquery.googleapis.com --project=PROJECT_ID
   ```
3. On the consent screen (**Google Auth Platform -> Data Access -> Add or Remove Scopes**), add `https://www.googleapis.com/auth/bigquery` ("View and manage your data in BigQuery and see the email address for your Google Account" per Google). Its sensitivity label (sensitive vs. restricted) is **(unconfirmed)**; the Data Access page shows it. In Testing mode with yourself as test user this does not block you.
4. Grant the three IAM roles above: **IAM & Admin -> IAM -> Grant access** (principal = the Google account you will sign in with).
5. Reuse the Desktop client's Client ID and Client secret. If you already set up another Google plugin, use the same pair.

Redirect URI: a Desktop client needs no redirect registration; Rox's loopback `http://127.0.0.1:8787/oauth/callback/google-cloud-bigquery` (confirm in Rox's plugin page) is accepted as in google.md. Google's MCP guide says "Custom redirect URIs aren't supported" for web clients, so do not create a Web application client; if you must, google.md's fallback applies and must use the exact callback string.

## What to paste into Rox

**Connect apps -> Google Cloud BigQuery -> settings -> Authenticate**

| Rox field | Value |
|---|---|
| Client ID | the Desktop app client ID (`....apps.googleusercontent.com`) |
| Client secret | the Desktop app client secret |
| Callback URL (shown by Rox, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/google-cloud-bigquery` |

Sign in, accept the "Google hasn't verified this app" warning (Advanced -> Continue) and tick the permission.

## Scopes requested

`https://www.googleapis.com/auth/bigquery` (the only scope the endpoint advertises). The catalog entry pins no scopes. Google notes "Additional scopes might be required on the resources accessed during a tool call"; for Google Drive-backed external tables, which the SQL tools do not support anyway, you would need Drive scopes.

## Alternative: bearer token

Google's authentication page documents an `Authorization: Bearer <token>` header with `x-goog-user-project: PROJECT_ID`, using `gcloud auth print-access-token`. Tokens last 1 hour by default (up to 12 with `--lifetime`) and must be re-minted by hand. API keys do **not** work: Google says standard API keys only suit services that do not require a principal, and BigQuery does. Only usable if Rox's plugin page offers a custom header field (unconfirmed); OAuth is the intended path.

## Verification

- Ask Rox: "List the datasets in project PROJECT_ID" (metadata only), then "Dry-run a query that counts the rows of `bigquery-public-data.samples.shakespeare`, billed to project PROJECT_ID, and tell me the bytes it would process", then ask it to run the query. Replace `PROJECT_ID` with the project that should be billed. The small public dataset is only for testing; the query is billed (or counts against the free 1 TiB) in that project.
- Provider side: Cloud console -> BigQuery -> **Personal history / Project history** shows the jobs; https://myaccount.google.com/permissions shows `Rox` with BigQuery access.

## Troubleshooting

- `403 ... mcp.tools.call` / "Permission denied": the signed-in account lacks **MCP Tool User** on the project. Grant `roles/mcp.toolUser`.
- `403 bigquery.jobs.create`: lacks **BigQuery Job User** on the project passed as `projectId`.
- `403 bigquery.tables.getData` / "Access Denied: Table": lacks Data Viewer on that dataset or table.
- `403 accessNotConfigured` / "BigQuery API has not been used in project": enable the API (step 2) in the project named in the call.
- `redirect_uri_mismatch`, `access_denied` (not a test user), weekly re-authentication, `invalid_client`: same causes and fixes as google.md.
- Query cancelled after 3 minutes or results cut at 3,000 rows: documented limits, narrow the query.
- Surprise bill: check which `projectId` the assistant used and whether the project is on-demand; see "Cost of queries".
- Sandbox project and DML fails: the sandbox does not support DML; attach billing or use a billed project.

## Sources

- https://docs.cloud.google.com/bigquery/docs/use-bigquery-mcp (roles, scope, limits, redirect-URI note)
- https://docs.cloud.google.com/mcp/enable-disable-mcp-servers (updated 2026-10-22)
- https://docs.cloud.google.com/mcp/set-up-authentication-mcp-servers (Desktop client, bearer token, API key limits)
- https://docs.cloud.google.com/mcp/authenticate-mcp
- https://docs.cloud.google.com/bigquery/pricing (on-demand per TiB, first 1 TiB/month free)
- https://docs.cloud.google.com/bigquery/docs/sandbox
- https://bigquery.googleapis.com/.well-known/oauth-protected-resource/mcp (live, 2026-10-25)
- https://accounts.google.com/.well-known/openid-configuration (live, 2026-10-25)
- `tools/list` against https://bigquery.googleapis.com/mcp (live, no auth, 2026-10-25)
- https://github.com/cursor/plugins/tree/main/third_party/google-cloud-bigquery (catalog entry)
- [google.md](google.md) in this folder
