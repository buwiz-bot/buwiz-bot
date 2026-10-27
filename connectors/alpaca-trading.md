# Alpaca Trading connector setup (alpaca-trading and alpaca-trading-paper)

Checked against Alpaca's docs, the `alpacahq/agentic` repo and live endpoints on 2026-10-25. Anything marked **(unconfirmed)** could not be verified and must be checked on screen or with Alpaca.

This one file covers both connector ids: `alpaca-trading` (live) and `alpaca-trading-paper` (paper). They share one OAuth client and one authorization server.

## What it unlocks

Rox connects to Alpaca's hosted Trading + Market Data MCP servers: account, positions, orders, portfolio and market data. Live trading places real orders.

| Item | Value |
|---|---|
| MCP server, live (`alpaca-trading`) | `https://api.alpaca.markets/mcp` |
| MCP server, paper (`alpaca-trading-paper`) | `https://paper-api.alpaca.markets/mcp` |
| Authorization server (issuer) | `https://authx.alpaca.markets/v1` |
| Authorize URL | `https://authx.alpaca.markets/v1/oauth2/authorize` (then hands off to `https://app.alpaca.markets/authorize` to sign in and consent) |
| Token URL | `https://authx.alpaca.markets/v1/oauth2/token` |
| PKCE | S256 only |
| Token endpoint client auth | `none`, `client_secret_post`, `private_key_jwt` |
| Grant types | `authorization_code`, `refresh_token`, `client_credentials`, `urn:ietf:params:oauth:grant-type:jwt-bearer` |
| Registration endpoint (DCR) | none |
| Client ID Metadata Documents (CIMD) | **supported** (`client_id_metadata_document_supported: true`) |
| Scopes | MCP resource `scopes_supported` is empty; the server lists `openid offline_access profile email`. The catalog pins no scopes. |

Probed at `https://api.alpaca.markets/.well-known/oauth-protected-resource/mcp` (and the paper host) and `https://authx.alpaca.markets/v1/.well-known/oauth-authorization-server`.

## Does Rox need its own app? There is no self-service app page

Alpaca has **no dashboard page where you create an OAuth app for the hosted MCP**. The pinned client ID `PCBXWA7PCN3S6662PZK44KLURJ` is Alpaca's own app for Cursor. Live probe: with that client ID, `redirect_uri=http://127.0.0.1:8787/...` returns HTTP 400, while `cursor://anysphere.cursor-mcp/oauth/callback` is accepted. The same ID is a different value for Codex (`PCIEJZTPCQEBUBAINMQOGDHF7I`); Claude Code uses no client ID at all.

Claude Code works because Alpaca accepts **CIMD**: the `client_id` is an https URL of a JSON file you host, and Alpaca reads the redirect URIs from it. Verified live on 2026-10-25 (read-only GET to the authorize endpoint): Claude Code's CIMD client (`https://claude.ai/oauth/claude-code-client-metadata`, which lists `http://127.0.0.1/callback`) is accepted from `http://127.0.0.1:8787/callback` (any loopback port) for all four MCP resources (302 to `app.alpaca.markets/authorize`), and rejected with 400 when the path is not in the document. So a third-party desktop app can work with no Alpaca approval, if it is its own CIMD client.

The old **Alpaca Connect** OAuth apps (dashboard "Alpaca Connect" tab, `app.alpaca.markets/oauth/authorize`) are a different system. A developer on `alpacahq/agentic` issue #5 states his Connect app "does not establish compatibility with the hosted MCP OAuth service". Do not use them here.

So there are three routes:

1. **CIMD (recommended, no waiting):** host a small JSON file and use its URL as the Client ID. Needs Rox's Client ID field to accept an https URL and send it unchanged **(unconfirmed: check Rox's plugin page; if it only takes opaque IDs, use route 2 or 3)**.
2. **Ask Alpaca to register a client for Rox:** open an issue at `https://github.com/alpacahq/agentic/issues` (issue #5, "Client support request: Minara", is the template; it was still open with no reply when checked) or write to `support@alpaca.markets`. Ask for a public client (auth method `none`), PKCE S256, loopback redirect URIs for the four connector ids below. Approval time unknown **(unconfirmed)**.
3. **Skip the hosted server:** run Alpaca's local `alpaca-mcp-server` with API keys (see "Alternative"). That is a stdio server, not this connector.

## Prerequisites

- An Alpaca account. Paper trading is for paper accounts; live needs an approved, funded brokerage account. Costs and approval times for account opening: **(unconfirmed, not covered by the pages checked)**.
- A public https host for one small JSON file (GitHub Pages, your own domain, any static host) for route 1. The URL must be https and have a non-root path; the `client_id` inside the file must equal that URL exactly.
- Nothing else is charged by the MCP server itself in the docs checked. **(unconfirmed)**

## Steps (route 1, CIMD)

1. Pick the URL where you will host the file, for example `https://YOURNAME.github.io/rox/alpaca-trading-client.json`.
2. Create the file with this content (replace the URL in `client_id`; it must match where you host it). Loopback redirect URIs are matched without the port, so the portless form is the one Alpaca accepted in testing; the paths are examples of Rox's callback form and must match what Rox's plugin page shows:

   ```json
   {
     "client_id": "https://YOURNAME.github.io/rox/alpaca-trading-client.json",
     "client_name": "Rox",
     "client_uri": "https://YOURNAME.github.io/rox/",
     "redirect_uris": [
       "http://127.0.0.1/oauth/callback/alpaca-trading",
       "http://127.0.0.1/oauth/callback/alpaca-trading-paper"
     ],
     "grant_types": ["authorization_code", "refresh_token"],
     "response_types": ["code"],
     "token_endpoint_auth_method": "none"
   }
   ```

   Callback URL form per connector id (**confirm in Rox's plugin page**, Rox's exact string is not final): `http://127.0.0.1:8787/oauth/callback/alpaca-trading` and `http://127.0.0.1:8787/oauth/callback/alpaca-trading-paper`. If Rox's page shows a different path, use that path in the file.
3. Publish it and check that `curl -s <your URL>` returns the JSON.
4. In Rox, paste the URL as Client ID (below) and click Authenticate. Sign in at Alpaca, pick the account, approve.
5. Repeat Authenticate for the paper connector (same URL as Client ID).

App name to use if a form ever asks: `Rox`. Client type: public (no secret).

## What to paste into Rox

Open **Connect apps -> Alpaca Trading API -> settings** (and the same for the paper entry):

| Rox field | Value |
|---|---|
| Client ID | route 1: the https URL of your JSON file; route 2: the ID Alpaca issues |
| Client Secret | leave empty (public client) |
| Callback URL (read-only, shown by Rox) | must equal `http://127.0.0.1:8787/oauth/callback/alpaca-trading` (paper: `.../alpaca-trading-paper`) |

Then click **Authenticate**.

## Scopes requested

None pinned in the catalog. Alpaca's server advertises none for the MCP resource; send no `scope`, or `openid offline_access` if Rox's flow wants a refresh token **(unconfirmed which Alpaca needs for refresh)**. Permission is whatever the account and consent screen grant, live or paper per the connector you authorize.

## Alternative: API key

- The hosted `/mcp` endpoints advertise `bearer_methods_supported: ["header"]` and tokens come from `authx`. Alpaca's docs do not say an Alpaca API key ID/secret (or `APCA-API-KEY-ID` headers) is accepted on the hosted endpoints. **(unverified; assume not)**
- Documented API-key route is the local open-source server: `uvx alpaca-mcp-server` with env vars `ALPACA_API_KEY`, `ALPACA_SECRET_KEY`, `ALPACA_PAPER_TRADE` (default `true`; set `false` for live). Keys come from the Alpaca dashboard. This is a local stdio process, not the hosted connector above.

## Verification

1. After Authenticate, Rox shows the connector connected.
2. Ask Rox: "On Alpaca paper, what is my account status and buying power?" then "Show my open positions."
3. For live, ask only read questions first; the live connector can place real orders.

## Troubleshooting

- **400 `invalid_request` at authorize, before any login:** the `redirect_uri` is not registered for that client. With the pinned Cursor ID this is expected. With CIMD, the redirect path in your JSON must equal the path Rox sends.
- **400 with `fields: code_challenge`:** the client did not send a valid S256 challenge (PKCE required).
- **`invalid_target` / `invalid resource`:** the `resource` parameter must be one of the four MCP URLs exactly.
- **CIMD not accepted by Rox:** Rox's Client ID field may reject URLs; fall back to route 2.
- **Wrong environment:** paper and live are different MCP URLs; authorize each separately.
- **http callback:** `127.0.0.1` over http worked in testing for CIMD clients. `localhost` also worked for Claude Code's document (listed by `http://localhost/callback`).

## Sources

- https://github.com/alpacahq/agentic (README, checked 2026-10-25)
- https://github.com/alpacahq/agentic/issues/5 and /issues/6
- https://docs.alpaca.markets/us/docs/alpaca-mcp-server and https://docs.alpaca.markets/us/docs/trading-api-mcp.md
- https://docs.alpaca.markets/us/docs/about-connect-api.md (Connect apps, a different system)
- https://api.alpaca.markets/.well-known/oauth-protected-resource/mcp (live)
- https://authx.alpaca.markets/v1/.well-known/oauth-authorization-server (live)
- https://claude.ai/oauth/claude-code-client-metadata (example CIMD document)
