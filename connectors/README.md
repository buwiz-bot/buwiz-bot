# Buwiz Bot connector setup guides

Step-by-step guides for plugins whose sign-in cannot self-register (no OAuth dynamic client registration), so you must create your own developer app, API key or token on the provider's site first. Written 2026-10-25 from a probe of the third-party plugin catalog Buwiz Bot then bundled (probed 2026-10-25) plus per-provider research. Every guide marks what could not be confirmed as **(unconfirmed)**. Nothing here was created or submitted on any provider; all credentials are still to do.

Buwiz Bot no longer bundles that catalog (or its logos): it was removed for the Apache-2.0 relicense (#93). The plugins below are no longer listed in **Connect apps**. The guides still apply to a copy installed before the removal (it keeps loading and running) and to adding a provider's MCP endpoint yourself as an MCP server.

Status for every row: **needs setup**. "Callback" in each guide is `http://127.0.0.1:8787/oauth/callback/<connector id>` unless the provider forces otherwise; confirm the exact string on Buwiz Bot's plugin page.

## How to enable in Buwiz Bot once you have credentials

1. Open **Connect apps** and find the plugin (an installed one is listed under the plugins you added).
2. Open the plugin's settings and paste the credentials the guide lists (Client ID, and Client Secret when the guide says so). Google plugins share one Desktop-app client, so enter it once.
3. Click **Authenticate** and approve in the browser window that opens.

Plugins that need an API key / token instead of OAuth credentials depend on Buwiz Bot letting you send a custom `Authorization` header; none of the guides could confirm that field exists.

## Index

| Plugin | Provider | What to create | Cost / approval | Guide | Status |
|---|---|---|---|---|---|
| x | X | Developer app, OAuth 2.0 (Native or Web App) | Pay-per-usage credits; card needed | [x.md](x.md) | needs setup |
| x-ads | X | Separate X app + Ads API access form | Ads account; X says up to 3 business days, forums report longer | [x-ads.md](x-ads.md) | needs setup |
| gmail | Google | Cloud project + Desktop OAuth client (shared) | Free; Testing mode = 7-day refresh tokens; restricted scopes need verification for public use | [google.md](google.md), [gmail.md](gmail.md) | set up, parked: needs Developer Preview enrollment (local `google-workspace` plugin in use meanwhile) |
| google-calendar | Google | same shared client | same | [google.md](google.md), [google-calendar.md](google-calendar.md) | needs setup |
| google-drive | Google | same shared client | same (now Google's own `drivemcp` endpoint) | [google.md](google.md), [google-drive.md](google-drive.md) | needs setup |
| google-sheets | Google | same shared client | same | [google.md](google.md), [google-sheets.md](google-sheets.md) | needs setup |
| google-docs | Google | same shared client | same | [google.md](google.md), [google-docs.md](google-docs.md) | needs setup |
| google-slides | Google | same shared client | same | [google.md](google.md), [google-slides.md](google-slides.md) | needs setup |
| google-cloud-bigquery | Google | same Desktop client + IAM roles | Free client; queries billed per TiB scanned (first 1 TiB/month free) | [google-cloud-bigquery.md](google-cloud-bigquery.md) | needs setup |
| slack | Slack | Your own Slack app (PKCE, loopback redirect, MCP on) | Free; workspace admin approval | [slack.md](slack.md) | needs setup |
| hubspot | HubSpot | MCP connector / MCP auth app (confidential) | No cost documented; some tools need paid hubs | [hubspot.md](hubspot.md) | needs setup |
| salesforce | Salesforce | External Client App (public, PKCE) | Org admin; Developer Edition is free; up to 30 min to propagate | [salesforce.md](salesforce.md) | needs setup |
| salesforce-headless-360 | Salesforce | External Client App with secret | Same; Beta | [salesforce-headless-360.md](salesforce-headless-360.md) | needs setup |
| zoom | Zoom | General app on Zoom Marketplace, public client | Free; no publishing needed for personal use | [zoom.md](zoom.md) | needs setup |
| box | Box | Admin Console MCP credentials or Developer Console app | Box admin; no fee for credentials | [box.md](box.md) | needs setup |
| docusign | Docusign | Integration Key + Secret | Demo account free; production needs Go-Live + allow-list | [docusign.md](docusign.md) | needs setup |
| databricks-genie-one | Databricks | Custom OAuth app connection (account console) | Account admin; ~30 min to apply | [databricks-genie-one.md](databricks-genie-one.md) | needs setup |
| atscale | AtScale | Reuse built-in `atscale-mcp` client; add redirect URI | AtScale licence; admin role | [atscale.md](atscale.md) | needs setup |
| dnb-risk-analytics | Dun & Bradstreet | Client ID/secret from Risk Analytics admin | Existing workspace + admin; price unpublished | [dnb-risk-analytics.md](dnb-risk-analytics.md) | needs setup |
| dnb-dplus-mcp | Dun & Bradstreet | Credentials issued by D&B | Enterprise entitlement; wait unpublished | [dnb-dplus-mcp.md](dnb-dplus-mcp.md) | needs setup |
| alpaca-trading (+paper) | Alpaca | Hosted client-metadata JSON (CIMD) as Client ID, or ask Alpaca | No approval for CIMD; live trading needs funded account | [alpaca-trading.md](alpaca-trading.md) | needs setup |
| alpaca-broker (+sandbox) | Alpaca | Same CIMD approach, two clients | Approved broker partner required | [alpaca-broker.md](alpaca-broker.md) | needs setup |
| bitrise | Bitrise | Personal Access Token (Bearer) or CIMD | Free token | [bitrise.md](bitrise.md) | needs setup |
| render | Render | API key (Bearer); no OAuth app route | Free key; covers all workspaces | [render.md](render.md) | needs setup |
| mongodb-atlas | MongoDB | Atlas MCP configuration (client credentials) | Org/Project Owner | [mongodb-atlas.md](mongodb-atlas.md) | needs setup |
| elevenlabs | ElevenLabs | Probably nothing; CIMD | Usage billing | [elevenlabs.md](elevenlabs.md) | needs setup |
| onesignal | OneSignal | Nothing: supports dynamic registration, leave Client ID blank | Free; beta may need per-app enablement | [onesignal.md](onesignal.md) | likely works without setup |
| adapter | Adapter | No self-service path; ask vendor or CIMD | Unpublished | [adapter.md](adapter.md) | needs setup (vendor contact) |
| adaption | Adaption Labs | API key (Bearer) or CIMD | Write tools spend credits | [adaption.md](adaption.md) | needs setup |
| allo | Allo | API key (Settings > API) or ask Allo | MCP included in all plans | [allo.md](allo.md) | needs setup |
| transcend-agent-governance | Transcend | Ask Transcend for a client, or CIMD | Enterprise, beta | [transcend-agent-governance.md](transcend-agent-governance.md) | needs setup (vendor contact) |
| pascal (pascal-hosted) | Pascal | API key in Settings (Bearer); cannot create an OAuth app | Free account, 150 free credits/day | [pascal.md](pascal.md) | needs setup |
| shopify | Shopify | Nothing you can create; Shopify allowlists clients | Unknown | [shopify.md](shopify.md) | blocked on vendor |
| shop | Shopify | Nothing needed for anonymous search/cart; sign-in blocked | Free | [shop.md](shop.md) | partly usable without sign-in |

## Not possible (no guide)

Hosted behind another vendor's account gateway, per the 2026-10-25 probe: `finance` (Grok connector gateway), `link`, `onedrive`, `outlook`, `outlook-calendar`, `power-bi`, `sharepoint`, `teams` (all Cursor-hosted, need a Cursor account).

## Cross-cutting findings

- Several providers (Alpaca, Bitrise, MongoDB, ElevenLabs, Adapter, Adaption, Allo, Transcend) advertise Client ID Metadata Document (CIMD) support. If Buwiz Bot gains CIMD support, most manual registration steps for them go away.
- Slack, HubSpot, Docusign, Databricks and AtScale loopback `http://127.0.0.1` redirects were not confirmed; their guides say to try and what to register as a fallback.
- Catalog entries that pin another vendor's client ID (`cursor`, `anysphere-cursor`, ...) will not accept Buwiz Bot's callback; that is why the guides ask for your own.
