# Alpaca Broker connector setup (alpaca-broker and alpaca-broker-sandbox)

Checked against Alpaca's docs, the `alpacahq/agentic` repo and live endpoints on 2026-10-25. Anything marked **(unconfirmed)** could not be verified.

One file covers both connector ids: `alpaca-broker` (live) and `alpaca-broker-sandbox` (sandbox). Unlike the Trading plugin, they use **separate authorization servers and separate OAuth clients**.

## What it unlocks

Rox connects to Alpaca's hosted Broker API MCP (beta): account operations for a broker partner's end customers. Per Alpaca, sandbox is used for writes, account creation, orders and funding; production is read-only for now.

| Item | Live (`alpaca-broker`) | Sandbox (`alpaca-broker-sandbox`) |
|---|---|---|
| MCP server | `https://broker-api.alpaca.markets/mcp` | `https://broker-api.sandbox.alpaca.markets/mcp` |
| Authorization server | `https://authx.alpaca.markets/v1` | `https://authx.sandbox.alpaca.markets/v1` |
| Authorize URL | `.../v1/oauth2/authorize` (hands off to `https://broker-app.alpaca.markets/authorize`) | same pattern on the sandbox host |
| Token URL | `.../v1/oauth2/token` | same pattern |
| PKCE / auth methods | S256; `none`, `client_secret_post`, `private_key_jwt` | same |
| DCR registration endpoint | none | none |
| CIMD | supported | supported (sandbox host metadata also says `client_id_metadata_document_supported: true`) |
| Catalog's pinned client ID | `PCBXWA7PCN3S6662PZK44KLURJ` (Cursor's) | `PCKU56KZNL2JHDU2BFNNNU4KVZ` (Cursor's) |

Probed live: `https://broker-api.alpaca.markets/.well-known/oauth-protected-resource/mcp` returns `scopes_supported: []`, `bearer_methods_supported: ["header"]`.

## Prerequisites (this is the hard part)

- You must be an approved **Alpaca broker partner** with access to the **Broker Dashboard**. Alpaca's launch post (2026-07-20) says access is "limited to only approved partner super users". To get in: your Alpaca Customer Success Manager / Sales, or `https://alpaca.markets/support`.
- The person who approves the consent request needs the `manage_keys` entitlement, which only the **Superuser** role has. Only Broker Dashboard super users can authorize and use it during beta.
- If your correspondent uses IP filters, the hosted connection does not work yet.
- An individual (non-partner) trading account cannot use this plugin; use `alpaca-trading` (see `alpaca-trading.md`).
- Cost and review time: **(unconfirmed)**, not stated in the docs.

## Does Rox need its own app?

Yes, in the sense that the pinned IDs are Cursor's and will not accept Rox's loopback callback. Each endpoint "uses its own OAuth client", and live and sandbox credentials are not interchangeable (per Alpaca's Broker MCP page). There is no self-service app page. Routes:

1. **CIMD (no Alpaca approval of the client itself):** Alpaca's authx accepts CIMD. Verified 2026-10-25 by read-only GETs: Claude Code's CIMD client with `resource=https://broker-api.alpaca.markets/mcp` on the live host and `resource=https://broker-api.sandbox.alpaca.markets/mcp` on the sandbox host both return 302 to `broker-app.alpaca.markets/authorize` (a login step). You still need to be a partner super user to complete consent.
2. **Ask Alpaca to register Rox** as an OAuth client per environment (your CSM, or an issue on `https://github.com/alpacahq/agentic/issues`).

CIMD steps: host one JSON file per environment at an https URL with a non-root path (the `client_id` in the file must equal the URL; use the sandbox one first):

```json
{
  "client_id": "https://YOURNAME.github.io/rox/alpaca-broker-sandbox-client.json",
  "client_name": "Rox",
  "client_uri": "https://YOURNAME.github.io/rox/",
  "redirect_uris": ["http://127.0.0.1/oauth/callback/alpaca-broker-sandbox"],
  "grant_types": ["authorization_code", "refresh_token"],
  "response_types": ["code"],
  "token_endpoint_auth_method": "none"
}
```

For live make a second file with `.../alpaca-broker-client.json` and redirect `http://127.0.0.1/oauth/callback/alpaca-broker`. Callback URLs to confirm in Rox's plugin page (Rox's exact string is not final): `http://127.0.0.1:8787/oauth/callback/alpaca-broker` and `http://127.0.0.1:8787/oauth/callback/alpaca-broker-sandbox`. Loopback matching ignored the port in testing; the path must be listed. Whether Rox's Client ID field accepts an https URL: **(unconfirmed)**. Whether Alpaca will let a CIMD client through the final Broker Dashboard consent for a given partner: **(unconfirmed)**.

App name if a form asks: `Rox`. Client type: public (no secret).

## What to paste into Rox

Open **Connect apps -> Alpaca Broker API -> settings** (sandbox is its own entry):

| Rox field | Value |
|---|---|
| Client ID | the https URL of the matching JSON file (or the ID Alpaca issues) |
| Client Secret | empty (public client) |
| Callback URL (read-only) | `http://127.0.0.1:8787/oauth/callback/alpaca-broker` (sandbox: `.../alpaca-broker-sandbox`) |

Then **Authenticate**; a super user approves the consent request in the Broker Dashboard.

## Scopes requested

None pinned; the Broker MCP page does not mention scopes, redirect URIs or token details. Access is governed by the approving super user.

## Alternative: API key

Alpaca states the connection is OAuth only and "never paste API keys or secrets into chat"; no API key or basic auth alternative is documented for the hosted Broker MCP. Broker API HTTP basic keys exist for the REST API but are not documented for `/mcp`. **(unverified; assume not)**

## Verification

Authorize sandbox first and ask Rox: "List the accounts in my Alpaca broker sandbox." Review/revoke authorizations under **Team Management** in the Broker Dashboard.

## Troubleshooting

- Authorize returns 400 with the pinned Cursor ID: expected (redirect not registered); use your own client.
- Consent never completes: the approving user lacks the Superuser role / `manage_keys`.
- Hosted connection fails but credentials are right: correspondent IP filters.
- Using sandbox credentials against live (or the reverse): not interchangeable; authorize each connector separately.

## Sources

- https://docs.alpaca.markets/us/docs/broker-mcp-server and https://docs.alpaca.markets/us/docs/broker-api-mcp.md
- https://alpaca.markets/blog/alpaca-launches-mcp-servers-in-public-beta-for-broker-partners/
- https://github.com/alpacahq/agentic (README)
- https://broker-api.alpaca.markets/.well-known/oauth-protected-resource/mcp (live)
- https://authx.alpaca.markets/v1/.well-known/oauth-authorization-server and https://authx.sandbox.alpaca.markets/v1/.well-known/openid-configuration (live)
