# ElevenLabs connector setup

Checked against ElevenLabs' docs, the `elevenlabs/plugin` repo and live endpoints on 2026-10-25. Anything marked **(unconfirmed)** could not be verified.

## What it unlocks

Rox connects to ElevenLabs' hosted MCP server: create, update, list, duplicate and delete voice agents; review conversations and tests; manage branches; generate speech (returned as a short-lived download link). Per the plugin README, also images and video (scopes below).

| Item | Value |
|---|---|
| MCP server (catalog) | `https://api.us.elevenlabs.io/v1/mcp` (docs also show `https://api.elevenlabs.io/v1/mcp`) |
| Data-residency servers | EU `https://api.eu.residency.elevenlabs.io/v1/mcp`, India `https://api.in.residency.elevenlabs.io/v1/mcp`, Singapore `https://api.sg.residency.elevenlabs.io/v1/mcp` (separate accounts) |
| Authorization server (issuer) | `https://api.us.elevenlabs.io` |
| Authorize URL | `https://elevenlabs.io/app/oauth/authorize` |
| Token URL | `https://api.us.elevenlabs.io/v1/oauth/token` |
| Revocation URL | `https://api.us.elevenlabs.io/v1/oauth/revoke` |
| PKCE | S256 |
| Token endpoint client auth | `none`, `client_secret_post`, `private_key_jwt` |
| Grant types | `authorization_code`, `refresh_token` |
| Registration endpoint (DCR) | none |
| CIMD | **supported** (`client_id_metadata_document_supported: true`) |

Probed: `https://api.us.elevenlabs.io/.well-known/oauth-protected-resource[/v1/mcp]` and `/.well-known/oauth-authorization-server`.

## Does Rox need its own app? Probably not, if Rox can use CIMD

The pinned client ID `0d057191-f2bd-4516-89f8-997e7bfe6bc6` is the plugin's pre-registered client for Cursor and will not carry Rox's callback **(unconfirmed by probe: the authorize page is a browser app that validates the client after load)**. But ElevenLabs states in its hosted-MCP docs: "No client registration is needed for clients that support hosted client metadata (CIMD)", and Claude's custom connector uses "Anthropic's hosted client metadata". There is no developer page to create an OAuth app that I could find.

So: host a small JSON file and use its https URL as Rox's Client ID (needs Rox's field to accept an https URL **(unconfirmed)**). Whether ElevenLabs accepts http loopback `127.0.0.1:8787` redirects in a CIMD document is **(unconfirmed)**; it was not testable without signing in.

## Prerequisites

- An ElevenLabs account (sign in at elevenlabs.io). Plan requirements and price: ElevenLabs bills usage (for example speech generation) to your account; check `elevenlabs.io/pricing` **(unconfirmed, not read)**.
- A workspace you can administer if the assistant will edit agents; the consent screen has a workspace picker.
- No approval wait documented.

## Steps

1. Host this at an https URL with a non-root path, e.g. `https://YOURNAME.github.io/rox/elevenlabs-client.json` (the `client_id` must equal the URL):

   ```json
   {
     "client_id": "https://YOURNAME.github.io/rox/elevenlabs-client.json",
     "client_name": "Rox",
     "client_uri": "https://YOURNAME.github.io/rox/",
     "redirect_uris": ["http://127.0.0.1/oauth/callback/elevenlabs"],
     "grant_types": ["authorization_code", "refresh_token"],
     "response_types": ["code"],
     "token_endpoint_auth_method": "none"
   }
   ```

   Callback URL to confirm in Rox's plugin page (Rox's exact string is not final): `http://127.0.0.1:8787/oauth/callback/elevenlabs`. If ElevenLabs rejects the portless form, list `http://127.0.0.1:8787/oauth/callback/elevenlabs` explicitly.
2. In Rox, paste the URL as Client ID, leave the secret empty, click **Authenticate**.
3. In the ElevenLabs browser page: sign in (for a residency region, use that region's account and its MCP URL), choose the workspace, review the permissions, click **Authorize**.

App name: `Rox`. Client type: public.

## What to paste into Rox

Open **Connect apps -> ElevenLabs -> settings**:

| Rox field | Value |
|---|---|
| Client ID | https URL of your CIMD JSON |
| Client Secret | empty |
| Callback URL (read-only) | should equal `http://127.0.0.1:8787/oauth/callback/elevenlabs` |

## Scopes requested

From the catalog and server metadata (identical): `convai_read convai_write text_to_speech speech_history_read flows image_video_generation voice_generation`.

## Alternative: API key

ElevenLabs' hosted-MCP page says authentication uses OAuth and "No API keys are copied into the client." The REST API uses an `xi-api-key` header, but nothing in the docs says `/v1/mcp` accepts it or an `Authorization: Bearer <API key>`. **(unverified; assume not)** An API key can be restricted by endpoint scope, credit quota and IP allowlist (REST docs), which is why a key would be the safer option if ElevenLabs confirms it works; ask ElevenLabs support. The documented API-key path is ElevenLabs' separate local MCP server, which is not this connector and was not checked here.

## Verification

Ask Rox: "List my ElevenLabs agents." Then "Generate a short sample of my agent's voice saying hello" (this bills your account). In ElevenLabs you can revoke the connection from account settings **(exact location unconfirmed)**.

## Troubleshooting

- Authorize page error about the client or redirect: the CIMD `client_id` differs from the hosted URL, the file is not reachable, or the callback is not listed exactly.
- Signed into the wrong environment: residency regions are separate accounts and need their own MCP URL; the catalog URL is the US/global one.
- Edits go live immediately: MCP changes publish on the target branch or `main`, not a draft.
- 401 later: token expired and refresh failed; re-authenticate.

## Sources

- https://elevenlabs.io/docs/eleven-agents/operate/hosted-mcp.md
- https://elevenlabs.io/docs/api-reference/authentication.md
- https://github.com/elevenlabs/plugin (README, mcp.json, server.json)
- https://api.us.elevenlabs.io/.well-known/oauth-authorization-server and /.well-known/oauth-protected-resource (live)
