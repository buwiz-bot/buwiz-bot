# AtScale connector setup

Checked against AtScale's documentation on 2026-10-25. Items marked **(unconfirmed)** could not be verified. AtScale is self-hosted/per-tenant, so there is no public endpoint to probe.

## What it unlocks

The AtScale plugin connects Rox to your AtScale instance's MCP server: query governed business metrics, definitions and context from the semantic layer. The MCP server is described by AtScale as a **Public Preview** feature.

You do **not** create a new OAuth app on a vendor website. `atscale-mcp` is a built-in client in **your own** AtScale Identity Broker, and the catalog's `CLIENT_ID` is its name. It is a confidential client, so you copy its secret and add Rox's callback to its redirect list.

| Item | Value |
|---|---|
| MCP server | `https://<atscale_url>/mcp` (your instance; Rox's `ATSCALE_MCP_URL` must be this full URL including `/mcp`) |
| Authorization server | your AtScale Identity Broker (discovered from the MCP URL; authorize/token URLs not probeable without a tenant) **(unconfirmed)** |
| OAuth client | `atscale-mcp`, with a Client Secret |

## Prerequisites

- An AtScale deployment (container/Helm install) with the MCP server enabled. Enabling is done in the Helm values override file (`atscale-mcp` block with `enabled: true`, then `helm upgrade --install`), by whoever runs your AtScale cluster.
- Your account needs the `admin` or `application_admin` role in the Identity Broker to open Security > Clients.
- Rox must be able to reach your AtScale URL from your machine (VPN/ports). If the instance uses a self-signed certificate, the desktop app may reject TLS; AtScale's own examples set `NODE_TLS_REJECT_UNAUTHORIZED=0` for `mcp-remote`, which Rox does not do **(unconfirmed how Rox handles this)**.
- Cost: AtScale licence; Public Preview terms **(unconfirmed)**.

## Steps

1. Find your URL: it is the base URL you use for AtScale Design Center, plus `/mcp`, for example `https://atscale.example.com/mcp`. Ask your AtScale admin if unsure.
2. In **Design Center**, click **Security** in the left navigation (this opens the Identity Broker).
3. Click **Clients**, then select `atscale-mcp`.
4. Open the **Credentials** tab and copy the **Client Secret**. **Do not regenerate it**; AtScale warns this breaks existing connections.
5. Open the client's **Settings** tab, find **Valid redirect URIs** under **Access Settings**, click **Add valid redirect URIs**, enter `http://127.0.0.1:8787/oauth/callback/atscale` (confirm in Rox's plugin page), and click **Save**. AtScale's examples use patterns such as `http://example.com/*`; an exact URI should work, a wildcard is broader than needed. Whether the Identity Broker accepts `http` with `127.0.0.1` is **(unconfirmed)**; Keycloak-style brokers normally do.
6. Do not enable other redirect patterns you do not need; the hardening page recommends restricting redirects to the URLs your AI applications use.

## What to paste into Rox

Open **Connect apps -> AtScale** -> settings:

| Rox field | Value |
|---|---|
| ATSCALE_MCP_URL | `https://<atscale_url>/mcp` |
| Client ID | `atscale-mcp` (pinned by the plugin; leave as is) |
| ATSCALE_MCP_CLIENT_SECRET / Client Secret | the Client Secret from step 4 |
| Callback URL (shown by Rox, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/atscale` |

Then click **Authenticate** and sign in to AtScale in the browser.

## Scopes requested by the plugin

None declared in the catalog. Scopes are whatever the `atscale-mcp` client grants. **(unconfirmed)**

## Alternative: API token

AtScale documents a token route: in Design Center click the profile icon (top right) to open the **Account** panel, and in the **API Token** section click the generate icon. Use it as `Authorization: Bearer <token>` against `https://<atscale_url>/mcp`. Limits: token lifetime/rotation not documented **(unconfirmed)**; needs a custom-header option in Rox, otherwise use OAuth.

## Verification

Ask Rox: "List the AtScale semantic models/catalogs I can query" (read-only). Provider side: sign-in appears in the Identity Broker's sessions for the `atscale-mcp` client.

## Troubleshooting

- **Invalid redirect_uri**: the callback is not in the client's Valid redirect URIs, or differs by a character. Re-check step 5.
- **Invalid client credentials**: the secret was regenerated or mis-pasted; copy it again from the Credentials tab.
- **Connection fails before sign-in**: the MCP server is not enabled, the URL lacks `/mcp`, or a custom reverse proxy does not forward the MCP paths (AtScale's cluster route-mapping page lists four path prefixes to port 3003).
- **No permission to see Security > Clients**: you lack `admin`/`application_admin`.

## Sources

- https://documentation.atscale.com/container/connecting-with-ai/manage-mcp-server
- https://documentation.atscale.com/container/connecting-with-ai/manage-mcp-server/development-environment
- https://documentation.atscale.com/container/connecting-with-ai/manage-mcp-server/hardening
- https://documentation.atscale.com/container/connecting-with-ai/connect-ai-applications/claude
- https://github.com/AtScaleInc/atscale-cursor-plugin (README)
