# Google connectors setup (Gmail, Drive, Calendar, Sheets, Docs, Slides)

Checked on 2026-10-25 against Google's docs and live MCP metadata. Items marked **(unconfirmed)** could not be verified.

One Google Cloud project and **one OAuth client (type: Desktop app)** serve all six plugins. You create it once, enable the APIs for the plugins you want, and paste the same Client ID and Client Secret into each Google plugin in Buwiz Bot.

## Your setup (as of 2026-10-25) — read this first

| Item | Value |
|---|---|
| Google account | `codeitlikemiley@gmail.com` (gcloud has several accounts; always pass `--account=codeitlikemiley@gmail.com`) |
| Cloud project | **BudgetWise**, id `budgetwise-bqkm6`, number `537583266937` (the number is the start of the Client ID) |
| OAuth client | Desktop app, Client ID `537583266937-l32ec2n1…apps.googleusercontent.com`, saved once in Buwiz Bot under the shared Google setup |
| Consent screen | External, Testing, `codeitlikemiley@gmail.com` added as test user |
| APIs enabled | all 12 from step 2 (6 base APIs + 6 `*mcp` services) |

**Where it stands:** sign-in works and the tokens are saved, but **Google's own MCP servers (`*mcp.googleapis.com`) refuse every tool call** with:

> Access to this tool requires that your Google Cloud project (537583266937) is enrolled in the Google Workspace Developer Preview Program.

This applies to personal Gmail accounts too (confirmed live). So, for now:

- **In use: the `Google Workspace` plugin** (local, open source — [taylorwilsdon/google_workspace_mcp](https://github.com/taylorwilsdon/google_workspace_mcp), run as `uvx workspace-mcp`). It calls the regular Gmail / Calendar / Drive / Docs / Sheets / Slides APIs, which need no preview enrollment, with the same Client ID and Secret. See [Google Workspace (local) plugin](#google-workspace-local-plugin) below.
- **Parked: the official `Gmail` connector** — installed and signed in, but turned **off** so the AI doesn't try its blocked tools. Don't uninstall it.

### To switch to Google's official servers later

1. Signed in as `codeitlikemiley@gmail.com`, apply at https://developers.google.com/workspace/preview and name project `budgetwise-bqkm6` (537583266937). Wait for the approval email.
2. Check it took: in Buwiz Bot, turn the **Gmail** connector back on (Connect apps → Gmail → **Connect**) and ask "list my last 3 email subjects". The preview error should be gone. If it says "Sign-in expired", press **Authenticate** (Testing-mode tokens last 7 days, see below).
3. Add the other official Google plugins you want (Calendar, Drive, Docs, Sheets, Slides); they reuse the same client, the APIs are already enabled.
4. Then turn the `Google Workspace` (local) plugin off, or uninstall it, so the AI sees one set of Google tools.

### Symptom map

| What you see | Cause | Fix |
|---|---|---|
| "Buwiz Bot has not completed the Google verification process … Error 403: access_denied" | your address isn't a test user | Google Auth Platform → **Audience** → Test users → add it |
| Browser says "Connected" but Gmail row still shows **Authenticate** / **Error** "service is probably turned off" | the `gmailmcp.googleapis.com` service is off | enable it (step 2), wait a minute, **Reconnect** |
| Connected, but the AI never answers / tool calls fail with "Developer Preview Program" | project not enrolled in the preview | use the local `Google Workspace` plugin, or enroll (above) |
| Asked to sign in again every week | Testing mode refresh tokens expire after 7 days | Audience → **Publish app** (see "Testing vs. In production") |

## What it unlocks

| Buwiz Bot plugin | MCP endpoint (from the plugin catalog) | What the assistant can do |
|---|---|---|
| Gmail (`gmail`) | `https://gmailmcp.googleapis.com/mcp/v1` | search, read, draft, send, label mail |
| Google Calendar (`google-calendar`) | `https://calendarmcp.googleapis.com/mcp/v1` | list/search events, schedule, respond |
| Google Drive (`google-drive`) | `https://drivemcp.googleapis.com/mcp/v1` | search, read, create, update, share files |
| Google Sheets (`google-sheets`) | `https://sheetsmcp.googleapis.com/mcp/v1` | read, write, append, format sheets |
| Google Docs (`google-docs`) | `https://docsmcp.googleapis.com/mcp/v1` | read, create, edit documents |
| Google Slides (`google-slides`) | `https://slidesmcp.googleapis.com/mcp/v1` | create, edit, render presentations |
| **Google Workspace (`google-workspace`, local)** | `uvx workspace-mcp` (stdio, on your Mac) | all of the above through the regular Google APIs — no preview enrollment |

Buwiz Bot points all six official plugins at Google's own `*mcp.googleapis.com` servers (the Cursor catalog's `api.cursor.com` proxies for Drive/Docs/Sheets/Slides only work inside Cursor). The notes below about the Cursor proxies are background only.

Per-plugin stubs: [gmail.md](gmail.md), [google-calendar.md](google-calendar.md), [google-drive.md](google-drive.md), [google-sheets.md](google-sheets.md), [google-docs.md](google-docs.md), [google-slides.md](google-slides.md).

### Important finding: two kinds of endpoint

- Gmail and Calendar point at **Google's own MCP servers** (`*mcp.googleapis.com`). Their protected-resource metadata names `https://accounts.google.com/` as the authorization server, which has no dynamic registration, so your own client is required. Confirmed live.
- Drive, Sheets, Docs and Slides in the Cursor catalog point at **Cursor-hosted proxies** (`api.cursor.com/rest-mcp/google-*/mcp`). Their metadata points at a Cursor "DCR" authorization server (`https://api.cursor.com/rest-mcp/google-drive/dcr`) that fronts Google's endpoints (authorize `https://accounts.google.com/o/oauth2/v2/auth`, token `https://oauth2.googleapis.com/token`) and advertises a `registration_endpoint`. So these four may or may not need your own client in Buwiz Bot, and **whether a token minted by your own Google client is accepted by the Cursor proxy is unconfirmed** (I did not call the registration endpoint, to avoid creating registrations). If Buwiz Bot's plugin page asks for a Client ID for these, use the shared client; if it fails, see "Official Google endpoints for Drive/Docs/Sheets/Slides" below.
- Google also publishes official MCP servers for Drive (`https://drivemcp.googleapis.com/mcp/v1`), Docs (`docsmcp`), Sheets (`sheetsmcp`) and Slides (`slidesmcp`), same URL pattern. Live metadata confirmed for all four. Switching a plugin to those is a catalog change in Buwiz Bot, not something this guide can do.

### Developer Preview

Google's "Configure the Google Workspace MCP servers" page (last updated 2026-10-19) describes these servers as a **Developer Preview** available through the Google Workspace Developer Preview Program, with no start/end date given. **Confirmed 2026-10-25 for a personal Gmail account:** sign-in and `tools/list` work, but every tool call returns "Access to this tool requires that your Google Cloud project (…) is enrolled in the Google Workspace Developer Preview Program". Enroll at https://developers.google.com/workspace/preview, or use the local `Google Workspace` plugin meanwhile. Buwiz Bot notices this error, drops that server's tools for the session and shows the message on the plugin row.

## Prerequisites

- A Google account (personal Gmail works; Google Workspace works too). The Google Cloud console may ask you to accept terms and may require a billing account to be attached for some features. The APIs and OAuth clients used here are free; billing is **not** required to create an OAuth client or to enable these APIs **(billing requirement for the MCP services is unconfirmed)**.
- No approval wait if you stay in "Testing" mode with yourself as test user. App verification (only needed to leave Testing for many users) takes about 3 to 5 business days for sensitive scopes per Google; restricted scopes (see table below) need more. Not needed for personal use.
- Cost: free.

## Steps

### 1. Create a Google Cloud project

1. Open https://console.cloud.google.com/ and sign in.
2. Top bar project picker -> **New Project**. Name: `Buwiz Bot`. Location: "No organization" is fine for a personal account. **Create**, then select the new project.

### 2. Enable the APIs

For each plugin you want, enable the underlying API **and** its MCP service. Per Google's guide the Console route is the "Enable the MCP services" link on their page, or use the CLI (replace `PROJECT_ID`):

```
gcloud services enable gmail.googleapis.com gmailmcp.googleapis.com \
  calendar-json.googleapis.com calendarmcp.googleapis.com \
  drive.googleapis.com drivemcp.googleapis.com \
  docs.googleapis.com docsmcp.googleapis.com \
  sheets.googleapis.com sheetsmcp.googleapis.com \
  slides.googleapis.com slidesmcp.googleapis.com \
  --project=PROJECT_ID
```

Or in the console: **APIs & Services -> Library**, search each name and click **Enable**.

| Plugin | Base API (enable) | MCP service (enable) |
|---|---|---|
| Gmail | Gmail API (`gmail.googleapis.com`) | `gmailmcp.googleapis.com` |
| Calendar | Google Calendar API (`calendar-json.googleapis.com`; not `calendar.googleapis.com`) | `calendarmcp.googleapis.com` |
| Drive | Google Drive API (`drive.googleapis.com`) | `drivemcp.googleapis.com` |
| Sheets | Google Sheets API (`sheets.googleapis.com`) | `sheetsmcp.googleapis.com` |
| Docs | Google Docs API (`docs.googleapis.com`) | `docsmcp.googleapis.com` |
| Slides | Google Slides API (`slides.googleapis.com`) | `slidesmcp.googleapis.com` |

`calendar-json`, `gmailmcp`, `drivemcp`, `calendarmcp` names are from Google's guide; the Docs/Sheets/Slides base API ids are standard Google API ids and the `docsmcp`/`sheetsmcp`/`slidesmcp` names come from the same guide. For the Cursor-proxied four, enabling the base API is required (calls count against your client's project); the `*mcp` service is only needed if you use Google's official endpoints. **(unconfirmed for the proxied endpoints)** Enabling extra APIs is harmless.

### 3. Configure the Google Auth Platform (consent screen)

Menu -> **Google Auth Platform** (older UI: APIs & Services -> OAuth consent screen). If you see **Get started**, click it.

1. **Branding** (App Information): App name `Buwiz Bot`; User support email = your email; Developer contact email = your email. Logo and domains are optional.
2. **Audience:** choose **External** (personal Gmail accounts can only use External; **Internal** is available only inside a Google Workspace organization and avoids most of the limits below). Leave publishing status as **Testing**.
3. **Audience -> Test users -> Add users:** add every Google address that will sign in to Buwiz Bot (including your own). Up to 100.
4. **Data Access -> Add or Remove Scopes:** add the scopes for the plugins you will use (table below), using **Manually add scopes** -> **Add to Table** -> **Update** -> **Save**. This list is for the consent screen; with a Desktop client the scopes are actually requested by Buwiz Bot at sign-in, and unlisted scopes may still work in Testing, but listing them avoids surprises and is required for verification later.
5. Save.

### 4. Create the OAuth client

1. **Google Auth Platform -> Clients -> Create client**.
2. **Application type: Desktop app**. Name: `Buwiz Bot desktop`. **Create**. (No redirect URI field is shown for Desktop clients.)
3. Copy the **Client ID** (`....apps.googleusercontent.com`) and **Client secret**. Google's docs say the secret is shown after creation and "can't be viewed again" in some flows; on current console you can add a new secret under the client's page if you lose it (up to two secrets per client). Store it in a password manager.
4. Optional checkbox **Use this client for an AI-powered agent**: Google's guide tells you to tick this for MCP clients. It tags the client and grants no extra scopes; ticking it is harmless. **(applicability to Desktop clients unconfirmed)**

Notes:
- Google's own MCP guide uses a **Web application** client with fixed redirect URIs (e.g. `https://claude.ai/api/mcp/auth_callback`). For a local desktop app like Buwiz Bot, a Desktop client with loopback redirect is the intended type. Google's native-app docs confirm loopback `http://127.0.0.1:<port>` redirects for Desktop clients, say Desktop apps are public clients that "cannot keep secrets confidential," and recommend PKCE. The docs excerpt I read does not say explicitly that *any* port is accepted, only that the redirect URI must match; I am relying on the brief. If Google returns `redirect_uri_mismatch`, see Troubleshooting.
- Inactive OAuth clients are deleted automatically after 6 months (with an email 30 days before).

### 5. Paste into Buwiz Bot

For each Google plugin you want: **Connect apps -> (plugin) -> settings**:

| Buwiz Bot field | Value |
|---|---|
| Client ID | the Desktop client ID |
| Client Secret | the Desktop client secret (Buwiz Bot sends it in token requests) |

Then **Authenticate**. Google shows the account picker; if the app is in Testing, you see a "Google hasn't verified this app" warning. Click **Continue** (via "Advanced" if shown), tick all requested permissions, and continue. Buwiz Bot requests offline access so Google returns a refresh token.

Re-use the same ID/secret for every Google plugin. Each plugin is its own authorization (different scopes), so you sign in once per plugin.

## Scopes by plugin

Gmail, Calendar: taken from the live `oauth-protected-resource` metadata of each endpoint. Drive/Sheets/Docs/Slides: the Cursor proxy metadata (what the catalog plugins use) and, in the third column, Google's official endpoints. All scopes are prefixed `https://www.googleapis.com/auth/` except where shown.

| Plugin | Scopes advertised by the catalog endpoint | Extra scopes on Google's official endpoint |
|---|---|---|
| Gmail | `https://mail.google.com/`, `gmail.compose`, `gmail.drafts`, `gmail.drafts.create`, `gmail.drafts.readonly`, `gmail.labels`, `gmail.metadata`, `gmail.modify`, `gmail.readonly`, `gmail.send`, `gmail.settings.basic` | same endpoint |
| Calendar | `calendar`, `calendar.acls`, `calendar.calendarlist`, `calendar.calendarlist.readonly`, `calendar.calendars`, `calendar.calendars.readonly`, `calendar.events`, `calendar.events.freebusy`, `calendar.events.readonly`, `calendar.freebusy`, `calendar.readonly`, `calendar.settings.readonly` | same endpoint |
| Drive | `drive.readonly`, `drive.file` | `drive` |
| Sheets | `spreadsheets` | `drive.readonly`, `drive`, `spreadsheets.readonly` |
| Docs | `documents` | `drive.readonly`, `drive`, `documents.readonly` |
| Slides | `presentations` | `drive.readonly`, `drive`, `drive.file`, `presentations.readonly` |

Google's setup guide lists a minimal set per plugin: Gmail `gmail.readonly` + `gmail.compose`; Drive `drive.readonly` + `drive.file`; Docs `drive.readonly`, `drive.file`, `documents.readonly`, `documents`; Sheets `drive.readonly`, `drive.file`, `spreadsheets.readonly`, `spreadsheets`; Slides `drive.readonly`, `drive.file`, `presentations.readonly`, `presentations`; Calendar `calendar.calendarlist.readonly`, `calendar.events.freebusy`, `calendar.events.readonly`. Which exact scopes Buwiz Bot requests is determined by the Buwiz Bot plugin and is **unconfirmed**; add the union you are comfortable with to the consent screen.

### Sensitivity (drives verification)

From Google's scope tables:

- Restricted: `https://mail.google.com/`, `gmail.readonly`, `gmail.compose`, `gmail.insert`, `gmail.modify`, `gmail.metadata`, `gmail.settings.basic`, `gmail.settings.sharing`, `drive`, `drive.readonly`, `drive.metadata*`.
- Sensitive: `gmail.send`; Calendar scopes, `spreadsheets`, `documents`, `presentations` are normally sensitive too **(not shown on the pages I could read; check the "Data Access" page in the console, which labels each scope)**.
- Non-sensitive: `gmail.labels`, `drive.file`.

## Testing vs. In production (read this)

Facts from Google's docs (checked 2026-10-25):

- **Testing + External:** up to 100 test users; users see an unverified warning; **refresh tokens expire after 7 days** (exception only when requesting nothing but name/email/profile/openid). Practical effect: every week Buwiz Bot will tell you to re-authenticate.
- **In production + External, unverified:** the 7-day expiry no longer applies; users get the "Unverified apps" warning, and there is a lifetime cap of 100 new users that cannot be reset (the cap applies to unverified apps with sensitive/restricted scopes). To switch: Google Auth Platform -> **Audience** -> **Publish app**.
- Google's own wording on the personal-use case: apps used by you alone or a few known users may proceed through the unverified-app screen. Whether restricted Gmail/Drive scopes are *blocked* (rather than warned) for unverified production apps is **unconfirmed**; try it and see.
- **Verification** (removes the warning/cap): needs published branding, a verified domain, a public home page and a privacy policy on the same domain, a justification per scope and an unlisted YouTube demo; sensitive-scope review typically takes 3 to 5 business days. Restricted scopes (Gmail/Drive) additionally need a separate restricted-scope review (security assessment details not covered by the pages I read, so cost and timeline are **unconfirmed**). For a personal Buwiz Bot install, skip this.
- Refresh tokens also die if: user revokes access, 6 months unused, password change when Gmail scopes were granted, more than 100 live refresh tokens for the same user + client (oldest silently invalidated), or an admin blocks the service (`admin_policy_enforced`).

Recommendation for personal use: stay in Testing and accept weekly re-auth, or click **Publish app** and live with the unverified warning (no weekly expiry).

## Verification

1. After Authenticate, the plugin shows connected in Buwiz Bot.
2. Gmail: "List my last 3 emails' subjects." Calendar: "What's on my calendar tomorrow?" Drive: "List my 5 most recent files." Sheets/Docs/Slides: ask it to read a file you give it by name (note: with only `drive.file`, apps can only see files they created or that you picked; `drive.readonly` is needed to browse).
3. In https://myaccount.google.com/permissions you should see `Buwiz Bot` with the requested access.

## Troubleshooting

- **`access_denied` / "app has not completed the Google verification process"** with no way to continue: your address is not in **Audience -> Test users**. Add it.
- **`redirect_uri_mismatch`**: the client is not a Desktop app (e.g. created as Web application). Create a new Desktop client. If Google still rejects the loopback redirect, fall back to a Web application client and register the exact Buwiz Bot callback `http://127.0.0.1:8787/oauth/callback/google` (confirm the exact string in Buwiz Bot's plugin page; **unconfirmed**) and send that back to the Buwiz Bot developers.
- **`invalid_client` / `Unauthorized`**: wrong secret, pasted with a trailing space, or client deleted. Create a new secret.
- **Works for a week, then asks to sign in again**: Testing-mode 7-day refresh expiry. Publish the app to production or accept re-auth.
- **403 `accessNotConfigured` / "API has not been used in project ... or it is disabled"**: enable the base API (step 2) in the *same project* that owns the client, wait a minute.
- **`PERMISSION_DENIED` on an MCP call although sign-in worked**: the `*mcp.googleapis.com` service is not enabled, or the Developer Preview enrollment is missing (see above).
- **Missing scope errors**: the scope was not granted on the consent screen (users can untick scopes); re-authenticate and tick all boxes.
- **`Error 400: invalid_scope`**: the scope string is not enabled/known for that project; re-add it under Data Access.

## Google Workspace (local) plugin

The stopgap until the preview enrollment comes through. Plugin files: `grok-shell/server/plugins/external_plugins/google-workspace/` (listed in `server/plugins/marketplace.json`).

- **What runs:** `uvx workspace-mcp --single-user --tool-tier extended --tools gmail calendar drive docs sheets slides` — 57 tools: mail, calendars and events, Drive search/read/create plus rename/copy/share (`update_drive_file`), Docs editing (`modify_doc_text`, `find_and_replace_doc`), Sheets and Slides. The smaller `core` tier (24 tools) can't rename or find-and-replace — the AI then reports the file "unchanged". Needs `uv` (`~/.local/bin/uvx`); the first start downloads the package.
- **Setup values:** Google Client ID and Secret are the shared ones (saving them in any Google plugin updates all). **Google Account Email** = `codeitlikemiley@gmail.com` (so the AI never has to ask which account).
- **Sign-in:** there is no Authenticate button — the row shows Connected because the local server is running. The first Google tool call opens a Google consent tab; approve every box. Google redirects to `http://localhost:8791/oauth2callback`, which the running server receives (Desktop clients accept any loopback port, nothing to register). If the redirect page says "can't connect", that sign-in was started by a server that has since stopped — ask the AI again to start a fresh one.
- **Where tokens live:** `~/.google_workspace_mcp/credentials/` (not in Buwiz Bot). Delete that folder to sign out.
- **More tools:** change `--tool-tier extended` to `complete` in the plugin's `.mcp.json` (more tools = bigger prompt for the AI).
- **APIs it needs:** only the base APIs from step 2 (`gmail`, `calendar-json`, `drive`, `docs`, `sheets`, `slides`) — already enabled.

## Sources

- https://developers.google.com/workspace/guides/configure-mcp-servers (last updated 2026-10-19)
- https://github.com/taylorwilsdon/google_workspace_mcp (workspace-mcp 2.2.0, MIT)
- https://gmailmcp.googleapis.com/.well-known/oauth-protected-resource/mcp/v1 (live)
- https://calendarmcp.googleapis.com/.well-known/oauth-protected-resource/mcp/v1 (live)
- https://drivemcp.googleapis.com/.well-known/oauth-protected-resource/mcp/v1, docsmcp, sheetsmcp, slidesmcp (live)
- https://api.cursor.com/.well-known/oauth-protected-resource/rest-mcp/google-drive/mcp (and google-sheets, google-docs, google-slides) (live)
- https://api.cursor.com/.well-known/oauth-authorization-server/rest-mcp/google-drive/dcr (live)
- https://github.com/cursor/plugins/tree/main/third_party (gmail, google-drive, google-calendar, google-sheets, google-docs, google-slides `mcp.json`)
- https://developers.google.com/identity/protocols/oauth2/native-app (updated 2026-10-16)
- https://developers.google.com/identity/protocols/oauth2#expiration (updated 2026-10-16)
- https://support.google.com/cloud/answer/15549945 (audience / publishing status)
- https://support.google.com/cloud/answer/15549257 (OAuth clients)
- https://developers.google.com/identity/protocols/oauth2/production-readiness/sensitive-scope-verification (updated 2026-08-19)
- https://developers.google.com/workspace/gmail/api/auth/scopes
- https://developers.google.com/workspace/drive/api/guides/api-specific-auth
- https://developers.google.com/identity/protocols/oauth2/scopes
