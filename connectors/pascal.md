# Pascal connector setup (connector id `pascal-hosted`)

Checked against Pascal's docs and live endpoints on 2026-10-25. Items marked **(unconfirmed)** could not be verified.

**Short version: you cannot create your own OAuth app for Pascal.** Pascal registers OAuth clients itself and has switched dynamic registration off. The practical way to connect Buwiz Bot is a **Pascal API key** sent as `Authorization: Bearer`, which needs a custom-header option in Buwiz Bot. The alternative is to ask Pascal to register Buwiz Bot as an OAuth client.

## What it unlocks

Pascal is a free, open-source 3D building editor. The hosted MCP server lets an agent create, inspect, edit, validate and save 3D building projects (walls, rooms, doors, windows, roofs, stairs, furniture), measure, check collisions, read and upload project files. Changes land in the same project store as the browser editor at https://editor.pascal.app.

| Item | Value (live, 2026-10-25) |
|---|---|
| MCP server | `https://editor.pascal.app/api/mcp` (GET returns a JSON info card listing `auth.methods: ["oauth","api-key"]`) |
| Protected-resource metadata | `https://editor.pascal.app/.well-known/oauth-protected-resource` (path form `.../oauth-protected-resource/api/mcp` also works) |
| Authorization server | `https://editor.pascal.app/api/auth` |
| Authorize URL | `https://editor.pascal.app/api/auth/oauth2/authorize` |
| Token URL | `https://editor.pascal.app/api/auth/oauth2/token` (send `resource=https://editor.pascal.app/api/mcp`) |
| PKCE | S256 only |
| Token endpoint client auth | `none` only (public clients; no secrets) |
| `scopes_supported` | `offline_access openid profile scene:read scene:write` |
| Registration endpoint | none. Pascal's auth page: "Dynamic client registration is off, so an unknown client_id is refused." |
| Token lifetime | access tokens 600 seconds, refresh tokens rotate on every use (request `offline_access`) |
| Other | CIMD (client ID metadata documents) advertised, but Pascal says only clients it registered (Claude by its published metadata document, Cursor by the pinned client) are accepted |

The catalog's pinned `CLIENT_ID` `pascal-cursor` belongs to the official Cursor connector. Buwiz Bot cannot use it (it will not accept Buwiz Bot's callback).

## Prerequisites

- A Pascal account with a verified email (free). Sign up at https://editor.pascal.app.
- Cost: Pascal is free to use; the Free plan gives 150 credits per day (resets midnight UTC). Credits are spent only by AI generations, Studio renders, assistant messages and credit-backed hosted services (for example `pull_site_data`, server-side view renders). Building and editing scenes through MCP is free. Paid plans (Pro, Max) and credit packs exist; whether API keys require a paid plan is **(unconfirmed)**, Pascal's docs describe keys without mentioning a plan.
- **Spending warning for API keys:** Pascal's docs say "A Pascal API key authorized for the project ... can spend once the checks above pass", while an OAuth connection spends only after you turn on **Can spend credits** for it. So a key can burn credits. The agent passes a `maxCredits` ceiling on paid tools, and `list_pascal_services` shows prices for free first. Tell the assistant not to call paid services unless asked.
- No approval wait for an API key. For a custom OAuth client registration, Pascal gives no process or timeline **(unconfirmed)**.

## Steps (API key)

1. Sign in at https://editor.pascal.app and open https://editor.pascal.app/settings.
2. In **API keys**: enter a name (`Buwiz Bot`), choose the **personal** or **organization** workspace, click **Create**, and copy the key immediately; Pascal does not show it again. Keys look like `sk_live_...` or `sk_test_...` (from Pascal's error text).
3. The key acts as its owner in the workspace you picked; it only sees projects of that workspace. Revoke it any time in Settings.
4. In Buwiz Bot, add the key as a header `Authorization: Bearer <key>` for the Pascal hosted server. **Buwiz Bot's current plugin page for `pascal-hosted` is OAuth-only (the catalog has no setup fields), so this needs a header/token field in Buwiz Bot (unconfirmed that one exists).**

## Steps (OAuth, only if Pascal registers Buwiz Bot)

Ask in the Pascal Discord (https://discord.gg/XRKsDcpqgS, the help link on Pascal's Claude page; no other contact is documented) for Buwiz Bot to be registered as an OAuth client with: type public (no secret, `token_endpoint_auth_method: none`), redirect URI `http://127.0.0.1:8787/oauth/callback/pascal-hosted` (confirm in Buwiz Bot's plugin page), scopes `openid profile offline_access scene:read scene:write`. Pascal's Claude page says Claude Code returns through a local `127.0.0.1` address and Pascal shows an extra warning, so loopback redirects are supported for at least that client; whether Pascal accepts a new loopback client is **(unconfirmed)**. If Pascal gives you a client ID, paste it as below.

## What to paste into Buwiz Bot

**Connect apps -> Pascal -> settings -> Authenticate**

| Buwiz Bot field | Value |
|---|---|
| Client ID | only if Pascal registers Buwiz Bot: the ID Pascal gives you. Do not use `pascal-cursor`. |
| Client Secret | none (public client) |
| Callback URL (shown by Buwiz Bot, read-only) | should equal `http://127.0.0.1:8787/oauth/callback/pascal-hosted` |
| API key (if Buwiz Bot offers a token/header field) | the `sk_live_...` key, sent as `Authorization: Bearer <key>` |

## Scopes requested

Catalog `auth.scopes`: `openid profile offline_access scene:read scene:write`. Pascal's review screen shows two access groups: read projects and room scans (`scene:read`), and create/edit/save/delete projects (`scene:write`). OAuth connections never include paid services. An API key carries its owner's access.

## Alternative: local Pascal, no account

Pascal's docs describe a local server that needs no hosted account or key: the Pascal CLI (`npm install --global @pascal-app/cli`, then `pascal mcp connect` as a stdio server) or the npm package `@pascal-app/mcp` (stdio, `npx`, local SQLite at `~/.pascal/data/pascal.db`). It reads local projects, not your hosted account. That is a different Buwiz Bot plugin shape (stdio, not `pascal-hosted`).

## Verification

1. Buwiz Bot shows Pascal connected (OAuth) or the plugin lists tools (key).
2. Ask: "Use the Pascal MCP server. Read pascal://agent-guide, then list my Pascal projects." Then: "Create a small one-bedroom project, validate it, save it and give me the editor URL."
3. Provider side: https://editor.pascal.app/settings/connected-apps lists OAuth connections (and the **Can spend credits** switch); Settings -> API keys lists keys.
4. A bad credential returns `401` with `error="invalid_token"` (Pascal's docs); `403 insufficient_scope` means the connection lacks `scene:write`.

## Troubleshooting

- **"unknown client_id" / authorize refused:** expected for any client Pascal has not registered. Use the API key.
- **`401 invalid_token`:** key revoked, mistyped or expired token. Create a new key; do not retry the same credential.
- **Cannot see a project:** the key (or OAuth workspace choice) belongs to a different workspace than the project owner. Create the key under the workspace that owns it.
- **`429`:** limit shown in `RateLimit` headers (live: 120 requests per 60 seconds per address). Wait `Retry-After`.
- **`spending_not_allowed`:** an OAuth connection tried a paid call. Turn on **Can spend credits** under Connected apps, or avoid paid tools.
- **Hosted vs local confusion:** if Pascal's Claude Code plugin (local server `plugin:pascal-agent-skills:pascal`) is also installed, projects differ between the two stores. Use the one that matches where your project lives.

## Sources

- https://editor.pascal.app/docs/developers/mcp.md (checked 2026-10-25; page dated 2026-10-23)
- https://editor.pascal.app/auth.md (agent auth methods, DCR off, CIMD note)
- https://editor.pascal.app/docs/developers/claude.md
- https://editor.pascal.app/docs/developers/hosted-services.md (credits and API-key spending)
- https://editor.pascal.app/docs/developers/errors.md
- https://editor.pascal.app/docs/account/credits.md
- https://editor.pascal.app/api/mcp (live info card)
- https://editor.pascal.app/.well-known/oauth-protected-resource/api/mcp (live)
- https://editor.pascal.app/.well-known/oauth-authorization-server/api/auth (live)
- https://github.com/pascalorg/editor/tree/main/skills (plugin; `.mcp.json` pins `pascal-cursor`)
