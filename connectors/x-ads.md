# X Ads connector setup

Checked against X's docs and live endpoints on 2026-10-25. Items marked **(unconfirmed)** could not be verified.

This plugin uses the same kind of X developer app as the X plugin. **Read [x.md](x.md) first** for creating the X developer account, the pay-per-usage credits, the Native App vs. Web App choice and the generic troubleshooting. This file covers only what X Ads adds: the Ads API access approval, the ads account requirement, the second callback URL, and one trap about sharing the app.

## What it unlocks

X's hosted Ads MCP server, built into the X API gateway. X's doc lists 74 tools: list ads accounts, campaigns, line items, funding instruments, promoted posts; analytics (`get_account_stats`, `get_campaign_reach`); targeting search and audience-size estimates; custom audiences and do-not-reach lists; cards, media library and media creatives; web and app conversion tags; and writes (`create_campaign`, `update_campaign`, `activate_campaign`, `create_line_item`, `activate_line_item`, `promote_post`, `create_ad_post`, targeting changes). Per X, campaigns and line items are always created **PAUSED**; nothing spends until the assistant calls an `activate_*` tool.

| Item | Value (live, 2026-10-25) |
|---|---|
| MCP server | `https://ads-api.x.com/mcp` (POST JSON-RPC, SSE-framed responses) |
| Protected-resource metadata | `https://ads-api.x.com/.well-known/oauth-protected-resource/mcp` |
| Authorization server (issuer) | `https://api.x.com` |
| Authorize URL | `https://x.com/i/oauth2/authorize` |
| Token URL | `https://api.x.com/2/oauth2/token` |
| PKCE | S256 |
| Token endpoint client auth | `none` (public) or `client_secret_basic`. X rejects `client_secret_post` (per X's Ads MCP doc). |
| Registration endpoint | none |
| Resource `scopes_supported` | `ads.read ads.write media.write offline.access` |

Observation for the Buwiz Bot developers: an unauthenticated request to `https://ads-api.x.com/mcp` returned `401` with `application/problem+json` and no `WWW-Authenticate` header, so Buwiz Bot must find the metadata by the RFC 9728 well-known path above rather than from a challenge header.

## Prerequisites

1. Everything in x.md (X account, developer account at https://console.x.com, a developer app, payment card/credits for the pay-per-usage X API). Whether calls to `ads-api.x.com/mcp` are billed against those pay-per-usage credits, or are free, is **(unconfirmed)**: X's Ads docs I read state no price for the Ads API or the MCP, and X's pricing page describes only `api.x.com` resources.
2. **Ads API access approval for your specific app.** X's docs: "Submit a request for Ads API access for each of your developer app(s) using the Ads API Access Form." Review time stated by X: "Allow up to 3 business days for review." Community forum posts report applicants waiting longer or getting no reply, and `UNAUTHORIZED_CLIENT_APPLICATION` errors while pending (anecdotal). You pick an access level on the form: **Conversion Only** (web/mobile conversion endpoints) or **Standard Access** (Analytics, Campaign Management, Creatives, Custom Audiences, Conversion). The MCP needs **Standard Access**. The form asks for your App ID (the number in the URL of the app's details page) and your use case; its exact fields are **(unconfirmed)**, I did not open or submit it. Form link in X's docs: https://help.x.com/en/forms/xdev/conversion-api-request (the URL slug says "conversion-api-request" although the docs label it "Ads API Access Form"; if it looks wrong, start from https://docs.x.com/forms/ads-api-access).
3. **An X Ads account.** The X user who signs in must already have access to at least one ads account (check at https://ads.x.com); the MCP only sees accounts that user can access. Creating campaigns also needs a funding instrument (payment method) on that ads account. Roles: ad-account permissions are Account administrator, Ad manager, Campaign analyst, Organic analyst, Creative Manager; write tools need an editing role (which role each tool needs is **(unconfirmed)**).
4. Ads spend is real money, separate from the X API credits. Writes start paused, but an `activate_*` call spends.

## Steps

Use a dedicated app (recommended, see "Sharing the X app" below), or reuse the X app.

1. https://console.x.com -> your app (or **Create App**, name `Buwiz Bot Ads`).
2. In the app's authentication settings (see x.md step 6) set:
   - **Type of App:** Native App (public client, no secret). X's Ads MCP doc recommends this. A confidential type also works if Buwiz Bot sends the secret with `client_secret_basic`.
   - **App permissions:** Read and Write (X's Ads MCP doc). If the app is shared with the X plugin, keep the broader option from x.md.
   - **Callback URI / Redirect URL:** `http://127.0.0.1:8787/oauth/callback/x-ads` (confirm in Buwiz Bot's plugin page).
     - X validates redirect URIs exactly (scheme, host, port, path, no trailing slash). The X plugin's callback `http://127.0.0.1:8787/oauth/callback/x` does **not** match this one, so the `x-ads` URL must be registered even if the app already works for X. Use `127.0.0.1`, not `localhost`. Up to 10 callback URLs per app per x.md.
   - **Website URL:** `http://x.com` (the value X's doc uses).
3. **Keys and tokens:** copy the **OAuth 2.0 Client ID**. X's doc warns it is the long string like `TzNHbE5X...6MTpjaQ`, **not** the numeric app ID.
4. **Project Access:** X's doc says click **MANAGE** and select **Ads Project** "to automatically enable Ads API access to your App ID". Exact menu text is as quoted; the screen layout is **(unconfirmed)**.
5. Submit the **Ads API Access Form** for this app (Prerequisites item 2) and wait for approval. Until then the MCP answers `403`.
6. Confirm your X user can see an ads account at https://ads.x.com.

### Sharing the X app: read this before reusing it

X's Ads MCP doc: "X keeps a single live OAuth grant per (app, user): signing in from a second client revokes the first client's tokens. In practice, use one client per X app." Buwiz Bot's `x` and `x-ads` connectors are two sign-ins by the same user against the same app, so connecting X Ads in a shared app will revoke the X plugin's tokens (and the reverse), forcing a re-authenticate each time you switch. Whether that applies when the scopes differ is **(unconfirmed)**, but X's wording does not exclude it. Create a separate app for X Ads. The Ads API access request is per app anyway, so a separate app also keeps the Ads enrollment off your main X app.

## What to paste into Buwiz Bot

**Connect apps -> X Ads -> settings -> Authenticate**

| Buwiz Bot field | Value |
|---|---|
| Client ID | OAuth 2.0 Client ID of the (Ads-approved) app |
| Client Secret | leave empty for a Native App |
| Callback URL (shown by Buwiz Bot, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/x-ads` |

If the shared-app route is used, the X plugin's Client ID is the same value but its callback stays `.../oauth/callback/x`.

## Scopes requested

`ads.read ads.write media.write offline.access` (catalog `auth.scopes`; identical to the server's `scopes_supported`).

- `ads.read`: read and analytics tools. `ads.write`: campaign and creative writes. `media.write`: media upload and media-library writes (optional for campaign work). `offline.access`: refresh token; without it tokens last about 2 hours with no refresh.
- X's doc: for a read-only agent omit `ads.write`; write tools then fail with authorization errors. Buwiz Bot's plugin requests the full set; narrowing needs a Buwiz Bot option **(unconfirmed)**.

## Alternative: user access token

X's Ads MCP page documents `Authorization: Bearer <user access token>`: mint a token once with the authorize URL and a `curl` to `https://api.x.com/2/oauth2/token` (their example uses `http://localhost:8080/callback` and `code_challenge_method=plain`), then refresh with `grant_type=refresh_token`. Access tokens last about 2 hours and refresh tokens rotate (save the newest). It still needs the approved app and an ads-enabled user. The app-only Bearer Token from x.md does not apply: X says authentication is "the user's own OAuth2 token". Requires a custom-header field in Buwiz Bot **(unconfirmed)**; not recommended over OAuth.

## Verification

1. Buwiz Bot shows X Ads as connected.
2. Ask: "List my X ads accounts." (read-only; fails with 403 if the app is not approved, or returns an empty list if the user has no ads account access).
3. Then: "Show campaign performance for the last 7 days." For writes, test with "Create a paused campaign called Buwiz Bot test" and delete it afterwards; do not activate it.
4. In X's settings, the connected-apps list should show the app **(page address unconfirmed; look under Settings -> Security and account access -> Apps and sessions)**.

## Troubleshooting

- **Redirect mismatch / "Something went wrong" at authorize:** the exact string `http://127.0.0.1:8787/oauth/callback/x-ads` is not registered in this app. Registering only `.../x` is the usual mistake. See x.md for the other causes.
- **403 from the MCP after sign-in works:** X's doc: "a 403 means your app isn't enrolled yet". The Ads API access form was not approved for this app, or **Ads Project** was not attached under Project Access. Also check pay-per-usage credits (x.md).
- **401:** expired/invalid token or missing `offline.access`; re-authenticate.
- **Empty account list:** the signed-in X user has no ads-account access; add them at ads.x.com (Account admin grants it), or sign in with the owner account.
- **X plugin stops working after connecting X Ads (or the reverse):** the single-grant behaviour above. Use separate apps.
- **`invalid_client` with a Web/Automated app:** Buwiz Bot must send the secret as `client_secret_basic`; X rejects `client_secret_post`. Switch the app to Native App.
- **Scope refused:** app permissions too narrow; set Read and Write and re-authorize.
- **Approval stuck beyond 3 business days:** use X's Ads API support page, https://developer.x.com/en/support/twitter-ads-api.html, or the Ads API access category of the X developer forum.

## Sources

- https://docs.x.com/x-ads-api/mcp.md (X Ads MCP: endpoint, app setup, callbacks, scopes, single-grant note, token mint; checked 2026-10-25)
- https://docs.x.com/x-ads-api/introduction.md ("Allow up to 3 business days for review")
- https://docs.x.com/x-ads-api/getting-started/step-by-step-guide.md (access tiers, per-app request)
- https://docs.x.com/x-ads-api/fundamentals/accessing-ads-accounts.md (ad-account roles)
- https://docs.x.com/x-ads-api/getting-started/increasing-access.md
- https://docs.x.com/forms/ads-api-access.md (links to the form)
- https://ads-api.x.com/.well-known/oauth-protected-resource/mcp (live)
- https://api.x.com/.well-known/oauth-authorization-server (live)
- https://github.com/cursor/plugins/tree/main/third_party/x-ads (catalog entry; its pinned client ID is Cursor's, not usable by Buwiz Bot)
- [x.md](x.md) in this folder
- https://devcommunity.x.com/c/advertiser-api/access/36 (community reports on approval waits; anecdotal)
