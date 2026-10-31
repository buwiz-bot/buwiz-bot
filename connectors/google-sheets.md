# Google Sheets (Buwiz Bot plugin `google-sheets`)

The sign-in for this plugin uses your own Google OAuth client. Follow the full guide: **[google.md](google.md)**. It covers the Google Cloud project, consent screen, Desktop app client, Testing vs. In production, and troubleshooting. One client serves all six Google plugins.

- **MCP endpoint (from the plugin catalog):** `https://api.cursor.com/rest-mcp/google-sheets/mcp (Cursor-hosted proxy; Google's official equivalent is https://sheetsmcp.googleapis.com/mcp/v1)`
- **API to enable in the Cloud project:** Google Sheets API (`sheets.googleapis.com`); `sheetsmcp.googleapis.com` too if you use Google's official endpoint
- **Scopes advertised by that endpoint** (prefix `https://www.googleapis.com/auth/` unless shown): `spreadsheets` (official endpoint also: `spreadsheets.readonly`, `drive.readonly`, `drive`).
- **Paste into Buwiz Bot** (Connect apps -> Google Sheets -> settings): Client ID and Client Secret of the shared Desktop-app client, then Authenticate.
- Sensitivity / verification notes: see "Scopes by plugin" and "Testing vs. In production" in [google.md](google.md).

Checked 2026-10-25.
