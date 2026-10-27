# Salesforce (Headless 360, Beta) connector setup

Checked 2026-10-25. The full guide covers both Salesforce connectors: see [salesforce.md](salesforce.md). Only the Headless 360 specifics are repeated here.

- Connector id: `salesforce-headless-360`
- Callback URL (confirm in Rox's plugin page): `http://127.0.0.1:8787/oauth/callback/salesforce-headless-360`
- MCP URL, production/Developer: `https://api.salesforce.com/platform/mcp/v1/platform/headless-360`; sandbox/scratch: `https://api.salesforce.com/platform/mcp/v1/sandbox/platform/headless-360`
- Create a Salesforce **External Client App** (not a Connected App) with scopes `mcp_api` and `refresh_token`, JWT-based access tokens for named users on, PKCE on, and **Require Secret for Web Server Flow** on (this plugin needs the Consumer Secret).
- Activate **headless-360** in Setup -> MCP Servers first (needs API version 67.0 or later; it is a Beta).
- In Rox (**Connect apps -> Salesforce (Headless 360, Beta) -> settings**) paste Consumer Key, Consumer Secret and the server URL, then Authenticate.
- Salesforce admin sets these once for a team; members only sign in. For a single user, you are the admin.
