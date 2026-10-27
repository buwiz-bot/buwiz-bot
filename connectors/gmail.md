# Gmail (Rox plugin `gmail`)

The sign-in for this plugin uses your own Google OAuth client. Follow the full guide: **[google.md](google.md)**. It covers the Google Cloud project, consent screen, Desktop app client, Testing vs. In production, and troubleshooting. One client serves all six Google plugins.

- **MCP endpoint (from the plugin catalog):** `https://gmailmcp.googleapis.com/mcp/v1`
- **API to enable in the Cloud project:** Gmail API (`gmail.googleapis.com`) and the MCP service `gmailmcp.googleapis.com`
- **Scopes advertised by that endpoint** (prefix `https://www.googleapis.com/auth/` unless shown): `https://mail.google.com/`, `gmail.compose`, `gmail.drafts`, `gmail.drafts.create`, `gmail.drafts.readonly`, `gmail.labels`, `gmail.metadata`, `gmail.modify`, `gmail.readonly`, `gmail.send`, `gmail.settings.basic` (Google's guide suggests `gmail.readonly` + `gmail.compose` as the minimum). Most are restricted scopes.
- **Paste into Rox** (Connect apps -> Gmail -> settings): Client ID and Client Secret of the shared Desktop-app client, then Authenticate.
- Sensitivity / verification notes: see "Scopes by plugin" and "Testing vs. In production" in [google.md](google.md).

Gmail-specific: refresh tokens are also revoked when you change your Google password (Gmail scopes).

Checked 2026-10-25.
