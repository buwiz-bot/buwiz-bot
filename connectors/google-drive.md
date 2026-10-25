# Google Drive (Rox plugin `google-drive`)

The sign-in for this plugin uses your own Google OAuth client. Follow the full guide: **[google.md](google.md)**. It covers the Google Cloud project, consent screen, Desktop app client, Testing vs. In production, and troubleshooting. One client serves all six Google plugins.

- **MCP endpoint (from the plugin catalog):** `https://api.cursor.com/rest-mcp/google-drive/mcp (Cursor-hosted proxy; Google's official equivalent is https://drivemcp.googleapis.com/mcp/v1)`
- **API to enable in the Cloud project:** Google Drive API (`drive.googleapis.com`); `drivemcp.googleapis.com` too if you use Google's official endpoint
- **Scopes advertised by that endpoint** (prefix `https://www.googleapis.com/auth/` unless shown): `drive.readonly`, `drive.file` (official endpoint also: `drive`). `drive.file` is non-sensitive; `drive.readonly` and `drive` are restricted.
- **Paste into Rox** (Connect apps -> Google Drive -> settings): Client ID and Client Secret of the shared Desktop-app client, then Authenticate.
- Sensitivity / verification notes: see "Scopes by plugin" and "Testing vs. In production" in [google.md](google.md).

Drive-specific: with only `drive.file` the assistant sees just files it created or you opened with it; `drive.readonly` is needed to browse and search everything.

Checked 2026-10-25.
