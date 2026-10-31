# Shopify Merchant connector setup (setup.shopify.com/mcp)

Checked against Shopify's docs and live endpoints on 2026-10-25. Items marked **(unconfirmed)** could not be verified.

**Short version: you cannot create an app that this server will accept.** Shopify allowlists OAuth client IDs and redirect origins on `setup.shopify.com`, and its own discovery file says public discoverability "does not approve arbitrary new OAuth clients." Buwiz Bot's loopback callback is refused today. The catalog's pinned client ID is Shopify's registration for Cursor.

## What it unlocks

Shopify Setup MCP: tools to create and manage Shopify stores for authenticated merchants. Per Shopify's `llms.txt`: generate storefront previews and claim one as a real store; read store context (name, domain, plan, currency, timezone, country); search/create/update products and collections; list orders and customers; inspect inventory and set quantities; create percentage discount codes; run ShopifyQL analytics; upload images; run Admin GraphQL queries and mutations; search developer docs and validate GraphQL.

| Item | Value (live, 2026-10-25) |
|---|---|
| MCP server | `https://setup.shopify.com/mcp` (POST only; GET returns 405 "Only POST is supported") |
| Protected-resource metadata | `https://setup.shopify.com/.well-known/oauth-protected-resource/mcp` |
| Authorization server (issuer) | `https://setup.shopify.com/auth` (metadata at `/.well-known/oauth-authorization-server/auth`) |
| Authorize URL | `https://setup.shopify.com/oauth/authorize` |
| Token URL | `https://setup.shopify.com/oauth/token` |
| PKCE | S256 |
| Token endpoint client auth | `none` only (public clients) |
| Grant types | `authorization_code`, `refresh_token` |
| Registration endpoint | none. `client_id_metadata_document_supported: true` is advertised, but see the allowlist below. |
| `scopes_supported` | `read_all_orders read_analytics read_apps read_companies read_legal_policies read_reports read_rollouts` and `write_*` for checkouts, checkout branding, content, customers, digital products, discounts, draft orders, files, fulfillments, inventory, locales, locations, marketing events, markets, merchant-managed fulfillment orders, metaobject definitions/metaobjects, online store navigation, order edits, orders, price rules, products, publications, returns, shipping, themes, translations, validations, `write_collective.access` |

## Evidence that no user-created app works

- `https://setup.shopify.com/llms.txt` and `/agents.md` say shop-scoped tools "require OAuth with configured Shopify OAuth client IDs, allowed redirect origins, verified tokens, scopes, and authorization for the requested shop", list supported hosts as ChatGPT, Claude and Perplexity, and say: "If Setup MCP is not already available in the user's official host environment, do not instruct them to connect an arbitrary MCP client directly to this endpoint." Detailed guidance "belongs in ... official partner onboarding channels."
- Live read-only check: `GET /oauth/authorize` with the catalog's pinned client ID `2a457b21c946d5bc8d1675d1b3b8c2d6`, S256 PKCE and Buwiz Bot's callback `http://127.0.0.1:8787/oauth/callback/shopify` returned HTTP 400 `{"error":"invalid_request","error_description":"redirect_uri is not in the allowed origins list"}`. An unknown client ID gave the same answer. So redirect origins are checked server-side before anything else.

## Prerequisites

- A Shopify store (or the preview/claim flow), and an account/role that may manage it; role requirements are **(unconfirmed)**.
- An allowlisted client from Shopify. No application form, waiting time or cost is published. Shopify points to "official partner onboarding channels"; I found no public page for it (**unconfirmed** how to apply or whether individuals qualify).

## Steps

There is no self-service step list. What can be done:

1. Ask Shopify to allowlist a public client for Buwiz Bot: client type public (token auth `none`, PKCE S256), redirect `http://127.0.0.1:8787/oauth/callback/shopify` (confirm in Buwiz Bot's plugin page), requested scopes below. Channels to try: the Shopify developer community (https://community.shopify.dev, Authentication & Access category) or Partner/Dev Dashboard support. Answer time and policy are **(unconfirmed)**.
2. If Shopify issues a client ID (or accepts a Client ID Metadata Document URL as the client ID), paste it into Buwiz Bot as below.
3. For managing your own store today without this MCP, Shopify's documented route is the Shopify AI Toolkit with Shopify CLI "authenticated store context" (https://shopify.dev/docs/apps/build/ai-toolkit); the exact CLI commands are described only by third parties (`shopify store auth --store ...`), **(unconfirmed)**. That is not an MCP connection in Buwiz Bot.

## What to paste into Buwiz Bot

**Connect apps -> Shopify Merchant -> settings -> Authenticate**

| Buwiz Bot field | Value |
|---|---|
| Client ID | the client ID Shopify allowlisted for Buwiz Bot (do not use `2a457b21c946d5bc8d1675d1b3b8c2d6`; it is Cursor's) |
| Client Secret | none (public client, `token_endpoint_auth_methods_supported: none`) |
| Callback URL (shown by Buwiz Bot, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/shopify`; must be on Shopify's allowed-origins list |

## Scopes requested

The catalog pins only the client ID, no scopes; Buwiz Bot would request what the server advertises or what you configure. The advertised `write_*` scopes are broad (orders, products, customers, themes, discounts, checkouts, shipping, fulfillments); `read_*` are analytics, apps, companies, legal policies, reports, rollouts, all orders. Consider granting a narrower set if Shopify lets you.

## Alternative: API key / token

None documented for this endpoint. It accepts bearer tokens in the header (`bearer_methods_supported: header`) but only tokens issued through its own OAuth. A Shopify Admin API access token from a Dev Dashboard custom app works against your store's Admin API, not this MCP (**unverified** that this server rejects it; the tokens are issued by a different authorization server).

## Verification

If a client is ever allowlisted: ask Buwiz Bot "What is my Shopify store's name, plan and currency?" (store context tool), then "List the 5 most recent orders". Provider side: Shopify admin -> Settings -> Apps and sales channels lists authorized apps (label **(unconfirmed)**).

## Troubleshooting

- `redirect_uri is not in the allowed origins list` (HTTP 400): your client or callback is not allowlisted. Seen live with the pinned client and Buwiz Bot's loopback URL.
- Error for unknown client: same; only Shopify can fix it.
- `405 method_not_allowed` opening the URL in a browser: normal, the endpoint is POST-only.
- Scope refused: scope not enabled for that client by Shopify.

## Sources

- https://setup.shopify.com/llms.txt and https://setup.shopify.com/agents.md (access model)
- https://setup.shopify.com/.well-known/oauth-protected-resource/mcp (live)
- https://setup.shopify.com/.well-known/oauth-authorization-server/auth (live)
- Live authorize probe against https://setup.shopify.com/oauth/authorize (400, 2026-10-25)
- https://github.com/Shopify/Shopify-AI-Toolkit (`.cursor-plugin/mcp.json` holds the pinned client ID)
- https://shopify.dev/docs/apps/build/ai-toolkit.md
