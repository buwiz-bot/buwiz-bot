# Docusign connector setup

Checked against Docusign's developer docs (page data fetched 2026-10-25) and live endpoints on 2026-10-25. Anything marked **(unconfirmed)** must be checked on screen.

## What it unlocks

Buwiz Bot connects to Docusign's hosted MCP server. The assistant can work with eSignature envelopes and templates, users and account info, and (with an IAM subscription) Workflow Builder, Agreement Manager and CLM tools.

Docusign has no dynamic client registration. You create your own **Integration Key** (client ID) plus **Secret Key** and paste both into Buwiz Bot. Docusign states the server only supports **Confidential Authorization Code Grant** (and JWT Grant) tokens, so the secret is mandatory.

| Item | Demo (developer account) | Production |
|---|---|---|
| MCP URL | `https://mcp-d.docusign.com/mcp` | `https://mcp.docusign.com/mcp` (the catalog default) |
| Issuer | `https://account-d.docusign.com` | `https://account.docusign.com` |
| Authorize URL | `https://account-d.docusign.com/oauth/auth` | `https://account.docusign.com/oauth/auth` |
| Token URL | `https://account-d.docusign.com/oauth/token` | `https://account.docusign.com/oauth/token` |

Both read from `/.well-known/oauth-authorization-server` on the MCP host (confirmed live). PKCE: S256. Grants: `authorization_code`, `refresh_token`. `token_endpoint_auth_methods_supported` and `registration_endpoint` are not advertised.

Live-probe caveat: an unauthenticated request to `.../mcp` returns **403 "RBAC: access denied"** (no 401 and no `WWW-Authenticate`), and `/.well-known/oauth-protected-resource/mcp` also returns 403. Only the root-level `/.well-known/oauth-protected-resource` (and `/oauth-authorization-server`) answer 200. A client that insists on path-based discovery may fail; Buwiz Bot may need to fall back to the root metadata.

## Prerequisites

- Developer (demo) route: a free Docusign developer account (https://www.docusign.com/developers/sandbox). Docusign recommends this for first-time users and testing.
- Production route (custom connector, i.e. what Buwiz Bot is): Docusign's docs say you must **submit a form with your integration key and email to allow-list your production Docusign account**: https://docusign.co1.qualtrics.com/jfe/form/SV_7ZGKu320DhPkPrw. No waiting time is documented (**unconfirmed**). Other third-party pages say no approval is needed; the official page above is more specific, so follow it.
- Production also needs the integration key promoted via **Go-Live**: a paid production account, with administrator access, and the account must have access to each API used. Go-Live copies the key to production; **secrets and redirect URIs are not copied**, so set them again in production. Go-Live status "Declined" means manual review, "typically completed within 24-48 hours" (Docusign docs). Go-Live is in the Developer Console and is described as open beta. Choose classification **Private custom integration** (internal use).
- Docusign's docs say production users should prefer the official connectors (Claude, ChatGPT, etc.); a custom connector on the production URL is "technically possible" but "not recommended". Buwiz Bot is a custom client, so this is the only route.
- Workflow Builder and Agreement Manager tools need an active Docusign **IAM** subscription (paid).
- Demo accounts and production accounts have different rate limits.

## Steps (demo / developer account)

1. Sign in to your developer account and open **Settings -> Apps and Keys** (Docusign's own blog uses "the Apps and Keys page").
2. Select **Add App and Integration Key**, name it `Buwiz Bot`.
3. Copy the **Integration Key**.
4. Under Authentication, select **Add Secret Key** and copy the secret immediately (shown once).
5. Make sure the app uses Authorization Code Grant with a secret (confidential). The exact label for this option is **(unconfirmed)**; Docusign's docs say JWT, Implicit and Public Authorization Code Grant are not supported.
6. Under **Redirect URIs**, add `http://127.0.0.1:8787/oauth/callback/docusign` (confirm in Buwiz Bot's plugin page). Docusign's redirect-URI rules for loopback are **unconfirmed** for `127.0.0.1`; community examples register `http://localhost:<port>/...`. If `127.0.0.1` is rejected, try `http://localhost:8787/oauth/callback/docusign` and tell Buwiz Bot to use it **(if Buwiz Bot allows)**.
7. Save.
8. In Buwiz Bot's plugin settings, set the MCP URL to `https://mcp-d.docusign.com/mcp` if Buwiz Bot lets you override it **(unconfirmed)**; the catalog default is the production URL.

## Steps (production)

1. Create the key in the developer account as above, then complete Go-Live from the Developer Console.
2. In the production account's **Apps and Keys**, re-add the secret and the redirect URI (not copied).
3. Submit the allow-list form with the Integration Key and your email (link above).
4. Use the production MCP URL `https://mcp.docusign.com/mcp`.

## What to paste into Buwiz Bot

Open **Connect apps -> Docusign** -> settings:

| Buwiz Bot field | Value |
|---|---|
| Client ID (Docusign Integration Key) | the Integration Key |
| Client Secret (Docusign Secret Key) | the Secret Key |
| Callback URL (read-only) | should equal `http://127.0.0.1:8787/oauth/callback/docusign` |

Then click **Authenticate**, sign in to Docusign and select **Allow Access**.

## Scopes requested

The catalog pins none. Server metadata lists: `signature`, `adm_store_unified_repo_read`, `aow_manage`, `cors`, `spring_read`, `spring_write`. Per Docusign's tool tables: eSignature tools need `signature`; Workflow Builder needs `adm_store_unified_repo_read`, `aow_manage`, `signature`; CLM needs `spring_read`, `spring_write`; Agreement Manager needs `adm_store_unified_repo_read` (the page also lists `models_read`, which the server does not advertise).

## Alternative: access token

The docs say JWT Grant tokens work too (they expire after 8 hours, no refresh), using a service account. That needs a JWT app, user consent and key pair, plus a Buwiz Bot option to send a static `Authorization: Bearer` header **(unverified for Buwiz Bot)**. Not recommended.

## Verification

1. Buwiz Bot shows Docusign connected.
2. Ask: "Who am I in Docusign?" (the user-profile tool) then "List my last 5 envelopes."
3. In the Docusign admin API dashboard, check the call appears.

## Troubleshooting

- **invalid_request / redirect URI not registered**: the URI must be registered on the key in the same environment (demo vs production) you authorize against.
- **403 RBAC: access denied from the MCP URL**: for production, your account/key has not been allow-listed (form above); for demo, check you are using `mcp-d` with a demo key.
- **Wrong environment**: demo keys do not work on `account.docusign.com`, and production keys do not work on `account-d`.
- **Tools return an upgrade prompt**: Workflow Builder and Agreement Manager need IAM.
- **invalid_client**: wrong secret, or the secret was not saved; add a new one.
- **Buwiz Bot says no 401 / discovery fails**: see the probe caveat above.

## Sources

- https://developers.docusign.com/platform/mcp-server/ (JS-rendered; text read from its page-data JSON, 2026-10-25)
- https://developers.docusign.com/platform/go-live/ (same method)
- https://github.com/cursor/plugins/tree/main/third_party/docusign
- https://www.docusign.com/blog/developers/claude-docusign-mcp-connector-guide
- https://mcp.docusign.com/.well-known/oauth-authorization-server and https://mcp-d.docusign.com/.well-known/oauth-authorization-server (live)
- https://account.docusign.com/.well-known/openid-configuration (live)
