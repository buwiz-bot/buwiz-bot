# Shop connector setup (Shop app, accounts.shop.app)

Checked against Shopify's docs and live endpoints on 2026-10-25. Items marked **(unconfirmed)** could not be verified.

**Short version:** the Shop plugin works **without signing in** for product search, carts and checkout creation. Signing in (OAuth against accounts.shop.app) adds your Shop account features, and I found **no documented self-service way to register a client that this sign-in will accept**. Treat sign-in as blocked until Shopify says otherwise.

## What it unlocks

Shopify's consumer Shop MCP server: search and compare products across Shopify stores, look up products, create carts and checkouts, and (signed in) read saved addresses and payment preferences, complete checkout with Shop Pay, search orders, and create spend requests so the agent can pay at non-Shop-Pay stores with a single-use card after you approve.

| Item | Value (live, 2026-10-25) |
|---|---|
| MCP server | `https://mcp.shop.com` (GET redirects to `/skill.md`; MCP is POST) |
| Protected-resource metadata | `https://mcp.shop.com/.well-known/oauth-protected-resource` |
| Authorization server | `https://accounts.shop.app` |
| Authorize URL | `https://accounts.shop.app/oauth/authorize` |
| Token URL | `https://accounts.shop.app/oauth/token` |
| Other endpoints | device `https://accounts.shop.app/oauth/device`, revoke `/oauth/revoke`, userinfo, jwks |
| PKCE | S256 |
| Token endpoint client auth | `client_secret_basic`, `client_secret_post`, `none` |
| Grant types | `authorization_code`, `refresh_token`, `device_code`, CIBA, token-exchange |
| Registration endpoint | none. `client_id_metadata_document_supported: true` is advertised (see below). |
| Resource scope | `personal_agent` (the server's `WWW-Authenticate: Bearer ... scope="personal_agent"` on a signed-in-only call) |
| Server scopes list | `openid email profile agent:access personal_agent:orders pay:wallet:read pay:wallet:payment_request:approve pay:addresses:read orders favorites ...` (full list in the metadata) |

Protocol note for the Rox developers: the server accepted only MCP protocol version `2026-07-28` in my probe (an `initialize` with `2025-06-18` returned error -32022 "Unsupported protocol version"), requires an `Mcp-Method` header, and wants `io.modelcontextprotocol/protocolVersion` and `clientCapabilities` in `_meta`. A Rox MCP client that cannot speak that revision will fail even before sign-in.

## Prerequisites

- Anonymous use: nothing. Confirmed live: a `search_catalog` call without any token returned products; `get_account` returned `401` with the `personal_agent` challenge. Shopify's README says "Sign-in is optional: search, carts, and checkouts work without it."
- Signed-in use: a Shop account (free; create at https://shop.com or the Shop app) and an OAuth client that accounts.shop.app accepts (see Steps). Cost: none documented; purchases are charged to your payment method at the merchant. Any card or Shop Pay purchase is real money; the server's own instructions require your explicit approval before completing a checkout.
- Approval/waiting time: unknown (see Steps).

## Steps

There is no confirmed path. What I found:

1. **Shopify's documented route for a developer-registered Shop OAuth client** is the Dev Dashboard: sign in with a Partner account, create an app, then Versions -> Release -> Access -> "Select scopes" -> **Shop App**, add redirect URLs, Release; Client ID and Client secret appear on the app's Settings page (https://shopify.dev/docs/api/shop/guides/creating-a-client). Limits stated there: redirect URLs must be fully-qualified `https` URLs ("Only fully-qualified `https` URLs are supported"), and the only scope documented is `shop_app:oauth` (basic profile, for Sign in with Shop). **The agent scopes the Shop MCP server uses (`personal_agent`, `agent:access`, ...) are not documented as available to such clients.**
2. A developer reported on 2026-10-24, using a Dev Dashboard client with `openid personal_agent` and resource `https://mcp.shop.com`, that Shop "redirects to my callback with `access_denied` before displaying a consent screen" and asked whether the client needs extra eligibility. No reply at the time I read it: https://community.shopify.dev/t/shop-mcp-oauth-returns-access-denied-before-showing-the-consent-screen/39005
3. My own read-only check: an authorize request with an unregistered client ID returned HTTP 403 (Shop's "authorize failed" page), so unknown clients are refused rather than auto-registered.
4. Rox's callback `http://127.0.0.1:8787/oauth/callback/shop` is `http`, which rule 1 excludes. Even with a valid client Rox would need an https relay, or Shopify would have to allow loopback. Unconfirmed whether Shopify makes an exception for native/loopback clients.

If you want to try anyway: create the Dev Dashboard app as in step 1 (app name `Rox`), add the callback, copy Client ID/Secret, and see whether accounts.shop.app accepts `personal_agent`. Expect the `access_denied` in step 2 and ask Shopify for agent-client enablement (the developer-forum thread above is the only public channel I found; a dedicated contact is **(unconfirmed)**). Whether the client ID can instead be a Client ID Metadata Document URL (CIMD, which the server advertises support for) is **(unconfirmed)**, and I did not test it.

## What to paste into Rox

**Connect apps -> Shop -> settings -> Authenticate** (only after a client exists)

| Rox field | Value |
|---|---|
| Client ID | the Shop/Dev Dashboard client ID that accounts.shop.app accepts (none confirmed) |
| Client Secret | the Dev Dashboard Client secret if Rox's field requires it; a public client can use `none` |
| Callback URL (shown by Rox, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/shop` (confirm in Rox's plugin page; Shopify requires https for Dev Dashboard clients) |

For anonymous use, leave the fields empty and do not click Authenticate, if Rox lets the plugin run without it **(unconfirmed)**.

## Scopes requested

The catalog entry pins no scopes and no client ID (the plugin has no `auth` block). The server's challenge asks for `personal_agent`. The forum reporter above requested `openid personal_agent`. Note that `personal_agent` appears in the MCP server's metadata but not in the authorization server's `scopes_supported` (which lists `agent:access`, `personal_agent:orders` and others), so the exact scope string Rox should send is **(unconfirmed)**.

## Alternative: token

Shopify's `shop` CLI (https://github.com/Shopify/shop, `npm install --global @shopify/shop@latest`) signs in with the OAuth **device flow** (`shop account login`, approve at https://accounts.shop.app/device) using Shopify's own client, and its README says a backend that holds a Shop token for this resource "can hand it to the CLI as `SHOP_ACCESS_TOKEN`; the CLI sends it unchanged". That implies the server accepts `Authorization: Bearer <Shop token>`. Where the CLI stores its token, its lifetime, and whether it may be reused in another client are **(unconfirmed)**; reusing it may breach Shopify's terms for that client. It needs a custom-header option in Rox.

## Verification

1. Without sign-in: ask Rox "Find three rain jackets under 100 USD that ship to the US." The answer should list products with store links.
2. After sign-in: "What is my Shop account's saved delivery address?" (calls `get_account`) or "Show my in-progress orders".
3. Provider side: Shop app -> your profile/settings (connected apps/agents list; exact location **(unconfirmed)**).

## Troubleshooting

- `access_denied` before any consent screen: the client is not enabled for agent scopes (the forum case above). Not fixable from your side.
- HTTP 403 "authorize failed" page: unknown/unregistered client ID.
- Redirect problems: Dev Dashboard accepts only `https` redirect URLs; Rox's loopback `http` URL will be refused or not saveable.
- `401` on `get_account`/orders while search works: expected when not signed in.
- Errors about protocol version or `Mcp-Method`: the Rox MCP client revision (see Protocol note).
- Never approve a purchase you did not request; the assistant must show merchant, items and total first.

## Sources

- https://mcp.shop.com/.well-known/oauth-protected-resource (live)
- https://accounts.shop.app/.well-known/oauth-authorization-server (live)
- https://mcp.shop.com/skill.md (server's agent instructions)
- https://github.com/Shopify/shop (README: optional sign-in, device flow, SHOP_ACCESS_TOKEN) and https://github.com/Shopify/shop/tree/main/plugins/shop (`mcp.json`: no auth block)
- https://shopify.dev/docs/api/shop/guides/creating-a-client
- https://shopify.dev/docs/api/shop/guides/use-cases/sign-in
- https://community.shopify.dev/t/shop-mcp-oauth-returns-access-denied-before-showing-the-consent-screen/39005
