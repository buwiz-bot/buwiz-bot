# X (Twitter) connector setup

Checked against X's docs and live endpoints on 2026-10-25. X changes its developer console often; anything marked **(unconfirmed)** could not be verified from official docs and must be checked on screen.

## What it unlocks

The X plugin connects Rox to X's hosted MCP server at `https://api.x.com/mcp`. Through it the assistant can (depending on the scopes you grant) read posts, users, follows, lists, spaces, likes, mutes, blocks and bookmarks, and write lists, blocks, bookmarks, DMs, media and call/broadcast objects.

X has no OAuth dynamic client registration, so Rox cannot register itself. You create your own X developer app and paste its **Client ID** (and **Client Secret**, if your app type has one) into Rox.

Endpoints (confirmed live via `https://api.x.com/.well-known/oauth-authorization-server` and `https://api.x.com/.well-known/oauth-protected-resource/mcp`):

| Item | Value |
|---|---|
| MCP server | `https://api.x.com/mcp` |
| Authorization server (issuer) | `https://api.x.com` |
| Authorize URL | `https://x.com/i/oauth2/authorize` |
| Token URL | `https://api.x.com/2/oauth2/token` |
| PKCE | S256 |
| Token endpoint client auth | `none` (public client) or `client_secret_basic` (confidential client) |
| Grant types | `authorization_code`, `refresh_token` |

## Prerequisites

- An X account (the account that will own the developer app). Accepting the Developer Agreement is part of sign-up.
- A payment card and prepaid credits. X now sells API access as **pay-per-usage** (see "Cost" below). Calls fail with 403 until the app has access/credits.
- No manual approval wait is documented for pay-per-use. **(unconfirmed)** Older tiers had human review; check whether the console asks for a use-case description.

## Cost (as of 2026-10-25)

From the X API pricing page (`docs.x.com/x-api/getting-started/pricing`, which carries no effective date and says prices can change):

- Pay-per-usage, no subscription, no minimum spend. You buy credits up front in the Developer Console.
- No free tier is described on that page.
- Examples per resource returned: Posts, Lists, Spaces $0.005; Users, DM events, followers/following $0.010; Likes, Mutes, Blocks $0.001. "Owned reads" (your own posts, mentions, bookmarks, followers) $0.001.
- Writes per request: creating a post $0.015 ($0.200 if it contains a URL); other writes $0.005 to $0.015.
- The same resource requested twice in one UTC day is not billed twice (documented as a soft guarantee).
- You can set a spending limit; requests are blocked when it is reached.
- Pay-per-usage is capped at 3 million post reads per monthly cycle.
- Promotions mentioned on the page: $20 in credits after saving a card and a first purchase; up to $50 matched on first auto-recharge. Treat as subject to change.

Everything the assistant reads or writes through the MCP server is billed to your credits. Set a spending limit before first use.

## Steps

1. Go to **https://console.x.com** and sign in with the X account that will own the app. (`developer.x.com/en/portal/dashboard` is the older address and may redirect; the docs now point to console.x.com.)
2. Accept the **Developer Agreement** if prompted.
3. Add credits / a payment method: open the billing or credits area of the console, save a card, and buy credits. Set a **spending limit** here too. **(exact menu labels unconfirmed)**
4. Click **Create App** (the docs also call the button "New App"). Enter:
   - **App name:** `Rox` (must be unique across X; add a suffix if taken).
   - **Description / use case:** e.g. "Personal desktop assistant that reads and manages my own X account."
5. After creation, X may show API Key / API Secret / Access Token credentials **once only**. You do not need those for Rox (they are OAuth 1.0a keys), but if you do not plan to use them, you can ignore or regenerate them later. Do not lose the Client Secret from step 7 the same way.
6. Open the app's settings and find the **authentication / User authentication settings** section (older docs: **User authentication settings -> Set up**; current docs call it "App's authentication settings"). **(exact label unconfirmed)** Enable **OAuth 2.0** (OAuth 2.0 works only with the v2 API, which the MCP server uses).
7. Fill in:
   - **Type of App:**
     - **Native App** (public client): no Client Secret, PKCE only. Simplest; works with X's `none` token auth method. Recommended unless Rox's plugin page insists on a secret.
     - **Web App** (confidential client): X issues a Client Secret; Rox then authenticates to the token endpoint with `client_secret_basic`.
     - Other options: *Automated App or Bot* (confidential), *Single Page App* (public).
   - **App permissions:** the plugin requests DM scopes (`dm.read`, `dm.write`), so choose the broadest option, labelled **Read, write, and DMs** in X's docs (other options: *Read only*, *Read and write*). The docs list these labels under OAuth 1.0a; whether the console still shows them for OAuth 2.0 is **unconfirmed**. If you pick a narrower option, the matching scopes will be refused at consent time.
   - **Callback URI / Redirect URL:** `http://127.0.0.1:8787/oauth/callback/x`
     - Must match exactly, including scheme, port and trailing slash (none here). Use `127.0.0.1`, **not** `localhost` (X's docs say so explicitly). Up to 10 callback URLs per app.
   - **Website URL:** if the form requires it, enter any valid URL you control, for example your GitHub profile or `https://example.com`. The docs do not say whether this field is required. **(unconfirmed)**
8. Save. X then shows the **Client ID** (and **Client Secret** for Web App / Automated App). Per X's docs, the Client ID lives under **Keys and Tokens** in the app's page; the secret is shown only once on creation/regeneration, so copy it immediately.
9. Make sure the app is attached to the pay-per-usage / production access (see Troubleshooting). **(exact console steps unconfirmed)**

## What to paste into Rox

Open **Connect apps -> X** -> settings:

| Rox field | Value |
|---|---|
| Client ID | OAuth 2.0 Client ID from Keys and Tokens |
| Client Secret | OAuth 2.0 Client Secret (leave empty for a Native App) |
| Callback URL (shown by Rox, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/x` |

Then click **Authenticate**. A browser window opens on `x.com/i/oauth2/authorize`; approve the scopes.

Do not paste the API Key, API Secret, Access Token or Bearer Token into the Client ID / Client Secret fields; they are different credentials.

## Scopes requested by the plugin

`tweet.read users.read follows.read space.read mute.read like.read list.read list.write block.read block.write bookmark.read bookmark.write dm.read dm.write calls.read calls.write broadcast.read broadcast.write media.write developer.billing.write developer.write offline.access`

The MCP server's metadata advertises exactly these. The authorization server also lists `ads.read` and `ads.write`, which the plugin does not request.

Be aware: `developer.write` and `developer.billing.write` let the assistant act on your developer account and billing, and `dm.write` can send DMs. Consider whether you want to grant all of them. `offline.access` is what gives Rox a refresh token so you do not re-authenticate every two hours.

## Alternative: app-only Bearer token (read-only)

Each app has a **Bearer Token** (app-only auth) shown under Keys and Tokens. Third-party write-ups of X's hosted MCP server say you can point a client straight at `https://api.x.com/mcp` with `Authorization: Bearer <token>`. **(unverified: not confirmed in X's own docs; the tools page the search found describes a local XMCP and the `xurl` bridge instead)**

Limits if it works: app-only, so no user context: no bookmarks, no DMs, no writes, nothing "as you". No refresh (the token is long-lived until you regenerate it). Billed the same. Rox would need a plugin option for a custom header; use OAuth if that field does not exist.

Also unverified: X's `xurl mcp` bridge (`npx -y @xdevplatform/xurl mcp https://api.x.com/mcp`) is described as the intended path for clients without native OAuth support, using redirect URI `http://localhost:8080/callback`. Rox does not use it; the native OAuth metadata at `api.x.com` is live.

## Verification

1. After Authenticate, Rox's plugin page should show X as connected.
2. Ask Rox: "Who am I on X?" (a `users.read` call; costs about $0.01 or less) or "List my bookmarks".
3. In the console, check that your credit balance dropped by a small amount.

## Troubleshooting

- **Redirect / "redirect_uri" mismatch, or X's page says "Something went wrong"** at the authorize step: the callback in the app settings is not byte-identical to the one Rox sends. Re-enter `http://127.0.0.1:8787/oauth/callback/x`; do not use `localhost`; check for a trailing slash; confirm Rox's plugin page shows the same string.
- **Invalid scope / "scope not allowed"**: the app permissions are too narrow (e.g. *Read only*), or the app is not a v2/OAuth 2.0 app. Raise permissions and re-authorize (changing permissions requires users to re-authorize).
- **401 Unauthorized**: wrong Client ID/Secret, regenerated credentials, or an expired token without a refresh token (re-authenticate; make sure `offline.access` was granted).
- **403 Forbidden** after sign-in works: X's docs list the usual causes as "app lacks access to the endpoint or needs enrollment/approval", missing scopes, or protected resources. In practice also check: no credits left, spending limit hit, or the app is not attached to the pay-per-usage access. X's problem type is `client-forbidden` ("App not enrolled or lacks required access"); a third-party write-up calls the same condition `client-not-enrolled`. Fix by buying credits and making sure the app is in the pay-per-usage / production project. **(console steps unconfirmed)**
- **429**: rate limit (`rate-limit-exceeded`) or usage cap (`usage-capped`). Check the `x-rate-limit-reset` header.
- **453**: this is the classic "you need a higher access level for this v1.1/v2 endpoint" error from the old tier system. X's current response-code docs do not define 453, so it is probably an old-tier message; treat it like 403 and verify your access/credits. **(unconfirmed)**
- **Client Secret lost**: regenerate it in the app's Keys and Tokens page, update Rox.

## Related: X Ads

The `x-ads` plugin (`https://ads-api.x.com/mcp`) also needs your own X app and extra Ads API approval; see [x-ads.md](x-ads.md). X's Ads MCP doc says X keeps a single live OAuth grant per (app, user), so signing in to X and X Ads from the same app can revoke each other's tokens. Use a separate app for each.

## Sources

- https://docs.x.com/resources/fundamentals/developer-apps (checked 2026-10-25)
- https://docs.x.com/resources/fundamentals/developer-portal
- https://docs.x.com/resources/fundamentals/authentication/oauth-2-0/authorization-code
- https://docs.x.com/resources/fundamentals/authentication/oauth-2-0/user-access-token
- https://docs.x.com/x-api/getting-started/pricing
- https://docs.x.com/x-api/fundamentals/response-codes-and-errors
- https://docs.x.com/tools/mcp
- https://api.x.com/.well-known/oauth-authorization-server (live, 2026-10-25)
- https://api.x.com/.well-known/oauth-protected-resource/mcp (live, 2026-10-25)
- https://explainx.ai/blog/x-hosted-mcp-servers-cursor-claude-grok-api-2026 (third party, for the Bearer and xurl claims)
