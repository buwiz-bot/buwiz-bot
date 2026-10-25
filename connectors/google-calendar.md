# Google Calendar (Rox plugin `google-calendar`)

The sign-in for this plugin uses your own Google OAuth client. Follow the full guide: **[google.md](google.md)**. It covers the Google Cloud project, consent screen, Desktop app client, Testing vs. In production, and troubleshooting. One client serves all six Google plugins.

- **MCP endpoint (from the plugin catalog):** `https://calendarmcp.googleapis.com/mcp/v1`
- **API to enable in the Cloud project:** Google Calendar API (`calendar-json.googleapis.com`, not `calendar.googleapis.com`) and the MCP service `calendarmcp.googleapis.com`
- **Scopes advertised by that endpoint** (prefix `https://www.googleapis.com/auth/` unless shown): `calendar`, `calendar.acls`, `calendar.calendarlist`, `calendar.calendarlist.readonly`, `calendar.calendars`, `calendar.calendars.readonly`, `calendar.events`, `calendar.events.freebusy`, `calendar.events.readonly`, `calendar.freebusy`, `calendar.readonly`, `calendar.settings.readonly` (Google's guide suggests `calendar.calendarlist.readonly`, `calendar.events.freebusy`, `calendar.events.readonly` as the minimum; add `calendar.events` to create/edit events).
- **Paste into Rox** (Connect apps -> Google Calendar -> settings): Client ID and Client Secret of the shared Desktop-app client, then Authenticate.
- Sensitivity / verification notes: see "Scopes by plugin" and "Testing vs. In production" in [google.md](google.md).

Checked 2026-10-25.
