//! The model: every piece of server state the shell shows, as signals in
//! one `Store` anchored by [`crate::live::store`]. Views read the signals;
//! the methods here move the data — fetches spawned on the engine write
//! the results back.

use std::collections::BTreeMap;

use nsh_core::prelude::*;

use crate::client;

// ---------------------------------------------------------------------------
// Server payloads
// ---------------------------------------------------------------------------

/// A row of `GET /threads` — the sidebar's chat list.
#[derive(Clone, PartialEq, Default)]
pub struct ThreadMeta {
    pub id: String,
    /// Its permanent UUID v7 (`uid`): what links name it by.
    pub uid: String,
    pub name: String,
    /// Turn count (`count` on the `/threads` row) — the row's subtitle.
    pub count: i64,
    pub created: f64,
    /// The bot that owns this thread (`bot` on the `/threads` row).
    pub bot: Option<ThreadBot>,
    /// Sidebar state (`hidden`/`archived` columns; `lastApp` for the
    /// detail pane's default tab).
    pub hidden: bool,
    pub archived: bool,
    pub last_app: String,
    /// A v1 routine's own thread (`routine` on the row): never a sidebar
    /// row nor a ⌘1–9 slot; its runs land here, its notices in `thread`.
    pub routine: Option<RoutineOf>,
    /// An agents' conversation's thread (`agents` on the row): never a
    /// sidebar row, a ⌘1–9 slot nor the chat restored at launch; read-only.
    pub agents: Option<crate::agents::AgentsOf>,
}

/// A routine's check time `ms` as local time: "3:42 PM", or with the date
/// when not today ("Oct 2, 3:42 PM").
pub fn checked_at(ms: f64) -> String {
    let at = crate::mini_v1::cards::as_of_label(ms as i64);
    at.strip_prefix("as of ").map(str::to_owned).unwrap_or(at)
}

/// The `routine` field of a `/threads` row: whose thread it is and the
/// chat the routine reports to (the way back).
#[derive(Clone, PartialEq, Default, Debug)]
pub struct RoutineOf {
    pub plugin: String,
    pub id: String,
    /// The instance whose thread it is ("" from a broker before instances).
    pub instance: String,
    pub name: String,
    pub thread: String,
}

impl RoutineOf {
    fn of(v: &serde_json::Value) -> Option<Self> {
        let r = Self { plugin: s(v, "plugin"), id: s(v, "id"), instance: s(v, "instance"), name: s(v, "name"), thread: s(v, "thread") };
        (!r.plugin.is_empty() && !r.id.is_empty()).then_some(r)
    }
}

/// A `notice` turn's payload: one run of a v1 routine, told in the chat it
/// reports to. `thread`/`turn` say where the run starts in the routine's
/// thread ("" when it ran nothing there); `error` is set on a failure.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct RoutineNotice {
    pub name: String,
    pub plugin: String,
    pub routine: String,
    pub thread: String,
    pub turn: String,
    pub items: i64,
    pub error: String,
}

impl RoutineNotice {
    fn of(v: &serde_json::Value) -> Option<Self> {
        if !v.is_object() || s(v, "kind") != "routine" {
            return None;
        }
        // the uid when there is one: it names the thread for good
        let thread = Some(s(v, "thread_uid")).filter(|u| !u.is_empty()).unwrap_or_else(|| s(v, "thread"));
        Some(Self {
            name: Some(s(v, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| "Routine".to_owned()),
            plugin: s(v, "plugin"),
            routine: s(v, "routine"),
            thread,
            turn: s(v, "turn"),
            items: i64of(v, "items"),
            error: s(v, "error"),
        })
    }

    /// The line it shows: "BIR payments ran · 2 new items".
    pub fn line(&self) -> String {
        if !self.error.is_empty() {
            let e: String = self.error.chars().take(160).collect();
            return format!("{} failed: {e}", self.name);
        }
        format!("{} ran · {} new item{}", self.name, self.items, if self.items == 1 { "" } else { "s" })
    }
}

/// A `notice` turn that asks the user to switch on a routine instance the
/// agent made (`routine_consent`, 5.9): its question (the turn's text),
/// and whether it was answered.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct ConsentNotice {
    pub plugin: String,
    pub routine: String,
    pub instance: String,
    pub name: String,
    pub text: String,
    /// `pending`, `enabled` or `declined`.
    pub state: String,
}

impl ConsentNotice {
    fn of(v: &serde_json::Value, text: String) -> Option<Self> {
        if !v.is_object() || s(v, "kind") != "routine_consent" || s(v, "instance").is_empty() {
            return None;
        }
        Some(Self {
            plugin: s(v, "plugin"),
            routine: s(v, "routine"),
            instance: s(v, "instance"),
            name: s(v, "name"),
            text,
            state: Some(s(v, "state")).filter(|x| !x.is_empty()).unwrap_or_else(|| "pending".to_owned()),
        })
    }

    /// Its state as the card shows it, the user's tap (`local`) over the
    /// one the chat last loaded: None while it still asks.
    pub fn answered(&self, local: Option<&str>) -> Option<&'static str> {
        match local.unwrap_or(&self.state) {
            "enabled" => Some("Enabled"),
            "declined" => Some("Not enabled"),
            _ => None,
        }
    }
}

/// The `bot` field of a `/threads` row — the persona behind the chat.
#[derive(Clone, PartialEq, Default)]
pub struct ThreadBot {
    pub id: String,
    pub name: String,
    pub shape: String,
    pub color: String,
}

/// An avatar photo's zoom/pan (`crop` on a bot): `{s, x, y}`.
#[derive(Clone, Copy, PartialEq)]
pub struct Crop {
    pub s: f64,
    pub x: f64,
    pub y: f64,
}

/// `Draft` — an image picked/generated for the avatar crop editor
/// (App.tsx:1747): `{uri,name,w,h}`.
#[derive(Clone, PartialEq, Default)]
pub struct AvatarDraft {
    pub uri: String,
    pub name: String,
    pub w: f64,
    pub h: f64,
}

/// A row of `GET /mcp/servers` — `{id,name,command,args,enabled,bot,status,tools,err}`.
#[derive(Clone, PartialEq, Default)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub command: String,
    /// `url` — set on remote transports (null on the wire → "").
    pub url: String,
    pub args: Vec<String>,
    pub enabled: bool,
    pub bot: String,
    pub status: String,
    pub tools: i64,
    pub err: String,
}


/// The market's pseudo-category for "Recommended for you" → View all.
pub const RECOMMENDED: &str = "Recommended for you";

/// A row of `GET /plugins/catalog` — `{name,description,category,homepage,
/// domains,keywords,installed,meta}` (App.tsx:1362).
#[derive(Clone, PartialEq, Default)]
pub struct PluginRow {
    pub name: String,
    pub description: String,
    pub category: String,
    pub homepage: String,
    pub domains: Vec<String>,
    pub keywords: Vec<String>,
    pub installed: bool,
    /// `meta.mcp` — the connector ids the plugin ships (the connected-page
    /// dedupe key; not counted twice against standalone servers).
    pub meta_mcp: Vec<String>,
    /// Grok Bot catalog fields: the shown name, the logo URL (served by
    /// the app server; empty = letter tile), Grok's category labels, the
    /// publisher, whether its setup has fields (Add opens Setup first).
    pub display_name: String,
    pub icon: String,
    pub cats: Vec<String>,
    pub publisher: String,
    pub setup: bool,
}

/// One setup field of a plugin (`/plugins/detail`'s `fields`) — Grok
/// Bot's plugin variables: label, placeholder, required/secret, hint,
/// select options; `set` = a value is stored (secrets never come back).
#[derive(Clone, PartialEq, Default)]
pub struct PluginField {
    pub key: String,
    pub label: String,
    pub placeholder: String,
    pub required: bool,
    pub secret: bool,
    pub hint: String,
    pub options: Vec<(String, String)>,
    pub set: bool,
    pub value: String,
    /// "google" — one credential shared by every Google plugin.
    pub shared: String,
}

/// One `pluginNav` frame — `{page,name?,id?}`; empty string = absent
/// (App.tsx:1308).
#[derive(Clone, PartialEq)]
pub struct PluginNav {
    pub page: String,
    pub name: String,
    pub id: String,
}

impl PluginNav {
    pub fn market() -> Self {
        Self { page: "market".into(), name: String::new(), id: String::new() }
    }
}

/// A connector row inside `GET /plugins/detail`'s `mcp` —
/// `{name,transport,id,status,tools,err}` (`id` may be null → "not-added").
#[derive(Clone, PartialEq, Default)]
pub struct PluginMcp {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub status: String,
    pub tools: i64,
    pub err: String,
    /// `accounts` — every `mcp_servers` row under the connector's name
    /// (first = the default account).
    pub accounts: Vec<PluginAccount>,
    /// Setup values still missing before it can sign in (labels).
    pub needs: Vec<String>,
    /// The fixed OAuth redirect to register with the provider (connectors
    /// on the user's own OAuth app), else empty.
    pub callback_url: String,
    /// Why it can't connect at all (config not published, vendor-hosted).
    pub unavailable: String,
}

/// One account of a plugin connector — `{id,label,status,tools,err,authed}`.
#[derive(Clone, PartialEq, Default)]
pub struct PluginAccount {
    pub id: String,
    pub label: String,
    pub status: String,
    pub tools: i64,
    pub err: String,
    /// Stored OAuth tokens exist (Disconnect signs out).
    pub authed: bool,
    /// The server's status note: what's missing, or how it signs in.
    pub note: String,
}

/// A plugin's skill/command row (`{name,description}`).
#[derive(Clone, PartialEq, Default)]
pub struct PluginSkill {
    pub name: String,
    pub description: String,
}

/// `GET /plugins/detail` — the plugin page's body (App.tsx:1332).
#[derive(Clone, PartialEq, Default)]
pub struct PluginDetail {
    pub installed: bool,
    pub description: String,
    pub category: String,
    pub homepage: String,
    /// `manifest.author` (falls back to the homepage host's first label).
    pub author: String,
    /// `meta.mcp` — connector ids for the connected-page dedupe.
    pub meta_mcp: Vec<String>,
    pub mcp: Vec<PluginMcp>,
    pub skills: Vec<PluginSkill>,
    pub commands: Vec<PluginSkill>,
    pub display_name: String,
    pub icon: String,
    pub availability: String,
    /// Source repository (View Source), else the homepage.
    pub source: String,
    pub fields: Vec<PluginField>,
}

/// A row of `GET /skills` — the user's own skill (App.tsx:1336).
#[derive(Clone, PartialEq, Default)]
pub struct PrivateSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub body: String,
    pub enabled: bool,
}

/// `GET /plugins/skill`'s `{description,body}` (App.tsx:1333).
#[derive(Clone, PartialEq, Default)]
pub struct SkillBody {
    pub description: String,
    pub body: String,
}

/// A row of `GET /mcp/tools?id=` — `{name,description,enabled}`
/// (App.tsx:1431).
#[derive(Clone, PartialEq, Default)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

/// `appRename` — the library's rename prompt: `{name, field, v?}` where
/// `field` is `title` | `file` | `label` (a branch's) (App.tsx:1637).
#[derive(Clone, PartialEq)]
pub struct AppRename {
    pub name: String,
    pub field: String,
    pub v: Option<i64>,
}

/// A row of the `bots` array of `GET /threads` — an agent (bot) persona.
#[derive(Clone, PartialEq, Default)]
pub struct RoxBot {
    pub id: String,
    pub name: String,
    pub emoji: String,
    /// The row's subtitle (`preview` on the server payload).
    pub desc: String,
    /// The blob's picker shape (`avatar.shape`, default `blob`).
    pub shape: String,
    /// The blob's picker color (`avatar.color`, default `#FFFFFF`).
    pub color: String,
    /// The thread this persona owns (opened on row press).
    pub thread_id: String,
    /// A photo avatar's URI (`image`); the mark is drawn when empty.
    pub image: String,
    /// The photo's crop (`{s,x,y}`).
    pub crop: Option<Crop>,
    pub hidden: bool,
    pub pinned: bool,
    pub section: String,
    /// The persona's own model choice — `choice()` prefers it over the
    /// app-wide pref (App.tsx:357).
    pub model: String,
    pub effort: String,
    pub fast: bool,
    /// Its tagline (Grok Bot's "Add a label"): under its name in the
    /// pane and on its tile.
    pub label: String,
    /// Its own context, which fills on demand: what it is, how it
    /// behaves (seeded at creation, rewritten by the bot when asked),
    /// what it remembers, and the integrations it has used or was given.
    pub description: String,
    pub instructions: String,
    pub memories: Vec<BotMemory>,
    pub integrations: Vec<BotIntegration>,
    /// Its last edit (Bot Details' "Last updated"), epoch ms; 0 if never.
    pub updated_at: f64,
    /// The agent that made it (`made_by`; "" when the user did), and
    /// what for (`purpose`).
    pub made_by: String,
    pub purpose: String,
    /// A temporary helper's parent and task (`helper`): no sidebar row.
    pub helper: Option<crate::agents::HelperOf>,
    /// Whom it takes messages from: `all`, `none` or `some` (a list).
    pub accepts: String,
}

/// A file the user attached or a turn produced (`attachments`, `files`).
#[derive(Clone, PartialEq, Default)]
pub struct Attach {
    pub kind: String,
    pub path: String,
    pub name: String,
    pub mime: String,
    pub url: String,
    pub thumb: String,
    pub ext: String,
    pub size: f64,
    pub feed: String,
    /// `fileMeta`'s card-illustration variant (`pdf`, `docx`,
    /// `spreadsheet`, `media`, `audio`, `generic`, …).
    pub variant: String,
    /// Audio/video length in seconds.
    pub duration: f64,
    /// The audio waveform's 48 peaks (0..1).
    pub peaks: Vec<f64>,
    /// Media pixel size.
    pub width: f64,
    pub height: f64,
    /// A video's poster frame (`file/raw` key).
    pub poster: String,
    /// Multi-frame source (gif / animated webp / apng).
    pub animated: bool,
}

/// `GET /file/render`'s answer (`Rendered` in fileviewer.tsx).
#[derive(Clone, PartialEq, Default)]
pub struct Rendered {
    pub error: String,
    /// The BetterOffice engine's path under the server (`/office/…`) — a
    /// WebView paints it; absent, the classic renderers apply.
    pub office: String,
    /// Markdown/docx extracted text.
    pub text: Option<String>,
    /// Shiki-highlighted lines: `[content, dark color, light color]`
    /// spans per line.
    pub lines: Vec<Vec<(String, String, String)>>,
    /// Workbook sheets.
    pub sheets: Vec<Sheet>,
    /// `r.json !== undefined` — `null` is a valid tree root.
    pub has_json: bool,
    pub json: serde_json::Value,
}

/// A workbook sheet (`{name, rows, bold, widths}`).
#[derive(Clone, PartialEq, Default)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
    pub bold: Vec<Vec<bool>>,
    /// Column widths in characters (the file's own), when it has them.
    pub widths: Vec<f64>,
}

impl Rendered {
    /// The `/file/render` response: `{...meta, error?, office?, text?,
    /// lines?, sheets?, json?}`.
    fn of(v: &serde_json::Value) -> Rendered {
        Rendered {
            error: s(v, "error"),
            office: s(v, "office"),
            text: v.get("text").and_then(|x| x.as_str()).map(String::from),
            lines: list(v, "lines")
                .iter()
                .map(|line| {
                    line.as_array()
                        .map(|spans| {
                            spans
                                .iter()
                                .map(|sp| {
                                    let a = sp.as_array();
                                    (
                                        a.and_then(|a| a.first())
                                            .and_then(|x| x.as_str())
                                            .unwrap_or("")
                                            .to_owned(),
                                        a.and_then(|a| a.get(1))
                                            .and_then(|x| x.as_str())
                                            .unwrap_or("")
                                            .to_owned(),
                                        a.and_then(|a| a.get(2))
                                            .and_then(|x| x.as_str())
                                            .unwrap_or("")
                                            .to_owned(),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .collect(),
            sheets: list(v, "sheets")
                .iter()
                .map(|sh| Sheet {
                    name: s(sh, "name"),
                    rows: list(sh, "rows")
                        .iter()
                        .map(|r| {
                            r.as_array()
                                .map(|cells| {
                                    cells
                                        .iter()
                                        .map(|c| c.as_str().unwrap_or("").to_owned())
                                        .collect()
                                })
                                .unwrap_or_default()
                        })
                        .collect(),
                    bold: list(sh, "bold")
                        .iter()
                        .map(|r| {
                            r.as_array()
                                .map(|cells| {
                                    cells.iter().map(|c| c.as_bool().unwrap_or(false)).collect()
                                })
                                .unwrap_or_default()
                        })
                        .collect(),
                    widths: list(sh, "widths")
                        .iter()
                        .map(|w| w.as_f64().unwrap_or(0.0))
                        .collect(),
                })
                .collect(),
            has_json: v.get("json").is_some(),
            json: v.get("json").cloned().unwrap_or(serde_json::Value::Null),
        }
    }
}

/// A lightbox item (`LightItem` in lightbox.tsx): image, video, or a
/// mermaid `diagram` (`code` is its source).
#[derive(Clone, PartialEq, Default)]
pub struct LightItem {
    pub kind: String,
    pub uri: String,
    pub name: String,
    pub path: String,
    pub poster: String,
    pub w: f64,
    pub h: f64,
    pub code: String,
}

/// The reply quote under a turn (`reply:{i,text}` — `i` jumps to it,
/// shell.tsx:454).
#[derive(Clone, PartialEq, Default)]
pub struct Reply {
    pub name: String,
    pub text: String,
    pub i: i64,
}

/// A generated artifact a turn's card points at (`art`).
#[derive(Clone, PartialEq, Default)]
pub struct Art {
    /// The module key for `/module?name=`: `R<n>` or a pinned `R<n>@v<k>`.
    pub app: String,
    pub file: String,
    pub title: String,
    pub desc: String,
    pub kind: String,
    pub w: f64,
    pub h: f64,
}

/// One turn of `GET /turns?thread=`.
#[derive(Clone, PartialEq, Default)]
pub struct Turn {
    pub i: i64,
    /// Its permanent UUID v7 (`id`): `i` shifts when earlier turns are
    /// deleted, this never does — links name a turn by it.
    pub id: String,
    pub role: String,
    pub text: String,
    pub name: String,
    pub label: String,
    pub caption: String,
    pub reaction: String,
    pub widget: bool,
    pub v: i64,
    pub reply: Option<Reply>,
    pub attachments: Vec<Attach>,
    pub files: Vec<Attach>,
    pub art: Option<Art>,
    pub building: bool,
    pub status: String,
    /// A revise of this card is building (Try Again, `revise_app`): its
    /// body shows the generation placeholder until the new version lands.
    pub revising: bool,
    /// Drive files the reply made or changed — chips under the message.
    pub links: Vec<DriveLink>,
    /// Interactive cards (approvals, choices, forms, plugin needs) under
    /// the reply, in document order: the optional `cards` array of raw wire
    /// objects (`cards::Card::from_wire`), or the dev hook's local turn.
    pub cards: Vec<crate::cards::TurnCard>,
    /// The v1 attachments that went to the model with this user message
    /// (`v1Attachments`, spec 10.3): (canonical token, title).
    pub v1_attachments: Vec<(String, String)>,
    /// An answer's next steps (9.8, `suggest`): (label, prompt).
    pub suggest: Vec<(String, String)>,
    /// A `notice` turn: a routine's run, drawn as a centered line.
    pub notice: Option<RoutineNotice>,
    /// A `notice` turn asking to switch on a routine (a card with Enable).
    pub consent: Option<ConsentNotice>,
    /// An item's instruction in a routine's thread (the user turn's
    /// `routine` mark): the routine's name, its caption ("" otherwise).
    pub instruction_from: String,
    /// An `agent_message` turn's envelope (an agents' conversation).
    pub message: Option<crate::agents::AgentMessage>,
    /// A `notice` turn about agents (messages, a new agent, a pause).
    pub agents_notice: Option<crate::agents::AgentsNotice>,
    /// A turn in an agent's chat marked `agent`: a user turn that is
    /// another agent's reply ("Reply from <name>"), or an answer made for
    /// another agent's request ("For <name>'s request").
    pub reply_from: Option<crate::agents::ReplyFrom>,
}

/// What a reply quotes of turn `t` (`onReply(i, text || caption || name)`):
/// a card's `app · <title> v<n>`, else its text, caption, the titles of
/// what it links (a v1 app card, a file), its files' names, its name.
/// Empty when it has nothing to show: no reply chip then.
pub(crate) fn reply_label(t: &Turn) -> String {
    if let Some(a) = &t.art {
        return format!(
            "{} · {}{}",
            if a.kind == "image" { "image" } else { "app" },
            if a.title.is_empty() { &t.name } else { &a.title },
            if t.v > 0 { format!(" v{}", t.v) } else { String::new() }
        );
    }
    let joined = |names: Vec<&str>| names.into_iter().filter(|n| !n.trim().is_empty()).collect::<Vec<_>>().join(", ");
    [
        t.text.trim().to_owned(),
        t.caption.trim().to_owned(),
        joined(t.links.iter().map(|l| l.name.as_str()).collect()),
        joined(t.files.iter().chain(&t.attachments).map(|f| f.name.as_str()).collect()),
        t.name.trim().to_owned(),
    ]
    .into_iter()
    .find(|s| !s.is_empty())
    .map(|s| s.chars().take(120).collect())
    .unwrap_or_default()
}

/// One of a turn's `links`: a connected app's file the AI made or changed
/// (`provider` "gdrive"), opened by its `link`.
#[derive(Clone, PartialEq, Default)]
pub struct DriveLink {
    pub provider: String,
    pub id: String,
    pub name: String,
    pub mime: String,
    pub link: String,
    pub action: String,
    /// Its Library kind ("document", …) and type's name ("Google Doc").
    pub kind: String,
    pub file_type: String,
    /// A v1 tool result's open (provider `buwiz-app`, 9.6): its card.
    pub card: Option<serde_json::Value>,
}

impl DriveLink {
    fn of(v: &serde_json::Value) -> Self {
        Self {
            provider: s(v, "provider"),
            id: s(v, "id"),
            name: s(v, "name"),
            mime: s(v, "mime"),
            link: s(v, "link"),
            action: s(v, "action"),
            kind: s(v, "kind"),
            file_type: s(v, "type"),
            card: v.get("card").filter(|c| c.is_object()).cloned(),
        }
    }
}

/// A model of `GET /models` for the picker — `modelpicker.tsx`'s
/// `Model` (`efforts`, `defaultEffort`, `fast`).
#[derive(Clone, PartialEq, Default)]
pub struct ModelDef {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub fast: bool,
    pub efforts: Vec<(String, String)>,
    pub default_effort: String,
    /// Reaches an image model (opencodex: image output, or the ChatGPT
    /// images relay every OpenAI-family model gets).
    pub image: bool,
    /// The route's display name ("OpenAI", "Anthropic", ...) and whether
    /// it is a subscription login (listed ungrouped) or an API connection
    /// (grouped under `group`).
    pub group: String,
    pub subscription: bool,
}

/// One Library artifact of a thread (`GET /thread/apps` — the server's
/// `artifacts.mjs` record). `app` is its id: `R<n>` for a mini app or an
/// image card, `file:<path>` for a file a reply made.
#[derive(Clone, PartialEq, Default)]
pub struct RoxApp {
    pub app: String,
    /// "app" (a mini app), "image" (an image card: `file` is its png — or
    /// an image file, with `attach`), "document", "presentation",
    /// "spreadsheet", "pdf", "media" or "file" (anything else); every file
    /// kind carries its viewer record in `attach`.
    pub kind: String,
    pub title: String,
    pub file: String,
    /// The display file name (an edit never renames the file on disk).
    pub filename: String,
    pub description: String,
    /// A user-set icon's URL; empty = the kind's default tile.
    pub icon: String,
    pub meta: ArtMeta,
    /// An image's pixel size, a mini app's window size.
    pub w: f64,
    pub h: f64,
    pub attach: Option<Attach>,
    /// The message that made it (its index in the chat): Show in Chat.
    pub i: i64,
    /// The chat's and the turn's permanent ids — Copy Link's pair.
    pub thread_uid: String,
    pub turn_id: String,
    /// `mtime` in epoch ms — the Library groups on it (Today / week / Older).
    pub mtime: f64,
    /// The current version pointer; 0 hides the `v<n>` chip.
    pub v: i64,
    /// `published` — the app is in Mini Apps (the menu's Promote/Remove).
    pub published: bool,
    /// Archived: out of the Library's lists, under its Archived chip (the
    /// chat card and its groups keep it).
    pub archived: bool,
    pub versions: Vec<AppVersion>,
    /// Where it lives; only "mac" has a local file.
    pub storage: ArtStorage,
    /// Who keeps its versions: "rox" (the `v<n>` tree), "provider" (the
    /// connected app's own history) or "none".
    pub versioning: String,
    /// An `mcp-app` item (a server's token attached here): the token, the
    /// `buwiz://app/` link it opens, its picture and what the server tells
    /// of it.
    pub token: String,
    pub open: String,
    pub image: String,
    pub details: McpDetails,
}

/// An attached token's `details` (its resolve's): a name and subtitle,
/// a picture (empty: a tile drawn from the name), its versions (each a
/// link) and facts.
#[derive(Clone, PartialEq, Default)]
pub struct McpDetails {
    pub name: String,
    pub subtitle: String,
    pub image: String,
    /// (label, detail, link)
    pub versions: Vec<(String, String, String)>,
    pub facts: Vec<(String, String)>,
}

impl McpDetails {
    pub(crate) fn of(v: Option<&serde_json::Value>) -> Self {
        let Some(v) = v.filter(|v| v.is_object()) else { return Self::default() };
        Self {
            name: s(v, "name"),
            subtitle: s(v, "subtitle"),
            image: s(v, "image"),
            versions: list(v, "versions").iter().map(|x| (s(x, "label"), s(x, "detail"), s(x, "open"))).collect(),
            facts: list(v, "facts")
                .iter()
                .filter_map(|f| {
                    let f = f.as_array()?;
                    let text = |x: Option<&serde_json::Value>| match x {
                        Some(serde_json::Value::String(t)) => t.clone(),
                        Some(x) if !x.is_null() => x.to_string(),
                        _ => String::new(),
                    };
                    Some((text(f.first()), text(f.get(1))))
                })
                .collect(),
        }
    }
}

/// A Library record's `storage`: `place` is the server's `where` — "mac"
/// (a file Rox wrote here, `uri` file://), "connector" (a connected app's
/// file: `provider` "gdrive", opened by `link`), or the not-yet-made
/// "agent" / "cloud". Open, download and thumbnails branch on it.
#[derive(Clone, PartialEq, Default)]
pub struct ArtStorage {
    pub place: String,
    pub uri: String,
    pub provider: String,
    pub link: String,
}

impl ArtStorage {
    fn of(v: Option<&serde_json::Value>) -> Self {
        let Some(v) = v.filter(|v| v.is_object()) else {
            return Self { place: "mac".to_owned(), ..Self::default() };
        };
        Self { place: s(v, "where"), uri: s(v, "uri"), provider: s(v, "provider"), link: s(v, "link") }
    }

    /// Kept by a connected app (Google Drive) rather than on this Mac.
    pub fn connector(&self) -> bool {
        self.place == "connector"
    }

    /// "Google Drive" for a connector's provider, else the place's name.
    pub fn place_name(&self) -> &'static str {
        match (self.place.as_str(), self.provider.as_str()) {
            ("connector", "gdrive") => "Google Drive",
            ("connector", _) => "A connected app",
            ("agent", _) => "The agent's computer",
            ("cloud", _) => "Buwiz Bot Cloud",
            _ => "This Mac",
        }
    }
}

impl RoxApp {
    /// One `artifacts.mjs` record (`/thread/apps`' rows, a group's
    /// artifact members).
    fn from_json(a: &serde_json::Value) -> Self {
        let meta = ArtMeta::of(a.get("meta"));
        let origin = a.get("origin").cloned().unwrap_or_default();
        let details = McpDetails::of(a.get("details"));
        RoxApp {
            // an attachment (`mcp-app`) may go by its `id`
            app: Some(s(a, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| s(a, "id")),
            kind: s(a, "kind"),
            title: s(a, "title"),
            file: s(a, "file"),
            filename: s(a, "filename"),
            description: s(a, "description"),
            icon: s(a, "icon"),
            // an app's window size rides `meta`
            w: Some(f64of(a, "w")).filter(|w| *w > 0.0).unwrap_or(meta.w),
            h: Some(f64of(a, "h")).filter(|h| *h > 0.0).unwrap_or(meta.h),
            meta,
            attach: a.get("attach").filter(|x| x.is_object()).map(Attach::of),
            // an attachment's card is its `origin`'s message
            i: a.get("i").or_else(|| origin.get("i")).and_then(|x| x.as_i64()).unwrap_or(-1),
            thread_uid: s(&origin, "uid"),
            turn_id: s(&origin, "turn"),
            mtime: f64of(a, "mtime"),
            v: a.get("v").and_then(|x| x.as_i64()).unwrap_or(0),
            published: b(a, "published"),
            archived: b(a, "archived"),
            versions: list(a, "versions").iter().map(AppVersion::from_json).collect(),
            storage: ArtStorage::of(a.get("storage")),
            versioning: s(a, "versioning"),
            token: s(a, "token"),
            open: s(a, "open"),
            // an attachment's picture rides `icon` (its `details.image`)
            image: [s(a, "image"), s(a, "icon"), details.image.clone()].into_iter().find(|i| !i.is_empty()).unwrap_or_default(),
            details,
        }
    }
}

/// A Library record's `meta`: what the server can tell cheaply — the
/// file's size and extension, a PDF's / Word file's pages, a deck's slides,
/// a workbook's sheet names, media length, what drew an image.
#[derive(Clone, PartialEq, Default)]
pub struct ArtMeta {
    pub size: f64,
    pub ext: String,
    pub pages: i64,
    pub slides: i64,
    pub sheets: Vec<String>,
    pub duration: f64,
    pub w: f64,
    pub h: f64,
    pub made_with: String,
    /// A connector's file: the app ("Google Drive"), the file's type
    /// ("Google Doc"), its mime and what the AI last did to it ("created",
    /// "edited", "renamed").
    pub provider: String,
    pub file_type: String,
    pub mime: String,
    pub action: String,
}

impl ArtMeta {
    fn of(v: Option<&serde_json::Value>) -> Self {
        let Some(v) = v.filter(|v| v.is_object()) else { return Self::default() };
        Self {
            size: f64of(v, "size"),
            ext: s(v, "ext"),
            pages: i64of(v, "pages"),
            slides: i64of(v, "slides"),
            sheets: list(v, "sheets").iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
            duration: f64of(v, "duration"),
            w: f64of(v, "w"),
            h: f64of(v, "h"),
            made_with: s(v, "made_with"),
            provider: s(v, "provider"),
            file_type: s(v, "type"),
            mime: s(v, "mime"),
            action: s(v, "action"),
        }
    }
}

/// Edit Details' source of truth: `/app/info` for a mini app,
/// `/artifact/info` for an image or a file.
fn details_url(id: &str, kind: &str) -> String {
    if kind == "app" {
        format!("/app/info?name={id}")
    } else {
        format!("/artifact/info?id={}", client::urlenc(id))
    }
}

/// The Library's kind chips after All, in order.
pub const LIB_KINDS: [(&str, &str); 8] = [
    ("app", "Apps"),
    ("image", "Images"),
    ("document", "Docs"),
    ("presentation", "Slides"),
    ("spreadsheet", "Sheets"),
    ("pdf", "PDFs"),
    ("media", "Media"),
    ("file", "Other"),
];

/// A Library record's chip; an unknown kind is Other, an attached
/// server app (`mcp-app`) an app.
pub fn lib_kind_of(a: &RoxApp) -> &str {
    if a.kind == "mcp-app" {
        "app"
    } else if LIB_KINDS.iter().any(|(k, _)| *k == a.kind) {
        &a.kind
    } else {
        "file"
    }
}

/// One `GET /apps/meta` row: what every mini-app tile and title reads
/// (the generated icon, the edited title / description / window size).
#[derive(Clone, PartialEq, Default)]
pub struct MiniMeta {
    pub name: String,
    pub title: String,
    pub file: String,
    pub description: String,
    /// The icon's URL on the server; empty = the default tile.
    pub icon: String,
    pub w: f32,
    pub h: f32,
}

/// `GET /app/info` — the App Details modal's read-only facts; an image's
/// or file's `GET /artifact/info` reads into the same shape (`kind` set,
/// `name` its Library id).
#[derive(Clone, PartialEq, Default)]
pub struct AppInfo {
    pub name: String,
    /// The Library kind; "app" for `/app/info`.
    pub kind: String,
    pub title: String,
    pub file: String,
    /// The display file name and its fixed extension.
    pub filename: String,
    pub ext: String,
    pub meta: ArtMeta,
    /// A thumbnail stands in when the icon is removed (images, and files
    /// with a picture).
    pub thumb: bool,
    /// The message that made it (-1 when unknown).
    pub i: i64,
    pub icon: String,
    pub v: i64,
    pub versions: i64,
    pub created_at: f64,
    pub updated_at: f64,
    pub prompt: String,
    pub path: String,
    pub bytes: f64,
    pub published: bool,
    pub thread_id: String,
    pub thread_title: String,
    pub bot_name: String,
    /// An icon is being drawn (`/app/icon/gen` answers before it lands).
    pub generating: bool,
    pub icon_error: String,
    pub history: Vec<AppVersion>,
    pub storage: ArtStorage,
    pub versioning: String,
    /// An `mcp-app` item's token, link, picture and details (read from
    /// its Library record, not `/app/info`).
    pub token: String,
    pub open: String,
    pub image: String,
    pub details: McpDetails,
}

impl AppInfo {
    /// An attached server app's details, from its Library record.
    fn of_mcp(a: &RoxApp) -> Self {
        Self {
            name: a.app.clone(),
            kind: a.kind.clone(),
            title: a.title.clone(),
            i: a.i,
            token: a.token.clone(),
            open: a.open.clone(),
            image: a.image.clone(),
            details: a.details.clone(),
            ..Default::default()
        }
    }

    fn from_json(v: &serde_json::Value) -> Self {
        let int = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) as i64;
        let kind = Some(s(v, "kind")).filter(|k| !k.is_empty()).unwrap_or_else(|| "app".to_owned());
        let filename = Some(s(v, "filename")).filter(|f| !f.is_empty()).unwrap_or_else(|| s(v, "file"));
        let meta = ArtMeta::of(v.get("meta"));
        let ext = if kind == "app" {
            "tsx".to_owned()
        } else if !meta.ext.is_empty() {
            meta.ext.clone()
        } else {
            crate::files::ext_of(&filename, "")
        };
        let attach = v.get("attach").filter(|x| x.is_object()).map(Attach::of);
        let thumb = match &attach {
            Some(a) => a.kind == "image" || !a.thumb.is_empty() || !a.poster.is_empty(),
            None => kind == "image",
        };
        Self {
            name: s(v, "name"),
            title: s(v, "title"),
            file: s(v, "file"),
            filename,
            ext,
            meta,
            thumb,
            i: v.get("i").and_then(|x| x.as_i64()).unwrap_or(-1),
            kind,
            icon: s(v, "icon"),
            v: int("v"),
            versions: int("versions"),
            created_at: f64of(v, "created_at"),
            updated_at: f64of(v, "updated_at"),
            prompt: s(v, "prompt"),
            path: s(v, "path"),
            bytes: f64of(v, "bytes"),
            published: self::b(v, "published"),
            thread_id: s(v, "thread_id"),
            thread_title: s(v, "thread_title"),
            bot_name: s(v, "bot_name"),
            generating: self::b(v, "generating"),
            icon_error: s(v, "icon_error"),
            history: list(v, "history").iter().map(AppVersion::from_json).collect(),
            storage: ArtStorage::of(v.get("storage")),
            versioning: s(v, "versioning"),
            ..Default::default()
        }
    }
}

/// A key/value pair of `GET /state/:app` for the detail pane.
#[derive(Clone, PartialEq)]
pub struct Kv(pub String, pub String);

/// `DetailMeta` — the detail pane's app card (`/catalog` row or a bare
/// `{name,title}` when unpublished): kinds/tags/published.
#[derive(Clone, PartialEq, Default)]
pub struct AppMeta {
    pub name: String,
    pub title: String,
    pub kinds: Vec<String>,
    pub tags: Vec<String>,
    pub published: bool,
}

/// `AppVersion` — one node of a library row's version tree
/// (`/thread/apps`'s `versions`).
#[derive(Clone, PartialEq, Default)]
pub struct AppVersion {
    pub v: i64,
    pub label: String,
    pub parent: Option<i64>,
    pub created_at: f64,
}

impl AppVersion {
    pub(crate) fn from_json(vr: &serde_json::Value) -> Self {
        Self {
            v: vr.get("v").and_then(|x| x.as_i64()).unwrap_or(0),
            label: s(vr, "label"),
            parent: vr.get("parent").and_then(|x| x.as_i64()),
            created_at: f64of(vr, "created_at"),
        }
    }
}

/// The version picker, open under a `v<n>` chip at (`x`, `y`).
#[derive(Clone, PartialEq)]
pub struct VersPop {
    pub app: String,
    pub x: f32,
    pub y: f32,
    pub cur: i64,
    pub versions: Vec<AppVersion>,
}

/// A context-menu item (the custom card, not a native menu) — the TSX's
/// `CtxItem`: label/icon/red checked rows, `sep` hairlines and `submenu`
/// flyouts. `act`/`arg` are the dispatch pair (`"verb"` + payload).
#[derive(Clone, PartialEq)]
pub struct CtxItem {
    pub label: String,
    pub icon: String,
    pub danger: bool,
    pub checked: bool,
    pub sep: bool,
    /// The toggle value for flip verbs (hide → true, show → false).
    pub flag: bool,
    pub act: String,
    pub arg: String,
    pub submenu: Vec<CtxItem>,
}

impl CtxItem {
    /// A labelled row.
    pub fn row(label: &str, icon: &str, act: &str, arg: String) -> Self {
        Self {
            label: label.to_owned(),
            icon: icon.to_owned(),
            danger: false,
            checked: false,
            sep: false,
            flag: false,
            act: act.to_owned(),
            arg,
            submenu: Vec::new(),
        }
    }

    /// `flag` — the toggle's target value.
    pub fn to(mut self, v: bool) -> Self {
        self.flag = v;
        self
    }

    /// `checked` — a submenu row's current membership mark.
    pub fn on(mut self, v: bool) -> Self {
        self.checked = v;
        self
    }

    /// A red destructive row.
    pub fn danger(label: &str, icon: &str, act: &str, arg: String) -> Self {
        let mut it = Self::row(label, icon, act, arg);
        it.danger = true;
        it
    }

    /// A hairline separator (`{sep: true}` in the TSX).
    pub fn separator() -> Self {
        let mut it = Self::row("", "", "", String::new());
        it.sep = true;
        it
    }

    /// A row that opens a flyout (`submenu`).
    pub fn parent(label: &str, icon: &str, submenu: Vec<CtxItem>) -> Self {
        let mut it = Self::row(label, icon, "", String::new());
        it.submenu = submenu;
        it
    }
}

/// The popover the shell shows on right-click / long-press / "more".
#[derive(Clone, PartialEq)]
pub struct CtxMenu {
    pub x: f32,
    pub y: f32,
    pub items: Vec<CtxItem>,
}

/// `railPop` — the bot hover card in the rail / pinned tiles
/// (App.tsx:738): `cy` is the hovered element's measured vertical
/// center, `x` its right edge plus the 6px gap.
#[derive(Clone, PartialEq)]
pub struct RailPop {
    pub bot: RoxBot,
    pub cy: f32,
    pub x: f32,
    /// A mini app's card instead (its bare name; `bot` carries the title
    /// and description): the app's icon replaces the bot's mark.
    pub app: String,
}

/// `agentRuns[thread]` — the run live in a thread (App.tsx:113):
/// `bot` is the credited persona (None when the thread has none), the
/// mark plays `state`, the status row reads `label`.
#[derive(Clone, PartialEq)]
pub struct AgentRun {
    pub state: String,
    pub label: String,
    pub bot: Option<String>,
}

/// The transient "job settled" flash (agent finished / failed): the bot's
/// mark plays `state` for a few seconds (`settle()` in App.tsx).
#[derive(Clone, PartialEq)]
pub struct Settle {
    pub name: String,
    pub ok: bool,
    /// The bot that ran (`agentRuns`' `bot`), for `bot_st`.
    pub bot: String,
    /// The state its mark shows while settled (`finished` / `failed`).
    pub state: String,
}

/// `PillAnchor` (shell.tsx:575) — the one emoji pill's measured bar rect
/// in page coords, whether it flips above / right-anchors, and the turn
/// it reacts to. `slot` names the owning bar — `"msg"` or
/// `"art:<path>"` — so only that bar stays lit while the pill is open.
#[derive(Clone, PartialEq)]
pub struct PillAnchor {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub flip: bool,
    pub right: bool,
    pub i: i64,
    pub slot: String,
    pub current: String,
}

/// A sidebar group (`GET /groups`): the user's named, colored, foldable
/// list of bots, chats and artifacts, in their order. An item may sit in
/// several groups.
#[derive(Clone, PartialEq, Default)]
pub struct RoxGroup {
    pub id: String,
    pub name: String,
    pub color: String,
    pub collapsed: bool,
    pub items: Vec<GroupItem>,
}

/// One member of a group: `kind` "bot" | "chat" | "artifact" | "app",
/// `id` the bot's id, the chat's thread id, the artifact's (`R<n>` |
/// `file:<path>`) or a server's mini app's module name
/// (`mcp:<server>/<app>[|<props>]`, a pinned `buwiz://app/` link). An
/// artifact carries its origin chat and its Library record — it opens
/// from whatever chat is open; an app its title.
#[derive(Clone, PartialEq, Default)]
pub struct GroupItem {
    pub kind: String,
    pub id: String,
    pub thread: String,
    pub artifact: Option<RoxApp>,
    pub title: String,
}

impl RoxGroup {
    fn from_json(v: &serde_json::Value) -> Self {
        Self {
            id: s(v, "id"),
            name: s(v, "name"),
            color: s(v, "color"),
            collapsed: b(v, "collapsed"),
            items: list(v, "items")
                .iter()
                .map(|it| GroupItem {
                    kind: s(it, "kind"),
                    id: s(it, "ref"),
                    thread: s(it, "thread"),
                    artifact: it.get("artifact").filter(|a| a.is_object()).map(RoxApp::from_json),
                    title: s(it, "title"),
                })
                .collect(),
        }
    }
}

/// A group member as the sidebar draws it, resolved against the live
/// bot and chat lists (a bot's or chat's own row look).
#[derive(Clone, PartialEq)]
pub enum GroupRow {
    Bot(RoxBot),
    Chat(ThreadMeta),
    Artifact(RoxApp),
    /// A server's mini app: its module name and title.
    App(String, String),
}

impl GroupRow {
    /// The row's `each` key — unique within a group.
    pub fn key(&self) -> String {
        match self {
            GroupRow::Bot(b) => format!("bot:{}", b.id),
            GroupRow::Chat(t) => format!("chat:{}", t.id),
            GroupRow::Artifact(a) => format!("artifact:{}", a.app),
            GroupRow::App(key, _) => format!("app:{key}"),
        }
    }
}

/// What the name prompt does on OK: make a group (with the item the menu
/// was opened on — kind, id, origin chat — as its first member) or rename
/// group `id`.
#[derive(Clone, PartialEq)]
pub enum GroupPrompt {
    New(Option<(String, String, String)>),
    Rename(String),
    /// One of the fixed sections ("__un" Unassigned, "__chats" Chats).
    RenameSection(String),
}

/// One row of the command palette (`PaletteItem`, palette.tsx:10).
#[derive(Clone, PartialEq)]
pub struct PaletteItem {
    pub title: String,
    pub subtitle: String,
    /// An icon name (an action's).
    pub icon: String,
    pub checked: bool,
    pub act: PaletteAct,
    /// An agent's row: drawn with its avatar.
    pub bot: Option<RoxBot>,
    /// A chat's row: its letter circle on this name's hue (the sidebar's).
    pub letter_of: String,
    /// A grouped mini app's row: its `</>` tile on this name's hue.
    pub app_of: String,
}

/// What a palette row runs (`run`, palette.tsx:16).
#[derive(Clone, PartialEq)]
pub enum PaletteAct {
    BotModal,
    /// The current bot's details: view (false) or edit (true).
    BotDetails(bool),
    Settings,
    NewChat,
    Theme(&'static str),
    /// A mini app in a sidebar group: open it in its window.
    App(String),
    /// Open a chat — an agent's session or a plain chat.
    Thread(String),
    /// A `buwiz://` link: a chat at a message (Show in Chat's places).
    Link(String),
    /// Attach a token to a chat (a bot's main chat): (token, thread, its
    /// name).
    Attach(String, String, String),
}

/// One thing a bot remembers (`memories` on a `/threads` bot).
#[derive(Clone, PartialEq, Default)]
pub struct BotMemory {
    pub id: String,
    pub text: String,
}

/// An integration a bot has used or was given (`integrations`).
#[derive(Clone, PartialEq, Default)]
pub struct BotIntegration {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// What it does and its logo (`/plugins/icon/…`, "" when none).
    pub description: String,
    pub icon: String,
}

impl BotIntegration {
    pub fn of(i: &serde_json::Value) -> Self {
        Self { id: s(i, "id"), name: s(i, "name"), kind: s(i, "kind"), description: s(i, "description"), icon: s(i, "icon") }
    }
}

/// The composer reply target. `files` are what the replied-to message
/// carries — a card's `module://R<n>[@v<k>]`, a file's path — sent along as
/// attachments so the model sees them (App.tsx:580).
#[derive(Clone, PartialEq, Default)]
pub struct ReplyTo {
    pub i: i64,
    pub name: String,
    pub text: String,
    pub files: Vec<String>,
}

/// Where a chat was scrolled when it was left (`ScrollMark`s in
/// `scroll_marks`): the turn at the viewport's top and how far into it,
/// or the end; `last` is the newest turn then, so newer ones are news.
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub struct ScrollMark {
    pub i: i64,
    pub into: f32,
    pub at_end: bool,
    pub last: i64,
}

/// One chat's unsent composer work (Grok Bot's per-agent `drafts`):
/// its text, the message it replies to and its attachments.
#[derive(Clone, PartialEq, Default)]
pub struct Draft {
    pub text: String,
    pub reply: Option<ReplyTo>,
    pub attachments: Vec<Attach>,
}

impl Draft {
    fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.reply.is_none() && self.attachments.is_empty()
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "text": self.text,
            "reply": self.reply.as_ref().map(|r| serde_json::json!({"i": r.i, "name": r.name, "text": r.text, "files": r.files})),
            "attachments": self.attachments.iter().map(Attach::to_json).collect::<Vec<_>>(),
        })
    }

    fn from_json(v: &serde_json::Value) -> Draft {
        Draft {
            text: s(v, "text"),
            reply: v.get("reply").filter(|r| r.is_object()).map(|r| ReplyTo {
                i: r.get("i").and_then(|x| x.as_i64()).unwrap_or(-1),
                name: s(r, "name"),
                text: s(r, "text"),
                files: list(r, "files").iter().filter_map(|f| f.as_str().map(str::to_owned)).collect(),
            }),
            attachments: list(v, "attachments").iter().map(Attach::of).collect(),
        }
    }
}

/// `open` (modelpicker.tsx:57) — the model popover's mode and the pill's
/// page rect it anchors to.
#[derive(Clone, PartialEq)]
pub struct ModelsOpen {
    /// "effort" | "list"
    pub mode: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// One `/artifacts` row for the @-mention picker
/// (`{kind, name, title, file, v, uri, scope}`).
#[derive(Clone, PartialEq)]
pub struct AtItem {
    pub kind: String,
    pub name: String,
    pub title: String,
    pub file: String,
    pub v: i64,
    pub uri: String,
    pub scope: String,
}

/// "Nm", "Nh", "Nh Nm" — how long a run status has shown.
fn elapsed_label(mins: u64) -> String {
    match (mins / 60, mins % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// Where the dev hook's local turns number from: above any server turn.
pub(crate) const LOCAL_TURN_BASE: i64 = 1_000_000_000;

/// `PHASE_STATE` (App.tsx:35) — a generation phase → a Grok agent state.
fn phase_state(phase: &str) -> &'static str {
    match phase {
        "thinking" => "thinking",
        "searching" => "searching",
        "browsing" => "reading",
        "reading" => "reading",
        "connecting" => "orbit",
        "writing" => "writing",
        "coding" => "coding",
        "generating" => "progress",
        "running-commands" => "running",
        "on-its-computer" => "computing",
        "on-your-computer" => "controlling",
        "working" => "working",
        "messaging" => "sending",
        "sending" => "sending",
        "waiting" => "waiting",
        "queued" => "working",
        "tool" => "working",
        "streaming" => "streaming",
        "checking" => "running",
        "retrying" => "writing",
        "fallback" => "working",
        "publishing" => "uploading",
        _ => "working",
    }
}

/// `activeBot()`'s merge: the open thread's `bot`, completed from the
/// catalog row when the persona exists there.
fn active_bot_of(id: &str, threads: &[ThreadMeta], bots: &[RoxBot]) -> Option<RoxBot> {
    let tb = threads.iter().find(|t| t.id == id)?.bot.as_ref()?;
    Some(
        bots.iter()
            .find(|b| b.id == tb.id)
            .cloned()
            .unwrap_or_else(|| RoxBot {
                id: tb.id.clone(),
                name: tb.name.clone(),
                shape: tb.shape.clone(),
                color: tb.color.clone(),
                ..Default::default()
            }),
    )
}

/// How many chats keep their transcript mounted (switching among them
/// is instant and keeps the scroll position exactly).
const ALIVE: usize = 3;

/// The window zoom's range, in percent (one step is 1%).
const ZOOM_MIN: i32 = 50;
const ZOOM_MAX: i32 = 300;

/// Whether turn `t` is the one `key` names: its permanent `id`, or (old
/// links) its index.
fn turn_is(t: &Turn, key: &str) -> bool {
    (!t.id.is_empty() && t.id == key)
        || t.i.to_string() == key
        // an agents' notice names the message (`msg_…`), not its turn
        || t.message.as_ref().is_some_and(|m| m.id == key)
}

/// `buwiz://chat/<thread uid|id>[?m=<turn id|index>]` → (thread, turn key).
fn parse_chat_link(url: &str) -> Option<(String, Option<String>)> {
    let rest = url.strip_prefix("buwiz://chat/")?;
    let (thread, query) = rest.split_once('?').unwrap_or((rest, ""));
    let thread = thread.trim_end_matches('/');
    if thread.is_empty() {
        return None;
    }
    let key = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("m="))
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    Some((thread.to_owned(), key))
}

/// The app a module key names: `R12@v2` → `R12`.
pub fn app_name(app: &str) -> &str {
    app.split('@').next().unwrap_or(app)
}

/// `/popout`'s body for module key `app`: `name` bare, a pinned version
/// as the number `v`.
fn popout_body(app: &str) -> serde_json::Value {
    let mut body = serde_json::json!({ "name": app });
    if let Some((n, v)) = app.split_once('@') {
        body["name"] = serde_json::json!(n);
        if let Ok(v) = v.trim_start_matches('v').parse::<i64>() {
            body["v"] = serde_json::json!(v);
        }
    }
    body
}

fn s(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned()
}
fn i64of(v: &serde_json::Value, k: &str) -> i64 {
    v.get(k).and_then(|x| x.as_i64()).unwrap_or_default()
}
fn f64of(v: &serde_json::Value, k: &str) -> f64 {
    v.get(k).and_then(|x| x.as_f64()).unwrap_or_default()
}
fn b(v: &serde_json::Value, k: &str) -> bool {
    v.get(k).and_then(|x| x.as_bool()).unwrap_or_default()
}
fn list(v: &serde_json::Value, k: &str) -> Vec<serde_json::Value> {
    v.get(k).and_then(|x| x.as_array()).cloned().unwrap_or_default()
}

impl Attach {
    /// What [`Attach::of`] reads back (a saved draft's attachments).
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind, "path": self.path, "name": self.name, "mime": self.mime,
            "url": self.url, "thumb": self.thumb, "ext": self.ext, "size": self.size,
            "feed": self.feed, "variant": self.variant, "duration": self.duration,
            "peaks": self.peaks, "width": self.width, "height": self.height,
            "poster": self.poster, "animated": self.animated,
        })
    }

    fn of(v: &serde_json::Value) -> Attach {
        Attach {
            kind: s(v, "kind"),
            path: s(v, "path"),
            name: s(v, "name"),
            mime: s(v, "mime"),
            // attachments carry `uri` (a full URL), reply files carry
            // `path` (a /file/raw key) — `url` accepts either spelling.
            url: if s(v, "url").is_empty() { s(v, "uri") } else { s(v, "url") },
            thumb: s(v, "thumb"),
            ext: s(v, "ext"),
            size: f64of(v, "size"),
            feed: s(v, "feed"),
            variant: s(v, "variant"),
            duration: f64of(v, "duration"),
            peaks: list(v, "peaks")
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0))
                .collect(),
            width: f64of(v, "width"),
            height: f64of(v, "height"),
            poster: s(v, "poster"),
            animated: b(v, "animated"),
        }
    }
}

impl Turn {
    pub(crate) fn of(v: &serde_json::Value) -> Turn {
        let files = list(v, "files");
        let mut attachments: Vec<Attach> = list(v, "attachments").iter().map(Attach::of).collect();
        attachments.extend(list(v, "art").iter().map(|a| Attach {
            kind: s(a, "kind"),
            path: s(a, "path"),
            name: s(a, "title"),
            mime: s(a, "mime"),
            ..Attach::default()
        }));
        Turn {
            i: i64of(v, "i"),
            id: s(v, "id"),
            role: s(v, "role"),
            text: s(v, "text"),
            name: s(v, "name"),
            label: s(v, "label"),
            caption: s(v, "caption"),
            reaction: s(v, "reaction"),
            widget: v.get("widget").and_then(|x| x.as_bool()).unwrap_or(false),
            v: i64of(v, "v"),
            // `reply` arrives `null` on ordinary turns — `get` still
            // returns `Some(&Null)`, so the null check is explicit.
            reply: v
                .get("reply")
                .filter(|r| !r.is_null())
                .map(|r| Reply {
                    name: s(r, "name"),
                    text: s(r, "text"),
                    i: i64of(r, "i"),
                }),
            attachments,
            files: files.iter().map(Attach::of).collect(),
            // A turn names a module in `name` ("R15") and describes the
            // artifact in `art`; `v` pins a version (`R15@v3`). The shell's
            // rule: `tt.name` set → app card, else message — and a building
            // artifact (no name until it lands) is a card already, so its
            // placeholder shows instead of an empty bubble.
            art: (!s(v, "name").is_empty()
                || v.get("building").and_then(|x| x.as_bool()).unwrap_or(false))
            .then(|| {
                let a = v.get("art").cloned().unwrap_or(serde_json::Value::Null);
                let a = &a;
                let name = s(v, "name");
                let ver = i64of(v, "v");
                Art {
                    // Grok's key is `v ? name@v<k> : widget ? name+"w" :
                    // name`, but the `w` suffix is Grok's *bundled
                    // registry* namespacing — `/module?name=` only accepts
                    // `R<n>` / `R<n>@v<k>` and searches WIDGETS_DIR itself
                    // for a bare name, so widget turns fetch `name` too.
                    app: if ver > 0 { format!("{name}@v{ver}") } else { name },
                    file: s(a, "file"),
                    title: s(a, "title"),
                    desc: s(a, "desc"),
                    kind: s(a, "kind"),
                    w: f64of(a, "w"),
                    h: f64of(a, "h"),
                }
            }),
            building: v.get("building").and_then(|x| x.as_bool()).unwrap_or(false),
            status: s(v, "status"),
            revising: v.get("revising").and_then(|x| x.as_bool()).unwrap_or(false),
            links: list(v, "links").iter().map(DriveLink::of).filter(|l| !l.id.is_empty()).collect(),
            cards: v.get("cards").map(crate::cards::parse_list).unwrap_or_default(),
            suggest: list(v, "suggest")
                .iter()
                .map(|x| (s(x, "label"), s(x, "prompt")))
                .filter(|(l, p)| !l.is_empty() && !p.is_empty())
                .take(3)
                .collect(),
            v1_attachments: list(v, "v1Attachments")
                .iter()
                .filter_map(|a| {
                    let token = s(a, "token");
                    crate::mini_v1::tokens::canonical(&token).map(|_| (token.clone(), s(a, "title")))
                })
                .collect(),
            notice: (s(v, "role") == "notice").then(|| v.get("notice").and_then(RoutineNotice::of)).flatten(),
            consent: (s(v, "role") == "notice").then(|| v.get("notice").and_then(|n| ConsentNotice::of(n, s(v, "text")))).flatten(),
            instruction_from: if s(v, "role") == "user" {
                v.get("routine").filter(|r| r.is_object()).map(|r| s(r, "name")).unwrap_or_default()
            } else {
                String::new()
            },
            message: (s(v, "role") == "agent_message").then(|| v.get("message").and_then(crate::agents::AgentMessage::of)).flatten(),
            agents_notice: (s(v, "role") == "notice")
                .then(|| v.get("notice").and_then(|n| crate::agents::AgentsNotice::of(n, s(v, "text"))))
                .flatten(),
            reply_from: match s(v, "role").as_str() {
                "user" => v.get("agent").and_then(|a| crate::agents::ReplyFrom::of(a, false)),
                "ai" => v.get("agent").and_then(|a| crate::agents::ReplyFrom::of(a, true)),
                _ => None,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// Every signal of the shell's model. Created once by
/// [`crate::live::store`] under the root owner; hot code reaches it by
/// `store()`.
pub struct Store {
    /// Sidebar data.
    pub threads: Signal<Vec<ThreadMeta>>,
    pub bots: Signal<Vec<RoxBot>>,
    /// The sidebar's groups (`GET /groups`), in the user's order.
    pub groups: Signal<Vec<RoxGroup>>,
    /// `collapsedSecs` — `__hidden`, `__hiddenChats`, `__archived` start
    /// collapsed; the rest start open (absent ⇒ false). A group's key is
    /// its id, its value the server's `collapsed`.
    pub collapsed: Signal<BTreeMap<String, bool>>,

    /// The open thread and its content.
    pub active_id: Signal<String>,
    pub turns: Signal<Vec<Turn>>,
    pub revealed: Signal<bool>,
    pub last_status: Signal<String>,

    /// Composer.
    pub draft: Signal<String>,
    pub attachments: Signal<Vec<Attach>>,
    pub reply_to: Signal<Option<ReplyTo>>,
    /// Every other chat's draft, by thread id (`track_drafts`): the open
    /// chat's lives in `draft` / `reply_to` / `attachments` themselves.
    /// Kept in `/state/rox-drafts`.
    pub drafts: std::cell::RefCell<std::collections::BTreeMap<String, Draft>>,
    /// Reactions on a reply's files and media, by `thread:turn:slot`
    /// (`slot` = `art:<path>` / `art:gallery`): the server keeps one per
    /// turn, so these live in its state store (`/state/rox-reactions`).
    pub art_reactions: Signal<std::collections::BTreeMap<String, String>>,
    /// `open` (modelpicker.tsx:57) — the model popover's mode and the
    /// pill's page rect it anchors to.
    pub models_pop: Signal<Option<ModelsOpen>>,
    /// `dockH` — the floating composer's measured height; the transcript
    /// pads its end by it (App.tsx:179).
    pub composer_dock_h: Signal<f32>,
    /// `rightW` — the right-controls row's width; the input's right
    /// padding reserves it (App.tsx:490).
    pub composer_right_w: Signal<f32>,
    /// `plusMenu` — the + button's measured page position.
    pub plus_menu: Signal<Option<(f32, f32)>>,
    /// `plusHover` — the + menu row under the pointer.
    pub plus_hover: Signal<String>,
    /// `attachHover` — hovered tile's uri, or "__dictate" / "__voice".
    pub attach_hover: Signal<String>,
    /// `atItems` — the @-mention picker's rows; empty closes it.
    pub at_items: Signal<Vec<AtItem>>,
    /// `atStart` — the draft offset the active @-query began at (-1 = none).
    pub at_start: Signal<i64>,
    /// `atSeq` — stale `/artifacts` answers are dropped by generation.
    pub at_seq: Signal<u64>,
    pub at_open: Signal<bool>,
    /// The commands, `@` scopes and apps MCP servers announce
    /// (`GET /mcp/manifest`, slash.rs).
    pub mcp_manifest: Signal<Vec<crate::slash::Manifest>>,
    /// The composer popup's own rows (commands, servers, scopes, values,
    /// a resolve's choices; the artifacts ride `at_items`), its header and
    /// the selected row.
    pub pop_rows: Signal<Vec<crate::slash::PopRow>>,
    pub pop_head: Signal<String>,
    pub pop_sel: Signal<usize>,
    /// The composer text the popup's rows were computed for: Return takes
    /// a row only while that is still the text (D20, slash.rs).
    pub pop_query: std::cell::RefCell<String>,
    /// When `GET /mcp/manifest` was last asked; whether it has answered.
    pub mcp_asked: std::cell::Cell<Option<std::time::Instant>>,
    pub mcp_loaded: std::cell::Cell<bool>,
    /// Stale `@` value completions are dropped by generation.
    pub complete_seq: std::cell::Cell<u64>,
    /// A picked `@` token's label (its chip shows it).
    pub token_labels: std::cell::RefCell<std::collections::HashMap<String, String>>,

    /// Sending / jobs: the chats whose message is on its way or being
    /// answered. Chats run in parallel: only the busy chat waits.
    pub sending: Signal<std::collections::BTreeSet<String>>,
    /// `agentRuns` — the live run per thread; a thread's status row and
    /// its bot's mark read their own entry (App.tsx:113).
    pub agent_runs: Signal<BTreeMap<String, AgentRun>>,
    /// Each interactive card's state by its key (`cards::CardState`): no
    /// entry is still open. Kept while Rox runs; a form's typed values are
    /// never here.
    pub card_states: Signal<std::collections::HashMap<String, crate::cards::CardState>>,
    /// A choice card's picks by card key (field id → option).
    pub card_picks: Signal<std::collections::HashMap<String, BTreeMap<String, String>>>,
    /// Plugin-needs cards whose "Remember for <bot>" is on.
    pub card_remember: Signal<std::collections::BTreeSet<String>>,
    /// The dev hook's card turns (`card <path>`): (chat, the server turn
    /// they follow, the turn). Never sent or saved; merged into every
    /// `/turns` fetch of their chat.
    pub local_turns: std::cell::RefCell<Vec<(String, i64, Turn)>>,
    /// `emphBot` — the bot id hovered through the status row; "" when off.
    /// Emphasizes both the status mark and the matching sidebar mark.
    pub emph_bot: Signal<String>,
    pub settled: Signal<Option<Settle>>,

    /// Models.
    pub models: Signal<Vec<ModelDef>>,
    pub model: Signal<String>,
    pub default_model: Signal<String>,
    pub effort: Signal<String>,
    pub fast: Signal<bool>,

    /// Layout.
    pub side_mode: Signal<String>,
    /// `sideShown` — `mod+b` toggles it; `sideEff = shown ? mode : "hidden"`.
    pub side_shown: Signal<bool>,
    pub right_open: Signal<bool>,
    pub detail_tab: Signal<String>,
    /// `detailState` — the open app's `/state/:app` k/v rows.
    pub detail: Signal<Vec<Kv>>,
    /// `detail` — the open app's `/catalog` metadata (variant B's header).
    pub detail_meta: Signal<Option<AppMeta>>,
    pub detail_app: Signal<String>,
    pub thread_apps: Signal<Vec<RoxApp>>,
    /// The Library's kind chip ("all", "app", "image", "document", …),
    /// kept for the session.
    pub lib_filter: Signal<String>,
    pub tab: Signal<String>,

    /// Overlays.
    pub ctx: Signal<Option<CtxMenu>>,
    /// The open menu's keyboard state: the row whose flyout is open (-1
    /// none), whether the keys act in that flyout, the highlighted row of
    /// the card the keys act in (-1 none).
    pub ctx_sub: Signal<i64>,
    pub ctx_in_sub: Signal<bool>,
    pub ctx_sel: Signal<i64>,
    pub palette: Signal<bool>,
    /// `query`/`sel` — the ⌘K palette's search text + keyboard index
    /// (palette.tsx:22-23).
    pub palette_q: Signal<String>,
    pub palette_sel: Signal<i64>,
    /// The palette as a picker: its own rows in place of the sessions and
    /// actions (the places an item is attached, the chats and bots to
    /// attach one to), and the search field's prompt.
    pub palette_pick: Signal<Option<Vec<PaletteItem>>>,
    pub palette_hint: Signal<String>,
    /// `pillAnchor` (shell.tsx:582) — the open emoji pill's anchor, or
    /// None. One app-level overlay renders it.
    pub pill: Signal<Option<PillAnchor>>,
    /// The hover bar (turn, slot) whose ⋯ opened the `ctx` menu: that bar
    /// stays up until the menu closes (the scrim takes the hover).
    pub ctx_owner: Signal<Option<(i64, String)>>,
    /// `work` / `extra` (shell.tsx:1106) — each card's status line, by
    /// module key: a retry's phase, or a short note ("↗ window opened");
    /// the flag marks a note.
    pub card_status: Signal<std::collections::BTreeMap<String, (String, bool)>>,
    /// The Library row a card's ⋯ → Details highlights (an app name).
    pub library_focus: Signal<String>,
    /// ⌘= / ⌘- / ⌘0 — the window's zoom (1 = 100%), as Grok Bot's page
    /// zoom: everything scales (`zoom.rs`); kept in `/state/rox-zoom`.
    pub ui_zoom: Signal<f32>,
    /// `pillGuard` (shell.tsx:570) — the pill's touch marks it before the
    /// root's bubble-phase handler runs, so a press inside the pill isn't
    /// read as a click-away. The root consumes and clears it per touch.
    pub pill_guard: std::cell::Cell<bool>,
    /// `viewing` (fileviewer.tsx) — the file the fullscreen viewer shows.
    pub viewing: Signal<Option<Attach>>,
    /// The mini apps an MCP server ships, open full screen, bottom first:
    /// an app opened over another stacks on it, ✕ / ⌘W takes the top one
    /// off.
    pub mini_full: Signal<Vec<MiniOpen>>,
    /// The last open's number (each open mounts a new app).
    pub mini_seq: std::cell::Cell<u64>,
    /// Dev hook: `<count>|<field> <text>` to type into the open mini app.
    pub mini_dev_type: Signal<String>,
    /// `rendered` — `GET /file/render`'s answer for `viewing`.
    pub rendered: Signal<Option<Rendered>>,
    /// `sheet` — the workbook tab open.
    pub sheet_idx: Signal<usize>,
    /// `htmlMode` — an html file's Preview / Source toggle.
    pub html_mode: Signal<String>,
    /// `selA`/`selB` — the sheet selection's anchor and moving end (r,c).
    pub sel_a: Signal<Option<(usize, usize)>>,
    pub sel_b: Signal<Option<(usize, usize)>>,
    /// `hintOpen` — the sheet grid's usage hint.
    pub hint_open: Signal<bool>,
    /// `audio_cur` — the id of the card/player currently playing; a card
    /// pauses when another claims it (`stopAudio`/`stopCurrent`).
    pub audio_cur: Signal<u64>,
    /// Lightbox (lightbox.tsx): the open strip, its index, the image zoom,
    /// the measured stage, and a diagram's reported card size.
    pub light_items: Signal<Vec<LightItem>>,
    pub light_idx: Signal<usize>,
    pub light_zoom: Signal<f64>,
    pub light_stage: Signal<(f64, f64)>,
    /// (the model popover's `open` lives in `models.rs` via `live::keep` —
    /// `models_open` is gone with the old card.)
    /// The group name prompt (New Group…, Rename), or None.
    pub group_prompt: Signal<Option<GroupPrompt>>,
    /// The prompt's draft.
    pub group_name: Signal<String>,
    /// The user's keyboard shortcuts: command id → keystrokes, only those
    /// changed from the defaults (`keymap.json`, `GET/POST /keymap`).
    pub keymap: Signal<std::collections::BTreeMap<String, Vec<String>>>,
    /// The user's diagram colors over Rox's (`--pele-*` token → color, per
    /// mode; `mermaid.colors`, kept in `/state/rox-mermaid` as `colors`;
    /// absent until set: the defaults).
    pub mermaid_colors: Signal<crate::mermaid::palette::Overrides>,
    /// Bot Details: the bot and mode, its page stack ("main", "context",
    /// "integrations", "add", "memory:<id>"), and the edit drafts.
    pub bot_details: Signal<Option<crate::botdetails::BotDetails>>,
    pub bd_nav: Signal<Vec<String>>,
    pub bd_desc: Signal<String>,
    pub bd_instr: Signal<String>,
    pub bd_mem: Signal<String>,
    /// Settings (⌘,) and its open page ("keyboard").
    pub settings_open: Signal<bool>,
    pub settings_page: Signal<String>,
    /// Settings › Keyboard is recording a key for (command id, slot).
    pub key_record: Signal<Option<(String, usize)>>,
    /// The open chat's message position (its turn index), for ⌘↑ / ⌘↓,
    /// ⌘U and ⌘Y: kept per chat (`sel_marks`), moved by a link to a
    /// message and, after a scroll settles, to the bottom message in view.
    /// The ring shows only while the keys walk it (`sel_shown`).
    pub sel_msg: Signal<Option<i64>>,
    pub sel_shown: Signal<bool>,
    pub sel_marks: std::cell::RefCell<std::collections::BTreeMap<String, i64>>,
    /// When the keys last moved the position (their scroll isn't the
    /// user's).
    pub sel_stepped: std::cell::Cell<Option<std::time::Instant>>,
    /// The selected message's ring, for placing its menu (⌘U).
    pub sel_ring: std::cell::Cell<Option<ElementRef>>,
    /// Settings › Keyboard's search and its last note; the highlighted
    /// shortcut (-1 none) and `$` suggestion.
    pub settings_query: Signal<String>,
    pub settings_sel: Signal<i64>,
    pub settings_sug: Signal<i64>,
    pub settings_note: Signal<String>,
    /// The server's own (v0) routines (`GET /routines`), for Details.
    pub routines: Signal<Vec<crate::routines::Routine>>,
    /// Run now in flight or just done, by routine (`routines::v1_key`,
    /// `v0_key`).
    pub routine_runs: Signal<Vec<(String, crate::routines::RunState)>>,
    /// A webhook instance's URL, just made: (instance, URL), shown once.
    pub routine_hook: Signal<Option<(String, String)>>,
    /// The instance whose params are being edited, and the JSON typed.
    pub routine_edit: Signal<Option<(String, String)>>,
    /// Consent cards answered here, before the chat reloads: (instance,
    /// "enabled" | "declined").
    pub routine_consented: Signal<Vec<(String, String)>>,
    /// The templates connected servers suggest (`GET /routines/templates`);
    /// the one whose Enable is asking for the Google address (server, id),
    /// and what's typed there.
    pub routine_templates: Signal<Vec<crate::routines::RoutineTemplate>>,
    pub routine_ask_email: Signal<Option<(String, String)>>,
    pub routine_email: Signal<String>,
    /// Agents' conversations' last messages, for a bot's Details:
    /// (thread, its turn count when read, message count, last message).
    pub agents_last: Signal<Vec<(String, i64, i64, String)>>,
    /// Conversations the user resumed here (their notices stop offering
    /// Resume before `/threads` says so).
    pub agents_resumed: Signal<Vec<String>>,
    /// The last menu action asked for, numbered (`rox/keys` sends it).
    pub keys_send: Signal<(u64, String)>,
    /// The last print job, numbered: (n, "text" | "html" | "image", payload,
    /// title).
    pub keys_print: Signal<(u64, String, String, String)>,
    /// The viewer's mini app code: (app, its versions, the current one).
    pub viewer_versions: Signal<(String, Vec<i64>, i64)>,
    /// The Office file open in a native view (`office/`).
    pub(crate) office: crate::office::Ui,
    /// The ra-jit stats pill's switch (⌘⇧R), set by the hot area.
    pub livebar: std::cell::Cell<Option<Signal<bool>>>,
    /// Color › Custom…: the group whose color is being picked, and the
    /// picker's `#rrggbb` draft.
    pub color_edit: Signal<Option<String>>,
    pub color_draft: Signal<String>,
    /// The fixed sections' names (`/prefs` unassigned_name, chats_name).
    pub unassigned_name: Signal<String>,
    pub chats_name: Signal<String>,
    /// `railPop` — the bot hover card's target, or None.
    pub rail_pop: Signal<Option<RailPop>>,
    /// `popSeq` — the measure-response generation: a stale measure (the
    /// pointer already left) must not open the card.
    pub pop_seq: std::cell::Cell<u64>,
    /// The transcript's `virtual_list` handle — the reply-quote jump
    /// scrolls it (`onJump`, shell.tsx:454).
    pub chat_list: std::cell::RefCell<Option<ElementRef>>,
    /// A jump to turn `i` of a thread waiting for that thread's
    /// transcript to mount: it then opens on the turn (`take_jump`).
    pub jump: std::cell::RefCell<Option<(String, String)>>,
    /// A link opened before the chats were loaded (Rox launched by it):
    /// it waits for them (`boot`), else the startup selection replaced
    /// its chat and its turn was never found. True when it came from
    /// another app ([`open_os_link`]).
    pub early_link: std::cell::RefCell<Option<(String, bool)>>,
    /// Whether `boot` has loaded the chats.
    pub booted: std::cell::Cell<bool>,
    /// Bumped by every navigation (`go_to_key`): a transcript's opening
    /// snaps stop once it moves, so they never undo a jump.
    pub nav_epoch: std::cell::Cell<u64>,
    /// The chat `turns` belong to: on a switch the old transcript stays up
    /// until the new chat's turns land, then a fresh one mounts (keyed by
    /// this) already at its place and fades in.
    pub turns_of: Signal<String>,
    /// The chats whose transcripts stay mounted (the ⌘1–9 chats and the
    /// `ALIVE` most recent others), in the order they came: switching among
    /// them only shows and hides. Kept in that order, not by recency:
    /// moving a mounted transcript in the list re-inserts its whole view
    /// tree (20–45 ms a switch).
    pub alive: Signal<Vec<String>>,
    /// The chats opened, most recent first: what `alive` keeps beyond the
    /// ⌘1–9 chats.
    pub recent_chats: std::cell::RefCell<Vec<String>>,
    /// Each chat's last scroll position, kept while Rox runs.
    pub scroll_marks: std::cell::RefCell<std::collections::HashMap<String, ScrollMark>>,
    /// The first turn that arrived while the open chat was away, until
    /// it is seen (the "New messages" pill).
    pub new_below: Signal<Option<i64>>,
    /// The turn a jump landed on, ringed for a moment (`flash`), and
    /// whether its ring still shows (it fades before it unmounts).
    pub highlight: Signal<Option<i64>>,
    pub highlight_on: Signal<bool>,
    /// `botModal` — the "Create New Bot" sheet is open.
    pub bot_modal: Signal<bool>,
    /// The primary Bot (Grok Bot's "Main Bot", its star badge): your
    /// main contact. Kept in `/state/rox-main-bot`.
    pub main_bot: Signal<String>,
    /// The "Choose a primary Bot" dialog: open with the bot picked so far.
    pub main_pick: Signal<Option<String>>,
    /// A link waiting on Open or Cancel in Rox's sheet
    /// (`botmodal::link_ask_layer`).
    pub link_ask: Signal<Option<crate::links::LinkAsk>>,
    /// What Rox keeps for v1 plugins (`mini_v1::ui`).
    pub v1: crate::mini_v1::ui::V1,
    /// The draft was rewritten for v1 plugins already: the next `send`
    /// sends it as it is.
    pub v1_resolved: std::cell::Cell<bool>,
    pub main_pick_q: Signal<String>,
    /// `botName`/`botShape`/`botColor` — the picker's draft.
    pub bot_name: Signal<String>,
    pub bot_shape: Signal<String>,
    pub bot_color: Signal<String>,
    /// `botHover` — the sheet's hovered control ("close", "cancel",
    /// "s:<shape>").
    pub bot_hover: Signal<String>,

    // ————— the right pane —————
    /// `paneBtnHover` — "theme" | "close" | "" on the pane's top buttons.
    pub pane_btn_hover: Signal<String>,
    /// `avatarHover`/`avatarEditing`/`avatarEditTab`/`avatarErr` — the
    /// profile avatar's editor (App.tsx:206-208, 3163).
    pub avatar_hover: Signal<bool>,
    pub avatar_editing: Signal<bool>,
    pub avatar_edit_tab: Signal<String>,
    pub avatar_err: Signal<String>,
    /// `botEditName` — the profile name field's draft, re-seeded by
    /// `seed_bot_edit_name` when the open bot (or its name) changes;
    /// `bot_edit_src` is the `(id,name)` the current draft came from
    /// (App.tsx:256-259).
    pub bot_edit_name: Signal<String>,
    pub bot_edit_src: std::cell::RefCell<Option<(String, String)>>,
    /// The avatar draft picked/generated for the crop editor
    /// (`draft`/`draftCrop`): `{uri,name,w,h}`.
    pub avatar_draft: Signal<Option<AvatarDraft>>,
    pub draft_crop: Signal<Crop>,
    /// `mcpServers` — the connect tab's server rows.
    pub mcp_servers: Signal<Vec<McpServer>>,
    /// `versOpen` — the library row's expanded version tree (app name).
    pub vers_pop: Signal<Option<VersPop>>,
    /// `appRename`/`appRenameText` — the rename prompt (App.tsx:1637).
    pub app_rename: Signal<Option<AppRename>>,
    pub app_rename_text: Signal<String>,
    /// `/apps/meta` by bare app name (`R17`).
    pub app_meta: Signal<std::collections::HashMap<String, MiniMeta>>,
    /// The App Details modal: open while `Some`; the `ad_*` drafts are
    /// its fields, `ad_busy` names the running job ("gen" | "pick" |
    /// "save"), `ad_err` its last failure.
    pub app_details: Signal<Option<AppInfo>>,
    pub ad_title: Signal<String>,
    pub ad_file: Signal<String>,
    pub ad_desc: Signal<String>,
    pub ad_w: Signal<String>,
    pub ad_h: Signal<String>,
    pub ad_prompt: Signal<String>,
    pub ad_busy: Signal<String>,
    pub ad_err: Signal<String>,
    /// `avatarGenPrompt`/`avatarGenBusy` — the Generate tab of the avatar
    /// editor (App.tsx:211).
    pub avatar_gen_prompt: Signal<String>,
    pub avatar_gen_busy: Signal<bool>,

    // ————— Plugins marketplace (App.tsx:1291–1640 + the modal) —————
    /// `pluginsOpen` — the modal's visibility.
    pub plugins_open: Signal<bool>,
    /// `pluginNav` — the nav stack; `[{page:"market"}]` at open.
    pub plugin_nav: Signal<Vec<PluginNav>>,
    /// `plugins` — the `/plugins/catalog` rows.
    pub plugins: Signal<Vec<PluginRow>>,
    /// `pluginDetails` — `/plugins/detail` per plugin name.
    pub plugin_details: Signal<BTreeMap<String, PluginDetail>>,
    /// `skillBodies` — `/plugins/skill` per `plugin/skill` key.
    pub skill_bodies: Signal<BTreeMap<String, SkillBody>>,
    /// `privateSkills` — `/skills`, the user's own skills.
    pub private_skills: Signal<Vec<PrivateSkill>>,
    /// `mcpToolsMap` — `/mcp/tools` per server id.
    pub mcp_tools: Signal<BTreeMap<String, Vec<McpTool>>>,
    /// `toolsOpenIds` — expanded "N of M enabled" sections.
    pub tools_open: Signal<BTreeMap<String, bool>>,
    /// `pluginBusy` — install/uninstall in flight (plugin name).
    pub plugin_busy: Signal<String>,
    /// Grok Bot's category labels in its order (`/plugins/catalog`), and
    /// the catalog's revision (an unchanged poll skips the rebuild).
    pub plugin_cat_order: Signal<Vec<String>>,
    /// How many rows a category / search list shows (Show more adds 60).
    pub plugin_limit: Signal<usize>,
    pub plugins_rev: Signal<String>,
    /// The Setup page's draft values (field key → text) and its error.
    pub setup_vals: Signal<BTreeMap<String, String>>,
    pub setup_err: Signal<String>,
    /// `pluginCat`/`pluginQuery` — the market's category pill + search.
    pub plugin_cat: Signal<String>,
    pub plugin_query: Signal<String>,
    /// `dropSel`/`dropHidden`/`fieldW` — the search combo box state.
    pub drop_sel: Signal<usize>,
    pub drop_hidden: Signal<bool>,
    pub field_w: Signal<f32>,
    /// Hover signals — `hoverPlugin`/`hoverAdd`/`hoverInst` names;
    /// `hoverChip`/`hoverTpl`/`hoverX` booleans.
    pub hover_plugin: Signal<String>,
    pub hover_add: Signal<String>,
    pub hover_inst: Signal<String>,
    pub hover_chip: Signal<bool>,
    pub hover_tpl: Signal<bool>,
    pub hover_x: Signal<bool>,
    /// `skillsExpanded`/`showAllInstalled` — page-local expands.
    pub skills_expanded: Signal<bool>,
    pub show_all_installed: Signal<bool>,
    /// `psId`/`psName`/`psDesc`/`psBody` — the private-skill editor draft.
    pub ps_id: Signal<String>,
    pub ps_name: Signal<String>,
    pub ps_desc: Signal<String>,
    pub ps_body: Signal<String>,
    /// The plugin page's account editing: `acctEdit` (row id being
    /// renamed) + its draft, `acctAdd` (connector whose "Add Another
    /// Account" form is open) + its draft, `acctConfirm` (row id awaiting
    /// a second press on Remove/Disconnect), `acctErr` (last error).
    pub acct_edit: Signal<String>,
    pub acct_edit_val: Signal<String>,
    pub acct_add: Signal<String>,
    pub acct_add_val: Signal<String>,
    pub acct_confirm: Signal<String>,
    pub acct_err: Signal<String>,
    /// Share / copy-link feedback on the plugin page ("Copied").
    pub plugin_copied: Signal<bool>,
    /// `accountOpen` — the account popover (rail plate / sidebar footer).
    pub account_open: Signal<bool>,

    /// `themePref` — "system" | "dark" | "light" (theme.ts). The shown
    /// scheme is the engine's: `ambient::dark()` resolves System for you.
    pub theme_pref: Signal<String>,
    /// `chatScrolling` (shell.tsx) — set while the transcript scrolls;
    /// hover handlers skip writes then (every write is a commit).
    pub chat_scrolling: Signal<bool>,
    /// `startClock`'s shared ~30fps tick — every animating canvas
    /// (blobs, status fx, shimmers) subscribes to this one signal
    /// instead of spawning its own interval.
    pub frame_clock: Signal<f64>,
    frame_started: std::cell::Cell<bool>,
    /// A canvas read the clock since the last tick: keep ticking.
    frame_want: std::cell::Cell<bool>,
    /// The detached root the ticker runs under — no component owns it,
    /// so unmounting the avatar that started it cannot stop it.
    frame_owner: std::cell::Cell<Option<Owner>>,

    /// The dedupe cache: raw JSON of the last /turns payload, so identical
    /// polls don't dirty the list.
    pub turns_json: std::cell::RefCell<String>,
    /// The `/threads` payload's lists and `/groups`, kept as raw JSON the
    /// same way — an unchanged poll must not re-render the sidebar
    /// (App.tsx's `threadsJ`/`botsJ` compare, App.tsx:384).
    threads_list_json: std::cell::RefCell<String>,
    bots_json: std::cell::RefCell<String>,
    pub(crate) groups_json: std::cell::RefCell<String>,

    /// Compiled mini-app sources, per module name (`R<n>`): the JS the
    /// `rox/mini-app` component loads. Filled lazily by [`mini_source`].
    pub minis: std::cell::RefCell<std::collections::HashMap<String, Signal<String>>>,
    /// Display titles of mini apps (from `/module`), for card headers.
    pub mini_titles: std::cell::RefCell<std::collections::HashMap<String, Signal<String>>>,
    /// Per-module load/compile errors, shown in the card.
    pub mini_errs: std::cell::RefCell<std::collections::HashMap<String, Signal<String>>>,
    /// `cardHeights` (shell.tsx) — each mini app's measured natural
    /// height, so a recycled row frames at it at once instead of the
    /// declared height and then growing.
    pub mini_heights: std::cell::RefCell<std::collections::HashMap<String, f32>>,
    /// When each generation placeholder first showed (`dotgrid.rs`), by
    /// key: its simulated progress counts from there, so a re-rendered
    /// row picks up where it was.
    pub art_started: std::cell::RefCell<std::collections::HashMap<String, std::time::Instant>>,

    /// The owner the store was made under (the anchor's — it outlives
    /// remounts). Lazily-made signals must live under it, not under
    /// whatever binding happened to ask first.
    owner: Owner,
}

impl Store {
    pub fn new() -> Store {
        Store {
            threads: signal(Vec::new()),
            bots: signal(Vec::new()),
            groups: signal(Vec::new()),
            collapsed: signal(BTreeMap::from([
                ("__hidden".to_owned(), true),
                ("__hiddenChats".to_owned(), true),
                ("__archived".to_owned(), true),
            ])),
            active_id: signal(String::new()),
            turns: signal(Vec::new()),
            revealed: signal(false),
            last_status: signal(String::new()),
            draft: signal(String::new()),
            attachments: signal(Vec::new()),
            reply_to: signal(None),
            drafts: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            art_reactions: signal(std::collections::BTreeMap::new()),
            models_pop: signal(None),
            composer_dock_h: signal(96.0),
            composer_right_w: signal(150.0),
            plus_menu: signal(None),
            plus_hover: signal(String::new()),
            attach_hover: signal(String::new()),
            at_items: signal(Vec::new()),
            at_start: signal(-1),
            at_seq: signal(0),
            mcp_manifest: signal(Vec::new()),
            pop_rows: signal(Vec::new()),
            pop_head: signal(String::new()),
            pop_sel: signal(0),
            pop_query: std::cell::RefCell::new(String::new()),
            mcp_asked: std::cell::Cell::new(None),
            mcp_loaded: std::cell::Cell::new(false),
            complete_seq: std::cell::Cell::new(0),
            token_labels: std::cell::RefCell::new(std::collections::HashMap::new()),
            at_open: signal(false),
            sending: signal(std::collections::BTreeSet::new()),
            agent_runs: signal(BTreeMap::new()),
            card_states: signal(std::collections::HashMap::new()),
            card_picks: signal(std::collections::HashMap::new()),
            card_remember: signal(std::collections::BTreeSet::new()),
            local_turns: std::cell::RefCell::new(Vec::new()),
            emph_bot: signal(String::new()),
            settled: signal(None),
            models: signal(Vec::new()),
            model: signal(String::new()),
            default_model: signal(String::new()),
            effort: signal(String::new()),
            fast: signal(false),
            side_mode: signal("full".to_owned()),
            side_shown: signal(true),
            right_open: signal(false),
            detail_tab: signal("state".to_owned()),
            detail: signal(Vec::new()),
            detail_app: signal(String::new()),
            detail_meta: signal(None),
            thread_apps: signal(Vec::new()),
            lib_filter: signal("all".to_owned()),
            tab: signal("chat".to_owned()),
            ctx: signal(None),
            ctx_sub: signal(-1),
            ctx_in_sub: signal(false),
            ctx_sel: signal(-1),
            group_prompt: signal(None),
            group_name: signal(String::new()),
            keymap: signal(Default::default()),
            mermaid_colors: signal(Default::default()),
            bot_details: signal(None),
            bd_nav: signal(vec!["main".to_owned()]),
            bd_desc: signal(String::new()),
            bd_instr: signal(String::new()),
            bd_mem: signal(String::new()),
            settings_open: signal(false),
            settings_page: signal("keyboard".to_owned()),
            key_record: signal(None),
            sel_msg: signal(None),
            sel_shown: signal(false),
            sel_marks: Default::default(),
            sel_stepped: std::cell::Cell::new(None),
            sel_ring: std::cell::Cell::new(None),
            settings_query: signal(String::new()),
            settings_sel: signal(-1),
            settings_sug: signal(0),
            settings_note: signal(String::new()),
            routines: signal(Vec::new()),
            routine_runs: signal(Vec::new()),
            routine_hook: signal(None),
            routine_edit: signal(None),
            routine_consented: signal(Vec::new()),
            routine_templates: signal(Vec::new()),
            routine_ask_email: signal(None),
            routine_email: signal(String::new()),
            agents_last: signal(Vec::new()),
            agents_resumed: signal(Vec::new()),
            keys_send: signal((0, String::new())),
            keys_print: signal((0, String::new(), String::new(), String::new())),
            viewer_versions: signal((String::new(), Vec::new(), 0)),
            office: crate::office::Ui::new(),
            livebar: std::cell::Cell::new(None),
            color_edit: signal(None),
            color_draft: signal(String::new()),
            unassigned_name: signal("Unassigned".to_owned()),
            chats_name: signal("Chats".to_owned()),
            rail_pop: signal(None),
            pop_seq: std::cell::Cell::new(0),
            chat_list: std::cell::RefCell::new(None),
            jump: std::cell::RefCell::new(None),
            early_link: std::cell::RefCell::new(None),
            booted: std::cell::Cell::new(false),
            nav_epoch: std::cell::Cell::new(0),
            turns_of: signal(String::new()),
            alive: signal(Vec::new()),
            recent_chats: std::cell::RefCell::new(Vec::new()),
            scroll_marks: std::cell::RefCell::new(std::collections::HashMap::new()),
            new_below: signal(None),
            highlight: signal(None),
            highlight_on: signal(false),
            bot_modal: signal(false),
            main_bot: signal(String::new()),
            main_pick: signal(None),
            link_ask: signal(None),
            v1: crate::mini_v1::ui::V1::new(),
            v1_resolved: std::cell::Cell::new(false),
            main_pick_q: signal(String::new()),
            bot_name: signal(String::new()),
            bot_shape: signal("blob".to_owned()),
            bot_color: signal("#1084FE".to_owned()),
            bot_hover: signal(String::new()),
            pane_btn_hover: signal(String::new()),
            avatar_hover: signal(false),
            avatar_editing: signal(false),
            avatar_edit_tab: signal("bot".into()),
            avatar_err: signal(String::new()),
            bot_edit_name: signal(String::new()),
            bot_edit_src: std::cell::RefCell::new(None),
            avatar_draft: signal(None),
            draft_crop: signal(Crop { s: 1.0, x: 0.0, y: 0.0 }),
            mcp_servers: signal(Vec::new()),
            vers_pop: signal(None),
            app_rename: signal(None),
            app_rename_text: signal(String::new()),
            app_meta: signal(std::collections::HashMap::new()),
            app_details: signal(None),
            ad_title: signal(String::new()),
            ad_file: signal(String::new()),
            ad_desc: signal(String::new()),
            ad_w: signal(String::new()),
            ad_h: signal(String::new()),
            ad_prompt: signal(String::new()),
            ad_busy: signal(String::new()),
            ad_err: signal(String::new()),
            avatar_gen_prompt: signal(String::new()),
            plugins_open: signal(false),
            plugin_nav: signal(vec![PluginNav::market()]),
            plugins: signal(Vec::new()),
            plugin_details: signal(BTreeMap::new()),
            skill_bodies: signal(BTreeMap::new()),
            private_skills: signal(Vec::new()),
            mcp_tools: signal(BTreeMap::new()),
            tools_open: signal(BTreeMap::new()),
            plugin_busy: signal(String::new()),
            plugin_cat: signal("All".to_owned()),
            plugin_cat_order: signal(Vec::new()),
            plugin_limit: signal(60),
            plugins_rev: signal(String::new()),
            setup_vals: signal(BTreeMap::new()),
            setup_err: signal(String::new()),
            plugin_query: signal(String::new()),
            drop_sel: signal(0),
            drop_hidden: signal(false),
            field_w: signal(0.0),
            hover_plugin: signal(String::new()),
            hover_add: signal(String::new()),
            hover_inst: signal(String::new()),
            hover_chip: signal(false),
            hover_tpl: signal(false),
            hover_x: signal(false),
            skills_expanded: signal(false),
            show_all_installed: signal(false),
            ps_id: signal(String::new()),
            ps_name: signal(String::new()),
            ps_desc: signal(String::new()),
            ps_body: signal(String::new()),
            acct_edit: signal(String::new()),
            acct_edit_val: signal(String::new()),
            acct_add: signal(String::new()),
            acct_add_val: signal(String::new()),
            acct_confirm: signal(String::new()),
            acct_err: signal(String::new()),
            plugin_copied: signal(false),
            account_open: signal(false),
            avatar_gen_busy: signal(false),
            palette: signal(false),
            palette_q: signal(String::new()),
            palette_sel: signal(0),
            palette_pick: signal(None),
            palette_hint: signal(String::new()),
            pill: signal(None),
            ctx_owner: signal(None),
            card_status: signal(std::collections::BTreeMap::new()),
            library_focus: signal(String::new()),
            ui_zoom: signal(1.0),
            pill_guard: std::cell::Cell::new(false),
            viewing: signal(None),
            mini_full: signal(Vec::new()),
            mini_seq: std::cell::Cell::new(0),
            mini_dev_type: signal(String::new()),
            rendered: signal(None),
            sheet_idx: signal(0usize),
            html_mode: signal("preview".to_owned()),
            sel_a: signal(None),
            sel_b: signal(None),
            hint_open: signal(true),
            audio_cur: signal(0u64),
            light_items: signal(Vec::new()),
            light_idx: signal(0usize),
            light_zoom: signal(1.0f64),
            light_stage: signal((0.0, 0.0)),
            // `models_open` removed — see the note on the struct.
            theme_pref: signal("dark".to_owned()),
            chat_scrolling: signal(false),
            frame_clock: signal(0.0),
            frame_started: std::cell::Cell::new(false),
            frame_want: std::cell::Cell::new(false),
            frame_owner: std::cell::Cell::new(None),
            turns_json: std::cell::RefCell::new(String::new()),
            threads_list_json: std::cell::RefCell::new(String::new()),
            bots_json: std::cell::RefCell::new(String::new()),
            groups_json: std::cell::RefCell::new(String::new()),
            minis: std::cell::RefCell::new(std::collections::HashMap::new()),
            mini_titles: std::cell::RefCell::new(std::collections::HashMap::new()),
            mini_errs: std::cell::RefCell::new(std::collections::HashMap::new()),
            mini_heights: std::cell::RefCell::new(std::collections::HashMap::new()),
            art_started: std::cell::RefCell::new(std::collections::HashMap::new()),
            owner: Owner::current(),
        }
    }

    // -- boot --------------------------------------------------------------

    /// The initial loads: threads, bots, models, prefs, groups, then the
    /// active thread's turns (`set_threads` picks the server's `active`).
    pub fn boot(&'static self) {
        self.track_drafts();
        crate::slash::boot();
        // v1 plugins: records, consent, the broker's events, the plugin list
        self.v1_boot();
        // The installed plugins' logos sit in the sidebar footer.
        self.fetch_plugins();
        self.fetch_app_meta();
        spawn(async move {
            if let Some(v) = client::get("/state/rox-main-bot").await {
                let id = v.get("state").map(|st| s(st, "id")).unwrap_or_default();
                self.main_bot.set(id);
            }
        });
        self.fetch_groups();
        spawn(async move {
            if let Some(v) = client::get("/state/rox-reactions").await
                && let Some(m) = v.get("state").and_then(|s| s.as_object()) {
                    self.art_reactions.set(
                        m.iter()
                            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                            .collect(),
                    );
            }
        });
        spawn(async move {
            // The zoom first: a transcript laid out before it lands is laid
            // out again at another size, and a link's turn drifts off.
            if let Some(v) = client::get("/state/rox-zoom").await {
                let z = v.get("state").map(|st| f64of(st, "zoom")).unwrap_or(0.0) as f32;
                if z > 0.0 {
                    self.ui_zoom.set(z.clamp(ZOOM_MIN as f32 / 100.0, ZOOM_MAX as f32 / 100.0));
                }
            }
            // Drafts before the chats: opening the active one restores its.
            if let Some(v) = client::get("/state/rox-drafts").await
                && let Some(m) = v.get("state").and_then(|st| st.get("drafts")).and_then(|d| d.as_object()) {
                    *self.drafts.borrow_mut() = m
                        .iter()
                        .map(|(k, d)| (k.clone(), Draft::from_json(d)))
                        .filter(|(_, d)| !d.is_empty())
                        .collect();
            }
            if let Some(v) = client::get("/threads").await {
                self.set_threads(v);
            }
            // Launched on a link (`open buwiz://…`, or `rox buwiz://…`): Rox
            // opens there once the chats are known.
            self.booted.set(true);
            let early = self.early_link.borrow_mut().take();
            if let Some((url, os)) = early
                .or_else(|| std::env::args().skip(1).find(|a| a.starts_with("buwiz://")).map(|u| (u, true)))
            {
                self.route_link(&url, os);
            }
            self.refresh_turns();
            self.refresh_thread_apps();
            // Bots ride the `/threads` payload (`bots`), not `/catalog`
            // (which is the widget shelf's data).
            if let Some(v) = client::get("/models").await {
                self.set_models(v);
            }
            if let Some(v) = client::get("/keymap").await {
                self.keymap.set(crate::keys::overrides_of(&v));
            }
            if let Some(v) = client::get("/state/rox-mermaid").await {
                self.mermaid_colors.set(crate::mermaid::palette::Overrides::from_state(&v));
            }
            if let Some(v) = client::get("/prefs").await {
                if let Some(m) = v.get("model").and_then(|x| x.as_str()) {
                    self.model.set(m.to_owned());
                }
                if let Some(e) = v.get("effort").and_then(|x| x.as_str()) {
                    self.effort.set(e.to_owned());
                }
                if let Some(f) = v.get("fast").and_then(|x| x.as_bool()) {
                    self.fast.set(f);
                }
                if let Some(t) = v.get("theme").and_then(|x| x.as_str()) {
                    self.theme_pref.set(t.to_owned());
                }
                for (k, sig) in [("unassigned_name", self.unassigned_name), ("chats_name", self.chats_name)] {
                    if let Some(n) = v.get(k).and_then(|x| x.as_str()).filter(|n| !n.trim().is_empty()) {
                        sig.set(n.to_owned());
                    }
                }
            }
        });
    }

    /// Runs `f` under the store's owner: signals made there live as long
    /// as the store (caches keyed outside any component).
    pub(crate) fn in_store<R>(&self, f: impl FnOnce() -> R) -> R {
        self.owner.run(f)
    }

    /// `startClock`/`clock()` — the shared ~30fps clock in seconds, read
    /// tracked: the canvas or binding reading it runs again next frame.
    /// Only what animates now reads it (blob.tsx reads `clock()` only
    /// while live), and the ticker stops one frame after nothing did, so
    /// a window of resting avatars costs no frames at all.
    pub fn tick(&'static self) -> f64 {
        self.frame_want.set(true);
        if !self.frame_started.replace(true) {
            let clock = self.frame_clock;
            let (owner, _) = nsh_core::element::create_root(|| {
                spawn(async move {
                    let st = crate::live::store();
                    // Real elapsed time on top of where the last run
                    // stopped: late ticks advance the marks by what passed,
                    // and a restart never jumps.
                    let base = clock.get_untracked();
                    let epoch = std::time::Instant::now();
                    loop {
                        timer(std::time::Duration::from_millis(33)).await;
                        if !st.frame_want.replace(false) {
                            break;
                        }
                        clock.set(base + epoch.elapsed().as_secs_f64());
                    }
                    st.frame_started.set(false);
                })
            });
            if let Some(done) = self.frame_owner.replace(Some(owner)) {
                done.dispose();
            }
        }
        self.frame_clock.get()
    }

    /// `setTheme` (App.tsx:765) — the pref and the POST.
    pub fn set_theme(&'static self, t: &str) {
        self.theme_pref.set(t.to_owned());
        client::post_bg("/prefs".to_owned(), serde_json::json!({"theme": t}));
    }

    /// `nextTheme` (theme.ts): Light → Dark → System → Light.
    pub fn next_theme(&self) -> &'static str {
        match self.theme_pref.get_untracked().as_str() {
            "light" => "dark",
            "dark" => "system",
            _ => "light",
        }
    }

    fn set_threads(&'static self, v: serde_json::Value) {
        // `threadsJ`/`botsJ` compares (App.tsx:384): a poll that changed
        // nothing must not re-render — and must not reseed drafts riding
        // on `bots`/`threads`.
        let list_j = v.get("list").map(|x| x.to_string()).unwrap_or_default();
        let bots_j = v.get("bots").map(|x| x.to_string()).unwrap_or_default();
        let threads_changed = list_j != *self.threads_list_json.borrow();
        let bots_changed = bots_j != *self.bots_json.borrow();
        if threads_changed {
            *self.threads_list_json.borrow_mut() = list_j;
        }
        if bots_changed {
            *self.bots_json.borrow_mut() = bots_j;
        }
        let rows: Vec<ThreadMeta> = list(&v, "list")
            .iter()
            .map(|t| ThreadMeta {
                id: s(t, "id"),
                uid: s(t, "uid"),
                // as stored: shown through `v1_plain` (no ticket), so a
                // title that lands later replaces `@short:scope`
                name: s(t, "title"),
                count: i64of(t, "count"),
                created: f64of(t, "created"),
                hidden: b(t, "hidden"),
                archived: b(t, "archived"),
                last_app: s(t, "lastApp"),
                routine: t.get("routine").and_then(RoutineOf::of),
                agents: t.get("agents").and_then(crate::agents::AgentsOf::of),
                bot: {
                    let b = &t["bot"];
                    if b.is_null() {
                        None
                    } else {
                        Some(ThreadBot {
                            id: s(b, "id"),
                            name: s(b, "name"),
                            shape: {
                                let v = s(b, "shape");
                                if v.is_empty() { "blob".into() } else { v }
                            },
                            color: {
                                let v = s(b, "color");
                                if v.is_empty() { "#FFFFFF".into() } else { v }
                            },
                        })
                    }
                },
            })
            .collect();
        // The personas (`bots` on the same payload) drive the Agents
        // section and `active_bot`/`bot_st`.
        if bots_changed {
            self.bots.set(
            list(&v, "bots")
                .iter()
                .map(|b| RoxBot {
                    id: s(b, "id"),
                    name: s(b, "name"),
                    emoji: s(b, "emoji"),
                    desc: {
                        let v = s(b, "preview");
                        let v = if v.is_empty() { s(b, "desc") } else { v };
                        if v.is_empty() { "No messages yet".into() } else { v }
                    },
                    shape: {
                        let v = s(&b["avatar"], "shape");
                        let v = if v.is_empty() { s(b, "shape") } else { v };
                        if v.is_empty() { "blob".into() } else { v }
                    },
                    color: {
                        let v = s(&b["avatar"], "color");
                        let v = if v.is_empty() { s(b, "color") } else { v };
                        if v.is_empty() { "#FFFFFF".into() } else { v }
                    },
                    thread_id: s(b, "threadId"),
                    image: s(b, "image"),
                    crop: {
                        let c = &b["crop"];
                        if c.is_null() {
                            None
                        } else {
                            Some(Crop {
                                s: f64of(c, "s"),
                                x: f64of(c, "x"),
                                y: f64of(c, "y"),
                            })
                        }
                    },
                    hidden: self::b(b, "hidden"),
                    pinned: self::b(b, "pinned"),
                    section: s(b, "section"),
                    model: s(b, "model"),
                    effort: s(b, "effort"),
                    fast: self::b(b, "fast"),
                    label: s(b, "label"),
                    description: s(b, "description"),
                    instructions: s(b, "instructions"),
                    memories: list(b, "memories")
                        .iter()
                        .map(|m| BotMemory { id: s(m, "id"), text: s(m, "text") })
                        .collect(),
                    integrations: list(b, "integrations")
                        .iter()
                        .map(BotIntegration::of)
                        .collect(),
                    updated_at: f64of(b, "updated_at"),
                    made_by: s(b, "made_by"),
                    purpose: s(b, "purpose"),
                    helper: b.get("helper").and_then(crate::agents::HelperOf::of),
                    accepts: crate::agents::accepts_of(b),
                })
                .collect(),
            );
        }
        // The server names its active thread; open that one on first load.
        // never an agents' conversation (read-only, reached from a chat)
        let active = crate::agents::restore_target(&s(&v, "active"), &rows, &self.bots.get_untracked());
        if !active.is_empty() && self.active_id.get_untracked().is_empty() {
            self.active_id.set(active);
        } else if !rows.is_empty() && self.active_id.get_untracked().is_empty() {
            self.active_id.set(rows[0].id.clone());
        }
        if threads_changed {
            self.threads.set(rows);
        }
        // `botEditName`'s seed effect (App.tsx:257) — with `bots` deduped
        // it only reseeds on a real bot/active change, never mid-typing.
        self.seed_bot_edit_name();
        // App.tsx's effect tracks `threads()` too — a refresh that lands
        // new app metadata (a run finishing) refetches the Library.
        if threads_changed || bots_changed {
            self.refresh_thread_apps();
        }
    }

    /// `activeBot()` — the bot owning the open thread, merged with the
    /// catalog row when one exists (the thread payload carries only
    /// `id/name/shape/color`; `bots` adds `image`/`crop`).
    pub fn active_bot(&'static self) -> Option<RoxBot> {
        let id = self.active_id.get();
        let threads = self.threads.get();
        let bots = self.bots.get();
        active_bot_of(&id, &threads, &bots)
    }

    /// `active_bot` without tracking — for one-shot reads inside element
    /// builders (a `when`'s `then`), where a tracked read would rebuild
    /// the branch on every `/threads` refresh.
    pub fn active_bot_untracked(&'static self) -> Option<RoxBot> {
        let id = self.active_id.get_untracked();
        let threads = self.threads.get_untracked();
        let bots = self.bots.get_untracked();
        active_bot_of(&id, &threads, &bots)
    }

    /// `pinnedBots()` — the rail tiles' set (pinned, not hidden).
    pub fn pinned_bots(&'static self) -> Vec<RoxBot> {
        self.bots
            .get()
            .into_iter()
            .filter(|b| b.pinned && !b.hidden && crate::agents::listed_bot(b))
            .collect()
    }

    /// `hiddenBots()` — the collapsed "Hidden Bots" section.
    pub fn hidden_bots(&'static self) -> Vec<RoxBot> {
        self.bots
            .get()
            .into_iter()
            .filter(|b| b.hidden && crate::agents::listed_bot(b))
            .collect()
    }

    /// Unassigned — visible, unpinned bots no group holds.
    pub fn ungrouped_bots(&'static self) -> Vec<RoxBot> {
        let grouped: std::collections::HashSet<String> = self.groups.with(|gs| {
            gs.iter()
                .flat_map(|g| g.items.iter())
                .filter(|it| it.kind == "bot")
                .map(|it| it.id.clone())
                .collect()
        });
        self.bots
            .get()
            .into_iter()
            .filter(|b| !b.hidden && !b.pinned && !grouped.contains(&b.id) && crate::agents::listed_bot(b))
            .collect()
    }

    /// Group `gid`'s members as the sidebar draws them: bots and chats
    /// from the live lists (hidden bots, hidden or archived chats and
    /// ones that are gone left out), artifacts as the server resolved them.
    pub fn group_rows(&'static self, gid: &str) -> Vec<GroupRow> {
        let Some(g) = self.groups.with(|gs| gs.iter().find(|g| g.id == gid).cloned()) else {
            return Vec::new();
        };
        let bots = self.bots.get();
        let threads = self.threads.get();
        g.items
            .iter()
            .filter_map(|it| match it.kind.as_str() {
                "bot" => bots.iter().find(|b| b.id == it.id && !b.hidden && crate::agents::listed_bot(b)).cloned().map(GroupRow::Bot),
                "chat" => threads
                    .iter()
                    .find(|t| t.id == it.id && !t.hidden && !t.archived && t.agents.is_none())
                    .cloned()
                    .map(GroupRow::Chat),
                "app" => Some(GroupRow::App(it.id.clone(), it.title.clone())),
                _ => it.artifact.clone().map(GroupRow::Artifact),
            })
            .collect()
    }

    /// Whether group `gid` holds item `kind`/`id`.
    pub fn in_group(&'static self, gid: &str, kind: &str, id: &str) -> bool {
        self.groups.with_untracked(|gs| {
            gs.iter()
                .find(|g| g.id == gid)
                .is_some_and(|g| g.items.iter().any(|it| it.kind == kind && it.id == id))
        })
    }

    /// The routine whose thread is open, when one is (tracked).
    pub fn active_routine(&'static self) -> Option<RoutineOf> {
        let id = self.active_id.get();
        self.threads.with(|t| t.iter().find(|t| t.id == id).and_then(|t| t.routine.clone()))
    }

    /// The open agents' conversation's mark, when one is open (tracked).
    pub fn active_agents(&'static self) -> Option<crate::agents::AgentsOf> {
        let id = self.active_id.get();
        self.threads.with(|t| t.iter().find(|t| t.id == id).and_then(|t| t.agents.clone()))
    }

    /// The names a conversation's participants go by: its bots', or what
    /// the open thread's messages call an outside endpoint (tracked).
    pub fn agents_names(&'static self, a: &crate::agents::AgentsOf) -> Vec<String> {
        let bots = self.bots.get();
        let seen = crate::agents::peers_in(self.turns.with(|t| t.iter().filter_map(|t| t.message.clone()).collect::<Vec<_>>()));
        a.participants.iter().map(|p| crate::agents::participant_name(p, &bots, &seen)).collect()
    }

    /// Bot `id`'s chat thread ("" when it has none).
    pub fn bot_thread(&'static self, id: &str) -> String {
        self.bots.with_untracked(|b| b.iter().find(|b| b.id == id).map(|b| b.thread_id.clone())).unwrap_or_default()
    }

    /// Archive or unarchive a conversation (`/v1/host/agents/archive`),
    /// then list the threads again.
    pub fn agents_archive(&'static self, conversation: String, archived: bool) {
        spawn_detached(async move {
            let r = client::post_any("/v1/host/agents/archive", serde_json::json!({"conversation": conversation, "archived": archived})).await;
            if r.as_ref().and_then(|v| v.get("ok")).and_then(|x| x.as_bool()) != Some(true) {
                log::warn!("agents archive {conversation}: {r:?}");
            }
            if let Some(v) = client::get("/threads").await {
                self.set_threads(v);
            }
        });
    }

    /// Resume a conversation the hourly cap paused
    /// (`/v1/host/agents/resume`).
    pub fn agents_resume(&'static self, conversation: String) {
        self.agents_resumed.update(|l| l.push(conversation.clone()));
        spawn_detached(async move {
            let r = client::post_any("/v1/host/agents/resume", serde_json::json!({"conversation": conversation})).await;
            if r.as_ref().and_then(|v| v.get("ok")).and_then(|x| x.as_bool()) != Some(true) {
                log::warn!("agents resume {conversation}: {r:?}");
                self.agents_resumed.update(|l| l.retain(|c| *c != conversation));
            }
            if let Some(v) = client::get("/threads").await {
                self.set_threads(v);
            }
        });
    }

    /// Whether conversation `c` is paused now (tracked): its row says so
    /// and the user hasn't resumed it here.
    pub fn agents_paused(&'static self, conversation: &str) -> bool {
        !self.agents_resumed.with(|l| l.iter().any(|c| c == conversation))
            && self.threads.with(|t| {
                t.iter().any(|t| t.agents.as_ref().is_some_and(|a| a.conversation == conversation && a.paused > 0.0))
            })
    }

    /// Whom bot `bot` takes messages from (`/v1/host/agents/accepts`):
    /// `all` or `none`.
    pub fn agents_accepts(&'static self, bot: String, accepts: &'static str) {
        self.bots.update(|bs| {
            if let Some(b) = bs.iter_mut().find(|b| b.id == bot) {
                b.accepts = accepts.to_owned();
            }
        });
        spawn_detached(async move {
            let r = client::post_any("/v1/host/agents/accepts", serde_json::json!({"bot": bot, "accepts": accepts})).await;
            if r.as_ref().and_then(|v| v.get("ok")).and_then(|x| x.as_bool()) != Some(true) {
                log::warn!("agents accepts {bot}: {r:?}");
            }
            if let Some(v) = client::get("/threads").await {
                self.set_threads(v);
            }
        });
    }

    /// Read the last message of each conversation in `rows` whose turn
    /// count changed since (`GET /turns?thread=`), for a bot's Details.
    pub fn load_agents_last(&'static self, rows: Vec<(String, i64)>) {
        let stale: Vec<(String, i64)> = self.agents_last.with_untracked(|l| {
            rows.into_iter().filter(|(t, n)| !l.iter().any(|(x, c, _, _)| x == t && c == n)).collect()
        });
        if stale.is_empty() {
            return;
        }
        spawn_detached(async move {
            for (thread, count) in stale {
                let Some(v) = client::get(&format!("/turns?thread={}", client::urlenc(&thread))).await else { continue };
                if s(&v, "thread") != thread {
                    continue;
                }
                let (n, last) = crate::agents::last_message(&v);
                self.agents_last.update(|l| {
                    l.retain(|(x, ..)| *x != thread);
                    l.push((thread.clone(), count, n, last));
                });
            }
        });
    }

    /// The open routine thread's quiet line (tracked): when it checked and
    /// what it found, or why it can't — and whether that is a failure.
    /// `None` outside a routine's thread, or before the plugins are listed.
    pub fn active_routine_status(&'static self) -> Option<(String, bool)> {
        let r = self.active_routine()?;
        let routine = self.v1.listed.with(|l| {
            l.iter().find(|p| p.id == r.plugin).and_then(|p| p.routines.iter().find(|x| x.id == r.id).cloned())
        })?;
        Some(routine.quiet_line(checked_at))
    }

    /// The routine thread of v1 routine `plugin`/`id`, once it is made.
    pub fn routine_thread(&'static self, plugin: &str, id: &str) -> Option<String> {
        self.threads.with_untracked(|t| {
            t.iter()
                .find(|t| t.routine.as_ref().is_some_and(|r| r.plugin == plugin && r.id == id))
                .map(|t| t.id.clone())
        })
    }

    /// `chats()` — visible chats: not bot-owned, not a routine's, not
    /// hidden, not archived.
    pub fn chats(&'static self) -> Vec<ThreadMeta> {
        self.threads
            .get()
            .into_iter()
            .filter(|t| crate::agents::listed_chat(t) && !t.hidden && !t.archived)
            .collect()
    }

    /// `hiddenChats()` / `archivedChats()` — the collapsed sections below.
    pub fn hidden_chats(&'static self) -> Vec<ThreadMeta> {
        self.threads
            .get()
            .into_iter()
            .filter(|t| crate::agents::listed_chat(t) && t.hidden)
            .collect()
    }

    pub fn archived_chats(&'static self) -> Vec<ThreadMeta> {
        self.threads
            .get()
            .into_iter()
            .filter(|t| crate::agents::listed_chat(t) && t.archived)
            .collect()
    }

    /// `agent()` — the run of the open thread (`agentRuns()[activeId()]`).
    pub fn agent(&'static self) -> Option<AgentRun> {
        self.agent_runs
            .get()
            .get(&self.active_id.get())
            .cloned()
    }

    /// `agentSet(thread, bot, v)` — set or clear a thread's run.
    pub fn agent_set(
        &'static self,
        thread: String,
        bot: Option<String>,
        v: Option<(String, String)>,
    ) {
        // Unchanged → no write: every write re-runs whatever reads the runs.
        let next = v.map(|(state, label)| AgentRun { state, label, bot });
        if self.agent_runs.with_untracked(|m| m.get(&thread) == next.as_ref()) {
            return;
        }
        self.agent_runs.update(|m| {
            match next {
                Some(run) => m.insert(thread, run),
                None => m.remove(&thread),
            };
        });
    }

    /// `botSt(id)` — a bot's mark state: its run's activity while it
    /// runs (any thread), the settle outcome for a few seconds after,
    /// else `idle` (App.tsx:249).
    pub fn bot_st(&'static self, id: &str) -> String {
        if let Some(r) = self
            .agent_runs
            .get()
            .values()
            .find(|r| r.bot.as_deref() == Some(id))
        {
            return r.state.clone();
        }
        if let Some(st) = self.settled.get().as_ref()
            && st.bot == id {
                return st.state.clone();
        }
        "idle".to_owned()
    }

    /// Re-pull `/threads` (rows + `bots`): run-status ticks call it like
    /// App.tsx's `refreshThreads`, so names, previews and avatars stay
    /// fresh while a run streams.
    pub fn refresh_threads(&'static self) {
        spawn(async move {
            if let Some(v) = client::get("/threads").await {
                self.set_threads(v);
            }
        });
    }

    /// `loadModels` — the pill's lazy fetch (modelpicker.tsx:64).
    pub fn load_models(&'static self) {
        spawn(async move {
            if let Some(v) = client::get("/models").await {
                self.set_models(v);
            }
        });
    }

    fn set_models(&self, v: serde_json::Value) {
        self.default_model.set(s(&v, "default"));
        self.models.set(
            list(&v, "models")
                .iter()
                .map(|m| ModelDef {
                    id: s(m, "id"),
                    label: pretty_model(&s(m, "id")),
                    provider: s(m, "provider"),
                    fast: m.get("fast").and_then(|x| x.as_bool()).unwrap_or(false),
                    efforts: list(m, "efforts")
                        .iter()
                        .map(|e| (s(e, "value"), s(e, "label")))
                        .collect(),
                    default_effort: s(m, "defaultEffort"),
                    image: self::b(m, "image"),
                    group: s(m, "group"),
                    subscription: m.get("subscription").and_then(|x| x.as_bool()).unwrap_or(true),
                })
                .collect(),
        );
    }

    // -- thread ------------------------------------------------------------

    /// Open a thread: select it server-side, then load its turns and
    /// start watching its status. (`/turns` always answers the server's
    /// active thread — `?thread=` is ignored.)
    pub fn open_thread(&'static self, id: String) {
        if self.active_id.get_untracked() == id {
            return;
        }
        // `selectThread` (App.tsx:613).
        self.tab.set("chat".to_owned());
        // `if (!agent()) emphBot.set("")` — the status row unmounts when
        // switching away from the run's thread.
        self.emph_bot.set(String::new());
        // The old chat stays on screen until this one's turns land
        // (`turns_of`); the next fetch always replaces them.
        *self.turns_json.borrow_mut() = String::new();
        let sel = id.clone();
        spawn(async move {
            client::post("/thread/select", serde_json::json!({"id": sel})).await;
            self.active_id.set(id.clone());
            self.seed_bot_edit_name();
            // `d.list.find(t => t.id === d.active)?.lastApp` — the pane
            // re-opens the thread's pinned app (or clears its state).
            let app = self
                .threads
                .get_untracked()
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.last_app.clone())
                .unwrap_or_default();
            if app.is_empty() {
                self.detail.set(Vec::new());
                self.detail_meta.set(None);
                self.detail_app.set(String::new());
            } else if self.detail_app.get_untracked() != app {
                self.load_detail(app);
            }
            self.refresh_turns();
            self.refresh_thread_apps();
        });
    }

    /// `effect(() => botEditName.set(activeBot()?.name || ""))`
    /// (App.tsx:257) — reseed the profile name draft when the open bot,
    /// or its name, changes; typed text (including the empty draft)
    /// survives between real changes.
    fn seed_bot_edit_name(&'static self) {
        // An agent's name, or a plain chat's title (the pane's name field
        // renames whichever is open).
        let key = self.active_bot_untracked().map(|b| (b.id, b.name)).or_else(|| {
            let id = self.active_id.get_untracked();
            self.threads.with_untracked(|t| {
                t.iter().find(|t| t.id == id).map(|t| (t.id.clone(), t.name.clone()))
            })
        });
        if *self.bot_edit_src.borrow() != key {
            self.bot_edit_name
                .set(key.as_ref().map(|(_, n)| n.clone()).unwrap_or_default());
            *self.bot_edit_src.borrow_mut() = key;
        }
    }

    /// The ⌘1…9 slots (`sidebarBots`, App.tsx:764) — pinned bots first,
    /// then the groups in the sidebar's order with each group's bots
    /// ranked by latest interaction (`count` is the only activity signal
    /// `/threads` carries), then the bots without a group, then the plain
    /// chats. Hidden bots never take a slot.
    pub fn shortcut_threads(&'static self) -> Vec<String> {
        // a temporary helper takes no slot (no sidebar row)
        let bots: Vec<RoxBot> = self.bots.get_untracked().into_iter().filter(crate::agents::listed_bot).collect();
        let threads = self.threads.get_untracked();
        let count_of = |tid: &str| {
            threads
                .iter()
                .find(|t| t.id == tid)
                .map(|t| t.count)
                .unwrap_or(0)
        };
        let mut out: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut push = |tid: &str| {
            if !tid.is_empty() && !seen.contains(tid) {
                seen.insert(tid.to_owned());
                out.push(tid.to_owned());
            }
        };
        for b in bots.iter().filter(|b| b.pinned && !b.hidden) {
            push(&b.thread_id);
        }
        let vis: Vec<&RoxBot> = bots.iter().filter(|b| !b.hidden && !b.pinned).collect();
        let groups = self.groups.get_untracked();
        let grouped = |id: &str| groups.iter().any(|g| g.items.iter().any(|it| it.kind == "bot" && it.id == id));
        for g in &groups {
            let mut items: Vec<&RoxBot> = g
                .items
                .iter()
                .filter(|it| it.kind == "bot")
                .filter_map(|it| vis.iter().find(|b| b.id == it.id).copied())
                .collect();
            items.sort_by_key(|a| std::cmp::Reverse(count_of(&a.thread_id)));
            for b in items {
                push(&b.thread_id);
            }
        }
        let mut un: Vec<&RoxBot> = vis.iter().filter(|b| !grouped(&b.id)).copied().collect();
        un.sort_by_key(|a| std::cmp::Reverse(count_of(&a.thread_id)));
        for b in un {
            push(&b.thread_id);
        }
        for t in threads
            .iter()
            .filter(|t| crate::agents::listed_chat(t) && !t.hidden && !t.archived)
        {
            push(&t.id);
        }
        out.truncate(9);
        out
    }

    /// `mod+k` — open the palette (or close it).
    pub fn toggle_palette(&'static self) {
        if self.palette.get_untracked() {
            self.close_palette();
            return;
        }
        self.palette_pick.set(None);
        self.palette_hint.set(String::new());
        self.palette.set(true);
        self.palette_q.set(String::new());
        self.palette_sel.set(0);
    }

    /// The palette as a picker over `items`, `hint` in its search field.
    pub fn open_palette_pick(&'static self, hint: String, items: Vec<PaletteItem>) {
        batch(|| {
            self.palette.set(false);
            self.palette_pick.set(Some(items));
            self.palette_hint.set(hint);
            self.palette_q.set(String::new());
            self.palette_sel.set(0);
            self.palette.set(true);
        });
    }

    /// `close` (palette.tsx:29) — hide and reset the query + selection.
    pub fn close_palette(&'static self) {
        self.palette.set(false);
        self.palette_q.set(String::new());
        self.palette_sel.set(0);
        self.palette_pick.set(None);
        self.palette_hint.set(String::new());
    }

    /// The palette's rows, sessions first (Grok Bot's order): the agents,
    /// then the plain chats, then the mini apps in the sidebar's groups,
    /// then the actions.
    pub fn palette_items(&'static self) -> Vec<PaletteItem> {
        if let Some(items) = self.palette_pick.get() {
            return items;
        }
        let row = |title: String, subtitle: String, act: PaletteAct| PaletteItem {
            title,
            subtitle,
            icon: String::new(),
            checked: false,
            act,
            bot: None,
            letter_of: String::new(),
            app_of: String::new(),
        };
        let mut items = Vec::new();
        for b in self.bots.get().into_iter().filter(|b| !b.hidden && !b.thread_id.is_empty() && crate::agents::listed_bot(b)) {
            items.push(PaletteItem {
                bot: Some(b.clone()),
                ..row(b.name.clone(), "Agent".into(), PaletteAct::Thread(b.thread_id.clone()))
            });
        }
        for t in self.chats() {
            let name = if t.name.is_empty() { "New chat".to_owned() } else { untracked(|| self.v1_plain(&t.name)) };
            items.push(PaletteItem {
                letter_of: name.clone(),
                ..row(name, format!("Chat · {} messages", t.count), PaletteAct::Thread(t.id.clone()))
            });
        }
        let mut apps = std::collections::HashSet::new();
        for a in self.groups.get().into_iter().flat_map(|g| g.items).filter_map(|it| it.artifact) {
            if a.kind == "app" && apps.insert(a.app.clone()) {
                items.push(PaletteItem {
                    app_of: a.app.clone(),
                    ..row(a.title.clone(), "Mini app".into(), PaletteAct::App(a.app.clone()))
                });
            }
        }
        let pref = self.theme_pref.get();
        let action = |title: &str, subtitle: &str, icon: &str, checked: bool, act: PaletteAct| PaletteItem {
            icon: icon.to_owned(),
            checked,
            ..row(title.to_owned(), subtitle.to_owned(), act)
        };
        items.push(action("New Chat", "", "newChat", false, PaletteAct::NewChat));
        items.push(action("New Bot", "", "newChat", false, PaletteAct::BotModal));
        if let Some(b) = self.active_bot() {
            items.push(action("Bot Details (current)", &b.name, "panel", false, PaletteAct::BotDetails(false)));
            items.push(action("Edit Bot Details (current)", &b.name, "edit", false, PaletteAct::BotDetails(true)));
        }
        items.push(action("Settings", "Keyboard shortcuts", "settings", false, PaletteAct::Settings));
        items.push(action("Theme: System", "Settings · Appearance", "systemTheme", pref == "system", PaletteAct::Theme("system")));
        items.push(action("Theme: Light", "Settings · Appearance", "sun", pref == "light", PaletteAct::Theme("light")));
        items.push(action("Theme: Dark", "Settings · Appearance", "moon", pref == "dark", PaletteAct::Theme("dark")));
        items
    }

    /// `results` (palette.tsx:24) — substring match on title+subtitle.
    pub fn palette_results(&'static self) -> Vec<PaletteItem> {
        let q = self.palette_q.get().trim().to_lowercase();
        let all = self.palette_items();
        if q.is_empty() {
            all
        } else {
            all.into_iter()
                .filter(|i| {
                    format!("{} {}", i.title, i.subtitle)
                        .to_lowercase()
                        .contains(&q)
                })
                .collect()
        }
    }

    /// `runAt(i)` (palette.tsx:34) — close, then run the result.
    pub fn palette_run_at(&'static self, i: usize) {
        let Some(it) = self.palette_results().get(i).cloned() else {
            return;
        };
        self.close_palette();
        match it.act {
            PaletteAct::BotModal => self.bot_modal.set(true),
            PaletteAct::BotDetails(edit) => crate::botdetails::open_current(edit),
            PaletteAct::Settings => crate::settings::toggle(),
            PaletteAct::NewChat => self.new_thread(),
            PaletteAct::Theme(t) => self.set_theme(t),
            PaletteAct::App(app) => self.popout(&app),
            PaletteAct::Thread(id) => self.go_to(&id, None),
            PaletteAct::Link(url) => self.open_link(&url),
            PaletteAct::Attach(tok, thread, place) => crate::slash::attach(vec![tok], thread, place, None),
        }
    }

    /// ⌘R — back to a fresh launch: reload the model from the server and
    /// drop every transient overlay, draft, and scroll state.
    pub fn reset_fresh(&'static self) {
        self.tab.set("chat".to_owned());
        self.palette.set(false);
        self.palette_q.set(String::new());
        self.palette_sel.set(0);
        self.ctx.set(None);
        self.pill.set(None);
        self.bot_modal.set(false);
        self.account_open.set(false);
        self.rail_pop.set(None);
        self.group_prompt.set(None);
        self.group_name.set(String::new());
        self.avatar_editing.set(false);
        self.avatar_hover.set(false);
        self.app_rename.set(None);
        self.vers_pop.set(None);
        self.reply_to.set(None);
        self.attachments.set(Vec::new());
        self.draft.set(String::new());
        self.at_items.set(Vec::new());
        self.at_open.set(false);
        self.at_start.set(-1);
        self.pop_rows.set(Vec::new());
        self.pop_head.set(String::new());
        self.emph_bot.set(String::new());
        self.settled.set(None);
        self.chat_scrolling.set(false);
        self.light_close();
        self.viewing.set(None);
        crate::models::close();
        crate::composer::plus_menu().set(None);
        if self.plugins_open.get_untracked() {
            self.close_plugins();
        }
        self.turns.set(Vec::new());
        self.turns_of.set(String::new());
        self.alive.set(Vec::new());
        self.recent_chats.borrow_mut().clear();
        *self.turns_json.borrow_mut() = String::new();
        spawn(async move {
            if let Some(v) = client::get("/threads").await {
                self.set_threads(v);
            }
            self.refresh_turns();
            self.refresh_thread_apps();
        });
    }

    /// One `/turns` fetch for the open thread; dedupes on raw JSON so a
    /// quiet poll doesn't re-render.
    pub fn refresh_turns(&'static self) {
        let id = self.active_id.get_untracked();
        if id.is_empty() {
            return;
        }
        spawn(async move {
            let Some(v) = client::get(&format!("/turns?thread={}", client::urlenc(&id))).await else { return };
            // A server that reads `?thread=` answers with that chat's turns;
            // an older one with its shared active thread (Grok Shell,
            // another window, a run on another chat can move it): a
            // response for another thread stays out of the list, and Rox
            // selects its own again so the next poll lands.
            if s(&v, "thread") != id && s(&v, "active") != id {
                if self.active_id.get_untracked() == id {
                    client::post("/thread/select", serde_json::json!({ "id": id })).await;
                }
                return;
            }
            let raw = v.to_string();
            if raw == *self.turns_json.borrow() {
                return;
            }
            *self.turns_json.borrow_mut() = raw;
            let mut turns: Vec<Turn> = list(&v, "turns").iter().map(Turn::of).collect();
            self.merge_local_turns(&id, &mut turns);
            batch(|| {
                self.turns.set(turns);
                if self.turns_of.get_untracked() != id {
                    self.turns_of.set(id.clone());
                    // The ⌘1–9 chats stay, and the `ALIVE` most recent
                    // others; the rest unmounts (its spot is kept as a
                    // `ScrollMark`). Cycling through ⌘1–9 rebuilt a chat on
                    // every press (200–1250 views, frames up to 50 ms).
                    let keys = self.shortcut_threads();
                    let keep: Vec<String> = {
                        let mut recent = self.recent_chats.borrow_mut();
                        recent.retain(|x| *x != id);
                        recent.insert(0, id.clone());
                        recent.truncate(ALIVE + 9);
                        recent
                            .iter()
                            .filter(|x| !keys.contains(x))
                            .take(ALIVE)
                            .cloned()
                            .collect()
                    };
                    let mut next = self.alive.get_untracked();
                    next.retain(|x| keys.contains(x) || keep.contains(x));
                    if !next.contains(&id) {
                        next.push(id.clone());
                    }
                    if next != self.alive.get_untracked() {
                        self.alive.set(next);
                    }
                }
            });
            if let Some(b) = v.get("revealed").and_then(|x| x.as_bool()) {
                self.revealed.set(b);
            }
            self.last_status.set(s(&v, "status"));
        });
    }

    pub fn refresh_thread_apps(&'static self) {
        self.fetch_app_meta();
        // a group's artifact rows carry the same records (titles, icons)
        self.fetch_groups();
        let id = self.active_id.get_untracked();
        if id.is_empty() {
            return;
        }
        spawn(async move {
            if let Some(v) = client::get(&format!("/thread/apps?thread={id}")).await {
                // A stale fetch must not overwrite the list after a switch.
                if self.active_id.get_untracked() != id {
                    return;
                }
                self.thread_apps.set(list(&v, "apps").iter().map(RoxApp::from_json).collect());
            }
        });
    }

    /// `detail`/`detailState` — `GET /catalog` for the app's meta (an
    /// unpublished app gets the bare `{name,title}` row), then
    /// `GET /state/:app` for the k/v rows (the `widget` key drops; the
    /// first 10 show) (App.tsx:404-431).
    pub fn load_detail(&'static self, app: String) {
        self.detail_app.set(app.clone());
        spawn(async move {
            if let Some(cat) = client::get("/catalog").await {
                let item = list(&cat, "list")
                    .iter()
                    .find(|x| s(x, "name") == app)
                    .cloned();
                self.detail_meta.set(Some(match item {
                    Some(it) => AppMeta {
                        name: s(&it, "name"),
                        title: s(&it, "title"),
                        kinds: list(&it, "kinds")
                            .iter()
                            .filter_map(|k| k.as_str().map(str::to_owned))
                            .collect(),
                        tags: list(&it, "tags")
                            .iter()
                            .filter_map(|t| t.as_str().map(str::to_owned))
                            .collect(),
                        published: true,
                    },
                    None => AppMeta {
                        name: app.clone(),
                        title: app.clone(),
                        ..Default::default()
                    },
                }));
            }
            if let Some(v) = client::get(&format!("/state/{app}")).await {
                let mut rows: Vec<Kv> = list(&v, "entries")
                    .iter()
                    .map(|e| Kv(s(e, "key"), s(e, "value")))
                    .collect();
                if rows.is_empty()
                    && let Some(obj) = v.get("state").and_then(|o| o.as_object()) {
                        rows = obj
                            .iter()
                            .take(10)
                            .map(|(k, x)| {
                                let v = if x.is_object() || x.is_array() {
                                    x.to_string().chars().take(60).collect()
                                } else {
                                    x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())
                                };
                                Kv(k.clone(), v)
                            })
                            .collect();
                }
                self.detail.set(rows);
            }
        });
    }

    // ————— the right pane —————

    /// `libraryGroups()` — thread apps bucketed by `mtime` into
    /// Today / This week / Older (App.tsx:1269).
    pub fn library_groups(&'static self) -> Vec<(String, Vec<RoxApp>)> {
        let day = 86_400_000.0f64;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0);
        let mut g: [(String, Vec<RoxApp>); 3] = [
            ("Today".to_owned(), Vec::new()),
            ("This week".to_owned(), Vec::new()),
            ("Older".to_owned(), Vec::new()),
        ];
        let filter = self.lib_kind();
        // the Archived chip lists the archived only; every other chip
        // leaves them out
        let shown = |a: &&RoxApp| {
            if filter == "archived" {
                a.archived
            } else {
                !a.archived && (filter == "all" || lib_kind_of(a) == filter)
            }
        };
        for a in self.thread_apps.get().iter().filter(shown) {
            let i = if now - a.mtime < day {
                0
            } else if now - a.mtime < 7.0 * day {
                1
            } else {
                2
            };
            g[i].1.push(a.clone());
        }
        g.into_iter().filter(|(_, items)| !items.is_empty()).collect()
    }

    /// The Library's chips: All, then each kind the thread has, with its
    /// count — (kind, label, count).
    pub fn lib_chips(&'static self) -> Vec<(&'static str, &'static str, usize)> {
        let apps = self.thread_apps.get();
        let live: Vec<&RoxApp> = apps.iter().filter(|a| !a.archived).collect();
        let mut out = vec![("all", "All", live.len())];
        for (kind, label) in LIB_KINDS {
            let n = live.iter().filter(|a| lib_kind_of(a) == kind).count();
            if n > 0 {
                out.push((kind, label, n));
            }
        }
        let n = apps.len() - live.len();
        if n > 0 {
            out.push(("archived", "Archived", n));
        }
        out
    }

    /// The chip in effect: the picked one while its kind is still in the
    /// list, else All.
    pub fn lib_kind(&'static self) -> String {
        let f = self.lib_filter.get();
        let held = |a: &RoxApp| if f == "archived" { a.archived } else { !a.archived && lib_kind_of(a) == f };
        if f != "all" && self.thread_apps.with(|v| v.iter().any(held)) {
            f
        } else {
            "all".to_owned()
        }
    }

    /// `ago(ms)` — m/h/d shorthand for the library rows (App.tsx:1259).
    pub fn ago(ms: f64) -> String {
        if ms == 0.0 {
            return String::new();
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0);
        let m = (now - ms) / 60_000.0;
        if m < 60.0 {
            format!("{}m", m.round().max(1.0) as i64)
        } else if m < 1440.0 {
            format!("{}h", (m / 60.0).round() as i64)
        } else {
            format!("{}d", (m / 1440.0).round() as i64)
        }
    }

    /// A version-tree row: `POST /app/version` moves the pointer, then
    /// apps + threads refresh (App.tsx:2692).
    /// Moves `app`'s current pointer to `v` (an open window of it
    /// reopens on `v`, server side); App Details follows.
    pub fn set_version(&'static self, app: String, v: i64) {
        spawn(async move {
            let _ = client::post("/app/version", serde_json::json!({"name": app, "v": v})).await;
            self.refresh_thread_apps();
            self.refresh_threads();
            if let Some(i) = self.app_details.get_untracked().filter(|i| i.name == app_name(&app))
                && let Some(v) = client::get(&details_url(&i.name, &i.kind)).await {
                    self.app_details.set(Some(AppInfo::from_json(&v)));
            }
        });
    }

    /// `GET /apps/meta` → `app_meta`.
    pub fn fetch_app_meta(&'static self) {
        spawn(async move {
            let Some(v) = client::get("/apps/meta").await else { return };
            let map: std::collections::HashMap<String, MiniMeta> = list(&v, "apps")
                .iter()
                .map(|a| {
                    let m = MiniMeta {
                        name: s(a, "name"),
                        title: s(a, "title"),
                        file: s(a, "file"),
                        description: s(a, "description"),
                        icon: s(a, "icon"),
                        w: f64of(a, "w") as f32,
                        h: f64of(a, "h") as f32,
                    };
                    (m.name.clone(), m)
                })
                .collect();
            if map != self.app_meta.get_untracked() {
                self.app_meta.set(map);
            }
        });
    }

    /// The meta of module key `app` (`R17` or `R17@v2`).
    pub fn meta_of(&self, app: &str) -> Option<MiniMeta> {
        self.app_meta.get().get(app_name(app)).cloned()
    }

    /// The icon generator runs after the agent's turn: look again while
    /// it lands.
    pub fn watch_app_icons(&'static self) {
        spawn(async move {
            for secs in [6, 15, 30] {
                timer(std::time::Duration::from_secs(secs)).await;
                self.fetch_app_meta();
            }
        });
    }

    /// Opens App Details for `app`: `GET /app/info`, then the drafts.
    pub fn open_app_details(&'static self, app: &str) {
        let name = app_name(app).to_owned();
        self.ad_err.set(String::new());
        self.ad_busy.set(String::new());
        spawn(async move {
            let Some(v) = client::get(&format!("/app/info?name={name}")).await else {
                return;
            };
            self.seed_app_details(&v);
        });
    }

    /// Edit Details for any Library item: a mini app's App Details, else
    /// `GET /artifact/info` for an image or a file.
    pub fn open_details(&'static self, id: &str) {
        let kind = self
            .thread_apps
            .with_untracked(|v| v.iter().find(|a| a.app == id).map(|a| a.kind.clone()))
            .unwrap_or_default();
        self.open_details_of(id, &kind)
    }

    /// [`open_details`] for an item of a known kind (a group's artifact,
    /// from any chat); "" = guess from the id.
    pub fn open_details_of(&'static self, id: &str, kind: &str) {
        if kind == "mcp-app" {
            return self.open_mcp_details(id);
        }
        if kind == "app" || (kind.is_empty() && !id.starts_with("file:") && !id.starts_with("gdrive:")) {
            return self.open_app_details(id);
        }
        let kind = kind.to_owned();
        let id = id.to_owned();
        self.ad_err.set(String::new());
        self.ad_busy.set(String::new());
        spawn_detached(async move {
            let Some(v) = client::get(&details_url(&id, &kind)).await else {
                return;
            };
            self.seed_app_details(&v);
        });
    }

    /// App Details for an attached server app: what its Library record
    /// (the open chat's, else a group's) tells — else
    /// `GET /artifact/info?id=attach:<id>`, the same record.
    fn open_mcp_details(&'static self, id: &str) {
        let a = self.thread_apps.with_untracked(|v| v.iter().find(|a| a.app == id).cloned()).or_else(|| {
            self.groups.with_untracked(|gs| {
                gs.iter().flat_map(|g| g.items.iter()).filter_map(|it| it.artifact.as_ref()).find(|a| a.app == id).cloned()
            })
        });
        self.ad_err.set(String::new());
        self.ad_busy.set(String::new());
        let show = move |a: RoxApp| {
            self.ad_title.set(a.title.clone());
            self.app_details.set(Some(AppInfo::of_mcp(&a)));
        };
        match a {
            Some(a) => show(a),
            None => {
                let id = id.to_owned();
                spawn_detached(async move {
                    if let Some(v) = client::get(&details_url(&id, "mcp-app")).await {
                        show(RoxApp::from_json(&v));
                    }
                });
            }
        }
    }

    fn seed_app_details(&'static self, v: &serde_json::Value) {
        let num = |k: &str| {
            let n = f64of(v, k);
            if n > 0.0 { format!("{}", n.round() as i64) } else { String::new() }
        };
        let info = AppInfo::from_json(v);
        // The name field edits the stem; the extension stays.
        let suffix = format!(".{}", info.ext);
        let file = &info.filename;
        let stem = if !info.ext.is_empty() && file.to_lowercase().ends_with(&suffix.to_lowercase()) {
            &file[..file.len() - suffix.len()]
        } else {
            file.as_str()
        };
        self.ad_title.set(s(v, "title"));
        self.ad_file.set(stem.to_owned());
        self.ad_desc.set(s(v, "description"));
        self.ad_w.set(num("w"));
        self.ad_h.set(num("h"));
        self.ad_prompt.set(s(v, "icon_prompt"));
        if info.generating {
            self.ad_busy.set("gen".into());
            self.poll_app_icon(info.name.clone());
        }
        self.app_details.set(Some(info));
    }

    /// After the server answers an edit: the modal's facts, every tile.
    fn app_details_landed(&'static self, v: Option<serde_json::Value>, what: &str) {
        self.ad_busy.set(String::new());
        match v {
            Some(v) if self::b(&v, "ok") => {
                if let Some(cur) = self.app_details.get_untracked()
                    && cur.name == s(&v, "name") {
                        self.app_details.set(Some(AppInfo::from_json(&v)));
                }
                self.refresh_thread_apps();
                // the chat's cards show the edited titles and file names
                self.refresh_turns();
            }
            _ => self.ad_err.set(format!("Couldn't {what}")),
        }
    }

    /// The chat's model, as App Details' Generate sees it: its label and
    /// whether it reaches an image model.
    pub fn icon_model(&'static self) -> (String, String, bool) {
        let id = self.choice().0;
        let m = self.models.get().into_iter().find(|m| m.id == id);
        let label = m.as_ref().map(|m| m.label.clone()).unwrap_or_else(|| pretty_model(&id));
        (id, label, m.is_some_and(|m| m.image))
    }

    /// Generate: `POST /app/icon/gen` with the prompt (empty = the server
    /// derives one from the title and description) and the chat's model;
    /// the server answers at once and the icon lands later.
    pub fn gen_app_icon(&'static self) {
        let Some(info) = self.app_details.get_untracked() else { return };
        let (model, label, can) = untracked(|| self.icon_model());
        if !can || !self.ad_busy.get_untracked().is_empty() {
            return;
        }
        self.ad_err.set(String::new());
        self.ad_busy.set("gen".into());
        let prompt = self.ad_prompt.get_untracked().trim().to_owned();
        spawn(async move {
            let v = client::post(
                "/app/icon/gen",
                serde_json::json!({"name": info.name, "prompt": prompt, "model": model}),
            )
            .await;
            match v {
                Some(v) if self::b(&v, "ok") => self.poll_app_icon(info.name),
                _ => {
                    self.ad_busy.set(String::new());
                    self.ad_err.set(format!("{label} couldn't start an icon"));
                }
            }
        });
    }

    /// Re-reads `/app/info` until the icon lands (or fails).
    fn poll_app_icon(&'static self, name: String) {
        spawn(async move {
            loop {
                timer(std::time::Duration::from_millis(2500)).await;
                if self.app_details.get_untracked().is_none_or(|i| i.name != name) {
                    return;
                }
                let Some(v) = client::get(&format!("/app/info?name={name}")).await else { continue };
                if self::b(&v, "generating") {
                    continue;
                }
                self.ad_busy.set(String::new());
                let err = s(&v, "icon_error");
                if !err.is_empty() {
                    self.ad_err.set(err);
                }
                self.app_details.set(Some(AppInfo::from_json(&v)));
                self.refresh_thread_apps();
                return;
            }
        });
    }

    /// Upload / Paste: the avatar picker hands back a temp image, which
    /// `/app/meta {icon}` adopts.
    pub fn pick_app_icon(&'static self, kind: &'static str) {
        let Some(info) = self.app_details.get_untracked() else { return };
        if !self.ad_busy.get_untracked().is_empty() {
            return;
        }
        self.ad_err.set(String::new());
        self.ad_busy.set("pick".into());
        spawn(async move {
            let picked = client::post(&format!("/avatar/image/{kind}"), serde_json::json!({})).await;
            let uri = picked.as_ref().map(|v| s(v, "uri")).unwrap_or_default();
            if uri.is_empty() {
                self.ad_busy.set(String::new());
                if !picked.as_ref().is_some_and(|v| self::b(v, "cancelled")) {
                    self.ad_err.set(if kind == "paste" {
                        "No image on the clipboard".into()
                    } else {
                        "Couldn't open that image".into()
                    });
                }
                return;
            }
            let v = if info.kind == "app" {
                client::post("/app/meta", serde_json::json!({"name": info.name, "icon": uri})).await
            } else {
                client::post("/artifact/meta", serde_json::json!({"id": info.name, "icon": uri})).await
            };
            self.app_details_landed(v, "use that image");
        });
    }

    /// Remove the icon (back to the default tile — an image's or file's
    /// thumbnail, where it has one).
    pub fn clear_app_icon(&'static self) {
        let Some(info) = self.app_details.get_untracked() else { return };
        spawn(async move {
            let v = if info.kind == "app" {
                client::post("/app/meta", serde_json::json!({"name": info.name, "icon": ""})).await
            } else {
                client::post("/artifact/meta", serde_json::json!({"id": info.name, "icon": ""})).await
            };
            self.app_details_landed(v, "remove the icon");
        });
    }

    /// Edit Details' "Made in": back to the message that made it.
    pub fn show_details_origin(&'static self) {
        let Some(i) = self.app_details.get_untracked() else { return };
        self.app_details.set(None);
        if i.kind == "mcp-app" {
            return crate::slash::show_in_chats(i.token, None);
        }
        if self.thread_apps.with_untracked(|v| v.iter().any(|a| a.app == i.name)) {
            self.show_library_item(&i.name);
        } else if !i.thread_id.is_empty() {
            self.go_to(&i.thread_id, (i.i >= 0).then_some(i.i));
        }
    }

    /// Save: title, file, description and window size in one
    /// `POST /app/meta`; pinned copies of the title follow.
    pub fn save_app_details(&'static self) {
        let Some(info) = self.app_details.get_untracked() else { return };
        let title = self.ad_title.get_untracked().trim().to_owned();
        let file = self.ad_file.get_untracked().trim().to_owned();
        // An attached server app: Rename is a display name in Rox, shared
        // by every chat and pin of its token (`POST /mcp/attachment/rename`;
        // empty goes back to the server's).
        if info.kind == "mcp-app" {
            if title == info.title {
                self.app_details.set(None);
                return;
            }
            let body = serde_json::json!({ "token": info.token, "title": title });
            self.ad_busy.set("save".into());
            spawn_detached(async move {
                let v = client::post_any("/mcp/attachment/rename", body).await;
                self.ad_busy.set(String::new());
                if v.as_ref().is_some_and(|v| self::b(v, "ok")) {
                    self.app_details.set(None);
                    self.refresh_thread_apps();
                    self.refresh_turns();
                } else {
                    let error = v.as_ref().map(|v| s(v, "error")).unwrap_or_default();
                    self.ad_err.set(if error.is_empty() { "Couldn't rename it".to_owned() } else { error });
                }
            });
            return;
        }
        // A connector's file: a new title renames it where it lives
        // (`POST /artifact/rename-remote` — Drive's update_drive_file).
        if info.storage.connector() {
            if title.is_empty() || title == info.title {
                self.app_details.set(None);
                return;
            }
            let body = serde_json::json!({ "id": info.name, "name": title });
            let place = info.storage.place_name();
            self.ad_busy.set("save".into());
            spawn_detached(async move {
                let v = client::post("/artifact/rename-remote", body).await;
                let ok = v.as_ref().is_some_and(|v| self::b(v, "ok"));
                self.app_details_landed(v, &format!("rename it in {place}"));
                if ok {
                    self.app_details.set(None);
                }
            });
            return;
        }
        // An image or a file: `POST /artifact/meta` (an empty field resets
        // it to the original).
        if info.kind != "app" {
            let body = serde_json::json!({
                "id": info.name,
                "title": title,
                "filename": file,
                "description": self.ad_desc.get_untracked().trim(),
            });
            self.ad_busy.set("save".into());
            spawn_detached(async move {
                let v = client::post("/artifact/meta", body).await;
                let ok = v.as_ref().is_some_and(|v| self::b(v, "ok"));
                self.app_details_landed(v, "save the details");
                if ok {
                    self.app_details.set(None);
                }
            });
            return;
        }
        let dim = |sig: Signal<String>| sig.get_untracked().trim().parse::<f64>().unwrap_or(0.0);
        let (w, h) = (dim(self.ad_w), dim(self.ad_h));
        let mut body = serde_json::json!({
            "name": info.name,
            "description": self.ad_desc.get_untracked().trim(),
            "w": w,
            "h": h,
        });
        if !title.is_empty() {
            body["title"] = title.clone().into();
        }
        if !file.is_empty() {
            body["file"] = file.into();
        }
        self.ad_busy.set("save".into());
        spawn_detached(async move {
            let v = client::post("/app/meta", body).await;
            let ok = v.as_ref().is_some_and(|v| self::b(v, "ok"));
            self.app_details_landed(v, "save the details");
            if ok {
                self.app_details.set(None);
            }
        });
    }

    /// `saveAppRename` — a branch label via `POST /app/version/label`
    /// (title and file moved to App Details).
    pub fn save_app_rename(&'static self) {
        let Some(r) = self.app_rename.get() else { return };
        let v = self.app_rename_text.get_untracked().trim().to_owned();
        self.app_rename.set(None);
        spawn(async move {
            let _ = client::post(
                "/app/version/label",
                serde_json::json!({"name": r.name, "v": r.v, "label": v}),
            )
            .await;
            self.refresh_thread_apps();
        });
    }

    /// The app-menu verbs — `/publish`, `/unpublish`, `/app/delete`
    /// (then apps + threads refresh) (App.tsx:1639).
    pub fn app_action(&'static self, verb: &str, name: String) {
        match verb {
            "publish" | "unpublish" => {
                let route = format!("/{verb}");
                spawn(async move {
                    let _ = client::post(&route, serde_json::json!({"name": name})).await;
                    self.refresh_thread_apps();
                    self.refresh_threads();
                });
            }
            "app-delete" | "app-delete-everywhere" => {
                let everywhere = verb == "app-delete-everywhere";
                // detached: the menu that asked closes right away
                spawn_detached(async move {
                    let _ = client::post(
                        "/app/delete",
                        serde_json::json!({"name": name, "everywhere": everywhere}),
                    )
                    .await;
                    self.refresh_thread_apps();
                    self.refresh_threads();
                    // its cards left the chat, its rows every group
                    self.refresh_turns();
                    self.fetch_groups();
                });
            }
            _ => {}
        }
    }

    /// `mcpMenu` verbs + `addMcpServer` — the connect tab (App.tsx:1605).
    pub fn fetch_mcp(&'static self) {
        spawn(async move {
            if let Some(v) = client::get("/mcp/servers").await {
                let rows: Vec<McpServer> = list(&v, "servers")
                    .iter()
                    .map(|r| McpServer {
                        id: s(r, "id"),
                        name: s(r, "name"),
                        command: s(r, "command"),
                        url: s(r, "url"),
                        args: list(r, "args")
                            .iter()
                            .filter_map(|a| a.as_str().map(str::to_owned))
                            .collect(),
                        enabled: self::b(r, "enabled"),
                        bot: s(r, "bot"),
                        status: s(r, "status"),
                        tools: r.get("tools").and_then(|x| x.as_i64()).unwrap_or(0),
                        err: s(r, "err"),
                    })
                    .collect();
                if rows != self.mcp_servers.get_untracked() {
                    self.mcp_servers.set(rows);
                }
            }
        });
    }



    // ————— Plugins marketplace (App.tsx:1291–1640) —————

    /// `pluginsOpen.set(true)` — reset the nav, fetch, start the 3s poll
    /// (the `effect` at App.tsx:1389).
    pub fn open_plugins(&'static self) {
        if self.plugins_open.get_untracked() {
            return;
        }
        self.plugins_open.set(true);
        self.plugin_nav.set(vec![PluginNav::market()]);
        self.plugin_query.set(String::new());
        self.fetch_plugins();
        self.fetch_mcp();
        self.fetch_skills();
    }

    /// `pluginsOpen.set(false)` — the effect's reset branch.
    pub fn close_plugins(&'static self) {
        self.plugins_open.set(false);
        self.plugin_nav.set(vec![PluginNav::market()]);
        self.plugin_query.set(String::new());
    }

    /// The 3s poll while the modal is open (App.tsx:1394): catalog +
    /// servers + skills, plus the top detail page (OAuth lands here).
    pub fn watch_plugins(&'static self) {
        set_interval(std::time::Duration::from_secs(3), move || {
            if !self.plugins_open.get_untracked() {
                return;
            }
            self.fetch_plugins();
            self.fetch_mcp();
            self.fetch_skills();
            let top = self.plugin_nav.get_untracked().last().cloned();
            if let Some(top) = top {
                if top.page == "plugin" && !top.name.is_empty() {
                    let name = top.name.clone();
                    self.fetch_plugin_detail(&name);
                    spawn(async move {
                        timer(std::time::Duration::from_millis(60)).await;
                        self.fetch_connector_tools(&name);
                    });
                }
                if top.page == "mcp" && !top.id.is_empty() && self.mcp_enabled(&top.id) {
                    self.fetch_mcp_tools(&top.id);
                }
            }
        });
    }

    /// `pushNav` — the page's fetch side effects (App.tsx:1311).
    pub fn push_nav(&'static self, page: &str, name: &str, id: &str) {
        let p = PluginNav {
            page: page.to_owned(),
            name: name.to_owned(),
            id: id.to_owned(),
        };
        self.plugin_nav.update(|n| n.push(p.clone()));
        self.reset_acct_ui();
        match page {
            "plugin" if !name.is_empty() => {
                self.fetch_plugin_detail(name);
                let name = name.to_owned();
                spawn(async move {
                    timer(std::time::Duration::from_millis(60)).await;
                    self.fetch_connector_tools(&name);
                });
            }
            "mcp" if !id.is_empty() => {
                if self.mcp_enabled(id) {
                    self.fetch_mcp_tools(id)
                }
            }
            "skill" if !name.is_empty() && !id.is_empty() => {
                self.fetch_skill_body(name, id);
            }
            "skill" => {
                // private editor — seed the fields (blank = new skill).
                let sk = self
                    .private_skills
                    .get_untracked()
                    .iter()
                    .find(|s| s.id == id)
                    .cloned();
                self.ps_id.set(sk.as_ref().map(|s| s.id.clone()).unwrap_or_default());
                self.ps_name.set(sk.as_ref().map(|s| s.name.clone()).unwrap_or_default());
                self.ps_desc.set(sk.as_ref().map(|s| s.description.clone()).unwrap_or_default());
                self.ps_body.set(sk.as_ref().map(|s| s.body.clone()).unwrap_or_default());
            }
            "connected" => {
                self.fetch_skills();
                for pl in self.plugins.get_untracked().iter().filter(|x| x.installed) {
                    let name = pl.name.clone();
                    self.fetch_plugin_detail(&name);
                }
            }
            _ => {}
        }
    }

    /// The tools of a plugin's connected connectors (the plugin page's
    /// Tools section; a connector that isn't connected has none to list).
    fn fetch_connector_tools(&'static self, plugin: &str) {
        if let Some(d) = self.plugin_details.get_untracked().get(plugin) {
            for m in &d.mcp {
                if !m.id.is_empty() && m.status == "connected" {
                    self.fetch_mcp_tools(&m.id);
                }
            }
        }
    }

    /// A disabled server isn't probed for its tools (`/mcp/tools` would
    /// spawn it while its status reads Disconnected).
    fn mcp_enabled(&self, id: &str) -> bool {
        self.mcp_servers
            .get_untracked()
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.enabled)
            .unwrap_or(true)
    }

    fn reset_acct_ui(&'static self) {
        self.acct_edit.set(String::new());
        self.acct_add.set(String::new());
        self.acct_confirm.set(String::new());
        self.acct_err.set(String::new());
        self.plugin_copied.set(false);
    }

    /// `POST /plugins/connect` — register `server` (one of `plugin`'s
    /// declared MCP servers) as an account and start its auth: no label =
    /// the default account, a label adds another. Remote servers open the
    /// OAuth tab; the 3s poll then walks the status to Connected.
    pub fn connect_account(&'static self, plugin: &str, server: &str, label: &str) {
        let (plugin, server, label) = (plugin.to_owned(), server.to_owned(), label.trim().to_owned());
        if label.eq_ignore_ascii_case("grok") || label.eq_ignore_ascii_case("default") {
            self.acct_err.set(format!("{label} is a reserved account label"));
            return;
        }
        self.acct_err.set(String::new());
        self.plugin_busy.set(format!("connect:{server}"));
        spawn(async move {
            let r = client::post(
                "/plugins/connect",
                serde_json::json!({"plugin": plugin, "server": server, "label": label}),
            )
            .await;
            // a 400 comes back as None (the label clash / missing manifest).
            let err = match &r {
                None => "Couldn't add the account. Try another label.".to_owned(),
                Some(v) => v.get("error").and_then(|e| e.as_str()).unwrap_or_default().to_owned(),
            };
            if err.is_empty() {
                self.acct_add.set(String::new());
                self.acct_add_val.set(String::new());
            } else {
                self.acct_err.set(err);
            }
            self.plugin_busy.set(String::new());
            self.fetch_mcp();
            self.fetch_plugin_detail(&plugin);
        });
    }

    /// `POST /mcp/auth` for an existing account row (Authenticate).
    pub fn auth_account(&'static self, id: &str, plugin: &str) {
        let (id, plugin) = (id.to_owned(), plugin.to_owned());
        self.plugin_busy.set(format!("auth:{id}"));
        spawn(async move {
            let _ = client::post("/mcp/auth", serde_json::json!({"id": id})).await;
            self.plugin_busy.set(String::new());
            self.fetch_plugin_detail(&plugin);
        });
    }

    /// Rename (`label`) an account row — `POST /mcp/account`.
    pub fn rename_account(&'static self, id: &str, label: &str, plugin: &str) {
        let (id, label, plugin) = (id.to_owned(), label.trim().to_owned(), plugin.to_owned());
        if label.eq_ignore_ascii_case("grok") {
            self.acct_err.set("Grok is a reserved account label".into());
            return;
        }
        self.acct_edit.set(String::new());
        self.acct_err.set(String::new());
        spawn(async move {
            let _ = client::post("/mcp/account", serde_json::json!({"id": id, "label": label})).await;
            self.fetch_plugin_detail(&plugin);
        });
    }

    /// Disconnect — drop the account's stored OAuth tokens.
    pub fn disconnect_account(&'static self, id: &str, plugin: &str) {
        let (id, plugin) = (id.to_owned(), plugin.to_owned());
        self.acct_confirm.set(String::new());
        spawn(async move {
            let _ = client::post("/mcp/account", serde_json::json!({"id": id, "disconnect": true})).await;
            self.fetch_mcp();
            self.fetch_plugin_detail(&plugin);
        });
    }

    /// Remove an added account — its `mcp_servers` row goes.
    pub fn remove_account(&'static self, id: &str, plugin: &str) {
        let (id, plugin) = (id.to_owned(), plugin.to_owned());
        self.acct_confirm.set(String::new());
        self.acct_edit.set(String::new());
        spawn(async move {
            let _ = client::post("/mcp/server/delete", serde_json::json!({"id": id})).await;
            self.fetch_mcp();
            self.fetch_plugin_detail(&plugin);
        });
    }

    /// `POST /mcp/server/state` — a standalone server's Connect (enable).
    pub fn set_server_enabled(&'static self, id: &str, enabled: bool) {
        let id = id.to_owned();
        spawn(async move {
            let _ = client::post("/mcp/server/state", serde_json::json!({"id": id, "enabled": enabled}))
                .await;
            self.fetch_mcp();
            if enabled {
                self.fetch_mcp_tools(&id);
            }
        });
    }

    /// Share / "Copy link to this plugin" — `buwiz://plugin/<name>` (opens
    /// this page through `open_link`), with a 1.5s "Copied".
    pub fn copy_plugin_link(&'static self, name: &str) {
        self.copy_text(&format!("buwiz://plugin/{name}"));
        self.plugin_copied.set(true);
        spawn(async move {
            timer(std::time::Duration::from_millis(1500)).await;
            self.plugin_copied.set(false);
        });
    }

    /// `popNav`.
    pub fn pop_nav(&'static self) {
        self.reset_acct_ui();
        self.plugin_nav.update(|n| {
            n.pop();
        });
        if self.plugin_nav.get_untracked().is_empty() {
            self.plugin_nav.set(vec![PluginNav::market()]);
        }
    }

    /// `pluginPage` — the top frame's page.
    pub fn plugin_page(&self) -> String {
        self.plugin_nav
            .get()
            .last()
            .map(|n| n.page.clone())
            .unwrap_or_else(|| "market".into())
    }

    /// `fetchPlugins` — catalog rows; dedupe like the TSX (`JSON.stringify`
    /// compare) so the 3s poll doesn't churn the grid.
    pub fn fetch_plugins(&'static self) {
        let rev = self.plugins_rev.get_untracked();
        let url = if rev.is_empty() {
            "/plugins/catalog".to_owned()
        } else {
            format!("/plugins/catalog?rev={}", client::urlenc(&rev))
        };
        spawn(async move {
            let Some(v) = client::get(&url).await else { return };
            if self::b(&v, "unchanged") {
                return;
            }
            let order: Vec<String> = list(&v, "categories").iter().map(|c| s(c, "label")).collect();
            if order != self.plugin_cat_order.get_untracked() {
                self.plugin_cat_order.set(order);
            }
            let rows: Vec<PluginRow> = list(&v, "plugins")
                .iter()
                .map(|r| PluginRow {
                    name: s(r, "name"),
                    description: s(r, "description"),
                    category: s(r, "category"),
                    homepage: s(r, "homepage"),
                    domains: list(r, "domains")
                        .iter()
                        .filter_map(|a| a.as_str().map(str::to_owned))
                        .collect(),
                    keywords: list(r, "keywords")
                        .iter()
                        .filter_map(|a| a.as_str().map(str::to_owned))
                        .collect(),
                    installed: self::b(r, "installed"),
                    meta_mcp: r
                        .get("meta")
                        .map(|m| {
                            list(m, "mcp")
                                .iter()
                                .filter_map(|a| a.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                    display_name: s(r, "displayName"),
                    icon: {
                        let i = s(r, "icon");
                        if i.is_empty() { i } else { format!("{}{}", client::backend(), i) }
                    },
                    cats: list(r, "cats").iter().filter_map(|a| a.as_str().map(str::to_owned)).collect(),
                    publisher: s(r, "publisher"),
                    setup: self::b(r, "setup"),
                })
                .collect();
            self.plugins_rev.set(s(&v, "rev"));
            if rows != self.plugins.get_untracked() {
                // fetch each installed plugin's detail once for the
                // connected-chip dedupe.
                for pl in rows.iter().filter(|x| x.installed) {
                    if !self.plugin_details.get_untracked().contains_key(&pl.name) {
                        self.fetch_plugin_detail(&pl.name);
                    }
                }
                self.plugins.set(rows);
            }
        });
    }

    /// `fetchPluginDetail` (App.tsx:1354).
    pub fn fetch_plugin_detail(&'static self, name: &str) {
        let url = format!("/plugins/detail?name={}", client::urlenc(name));
        let name = name.to_owned();
        spawn(async move {
            let Some(v) = client::get(&url).await else { return };
            if v.get("error").and_then(|e| e.as_str()).map(|e| !e.is_empty()).unwrap_or(false) {
                return;
            }
            let d = PluginDetail {
                installed: self::b(&v, "installed"),
                description: s(&v, "description"),
                category: s(&v, "category"),
                homepage: s(&v, "homepage"),
                author: v
                    .get("manifest")
                    .map(|m| s(m, "author"))
                    .unwrap_or_default(),
                meta_mcp: v
                    .get("meta")
                    .map(|m| {
                        list(m, "mcp")
                            .iter()
                            .filter_map(|a| a.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                mcp: list(&v, "mcp")
                    .iter()
                    .map(|m| PluginMcp {
                        id: s(m, "id"),
                        name: s(m, "name"),
                        transport: s(m, "transport"),
                        status: s(m, "status"),
                        tools: m.get("tools").and_then(|x| x.as_i64()).unwrap_or(0),
                        err: s(m, "err"),
                        accounts: list(m, "accounts")
                            .iter()
                            .map(|a| PluginAccount {
                                id: s(a, "id"),
                                label: s(a, "label"),
                                status: s(a, "status"),
                                tools: a.get("tools").and_then(|x| x.as_i64()).unwrap_or(0),
                                err: s(a, "err"),
                                authed: self::b(a, "authed"),
                                note: s(a, "note"),
                            })
                            .collect(),
                        needs: list(m, "needs").iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
                        callback_url: s(m, "callbackUrl"),
                        unavailable: s(m, "unavailable"),
                    })
                    .collect(),
                skills: list(&v, "skills")
                    .iter()
                    .map(|x| PluginSkill { name: s(x, "name"), description: s(x, "description") })
                    .collect(),
                commands: list(&v, "commands")
                    .iter()
                    .map(|x| PluginSkill { name: s(x, "name"), description: s(x, "description") })
                    .collect(),
                display_name: s(&v, "displayName"),
                icon: {
                    let i = s(&v, "icon");
                    if i.is_empty() { i } else { format!("{}{}", client::backend(), i) }
                },
                availability: s(&v, "availability"),
                source: s(&v, "source"),
                fields: list(&v, "fields")
                    .iter()
                    .map(|f| PluginField {
                        key: s(f, "key"),
                        label: s(f, "label"),
                        placeholder: s(f, "placeholder"),
                        required: self::b(f, "required"),
                        secret: self::b(f, "secret"),
                        hint: s(f, "hint"),
                        options: list(f, "options").iter().map(|o| (s(o, "label"), s(o, "value"))).collect(),
                        set: self::b(f, "set"),
                        value: s(f, "value"),
                        shared: s(f, "shared"),
                    })
                    .collect(),
            };
            // only a real change commits — the 3s poll would otherwise
            // rebuild the page (and drop a focused account field) each tick.
            if self.plugin_details.get_untracked().get(&name) != Some(&d) {
                self.plugin_details.update(|m| {
                    m.insert(name, d);
                });
            }
        });
    }

    /// `fetchSkillBody` (App.tsx:1358).
    pub fn fetch_skill_body(&'static self, plugin: &str, skill: &str) {
        let url = format!(
            "/plugins/skill?plugin={}&skill={}",
            client::urlenc(plugin),
            client::urlenc(skill)
        );
        let key = format!("{plugin}/{skill}");
        spawn(async move {
            let Some(v) = client::get(&url).await else { return };
            if v.get("error").and_then(|e| e.as_str()).map(|e| !e.is_empty()).unwrap_or(false) {
                return;
            }
            let d = SkillBody {
                description: s(&v, "description"),
                body: s(&v, "body"),
            };
            self.skill_bodies.update(|m| {
                m.insert(key, d);
            });
        });
    }

    /// `fetchSkills` — the private skills list (App.tsx:1337).
    pub fn fetch_skills(&'static self) {
        spawn(async move {
            let Some(v) = client::get("/skills").await else { return };
            let rows: Vec<PrivateSkill> = list(&v, "skills")
                .iter()
                .map(|r| PrivateSkill {
                    id: s(r, "id"),
                    name: s(r, "name"),
                    description: s(r, "description"),
                    body: s(r, "body"),
                    enabled: self::b(r, "enabled"),
                })
                .collect();
            if rows != self.private_skills.get_untracked() {
                self.private_skills.set(rows);
            }
        });
    }

    /// `fetchMcpTools` (App.tsx:1433).
    pub fn fetch_mcp_tools(&'static self, id: &str) {
        let url = format!("/mcp/tools?id={}", client::urlenc(id));
        let id = id.to_owned();
        spawn(async move {
            let Some(v) = client::get(&url).await else { return };
            let tools: Vec<McpTool> = list(&v, "tools")
                .iter()
                .map(|t| McpTool {
                    name: s(t, "name"),
                    description: s(t, "description"),
                    enabled: self::b(t, "enabled"),
                })
                .collect();
            if !tools.is_empty() {
                self.mcp_tools.update(|m| {
                    m.insert(id, tools);
                });
            }
        });
    }

    /// `toggleMcpTool` (App.tsx:1437).
    pub fn toggle_mcp_tool(&'static self, id: &str, tool: &str, enabled: bool) {
        let (id, tool) = (id.to_owned(), tool.to_owned());
        spawn(async move {
            let _ = client::post(
                "/mcp/tools/state",
                serde_json::json!({"id": id, "tool": tool, "enabled": enabled}),
            )
            .await;
            self.fetch_mcp_tools(&id);
        });
    }

    /// `retryServer` (App.tsx:1439).
    pub fn retry_server(&'static self, id: &str, plugin: &str) {
        let (id, plugin) = (id.to_owned(), plugin.to_owned());
        spawn(async move {
            let _ = client::post("/mcp/servers", serde_json::json!({"id": id})).await;
            self.fetch_mcp();
            if !plugin.is_empty() {
                self.fetch_plugin_detail(&plugin);
            }
        });
    }

    /// `needsAuth` — `/401|403|unauthor|forbidden|auth/i` (App.tsx:1449);
    /// "unauthor" contains "auth", so the substring set suffices.
    pub fn needs_auth(err: &str) -> bool {
        let e = err.to_lowercase();
        !e.is_empty()
            && (e.contains("401") || e.contains("403") || e.contains("auth") || e.contains("forbidden"))
    }

    /// `authOrRetry` — remote + auth-ish → browser OAuth, else re-probe
    /// (App.tsx:1442).
    pub fn auth_or_retry(&'static self, id: &str, transport: &str, err: &str, plugin: &str) {
        if id.is_empty() {
            return;
        }
        if transport == "remote" && Self::needs_auth(err) {
            let id = id.to_owned();
            spawn(async move {
                let _ = client::post("/mcp/auth", serde_json::json!({"id": id})).await;
            });
        } else {
            self.retry_server(id, plugin);
        }
    }

    /// `installPlugin` (App.tsx:1407). A plugin with setup fields opens
    /// its Setup page first (Grok Bot's "needs a few values" form).
    pub fn install_plugin(&'static self, name: &str) {
        if !self.plugin_busy.get_untracked().is_empty() {
            return;
        }
        let needs_setup = self.plugins.get_untracked().iter().any(|p| p.name == name && p.setup);
        if needs_setup {
            self.open_setup(name, "install");
            return;
        }
        let name = name.to_owned();
        self.plugin_busy.set(name.clone());
        spawn(async move {
            let _ = client::post("/plugins/install", serde_json::json!({"name": name})).await;
            self.fetch_plugins();
            self.fetch_mcp();
            self.fetch_plugin_detail(&name);
            self.plugin_busy.set(String::new());
        });
    }

    /// Push the Setup page (`mode` = "install" | "edit") with a fresh
    /// draft: stored plain values prefilled, secrets blank.
    pub fn open_setup(&'static self, name: &str, mode: &str) {
        self.setup_vals.set(BTreeMap::new());
        self.setup_err.set(String::new());
        self.push_nav("setup", name, mode);
        let name = name.to_owned();
        let url = format!("/plugins/detail?name={}", client::urlenc(&name));
        spawn(async move {
            // the detail may not be loaded yet (Add from the market row).
            if let Some(v) = client::get(&url).await {
                let mut vals = BTreeMap::new();
                for f in list(&v, "fields") {
                    let val = s(&f, "value");
                    if !val.is_empty() {
                        vals.insert(s(&f, "key"), val);
                    }
                }
                self.setup_vals.set(vals);
            }
            self.fetch_plugin_detail(&name);
        });
    }

    /// Submit the Setup page: save the values (`/plugins/settings`), then
    /// install when adding. Required values are checked first.
    pub fn submit_setup(&'static self, name: &str, install: bool) {
        let d = self.plugin_details.get_untracked().get(name).cloned().unwrap_or_default();
        let vals = self.setup_vals.get_untracked();
        let missing: Vec<String> = d
            .fields
            .iter()
            .filter(|f| f.required && !f.set && vals.get(&f.key).map(|v| v.trim().is_empty()).unwrap_or(true))
            // a pre-registered-client field may wait (the account row says so)
            .filter(|f| !(f.key == "CLIENT_ID" || f.key.starts_with("GOOGLE_")))
            .map(|f| format!("{} is required", f.label))
            .collect();
        if let Some(m) = missing.first() {
            self.setup_err.set(m.clone());
            return;
        }
        let name = name.to_owned();
        self.plugin_busy.set(name.clone());
        self.setup_err.set(String::new());
        spawn(async move {
            let values: serde_json::Map<String, serde_json::Value> =
                vals.into_iter().map(|(k, v)| (k, serde_json::json!(v))).collect();
            let r = if install {
                client::post("/plugins/install", serde_json::json!({"name": name, "values": values})).await
            } else {
                client::post("/plugins/settings", serde_json::json!({"plugin": name, "values": values})).await
            };
            self.plugin_busy.set(String::new());
            match r {
                Some(v) if self::b(&v, "ok") => {
                    self.fetch_plugins();
                    self.fetch_mcp();
                    self.fetch_plugin_detail(&name);
                    self.pop_nav();
                }
                _ => self.setup_err.set(if install {
                    "Couldn't add the plugin. Check the values and try again.".into()
                } else {
                    "Couldn't save the values.".into()
                }),
            }
        });
    }

    /// The plugin row's shown name (Grok Bot's displayName), else the
    /// title-cased key.
    pub fn plugin_display(&self, name: &str) -> String {
        self.plugins
            .get_untracked()
            .iter()
            .find(|p| p.name == name && !p.display_name.is_empty())
            .map(|p| p.display_name.clone())
            .unwrap_or_else(|| Self::plugin_title(name))
    }

    /// The plugin's logo URL (empty = none): by catalog key, else the
    /// installed plugin that registered this connector id.
    pub fn plugin_icon(&self, name: &str) -> String {
        let rows = self.plugins.get_untracked();
        rows.iter()
            .find(|p| p.name == name)
            .or_else(|| rows.iter().find(|p| p.installed && p.meta_mcp.iter().any(|m| m == name)))
            .map(|p| p.icon.clone())
            .unwrap_or_default()
    }

    /// `uninstallPlugin` (App.tsx:1419).
    pub fn uninstall_plugin(&'static self, name: &str) {
        let name = name.to_owned();
        self.plugin_busy.set(name.clone());
        spawn(async move {
            let _ = client::post("/plugins/uninstall", serde_json::json!({"name": name})).await;
            self.fetch_plugins();
            self.fetch_plugin_detail(&name);
            self.pop_nav();
            self.plugin_busy.set(String::new());
        });
    }

    /// `savePrivateSkill` — `POST /skills` then refetch (App.tsx:1346).
    pub fn save_private_skill(&'static self) {
        let (id, name, desc, body) = (
            self.ps_id.get_untracked(),
            self.ps_name.get_untracked(),
            self.ps_desc.get_untracked(),
            self.ps_body.get_untracked(),
        );
        spawn(async move {
            let mut j = serde_json::json!({
                "name": name.trim(),
                "description": desc.trim(),
                "instructions": body,
            });
            if !id.is_empty() {
                j["id"] = serde_json::json!(id);
            }
            // a new skill keeps editing the row it just created.
            if let Some(v) = client::post("/skills", j).await {
                let new_id = s(&v, "id");
                if !new_id.is_empty() {
                    self.ps_id.set(new_id);
                }
            }
            self.fetch_skills();
        });
    }

    /// The private editor's Delete Skill (App.tsx:4152).
    pub fn delete_private_skill(&'static self) {
        let id = self.ps_id.get_untracked();
        if id.is_empty() {
            return;
        }
        spawn(async move {
            let _ = client::post("/skills/delete", serde_json::json!({"id": id})).await;
            self.fetch_skills();
            self.pop_nav();
        });
    }

    /// `pluginCats` — `["All", ...unique categories]` (App.tsx:1450).
    /// "All" + Grok Bot's categories (its order) that have plugins.
    pub fn plugin_cats(&self) -> Vec<String> {
        let rows = self.plugins.get();
        let mut cats: Vec<String> = vec!["All".into()];
        for c in self.plugin_cat_order.get() {
            if rows.iter().any(|p| p.cats.contains(&c)) {
                cats.push(c);
            }
        }
        cats
    }

    /// The plugins filed under a category (Grok's labels).
    pub fn plugins_in(&self, cat: &str) -> Vec<PluginRow> {
        self.plugins.get().into_iter().filter(|p| p.cats.iter().any(|c| c == cat)).collect()
    }

    /// "Recommended for you": the most popular plugins not added yet.
    pub fn plugin_recommended(&self) -> Vec<PluginRow> {
        self.plugins.get().into_iter().filter(|p| !p.installed).collect()
    }

    /// `pluginList` — category + query filters (App.tsx:1451).
    pub fn plugin_list(&self) -> Vec<PluginRow> {
        let q = self.plugin_query.get().trim().to_lowercase();
        let cat = self.plugin_cat.get();
        let mut list = if cat == RECOMMENDED { self.plugin_recommended() } else { self.plugins.get() };
        if cat != "All" && cat != RECOMMENDED {
            list.retain(|p| p.cats.iter().any(|c| c == &cat));
        }
        if !q.is_empty() {
            list.retain(|p| {
                format!("{} {} {} {} {}", p.display_name, p.name, p.publisher, p.description, p.keywords.join(" "))
                    .to_lowercase()
                    .contains(&q)
            });
        }
        list
    }

    /// `pluginTitle` — `n.replace(/[-_]+/g," ").replace(/\b\w/g,upper)`
    /// (App.tsx:1458).
    pub fn plugin_title(n: &str) -> String {
        let mut out = String::with_capacity(n.len());
        let mut word_start = true;
        for c in n.chars() {
            if c == '-' || c == '_' {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
                word_start = true;
            } else {
                out.push(if word_start { c.to_ascii_uppercase() } else { c });
                word_start = false;
            }
        }
        out
    }

    /// What the Connect Apps chip counts and shows: the v1 mini app
    /// plugins (BIR, Books…) first, then [`Store::conn_items`].
    pub fn connected_names(&self) -> Vec<String> {
        let (inst, standalone) = self.conn_items();
        let v1: Vec<(String, String)> = self.v1.listed.get().into_iter().map(|p| (p.id, p.title)).collect();
        conn_names(&v1, &inst, &standalone)
    }

    /// `connItems` — installed plugins + standalone MCP servers
    /// (App.tsx:3528): `(installed, standalone)`.
    pub fn conn_items(&self) -> (Vec<PluginRow>, Vec<McpServer>) {
        let inst: Vec<PluginRow> = self
            .plugins
            .get()
            .into_iter()
            .filter(|p| p.installed)
            .collect();
        let details = self.plugin_details.get();
        let ids: std::collections::BTreeSet<String> = inst
            .iter()
            .flat_map(|p| {
                details
                    .get(&p.name)
                    .map(|d| {
                        let mut ids = d.meta_mcp.clone();
                        for m in &d.mcp {
                            ids.extend(m.accounts.iter().map(|a| a.id.clone()));
                        }
                        ids
                    })
                    .unwrap_or_default()
            })
            .collect();
        let standalone: Vec<McpServer> = self
            .mcp_servers
            .get()
            .into_iter()
            .filter(|s| !ids.contains(&s.id))
            .collect();
        (inst, standalone)
    }

    /// `dropOpen` — the search combo box (App.tsx:1524).
    pub fn drop_open(&self) -> bool {
        self.plugins_open.get()
            && self.plugin_page() == "market"
            && !self.plugin_query.get().is_empty()
            && !self.drop_hidden.get()
            && !self.plugin_list().is_empty()
    }

    /// `dropMove` — ↑/↓ over `pluginList` (App.tsx:1529).
    pub fn drop_move(&self, d: i64) {
        let n = self.plugin_list().len() as i64;
        if n == 0 {
            return;
        }
        let i = (self.drop_sel.get_untracked() as i64 + d).clamp(0, n - 1);
        self.drop_sel.set(i as usize);
    }

    /// `dropChoose` — Enter opens the selected row (App.tsx:1525).
    pub fn drop_choose(&'static self) {
        let name = self
            .plugin_list()
            .get(self.drop_sel.get_untracked())
            .map(|p| p.name.clone());
        if let Some(name) = name {
            self.push_nav("plugin", &name, "");
        }
    }

    /// `openAvatarEditor`/`closeAvatarEditor` (App.tsx:1666).
    pub fn open_avatar_editor(&'static self) {
        self.avatar_hover.set(false);
        self.avatar_err.set(String::new());
        self.avatar_editing.set(true);
    }
    pub fn close_avatar_editor(&'static self) {
        self.avatar_editing.set(false);
        self.avatar_draft.set(None);
        self.avatar_gen_busy.set(false);
        self.avatar_err.set(String::new());
    }

    /// `generateAvatar` — `POST /avatar/image/gen` → the crop draft
    /// (App.tsx:1697).
    pub fn generate_avatar(&'static self) {
        let d = self.avatar_gen_prompt.get_untracked().trim().to_owned();
        if d.is_empty() || self.avatar_gen_busy.get_untracked() {
            return;
        }
        self.avatar_err.set(String::new());
        self.avatar_gen_busy.set(true);
        spawn(async move {
            let j = client::post("/avatar/image/gen", serde_json::json!({"description": d})).await;
            match j {
                Some(v) if self::b(&v, "ok") => {
                    self.draft_crop.set(Crop { s: 1.0, x: 0.0, y: 0.0 });
                    self.avatar_draft.set(Some(AvatarDraft {
                        uri: s(&v, "uri"),
                        name: s(&v, "name"),
                        w: f64of(&v, "w"),
                        h: f64of(&v, "h"),
                    }));
                }
                Some(v) => self
                    .avatar_err
                    .set(if s(&v, "error").is_empty() { "Couldn't generate an avatar".into() } else { s(&v, "error") }),
                None => self.avatar_err.set("Couldn't generate an avatar".into()),
            }
            self.avatar_gen_busy.set(false);
        });
    }

    /// `pickAvatar("pick"|"paste")` — `POST /avatar/image/:kind`
    /// (App.tsx:1711).
    pub fn pick_avatar(&'static self, kind: &'static str) {
        self.avatar_err.set(String::new());
        spawn(async move {
            let j = client::post(&format!("/avatar/image/{kind}"), serde_json::json!({})).await;
            match j {
                Some(v) if self::b(&v, "ok") => {
                    self.draft_crop.set(Crop { s: 1.0, x: 0.0, y: 0.0 });
                    self.avatar_draft.set(Some(AvatarDraft {
                        uri: s(&v, "uri"),
                        name: s(&v, "name"),
                        w: f64of(&v, "w"),
                        h: f64of(&v, "h"),
                    }));
                }
                Some(v) if !self::b(&v, "cancelled") => self.avatar_err.set(
                    if kind == "paste" {
                        "No image on the clipboard".into()
                    } else if s(&v, "error").is_empty() {
                        "Couldn't open that image".into()
                    } else {
                        s(&v, "error")
                    },
                ),
                _ => {}
            }
        });
    }

    /// The crop editor's "Set avatar": `updateBot {image, crop}` then
    /// close (App.tsx:1792).
    pub fn set_avatar(&'static self) {
        let Some(d) = self.avatar_draft.get_untracked() else { return };
        let Some(b) = self.active_bot_untracked() else { return };
        let c = self.draft_crop.get_untracked();
        self.update_bot(
            &b.id,
            serde_json::json!({"image": d.uri, "crop": {"s": c.s, "x": c.x, "y": c.y}}),
        );
        self.close_avatar_editor();
    }

    /// `clampCrop` — the draft crop stays inside its zoom's rim
    /// (App.tsx:1726).
    pub fn clamp_crop(c: Crop) -> Crop {
        let lim = (c.s - 1.0) / 2.0;
        Crop {
            s: c.s,
            x: c.x.max(-lim).min(lim),
            y: c.y.max(-lim).min(lim),
        }
    }

    /// `zoomBy`/`imgPan`/`zoomPan` for the crop editor.
    pub fn crop_zoom(&'static self, d: f64) {
        let c = self.draft_crop.get_untracked();
        self.draft_crop
            .set(Self::clamp_crop(Crop { s: (c.s + d).max(1.0).min(3.0), ..c }));
    }
    pub fn crop_drag(&'static self, from: Crop, dx: f64, dy: f64) {
        self.draft_crop.set(Self::clamp_crop(Crop {
            s: from.s,
            x: from.x + dx / 84.0,
            y: from.y + dy / 84.0,
        }));
    }
    pub fn crop_scale(&'static self, from: Crop, dx: f64) {
        self.draft_crop.set(Self::clamp_crop(Crop {
            s: (from.s + (dx / 236.0) * 2.0).max(1.0).min(3.0),
            ..from
        }));
    }

    // -- chat --------------------------------------------------------------

    /// `mockStatus(arg)` — the `/status` slash command (App.tsx:143):
    /// `/status` or `/status:demo` cycles every state, `/status:<name>` sets one,
    /// `/status:off|idle|clear` stops. Local only — never hits `/chat`.
    pub fn mock_status(&'static self, arg: &str) {
        // Any new `/status…` stops a running demo (`clearInterval(mockTimer)`).
        static DEMO: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let run = DEMO.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let live = move || DEMO.load(std::sync::atomic::Ordering::Relaxed) == run;
        let thread = self.active_id.get_untracked();
        let bot = self.active_bot_untracked().map(|b| b.id);
        self.settled.set(None);
        if arg.is_empty() || arg == "off" || arg == "idle" || arg == "clear" {
            self.agent_set(thread, bot, None);
            return;
        }
        if arg == "demo" {
            // `names = STATES.filter(n => n !== "idle")`, 1.7s each.
            let names: Vec<&'static str> = crate::mark::STATES
                .iter()
                .map(|(n, _)| *n)
                .filter(|n| *n != "idle")
                .collect();
            if let Some(first) = names.first() {
                let label = crate::statusfx::status_label(first);
                self.agent_set(
                    thread.clone(),
                    bot.clone(),
                    Some((first.to_string(), format!("{first} — {label}"))),
                );
            }
            spawn(async move {
                for name in names.iter().skip(1) {
                    timer(std::time::Duration::from_millis(1700)).await;
                    if !live() {
                        return;
                    }
                    let label = crate::statusfx::status_label(name);
                    self.agent_set(
                        thread.clone(),
                        bot.clone(),
                        Some((name.to_string(), format!("{name} — {label}"))),
                    );
                }
                timer(std::time::Duration::from_millis(1700)).await;
                if live() {
                    self.agent_set(thread, bot, None);
                }
            });
            return;
        }
        if crate::mark::STATES.iter().any(|(n, _)| *n == arg) {
            let label = crate::statusfx::status_label(arg);
            self.agent_set(thread, bot, Some((arg.to_owned(), label)));
        } else {
            self.agent_set(
                thread,
                bot,
                Some((
                    "idle".to_owned(),
                    format!("unknown status \"{arg}\" — try /status:demo"),
                )),
            );
        }
    }

    /// `POST /chat` with the composer contents, then follow the run.
    pub fn send(&'static self) {
        // an agents' conversation is read-only (the broker refuses it too)
        if untracked(|| self.active_agents().is_some()) {
            return;
        }
        let text = self.draft.get_untracked().trim().to_owned();
        // A v1 plugin's tokens and commands (9.3): tokens are rewritten to
        // canonical ones before anything is sent; tokens or a command alone
        // never reach the model (mini_v1::ui).
        let resolved = self.v1_resolved.replace(false);
        if !resolved && !text.starts_with("/status") && self.v1_send(&text) {
            return;
        }
        // Commands (`/status`, a server's) and a message of `@` tokens
        // alone run here — they never reach the model (slash.rs).
        if !resolved && crate::slash::dispatch(&text) {
            return;
        }
        let reply_to = self.reply_to.get_untracked();
        // The replied-to message's files lead (a card's module:// uri, a
        // file path), then the attachments verbatim — the server forwards
        // them to the model (picked files carry `uri`, reply files `path`).
        let reply_files = reply_to.iter().flat_map(|r| r.files.iter()).map(|p| {
            let uri = if p.contains("://") { p.clone() } else { format!("file://{p}") };
            let name = uri.trim_end_matches('/').rsplit('/').next().unwrap_or(&uri).to_owned();
            serde_json::json!({ "uri": uri, "name": name, "kind": "file" })
        });
        let files: Vec<serde_json::Value> = reply_files
            .chain(self.attachments.get_untracked().iter().map(|a| {
                serde_json::json!({
                    "uri": a.url, "path": a.path, "name": a.name, "mime": a.mime,
                    "kind": a.kind, "ext": a.ext, "size": a.size,
                    "thumb": a.thumb, "feed": a.feed, "poster": a.poster,
                    "width": a.width, "height": a.height,
                    "duration": a.duration, "animated": a.animated,
                })
            }))
            .collect();
        if (text.is_empty() && files.is_empty())
            || !may_send(&self.sending.get_untracked(), &self.active_id.get_untracked())
        {
            log::debug!(
                "send: held in {} (busy: {:?})",
                self.active_id.get_untracked(),
                self.sending.get_untracked()
            );
            return;
        }
        log::debug!("send: to {} (busy: {:?})", self.active_id.get_untracked(), self.sending.get_untracked());
        let v1_attachments = self.v1_model_attachments();
        // what this message goes to the model with, for its bubble
        self.v1_note_sent(&text, &v1_attachments);
        // A new message moves the chat on: its unanswered cards are
        // superseded, as nativechat's steer closes them.
        if self.turns_of.get_untracked() == self.active_id.get_untracked() {
            crate::cards::supersede_open(&self.turns.get_untracked());
        }
        self.draft.set(String::new());
        self.attachments.set(Vec::new());
        self.reply_to.set(None);
        {
            let thread = self.active_id.get_untracked();
            self.sending.update(|s| {
                s.insert(thread);
            });
        }
        // `agentSet(thread, bot, {state:"working",label:statusLabel("working")})`
        // — the run is credited to the open thread's bot so its marks can
        // play the state.
        let thread = self.active_id.get_untracked();
        let bot = self.active_bot_untracked().map(|b| b.id);
        self.agent_set(
            thread.clone(),
            bot.clone(),
            Some(("working".to_owned(), crate::statusfx::status_label("working"))),
        );
        // under the store's owner: the press that sent it (the Send
        // button) is replaced as the draft clears, and its owner with it
        let busy = thread.clone();
        self.in_store(|| spawn(async move {
            // /chat reads {prompt, reply:{i,text,files}, attachments} and
            // runs on the server's active thread — select first so the
            // reply lands on the visible one.
            client::post(
                "/thread/select",
                serde_json::json!({"id": self.active_id.get_untracked()}),
            )
            .await;
            let body = serde_json::json!({
                // the chat it was sent from: several run at once (the
                // server's active one is wherever the last select went)
                "thread": thread,
                "prompt": text,
                // the quote keeps canonical tokens: the model gets the link
                "reply": reply_to.map(|r| serde_json::json!({"i": r.i, "text": r.text})),
                "attachments": files,
                // the chat's v1 attachments the model may see (10.3: none
                // that needs update, nor of a disabled or missing plugin)
                "v1_attachments": v1_attachments,
            });
            let mut job = None;
            if let Some(v) = client::post("/chat", body).await {
                // The job id is a number in the payload.
                job = v
                    .get("id")
                    .and_then(|x| x.as_i64().or_else(|| x.as_str().and_then(|s| s.parse().ok())));
            }
            match job {
                Some(id) => self.watch_run(id.to_string(), thread, bot).await,
                // `agentSet(thread, bot, {state:"failed",…})` then drop
                // the row — the server never queued the run.
                None => {
                    self.agent_set(
                        thread.clone(),
                        bot.clone(),
                        Some((
                            "failed".to_owned(),
                            "no generator — run the app server".to_owned(),
                        )),
                    );
                    self.agent_set(thread, bot, None);
                }
            }
            self.sending.update(|s| {
                s.remove(&busy);
            });
            // Grok: `if (!agent()) emphBot.set("")` — the row is gone.
            self.emph_bot.set(String::new());
            self.refresh_turns();
        }));
    }

    /// Sends `text` as the next message, as the composer would, leaving the
    /// composer's own draft, files and reply as they were (a choice card's
    /// answer, a plugin-needs card's Send again).
    pub fn send_text(&'static self, text: String) {
        let kept = (
            self.draft.get_untracked(),
            self.attachments.get_untracked(),
            self.reply_to.get_untracked(),
        );
        batch(|| {
            self.draft.set(text);
            self.attachments.set(Vec::new());
            self.reply_to.set(None);
        });
        self.send();
        batch(|| {
            self.draft.set(kept.0);
            self.attachments.set(kept.1);
            self.reply_to.set(kept.2);
        });
    }

    /// A v1 app the person opened inline (a command or token, spec Q9):
    /// a local reply at the end of the open chat holding its card, shown
    /// at once. The card's id (`<turn id>:<link id>`), "" without a chat.
    pub fn add_local_link(&'static self, link: DriveLink) -> String {
        let chat = self.active_id.get_untracked();
        if chat.is_empty() {
            crate::toast::show("Open a chat first");
            return String::new();
        }
        let n = self.local_turns.borrow().len() as i64;
        let after = self
            .turns
            .with_untracked(|t| t.iter().rev().find(|t| t.i < LOCAL_TURN_BASE).map(|t| t.i))
            .unwrap_or(-1);
        let id = format!("local-app-{n}");
        let card = format!("{id}:{}", link.id);
        let turn = Turn { i: LOCAL_TURN_BASE + n, id, role: "ai".to_owned(), links: vec![link], ..Turn::default() };
        self.local_turns.borrow_mut().push((chat.clone(), after, turn));
        if self.turns_of.get_untracked() == chat {
            self.turns.update(|t| {
                t.retain(|t| t.i < LOCAL_TURN_BASE);
                self.merge_local_turns(&chat, t);
            });
        }
        card
    }

    /// The dev hook's reply in the wire shape (`/turns`' turn: its links,
    /// answer cards and next steps) as a local reply in the open chat.
    pub fn add_local_wire_turn(&'static self, v: &serde_json::Value) {
        let chat = self.active_id.get_untracked();
        if chat.is_empty() {
            return crate::toast::show("Open a chat first");
        }
        let n = self.local_turns.borrow().len() as i64;
        let after = self
            .turns
            .with_untracked(|t| t.iter().rev().find(|t| t.i < LOCAL_TURN_BASE).map(|t| t.i))
            .unwrap_or(-1);
        let mut turn = Turn::of(v);
        turn.i = LOCAL_TURN_BASE + n;
        turn.id = format!("local-turn-{n}");
        self.local_turns.borrow_mut().push((chat.clone(), after, turn));
        if self.turns_of.get_untracked() == chat {
            self.turns.update(|t| {
                t.retain(|t| t.i < LOCAL_TURN_BASE);
                self.merge_local_turns(&chat, t);
            });
        }
    }

    /// The dev hook's cards as a local reply at the end of the open chat.
    pub fn add_local_turn(&'static self, cards: Vec<crate::cards::TurnCard>) {
        let chat = self.active_id.get_untracked();
        if chat.is_empty() {
            crate::toast::show("Open a chat first");
            return;
        }
        let n = self.local_turns.borrow().len() as i64;
        let after = self
            .turns
            .with_untracked(|t| t.iter().rev().find(|t| t.i < LOCAL_TURN_BASE).map(|t| t.i))
            .unwrap_or(-1);
        let turn = Turn {
            i: LOCAL_TURN_BASE + n,
            id: format!("local-cards-{n}"),
            role: "ai".to_owned(),
            cards,
            ..Turn::default()
        };
        self.local_turns.borrow_mut().push((chat.clone(), after, turn));
        if self.turns_of.get_untracked() == chat {
            self.turns.update(|t| {
                t.retain(|t| t.i < LOCAL_TURN_BASE);
                self.merge_local_turns(&chat, t);
            });
        }
    }

    /// Forgets the dev hook's turns.
    pub fn drop_local_turns(&'static self) {
        self.local_turns.borrow_mut().clear();
        self.turns.update(|t| t.retain(|t| t.i < LOCAL_TURN_BASE));
    }

    /// `chat`'s local turns, each after the server turn it followed.
    fn merge_local_turns(&self, chat: &str, turns: &mut Vec<Turn>) {
        for (c, after, turn) in self.local_turns.borrow().iter() {
            if c != chat {
                continue;
            }
            let at = turns
                .iter()
                .position(|t| t.i > *after && t.i < LOCAL_TURN_BASE)
                .unwrap_or(turns.len());
            turns.insert(at, turn.clone());
        }
    }

    /// The composer's status while the open chat waits on a card: "Waiting
    /// for approval" / "Waiting for you" (`phase_state("waiting")`), never
    /// over a run that is going; cleared once nothing waits.
    pub fn watch_cards(&'static self) {
        effect(move || {
            let chat = self.active_id.get();
            if chat.is_empty() || self.turns_of.get() != chat {
                return;
            }
            let label = self.turns.with(|t| crate::cards::waiting_label(t));
            let run = self.agent_runs.with(|m| m.get(&chat).cloned());
            let ours = |r: &AgentRun| {
                r.state == phase_state("waiting")
                    && matches!(r.label.as_str(), "Waiting for approval" | "Waiting for you")
            };
            untracked(|| match (label, run) {
                (Some(label), None) => self.agent_set(
                    chat,
                    self.active_bot_untracked().map(|b| b.id),
                    Some((phase_state("waiting").to_owned(), label.to_owned())),
                ),
                (Some(label), Some(r)) if ours(&r) && r.label != label => {
                    self.agent_set(chat, r.bot, Some((r.state, label.to_owned())))
                }
                (None, Some(r)) if ours(&r) => self.agent_set(chat, r.bot, None),
                _ => {}
            });
        });
    }

    /// ⌘D's dictation stub — a "coming soon" run that clears itself
    /// after 2.4s (hot.rs's `mod+d` handler / the mic button).
    pub fn dictate_stub(&'static self) {
        let tid = self.active_id.get_untracked();
        if tid.is_empty() {
            return;
        }
        let bot = self.active_bot_untracked().map(|b| b.id);
        self.agent_set(
            tid.clone(),
            bot,
            Some(("dictating".to_owned(), "Dictation is coming soon".to_owned())),
        );
        spawn(async move {
            timer(std::time::Duration::from_millis(2400)).await;
            if self.agent().map(|a| a.label) == Some("Dictation is coming soon".to_owned()) {
                self.agent_set(tid, self.active_bot_untracked().map(|b| b.id), None);
            }
        });
    }

    /// `pickAttachments` — `POST /attach/pick` runs the host's file
    /// picker; the answer's `files` join the staged attachments
    /// (App.tsx:493).
    /// The files belong to the chat the picker was opened from: switched
    /// away meanwhile, they join that chat's draft, not the open one's.
    pub fn pick_attachments(&'static self) {
        let thread = self.active_id.get_untracked();
        spawn(async move {
            if let Some(v) = client::post("/attach/pick", serde_json::json!({})).await {
                let files: Vec<Attach> = v
                    .get("files")
                    .and_then(|x| x.as_array())
                    .map(|l| l.iter().map(Attach::of).collect())
                    .unwrap_or_default();
                if files.is_empty() {
                    return;
                }
                if self.active_id.get_untracked() == thread {
                    self.attachments.update(|a| a.extend(files));
                } else {
                    self.drafts.borrow_mut().entry(thread).or_default().attachments.extend(files);
                    self.save_drafts();
                }
            }
        });
    }

    /// Poll `/status?id=` until the run settles; each tick refreshes turns
    /// so streamed text lands without waiting for the end. The payload is
    /// `{id, seq, phase, detail, done}`. `thread`/`bot` are the run's
    /// `agentRuns` key and credit.
    async fn watch_run(&'static self, job: String, thread: String, bot: Option<String>) {
        // Grok Bot's status line: a status shows for at least HOLD before
        // the next replaces it, and gains "· Nm" once it has shown for a
        // minute. The server's recent `events` are queued,
        // so a step shorter than the poll (a fast tool) still shows.
        const HOLD: std::time::Duration = std::time::Duration::from_millis(800);
        let started = std::time::Instant::now();
        let mut shown: Option<(String, String)> = None;
        let mut queue: std::collections::VecDeque<(String, String)> = Default::default();
        let mut last_seq = 0i64;
        let mut shown_at = std::time::Instant::now();
        let mut tick = 0u32;
        let mut opened: Option<String> = None;
        while started.elapsed() < std::time::Duration::from_secs(30 * 60) {
            let Some(v) = client::get(&format!("/status?id={job}")).await else { break };
            let phase = s(&v, "phase");
            let detail = s(&v, "detail");
            let status = if detail.is_empty() { phase.clone() } else { detail.clone() };
            let shape = |phase: &str, detail: String| {
                let state = phase_state(phase).to_owned();
                // The server's phrase verbatim, as Grok Bot shows it
                // ("Searching for bbtree", "Typing...").
                let label = if detail.is_empty() { crate::statusfx::status_label(&state) } else { detail };
                (state, label)
            };
            // New steps since the last poll (an older server: just the latest).
            let mut fresh: Vec<(String, String)> = Vec::new();
            match v.get("events").and_then(|e| e.as_array()) {
                Some(evs) => {
                    for e in evs {
                        let seq = e.get("seq").and_then(|x| x.as_i64()).unwrap_or(0);
                        if seq > last_seq {
                            last_seq = seq;
                            let ph = s(e, "phase");
                            if ph != "done" {
                                fresh.push(shape(&ph, s(e, "detail")));
                            }
                        }
                    }
                }
                None if phase != "done" => fresh.push(shape(&phase, detail)),
                None => {}
            }
            for next in fresh {
                let tail = queue.back().or(shown.as_ref());
                if tail != Some(&next) {
                    queue.push_back(next);
                }
            }
            // A backlog keeps only the newest few, so the line never lags far.
            while queue.len() > 3 {
                queue.pop_front();
            }
            if (shown.is_none() || shown_at.elapsed() >= HOLD)
                && let Some(next) = queue.pop_front() {
                    shown = Some(next);
                    shown_at = std::time::Instant::now();
            }
            if let Some((state, label)) = shown.clone() {
                let mins = shown_at.elapsed().as_secs() / 60;
                let label = if mins >= 1 { format!("{label} · {}", elapsed_label(mins)) } else { label };
                self.agent_set(thread.clone(), bot.clone(), Some((state, label)));
            }
            // a server asked to show one of its mini apps (once per job),
            // after Open in the link sheet
            if let Some(open) = v.get("open").filter(|o| o.is_object()) {
                let key = s(open, "key");
                if !key.is_empty() && opened.as_deref() != Some(key.as_str()) {
                    opened = Some(key.clone());
                    self.open_mini_from(crate::links::AppOpener::ToolResult, &key, &s(open, "title"), "");
                }
            }
            // The reply streams into the transcript; the chat list less often.
            self.refresh_turns();
            if tick.is_multiple_of(4) {
                self.refresh_threads();
            }
            tick += 1;
            if v.get("done").and_then(|x| x.as_bool()).unwrap_or(false)
                || status == "done"
                || status == "error"
                || status == "failed"
            {
                // `agentSet(thread, bot, null)` then `settle(bot, …)`:
                // the status row hides, and the bot's mark flashes the
                // outcome for 8s.
                // Grok: `settle(bot, /^(failed|generator error)/.test(detail)
                //   ? "failed" : "finished")`.
                let ok = !(status.starts_with("failed") || status.starts_with("generator error"));
                // A new app's icon is drawn after the turn ends.
                self.watch_app_icons();
                // the files, images and Drive links the reply made join
                // the open chat's Library
                self.refresh_thread_apps();
                self.agent_set(thread, bot.clone(), None);
                self.emph_bot.set(String::new());
                self.settled.set(Some(Settle {
                    name: s(&v, "name"),
                    ok,
                    bot: bot.unwrap_or_default(),
                    state: if ok { "finished".into() } else { "failed".into() },
                }));
                let settled = self.settled;
                spawn(async move {
                    timer(std::time::Duration::from_secs(8)).await;
                    settled.set(None);
                });
                break;
            }
            timer(std::time::Duration::from_millis(250)).await;
        }
    }

    /// `POST /thread/new` — a fresh chat.
    pub fn new_thread(&'static self) {
        self.new_thread_with(String::new());
    }

    /// A fresh chat whose composer holds `draft`, unsent
    /// (`buwiz://chat/new?draft=`).
    pub fn new_thread_with(&'static self, draft: String) {
        spawn(async move {
            if let Some(v) = client::post("/thread/new", serde_json::json!({})).await {
                if let Some(v) = client::get("/threads").await {
                    self.set_threads(v);
                }
                if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                    // the switch to it loads its draft (`track_drafts`)
                    if !draft.is_empty() {
                        self.keep_draft(id, Draft { text: draft, ..Default::default() });
                    }
                    self.open_thread(id.to_owned());
                }
            }
        });
    }

    /// `createBot` (App.tsx:627): `POST /bot/new` with the picker's
    /// draft, open the bot's new thread, adopt the returned turns.
    pub fn create_bot(&'static self) {
        let name = self.bot_name.get_untracked().trim().to_owned();
        if name.is_empty() {
            return;
        }
        self.bot_modal.set(false);
        let shape = self.bot_shape.get_untracked();
        let color = self.bot_color.get_untracked();
        spawn(async move {
            let Some(v) = client::post(
                "/bot/new",
                serde_json::json!({"name": name, "shape": shape, "color": color}),
            )
            .await
            else {
                return;
            };
            if let Some(tid) = v
                .get("bot")
                .and_then(|b| b.get("threadId"))
                .and_then(|t| t.as_str())
            {
                self.open_thread(tid.to_owned());
            }
            self.bot_name.set(String::new());
            if let Some(v) = client::get("/threads").await {
                self.set_threads(v);
            }
        });
    }

    /// The open plain chat's title, from the pane's name field.
    pub fn rename_chat(&'static self, name: String) {
        let id = self.active_id.get_untracked();
        if id.is_empty() {
            return;
        }
        self.threads.update(|t| {
            if let Some(t) = t.iter_mut().find(|t| t.id == id) {
                t.name = name.clone();
            }
        });
        self.rename_thread(id, name);
    }

    pub fn rename_thread(&'static self, id: String, name: String) {
        client::post_bg(
            "/thread/update".to_owned(),
            serde_json::json!({"id": id, "title": name}),
        );
    }

    /// `deleteThread` — `POST /thread/delete`; the reply carries the
    /// server's new `active` when the open thread was removed.
    pub fn delete_thread(&'static self, id: String) {
        spawn(async move {
            let r = client::post("/thread/delete", serde_json::json!({"id": id})).await;
            let next = r.as_ref().map(|v| s(v, "active")).unwrap_or_default();
            if let Some(v) = client::get("/threads").await {
                self.set_threads(v);
            }
            if self.active_id.get_untracked() == id {
                if !next.is_empty() {
                    self.open_thread(next);
                } else if let Some(t) = self.threads.get_untracked().first() {
                    self.open_thread(t.id.clone());
                }
            } else {
                self.refresh_turns();
            }
        });
    }

    /// `updateThread` — optimistic `hidden`/`archived` patch +
    /// `POST /thread/update` (App.tsx:853).
    pub fn update_thread(&'static self, id: &str, hidden: Option<bool>, archived: Option<bool>) {
        self.threads.update(|rows| {
            for t in rows.iter_mut() {
                if t.id == id {
                    if let Some(h) = hidden {
                        t.hidden = h;
                    }
                    if let Some(a) = archived {
                        t.archived = a;
                    }
                }
            }
        });
        let mut body = serde_json::json!({"id": id});
        if let Some(h) = hidden {
            body["hidden"] = h.into();
        }
        if let Some(a) = archived {
            body["archived"] = a.into();
        }
        client::post_bg("/thread/update".to_owned(), body);
    }

    /// `updateBot` — optimistic patch + `POST /bot/update` (App.tsx's
    /// updateBot: pinned/section/hidden/shape/color/model…).
    pub fn update_bot(&'static self, id: &str, patch: serde_json::Value) {
        self.bots.update(|rows| {
            for b in rows.iter_mut() {
                if b.id != id {
                    continue;
                }
                if let Some(v) = patch.get("pinned").and_then(|x| x.as_bool()) {
                    b.pinned = v;
                }
                if let Some(v) = patch.get("hidden").and_then(|x| x.as_bool()) {
                    b.hidden = v;
                }
                if let Some(v) = patch.get("section").and_then(|x| x.as_str()) {
                    b.section = v.to_owned();
                }
                if let Some(v) = patch.get("name").and_then(|x| x.as_str()) {
                    b.name = v.to_owned();
                }
                if let Some(v) = patch.get("shape").and_then(|x| x.as_str()) {
                    b.shape = v.to_owned();
                }
                if let Some(v) = patch.get("color").and_then(|x| x.as_str()) {
                    b.color = v.to_owned();
                }
                if let Some(v) = patch.get("image").and_then(|x| x.as_str()) {
                    b.image = v.to_owned();
                }
                if patch.get("crop").is_some() {
                    b.crop = patch["crop"].as_object().map(|_| Crop {
                        s: f64of(&patch["crop"], "s"),
                        x: f64of(&patch["crop"], "x"),
                        y: f64of(&patch["crop"], "y"),
                    });
                }
                if let Some(v) = patch.get("model").and_then(|x| x.as_str()) {
                    b.model = v.to_owned();
                }
                if let Some(v) = patch.get("effort").and_then(|x| x.as_str()) {
                    b.effort = v.to_owned();
                }
                if let Some(v) = patch.get("fast").and_then(|x| x.as_bool()) {
                    b.fast = v;
                }
                if let Some(v) = patch.get("description").and_then(|x| x.as_str()) {
                    b.description = v.to_owned();
                }
                if let Some(v) = patch.get("instructions").and_then(|x| x.as_str()) {
                    b.instructions = v.to_owned();
                }
            }
        });
        // The slim `bot` embedded on each `/threads` row carries
        // name/shape/color — keep it in step so anything reading it
        // (`active_bot`'s fallback, thread pills) reflects the edit.
        self.threads.update(|rows| {
            for t in rows.iter_mut() {
                let Some(tb) = t.bot.as_mut() else { continue };
                if tb.id != id {
                    continue;
                }
                if let Some(v) = patch.get("name").and_then(|x| x.as_str()) {
                    tb.name = v.to_owned();
                }
                if let Some(v) = patch.get("shape").and_then(|x| x.as_str()) {
                    tb.shape = v.to_owned();
                }
                if let Some(v) = patch.get("color").and_then(|x| x.as_str()) {
                    tb.color = v.to_owned();
                }
            }
        });
        // The optimistic `bots` write already carries a `name` patch —
        // advance the draft seed so a same-name server echo doesn't
        // reseed over a fresh edit.
        self.seed_bot_edit_name();
        let mut body = patch;
        body["id"] = id.into();
        client::post_bg("/bot/update".to_owned(), body);
    }

    // ——————————————————— files (fileviewer.tsx / lightbox.tsx) ———————————————————

    /// `raw(f)` — the server's bytes of a reply file.
    pub fn file_raw_url(path: &str) -> String {
        // a file on this Mac outside the server's (`buwiz://view`'s)
        if path.starts_with('/') {
            let enc = path.split('/').map(crate::client::urlenc).collect::<Vec<_>>().join("/");
            return format!("file://{enc}");
        }
        let enc = path
            .split('/')
            .map(crate::client::urlenc)
            .collect::<Vec<_>>()
            .join("/");
        format!("{}/file/raw/{enc}", crate::client::backend())
    }

    /// `toLight(f)` — a reply file as a lightbox item.
    pub fn to_light(f: &Attach) -> LightItem {
        if f.kind == "video" {
            LightItem {
                kind: "video".into(),
                uri: Self::file_raw_url(&f.path),
                name: f.name.clone(),
                path: f.path.clone(),
                poster: if f.poster.is_empty() {
                    String::new()
                } else {
                    Self::file_raw_url(&f.poster)
                },
                w: f.width,
                h: f.height,
                ..Default::default()
            }
        } else {
            // attachments have a full `uri` and no path — the raw key is
            // only for generated files.
            let uri = if f.path.is_empty() {
                f.url.clone()
            } else {
                Self::file_raw_url(&f.path)
            };
            LightItem {
                kind: "image".into(),
                uri,
                name: f.name.clone(),
                path: f.path.clone(),
                w: f.width,
                h: f.height,
                ..Default::default()
            }
        }
    }

    /// `openLightbox(list, at)`.
    pub fn open_lightbox(&'static self, items: Vec<LightItem>, at: usize) {
        self.light_zoom.set(1.0);
        let last = items.len().saturating_sub(1);
        self.light_idx.set(at.min(last));
        self.light_items.set(items);
    }

    /// `step(d)` — wraps, resets zoom (lightbox.tsx).
    pub fn light_step(&'static self, d: i64) {
        let n = self.light_items.get_untracked().len() as i64;
        if n < 2 {
            return;
        }
        let i = (self.light_idx.get_untracked() as i64 + d).rem_euclid(n) as usize;
        self.light_idx.set(i);
        self.light_zoom.set(1.0);
    }

    pub fn light_close(&'static self) {
        self.light_items.set(Vec::new());
    }

    pub fn light_open(&'static self) -> bool {
        !self.light_items.get().is_empty()
    }

    /// `stopAudio` — pauses whichever card/player is sounding by clearing
    /// the claimed id (players bind `paused` to "not me").
    pub fn stop_audio(&'static self) {
        self.audio_cur.set(0);
    }

    /// `openFile(f)` — media opens the lightbox, everything else the
    /// fullscreen viewer fed by `GET /file/render?path=`.
    pub fn open_file(&'static self, f: &Attach) {
        self.stop_audio();
        if f.kind == "image" || f.kind == "video" {
            self.open_lightbox(vec![Self::to_light(f)], 0);
            return;
        }
        self.viewing.set(Some(f.clone()));
        self.rendered.set(None);
        // a reply file's saved versions (a mini app's come with View Code)
        let local = f.path.starts_with('/');
        if !f.path.starts_with("app:") && !local {
            crate::files::load_file_versions(&f.path);
        }
        self.sheet_idx.set(0);
        self.html_mode.set("preview".to_owned());
        self.sel_a.set(None);
        self.sel_b.set(None);
        // A spreadsheet opens in its native view (`office/`) when it can:
        // nothing to render on the server
        if crate::office::open(f) {
            self.rendered.set(Some(Rendered::default()));
            return;
        }
        // A file on this Mac outside the server's files (`buwiz://view`'s, a
        // mini app's print preview): read here, its text the source view
        // and what prints.
        if local {
            if f.kind != "pdf" {
                self.rendered.set(Some(match std::fs::read_to_string(&f.path) {
                    Ok(t) => Rendered {
                        lines: t.lines().map(|l| vec![(l.to_owned(), String::new(), String::new())]).collect(),
                        text: (f.kind != "text").then_some(t),
                        ..Default::default()
                    },
                    Err(e) => Rendered { error: e.to_string(), ..Default::default() },
                }));
            }
            return;
        }
        let path = f.path.clone();
        spawn(async move {
            let url = format!(
                "{}/file/render?path={}",
                crate::client::backend(),
                crate::client::urlenc(&path)
            );
            let r = match client::get(&url).await {
                Some(d) => Rendered::of(&d),
                None => Rendered {
                    error: "fetch failed".to_owned(),
                    ..Default::default()
                },
            };
            let st = crate::live::store();
            if st.viewing.get_untracked().map(|v| v.path) == Some(path.clone()) {
                st.rendered.set(Some(r));
            }
        });
    }

    /// Moves the open chat's message position to turn `i`, showing its
    /// ring (`shown`) or not, and remembers it for that chat.
    pub fn set_sel(&'static self, i: i64, shown: bool) {
        let chat = self.active_id.get_untracked();
        self.sel_marks.borrow_mut().insert(chat, i);
        batch(|| {
            self.sel_msg.set(Some(i));
            self.sel_shown.set(shown);
        });
    }

    /// Reply to turn `i` (⌘Y), as its ↩ does: a card's reply names it
    /// (`app · <title> v<n>`, `image · …`) and sends its module along.
    pub fn reply_to_turn(&'static self, i: i64) {
        let tr = self.turns.get_untracked();
        let Some(tt) = tr.iter().find(|x| x.i == i) else { return };
        let label = reply_label(tt);
        // nothing to quote: no empty chip
        if label.is_empty() {
            return crate::toast::show("This message has nothing to quote");
        }
        let files = tt
            .art
            .as_ref()
            .filter(|a| !a.app.is_empty())
            .map(|a| vec![format!("module://{}", a.app)])
            .unwrap_or_default();
        self.reply_to.set(Some(ReplyTo { i: tt.i, name: tt.name.clone(), text: label, files }));
    }

    /// View Code: mini app `key`'s source in the file viewer, highlighted
    /// like any code file — `R<n>` its current version, a card's
    /// `R<n>@v<k>` the version it shows. `file` names it (else the
    /// Library's file name, else `R<n>.tsx`).
    pub fn view_code(&'static self, key: &str, file: &str) {
        let name = app_name(key);
        let file = Some(file.to_owned())
            .filter(|f| !f.is_empty())
            .or_else(|| {
                self.thread_apps
                    .with_untracked(|v| v.iter().find(|a| a.app == name).map(|a| a.filename.clone()))
            })
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| format!("{name}.tsx"));
        self.open_file(&Attach {
            kind: "text".into(),
            path: format!("app:{key}"),
            name: file,
            ext: "tsx".into(),
            ..Default::default()
        });
        // the version picker's list (`/artifact/info`'s history)
        let name = name.to_owned();
        spawn_detached(async move {
            let Some(v) = client::get(&format!("/artifact/info?id={name}")).await else { return };
            let mut vs: Vec<i64> = list(&v, "history").iter().filter_map(|h| h.get("v")?.as_i64()).collect();
            vs.sort_unstable();
            let head = v.get("v").and_then(|x| x.as_i64()).unwrap_or(0);
            crate::files::viewer_versions().set((name, vs, head));
        });
    }

    /// Opens a reply's file in its own app (Preview, Pages, Keynote, …):
    /// `POST /file/open`.
    pub fn open_externally(&'static self, path: &str) {
        client::post_bg("/file/open".to_owned(), serde_json::json!({ "path": path }));
    }

    /// `saveFile(f)` — the download button posts `/file/save`.
    pub fn save_file(&'static self, path: &str) {
        client::post_bg(
            "/file/save".to_owned(),
            serde_json::json!({ "path": path }),
        );
    }

    /// `post("/copy", {text})` — the server's clipboard route.
    pub fn copy_text(&'static self, text: &str) {
        client::post_bg("/copy".to_owned(), serde_json::json!({ "text": text }));
    }

    /// `curSheet` — the open workbook tab, clamped to what rendered.
    pub fn cur_sheet(&'static self) -> Option<Sheet> {
        let r = self.rendered.get()?;
        let sh = r.sheets.get(self.sheet_idx.get_untracked().min(r.sheets.len().saturating_sub(1)))?;
        Some(sh.clone())
    }

    /// `inSel(r,c)` — inside the selection rectangle.
    pub fn in_sel(&'static self, r: usize, c: usize) -> bool {
        let (Some(a), Some(b)) = (self.sel_a.get(), self.sel_b.get()) else {
            return false;
        };
        r >= a.0.min(b.0) && r <= a.0.max(b.0) && c >= a.1.min(b.1) && c <= a.1.max(b.1)
    }

    /// The arrow keys' selection move (`useTableKeys`): Shift extends.
    pub fn sel_move(&'static self, dr: i64, dc: i64, extend: bool) {
        let (Some(sh), Some(b)) = (self.cur_sheet(), self.sel_b.get_untracked()) else {
            return;
        };
        let rows = sh.rows.len().max(1) as i64 - 1;
        let cols = (sh.rows.iter().map(|x| x.len()).max().unwrap_or(1).max(1)) as i64 - 1;
        let n = (
            (b.0 as i64 + dr).clamp(0, rows) as usize,
            (b.1 as i64 + dc).clamp(0, cols) as usize,
        );
        self.sel_b.set(Some(n));
        if !extend {
            self.sel_a.set(Some(n));
        }
    }

    /// `mod+c` over a selection — TSV to the server's clipboard.
    pub fn copy_sel(&'static self) {
        let (Some(sh), Some(a), Some(b)) =
            (self.cur_sheet(), self.sel_a.get_untracked(), self.sel_b.get_untracked())
        else {
            return;
        };
        let text = sh.rows[a.0.min(b.0)..=a.0.max(b.0)]
            .iter()
            .map(|row| row[a.1.min(b.1)..=a.1.max(b.1).min(row.len().saturating_sub(1))].join("\t"))
            .collect::<Vec<_>>()
            .join("\n");
        self.copy_text(&text);
    }

    /// `deleteBot` — `POST /bot/delete`, then refresh threads + turns.
    pub fn delete_bot(&'static self, id: String) {
        spawn(async move {
            let _ = client::post("/bot/delete", serde_json::json!({"id": id})).await;
            if let Some(v) = client::get("/threads").await {
                self.set_threads(v);
            }
            self.refresh_turns();
        });
    }

    /// `react` (shell.tsx:299) — `POST /react {key: "idx:<i>", r}`; the
    /// empty string clears the tapback. The shell updates its signal
    /// before the post (`reaction.set(next)`): the turn updates first,
    /// and the refetch waits on the post inside the same task. The task
    /// is detached — `react` runs from the emoji pill's press, which
    /// closes (disposes) the pill right after, and a task spawned under
    /// the pill's owner would be cancelled before the post even sent.
    fn art_key(&self, i: i64, slot: &str) -> String {
        format!("{}:{i}:{slot}", self.active_id.get_untracked())
    }

    /// Whether bar `(i, slot)`'s ⋯ menu is the open one (tracked).
    pub fn ctx_open_for(&'static self, i: i64, slot: &str) -> bool {
        self.ctx.with(|c| c.is_some())
            && self
                .ctx_owner
                .with(|o| o.as_ref().is_some_and(|(oi, os)| *oi == i && os == slot))
    }

    /// The reaction on turn `i`'s `slot`: `msg` is the turn's own (the
    /// server's), any other a file's or the media's. Tracked.
    pub fn reaction_of(&'static self, i: i64, slot: &str) -> String {
        if slot == "msg" {
            return self
                .turns
                .with(|t| t.iter().find(|t| t.i == i).map(|t| t.reaction.clone()))
                .unwrap_or_default();
        }
        let key = self.art_key(i, slot);
        self.art_reactions.with(|m| m.get(&key).cloned()).unwrap_or_default()
    }

    /// `react` for one slot: the message's goes to the server's turn, a
    /// file's or the media's to Rox's reactions in the state store.
    pub fn react_slot(&'static self, i: i64, slot: &str, emoji: String) {
        if slot == "msg" {
            return self.react(i, emoji);
        }
        let key = self.art_key(i, slot);
        self.art_reactions.update(|m| {
            if emoji.is_empty() {
                m.remove(&key);
            } else {
                m.insert(key, emoji);
            }
        });
        let state: serde_json::Map<String, serde_json::Value> = self
            .art_reactions
            .get_untracked()
            .into_iter()
            .map(|(k, v)| (k, serde_json::Value::String(v)))
            .collect();
        client::post_bg("/state/rox-reactions".to_owned(), serde_json::Value::Object(state));
    }

    pub fn react(&'static self, i: i64, emoji: String) {
        let mut turns = self.turns.get_untracked();
        if let Some(t) = turns.iter_mut().find(|t| t.i == i) {
            t.reaction = emoji.clone();
            self.turns.set(turns);
        }
        Owner::detached().run(|| {
            spawn(async move {
                let _ = client::post(
                    "/react",
                    serde_json::json!({"key": format!("idx:{i}"), "r": emoji}),
                )
                .await;
                self.refresh_turns();
            });
        });
    }

    /// `openPill` (shell.tsx:586) — a bar reported its measured rect;
    /// the pill anchors to it (`right` anchors the pill's right edge to
    /// the bar's right edge, `flip` lifts it above when it would clip
    /// the window's bottom).
    pub fn open_pill(&'static self, a: PillAnchor) {
        self.pill.set(Some(a));
    }
    /// `closeP` — the pill's owner, a scroll tick or `reset_fresh` closes
    /// it. `i = -1` closes unconditionally.
    pub fn close_pill(&'static self, i: i64) {
        if i < 0 || self.pill.get_untracked().as_ref().is_some_and(|a| a.i == i) {
            self.pill.set(None);
        }
    }

    /// `choice()` — the active bot's choice when one's open, else the
    /// app-wide pref (App.tsx:356).
    pub fn choice(&'static self) -> (String, String, bool) {
        match self.active_bot() {
            Some(b) => (b.model, b.effort, b.fast),
            None => (
                self.model.get(),
                self.effort.get(),
                self.fast.get(),
            ),
        }
    }

    /// `setChoice(c)` — a bot's own model when one's open, else `/prefs`
    /// (App.tsx:361).
    pub fn set_choice(&'static self, model: &str, effort: &str, fast: bool) {
        if let Some(b) = self.active_bot_untracked() {
            self.update_bot(
                &b.id,
                serde_json::json!({"model": model, "effort": effort, "fast": fast}),
            );
        } else {
            self.model.set(model.to_owned());
            self.effort.set(effort.to_owned());
            self.fast.set(fast);
            client::post_bg(
                "/prefs".to_owned(),
                serde_json::json!({"model": model, "effort": effort, "fast": fast}),
            );
        }
    }

    pub fn choose_model(&'static self, id: String) {
        let def = self
            .models
            .get_untracked()
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.default_effort.clone())
            .unwrap_or_default();
        let (_, _, fast) = self.choice();
        self.set_choice(&id, &def, fast);
    }

    pub fn choose_effort(&'static self, value: String) {
        let (m, _, f) = self.choice();
        self.set_choice(&m, &value, f);
    }

    pub fn toggle_fast(&'static self) {
        let (m, e, f) = self.choice();
        self.set_choice(&m, &e, !f);
    }

    // -- mini apps ---------------------------------------------------------

    /// The compiled JS of widget `app` (`R<n>`), compiled lazily:
    /// `GET /module`, then `compile_module`. The signal starts empty
    /// (the component shows nothing until the code lands).
    pub fn mini_source(&'static self, app: &str) -> Signal<String> {
        if let Some(s) = self.minis.borrow().get(app) {
            return *s;
        }
        let sig = self.owner.run(|| signal(String::new()));
        self.minis.borrow_mut().insert(app.to_owned(), sig);
        self.fetch_mini(app.to_owned(), sig);
        sig
    }

    /// Loads `name`'s module into `sig`. Detached: the cached signal
    /// outlives whichever card asked first. Under that card's owner, a
    /// rebuild before the fetch landed cancelled it and left every card
    /// on "Loading…" for good.
    fn fetch_mini(&'static self, name: String, sig: Signal<String>) {
        if crate::mini_v1::is_v1(&name) {
            return self.fetch_mini_v1(name, sig);
        }
        Owner::detached().run(|| spawn(async move {
            let Some(v) = client::get(&format!("/module?name={}", client::urlenc(&name))).await else {
                self.mini_err(&name).set("server unreachable".to_owned());
                return;
            };
            let title = v.get("title").and_then(|x| x.as_str()).unwrap_or_default();
            self.mini_title(&name).set(title.to_owned());
            let src = match crate::mini::module_source(&v) {
                Ok(src) => src,
                Err(e) => {
                    log::warn!("mini {name}: {e}");
                    self.mini_err(&name).set(e);
                    return;
                }
            };
            match crate::mini::compile_widget(&name, &src) {
                Ok(js) => {
                    self.mini_err(&name).set(String::new());
                    sig.set(js);
                }
                Err(e) => {
                    log::warn!("mini {name}: compile failed: {e}");
                    self.mini_err(&name).set(crate::mini::compile_error(&e));
                }
            }
        }));
    }

    /// [`fetch_mini`] for a v1 plugin's app: the broker's source, checked
    /// and compiled with the bridge off the main thread
    /// (`mini_v1::load::prepare`); the component opens its session.
    fn fetch_mini_v1(&'static self, name: String, sig: Signal<String>) {
        Owner::detached().run(|| spawn(async move {
            let key = name.clone();
            let prepared = crate::vault::off_main(move || {
                // the compiled-module cache (spec 14.6) unless turned off
                let root = crate::mini_v1::plugins_dir();
                let cache = crate::mini_v1::cache::enabled().then_some(root.as_path());
                crate::mini_v1::load::prepare_in(&crate::mini_v1::host_api::HttpBroker::configured(), &key, cache)
            })
            .await;
            match prepared {
                Some(Ok(mut p)) => {
                    self.mini_title(&name).set(p.title.clone());
                    // each fetch is a new open (Reload, a reconnect): the
                    // component reloads even when the source is the same
                    static FETCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    let n = FETCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    p.js = format!("{}\n// open {n}\n", p.js);
                    let js = p.js.clone();
                    crate::mini_v1::keep_prepared(&name, p);
                    self.mini_err(&name).set(String::new());
                    sig.set(js);
                }
                Some(Err(e)) => {
                    log::warn!("mini {name}: {e}");
                    self.mini_err(&name).set(e.message);
                }
                None => self.mini_err(&name).set("Could not open this app.".to_owned()),
            }
        }));
    }

    /// Refetches `app`'s module into its cached signal: a live card
    /// reloads in place (a new `source` restarts its engine).
    pub fn reload_mini(&'static self, app: &str) {
        let Some(sig) = self.minis.borrow().get(app).copied() else {
            return;
        };
        self.fetch_mini(app.to_owned(), sig);
    }

    /// The display title of widget `app` (set when `mini_source` loads).
    pub fn mini_source_title(&'static self, app: &str) -> Signal<String> {
        if let Some(s) = self.mini_titles.borrow().get(app) {
            return *s;
        }
        let sig = self.owner.run(|| signal(String::new()));
        self.mini_titles.borrow_mut().insert(app.to_owned(), sig);
        sig
    }

    /// The load/compile error of widget `app` (`""` = none/loading).
    pub fn mini_err(&'static self, app: &str) -> Signal<String> {
        if let Some(s) = self.mini_errs.borrow().get(app) {
            return *s;
        }
        let sig = self.owner.run(|| signal(String::new()));
        self.mini_errs.borrow_mut().insert(app.to_owned(), sig);
        sig
    }

    /// Drop the cached bundle + error so the next `mini_source` refetches.
    pub fn retry_mini(&'static self, app: &str) {
        self.minis.borrow_mut().remove(app);
        self.mini_err(app).set(String::new());
        self.mini_source(app);
    }

    /// Open the widget in the server's own window (`POST /popout`).
    /// `app` carries the version as `name@v<k>` (Grok's `Art.app` key);
    /// the route wants them split — `name` bare, `v` a number.
    pub fn popout(&'static self, app: &str) {
        client::post_bg("/popout".to_owned(), popout_body(app));
    }

    /// `popout()` (shell.tsx:760) at a size: the window opens at `w × h` —
    /// the size a card laid the app out at — instead of the server's
    /// default and then shrinking to fit; the card's status line notes the
    /// result.
    pub fn popout_sized(&'static self, app: &str, w: f32, h: f32) {
        let mut body = popout_body(app);
        if w > 0.0 {
            body["w"] = serde_json::json!(w.round());
        }
        if h > 0.0 {
            body["h"] = serde_json::json!(h.ceil());
        }
        let app = app.to_owned();
        Owner::detached().run(|| {
            spawn(async move {
                let d = client::post("/popout", body).await;
                let ok = d.as_ref().is_some_and(|d| b(d, "ok"));
                let note = match (ok, d.as_ref().is_some_and(|d| b(d, "already"))) {
                    (false, _) => "popout failed",
                    (true, true) => "window already open",
                    (true, false) => "↗ window opened",
                };
                self.card_note(&app, note);
            })
        });
    }

    /// The size card `i` (module `app`) lays its app out at: the declared
    /// width, the measured content height (the declared one until then).
    pub fn card_size(&'static self, i: i64, app: &str) -> (f32, f32) {
        let art = self
            .turns
            .with_untracked(|t| t.iter().find(|t| t.i == i).and_then(|t| t.art.clone()));
        let (aw, ah) = art.map_or((0.0, 0.0), |a| (a.w as f32, a.h as f32));
        let w = if aw > 0.0 { aw } else { 480.0 };
        let h = self
            .mini_heights
            .borrow()
            .get(app)
            .copied()
            .unwrap_or(if ah > 0.0 { ah } else { (w * 0.75).round() });
        (w, h)
    }

    /// A note on `app`'s card status line for 2s.
    fn card_note(&'static self, app: &str, text: &str) {
        let (app, text) = (app.to_owned(), text.to_owned());
        self.card_status.update(|m| {
            m.insert(app.clone(), (text.clone(), true));
        });
        Owner::detached().run(|| {
            spawn(async move {
                timer(std::time::Duration::from_millis(2000)).await;
                self.card_status.update(|m| {
                    if m.get(&app).is_some_and(|(t, note)| *note && *t == text) {
                        m.remove(&app);
                    }
                });
            })
        });
    }

    /// `app`'s card status line shows `work` in progress (None clears it).
    fn card_work(&'static self, app: &str, work: Option<&str>) {
        self.card_status.update(|m| match work {
            Some(t) => {
                m.insert(app.to_owned(), (t.to_owned(), false));
            }
            None => {
                m.remove(app);
            }
        });
    }

    /// `retry()` (shell.tsx:868) — a card's Try Again: the app is made
    /// again from the same request (`POST /retry`), the job followed on
    /// the card's status line (`GET /status`); when it lands the card
    /// reloads the new module in place.
    pub fn retry_card(&'static self, app: &str) {
        let busy = self
            .card_status
            .with_untracked(|m| m.get(app).is_some_and(|(_, note)| !note));
        if busy {
            return;
        }
        self.card_work(app, Some("queued"));
        let app = app.to_owned();
        Owner::detached().run(|| {
            spawn(async move {
                let name = app_name(&app).to_owned();
                let id = client::post("/retry", serde_json::json!({"name": name}))
                    .await
                    .and_then(|d| d.get("id").and_then(|x| x.as_i64()));
                let Some(id) = id else {
                    self.card_work(&app, None);
                    self.card_note(&app, "retry failed");
                    return;
                };
                let mut fails = 0;
                loop {
                    timer(std::time::Duration::from_millis(450)).await;
                    let Some(st) = client::get(&format!("/status?id={id}")).await else {
                        fails += 1;
                        if fails > 15 {
                            break;
                        }
                        continue;
                    };
                    fails = 0;
                    let detail = s(&st, "detail");
                    let phase = s(&st, "phase");
                    self.card_work(&app, Some(if detail.is_empty() { &phase } else { &detail }));
                    if b(&st, "done") {
                        break;
                    }
                }
                self.card_work(&app, None);
                self.mini_heights.borrow_mut().remove(&app);
                self.reload_mini(&app);
                self.refresh_turns();
            })
        });
    }

    /// A card's ⋯ → Details: the pane's Library with the app's row
    /// highlighted (agents and plain chats alike).
    pub fn show_in_library(&'static self, app: &str) {
        self.detail_tab.set("library".to_owned());
        self.library_focus.set(app_name(app).to_owned());
        self.refresh_thread_apps();
        self.right_open.set(true);
    }

    /// ⌘= (`dir` 1) / ⌘- (`dir` -1) move the window's zoom by 1% —
    /// finer than Chrome's steps; holding the key repeats; ⌘0 (`dir` 0)
    /// resets it.
    pub fn zoom_step(&'static self, dir: i32) {
        let pct = (self.ui_zoom.get_untracked() * 100.0).round() as i32;
        let next_pct = if dir == 0 { 100 } else { (pct + dir.signum()).clamp(ZOOM_MIN, ZOOM_MAX) };
        if next_pct == pct {
            return;
        }
        let next = next_pct as f32 / 100.0;
        self.ui_zoom.set(next);
        client::post_bg(
            "/state/rox-zoom".to_owned(),
            serde_json::json!({ "zoom": next_pct as f64 / 100.0 }),
        );
    }

    /// Adds a memory to bot `id` ("" `memory_id`) or rewrites one
    /// (`POST /bot/memory`); the list comes back from the server.
    pub fn save_memory(&'static self, id: &str, memory_id: &str, text: &str) {
        let body = if memory_id.is_empty() {
            serde_json::json!({ "id": id, "op": "add", "text": text })
        } else {
            serde_json::json!({ "id": id, "op": "update", "memoryId": memory_id, "text": text })
        };
        let id = id.to_owned();
        Owner::detached().run(|| {
            spawn(async move {
                let Some(r) = client::post("/bot/memory", body).await else { return };
                let list: Vec<BotMemory> =
                    self::list(&r, "memories").iter().map(|m| BotMemory { id: s(m, "id"), text: s(m, "text") }).collect();
                self.bots.update(|bots| {
                    if let Some(b) = bots.iter_mut().find(|b| b.id == id) {
                        b.memories = list;
                    }
                });
            })
        });
    }

    /// Forgets memory `memory_id` of bot `id` (`POST /bot/memory`).
    pub fn forget_memory(&'static self, id: &str, memory_id: &str) {
        let (id, memory_id) = (id.to_owned(), memory_id.to_owned());
        self.bots.update(|bots| {
            if let Some(b) = bots.iter_mut().find(|b| b.id == id) {
                b.memories.retain(|m| m.id != memory_id);
            }
        });
        client::post_bg(
            "/bot/memory".to_owned(),
            serde_json::json!({ "id": id, "op": "remove", "memoryId": memory_id }),
        );
    }

    /// Adds integration `integration` to bot `id`, or removes it
    /// (`POST /bot/integration`); the list comes back from the server.
    pub fn bot_integration(&'static self, id: &str, integration: &str, add: bool) {
        let (id, integration) = (id.to_owned(), integration.to_owned());
        Owner::detached().run(|| {
            spawn(async move {
                let op = if add { "add" } else { "remove" };
                let r = client::post(
                    "/bot/integration",
                    serde_json::json!({ "id": id, "op": op, "integration": integration }),
                )
                .await;
                let Some(r) = r else { return };
                let list: Vec<BotIntegration> = self::list(&r, "integrations")
                    .iter()
                    .map(BotIntegration::of)
                    .collect();
                self.bots.update(|bots| {
                    if let Some(b) = bots.iter_mut().find(|b| b.id == id) {
                        b.integrations = list;
                    }
                });
            })
        });
    }

    /// Makes bot `id` the primary Bot (one at a time).
    pub fn set_main_bot(&'static self, id: &str) {
        self.main_bot.set(id.to_owned());
        client::post_bg("/state/rox-main-bot".to_owned(), serde_json::json!({ "id": id }));
    }

    /// Whether bot `id` is the primary Bot (tracked).
    pub fn is_main_bot(&'static self, id: &str) -> bool {
        !id.is_empty() && self.main_bot.with(|m| m == id)
    }

    /// `GET /groups` — the sidebar's groups; a group's fold state is the
    /// server's (seeded into `collapsed` under its id).
    pub fn fetch_groups(&'static self) {
        spawn(async move {
            let Some(v) = client::get("/groups").await else { return };
            let j = v.get("groups").map(|x| x.to_string()).unwrap_or_default();
            if j == *self.groups_json.borrow() {
                return;
            }
            *self.groups_json.borrow_mut() = j;
            let groups: Vec<RoxGroup> = list(&v, "groups").iter().map(RoxGroup::from_json).collect();
            let folds: Vec<(String, bool)> = groups.iter().map(|g| (g.id.clone(), g.collapsed)).collect();
            if folds.iter().any(|(k, c)| self.collapsed.with_untracked(|m| m.get(k).copied().unwrap_or(false)) != *c) {
                self.collapsed.update(|m| m.extend(folds));
            }
            self.groups.set(groups);
        });
    }

    /// A `POST /group/*` call, then the groups again.
    fn group_post(&'static self, path: &'static str, body: serde_json::Value) {
        // Detached: these run from menu and prompt handlers that close
        // their own layer, and a task under that owner dies before the
        // post is sent (client::post_bg).
        Owner::detached().run(|| {
            spawn(async move {
                let _ = client::post(path, body).await;
                // refused or not, the server's answer replaces any optimistic edit
                self.groups_json.borrow_mut().clear();
                self.fetch_groups();
            });
        });
    }

    /// Add to Group › (a row's check mark): puts item `kind`/`id` in group
    /// `gid` (an artifact with its origin chat `thread`, an app with its
    /// title there), or takes it out — of that group only.
    pub fn group_item(&'static self, gid: &str, kind: &str, id: &str, thread: &str, add: bool) {
        // a v1 app's pin is Rox's too: {plugin, app, ticket?, placement}
        if kind == "app" && crate::mini_v1::is_v1(id) {
            // the app server's groups take v0 apps only: Rox keeps it
            return self.v1_pinned(id, thread, add);
        }
        let mut body = serde_json::json!({
            "id": gid, "kind": kind, "ref": id, "op": if add { "add" } else { "remove" },
        });
        if !thread.is_empty() {
            body[if kind == "app" { "title" } else { "thread" }] = thread.into();
        }
        if !add {
            let (gid, kind, id) = (gid.to_owned(), kind.to_owned(), id.to_owned());
            self.groups.update(|gs| {
                for g in gs.iter_mut().filter(|g| g.id == gid) {
                    g.items.retain(|it| !(it.kind == kind && it.id == id));
                }
            });
        }
        self.group_post("/group/item", body);
    }

    /// A drag dropped on group `gid`, or Move to Group ›: puts item
    /// `kind`/`id` at `index` of the group's items counted without it (the
    /// end without one) — moving it if the group holds it already — and,
    /// for a move, takes it out of group `from`. The groups change here
    /// first; the server's answer replaces them.
    pub fn group_place(
        &'static self,
        gid: &str,
        kind: &str,
        id: &str,
        thread: &str,
        index: Option<usize>,
        from: Option<&str>,
    ) {
        let from = from.filter(|f| *f != gid).map(str::to_owned);
        self.groups.update(|gs| {
            let same = |it: &GroupItem| it.kind == kind && it.id == id;
            // an artifact keeps the record a group already has for it
            let item = gs
                .iter()
                .flat_map(|g| g.items.iter())
                .find(|it| same(it))
                .cloned()
                .unwrap_or_else(|| GroupItem {
                    kind: kind.to_owned(),
                    id: id.to_owned(),
                    thread: thread.to_owned(),
                    artifact: None,
                    title: if kind == "app" { thread.to_owned() } else { String::new() },
                });
            if let Some(g) = gs.iter_mut().find(|g| g.id == gid) {
                g.items.retain(|it| !same(it));
                let at = index.unwrap_or(g.items.len()).min(g.items.len());
                g.items.insert(at, item);
            }
            if let Some(from) = &from {
                for g in gs.iter_mut().filter(|g| g.id == *from) {
                    g.items.retain(|it| !same(it));
                }
            }
        });
        let mut add = serde_json::json!({ "id": gid, "kind": kind, "ref": id, "op": "add" });
        if !thread.is_empty() {
            add[if kind == "app" { "title" } else { "thread" }] = thread.into();
        }
        if let Some(i) = index {
            add["index"] = i.into();
        }
        let remove = from.map(|f| serde_json::json!({ "id": f, "kind": kind, "ref": id, "op": "remove" }));
        spawn_detached(async move {
            // in turn, so the one fetch after them sees both
            let _ = client::post("/group/item", add).await;
            if let Some(remove) = remove {
                let _ = client::post("/group/item", remove).await;
            }
            self.groups_json.borrow_mut().clear();
            self.fetch_groups();
        });
    }

    /// New Group… — `POST /group/new`, with the item the menu was opened
    /// on as its first member (an app's third field is its title).
    pub fn new_group(&'static self, name: &str, item: Option<(String, String, String)>) {
        let mut body = serde_json::json!({ "name": name });
        if let Some((kind, id, thread)) = item {
            let field = if kind == "app" { "title" } else { "thread" };
            body["item"] = serde_json::json!({ "kind": kind, "ref": id, field: thread });
        }
        self.group_post("/group/new", body);
    }

    /// Rename / Color / Collapse — an optimistic patch of group `gid`,
    /// then `POST /group/update`.
    pub fn update_group(&'static self, gid: &str, patch: serde_json::Value) {
        self.groups.update(|gs| {
            for g in gs.iter_mut().filter(|g| g.id == gid) {
                if let Some(v) = patch.get("name").and_then(|x| x.as_str()) {
                    g.name = v.to_owned();
                }
                if let Some(v) = patch.get("color").and_then(|x| x.as_str()) {
                    g.color = v.to_owned();
                }
                if let Some(v) = patch.get("collapsed").and_then(|x| x.as_bool()) {
                    g.collapsed = v;
                }
            }
        });
        let mut body = patch;
        body["id"] = gid.into();
        self.group_post("/group/update", body);
    }

    /// Renames a fixed section ("__un" / "__chats"), kept in `/prefs`.
    pub fn rename_section(&'static self, key: &str, name: &str) {
        let (sig, field) = match key {
            "__un" => (self.unassigned_name, "unassigned_name"),
            "__chats" => (self.chats_name, "chats_name"),
            _ => return,
        };
        sig.set(name.to_owned());
        client::post_bg("/prefs".to_owned(), serde_json::json!({ field: name }));
    }

    /// Delete Group — the group goes, its items stay.
    pub fn delete_group(&'static self, gid: &str) {
        let gid = gid.to_owned();
        self.groups.update(|gs| gs.retain(|g| g.id != gid));
        self.group_post("/group/delete", serde_json::json!({ "id": gid }));
    }

    /// `toggleSec(k)` — fold or unfold a section; a group's fold is kept
    /// on the server.
    pub fn toggle_section(&'static self, key: &str) {
        let now = !self.collapsed.with_untracked(|m| m.get(key).copied().unwrap_or(false));
        self.collapsed.update(|m| {
            m.insert(key.to_owned(), now);
        });
        if self.groups.with_untracked(|gs| gs.iter().any(|g| g.id == key)) {
            self.update_group(key, serde_json::json!({ "collapsed": now }));
        }
    }

    /// Opens a Library item with its kind's handler: an app pops out, an
    /// image card opens in the lightbox, a file in the viewer — a file kept
    /// elsewhere (Google Drive) opens by its link in the browser.
    pub fn open_library_item(&'static self, a: &RoxApp) {
        // a server's app attached here: its link, full screen
        if a.kind == "mcp-app" {
            match mini_link(&a.open) {
                Some(key) => self.open_mini_token(&key, &a.title, &a.token),
                None if !a.open.is_empty() => self.open_link(&a.open),
                None => {}
            }
            return;
        }
        if a.storage.place != "mac" {
            if !a.storage.link.is_empty() {
                self.open_link(&a.storage.link);
            }
            return;
        }
        if a.kind == "app" {
            return self.popout(&a.app);
        }
        if let Some(f) = &a.attach {
            return self.open_file(f);
        }
        match a.kind.as_str() {
            "image" => self.open_lightbox(
                vec![LightItem {
                    kind: "image".to_owned(),
                    uri: format!("{}/asset/generated/assets/{}", client::backend(), a.file),
                    name: a.title.clone(),
                    path: String::new(),
                    poster: String::new(),
                    w: a.w,
                    h: a.h,
                    code: String::new(),
                }],
                0,
            ),
            _ => self.popout(&a.app),
        }
    }

    /// A Library item's message in the open chat.
    pub fn show_library_item(&'static self, app: &str) {
        let a = self.thread_apps.with_untracked(|v| v.iter().find(|a| a.app == app).cloned());
        if let Some(a) = a.filter(|a| a.kind == "mcp-app") {
            return self.show_group_item(&a);
        }
        let i = self.thread_apps.with_untracked(|v| v.iter().find(|a| a.app == app).map(|a| a.i));
        if let Some(i) = i.filter(|i| *i >= 0) {
            self.go_to(&self.active_id.get_untracked(), Some(i));
        }
    }

    /// A Library item's link — `buwiz://chat/<thread uid>?m=<turn id>`, as
    /// the card's Copy Link; a connector's file has its own (the Drive
    /// link).
    pub fn library_link(&'static self, a: &RoxApp) -> String {
        if a.storage.connector() {
            return a.storage.link.clone();
        }
        if a.thread_uid.is_empty() || a.turn_id.is_empty() {
            return self.turn_link(a.i);
        }
        format!("buwiz://chat/{}?m={}", a.thread_uid, a.turn_id)
    }

    /// Download: the save panel for an image card's png or a file, under
    /// its display name — only a file on this Mac has one.
    pub fn save_library_item(&'static self, a: &RoxApp) {
        if a.storage.place != "mac" {
            return;
        }
        let mut body = match &a.attach {
            Some(f) => serde_json::json!({ "path": f.path }),
            None if a.kind == "image" => serde_json::json!({ "asset": a.file }),
            None => return,
        };
        if !a.filename.is_empty() {
            body["name"] = a.filename.clone().into();
        }
        client::post_bg("/file/save".to_owned(), body);
    }

    /// Archive / Unarchive: `POST /artifact/hide` — an archived item
    /// leaves the Library's lists for its Archived chip; the file stays on
    /// disk, its card in the chat and its rows in groups.
    pub fn archive_library_item(&'static self, id: &str, archived: bool) {
        let id = app_name(id).to_owned();
        self.thread_apps.update(|v| {
            for a in v.iter_mut().filter(|a| a.app == id) {
                a.archived = archived;
            }
        });
        spawn_detached(async move {
            let _ = client::post("/artifact/hide", serde_json::json!({ "id": id, "hidden": archived })).await;
            self.refresh_thread_apps();
        });
    }

    /// Detach from Chat: `POST /mcp/detach` — the attached server app
    /// leaves this chat's Library, and its tools here lose its scope. Its
    /// Library id is `attach:<attachment id>`.
    pub fn detach_mcp(&'static self, id: &str) {
        let id = id.to_owned();
        self.thread_apps.update(|v| v.retain(|a| a.app != id));
        let aid = id.strip_prefix("attach:").unwrap_or(&id).to_owned();
        spawn_detached(async move {
            let v = client::post_any("/mcp/detach", serde_json::json!({ "id": aid })).await;
            if !v.as_ref().is_some_and(|v| self::b(v, "ok")) {
                crate::toast::show("Couldn't detach it".to_owned());
            }
            self.refresh_thread_apps();
        });
    }

    /// Whether the open chat's artifact `id` is archived.
    pub fn is_archived(&'static self, id: &str) -> bool {
        let id = app_name(id);
        self.thread_apps.with_untracked(|v| v.iter().any(|a| a.app == id && a.archived))
    }

    /// Delete a file a reply made (`file:<path>`): `POST /artifact/delete`
    /// — its cards leave every chat, the file the disk, and it leaves every
    /// group. (Mini apps and image cards go by `app-delete`.)
    pub fn delete_library_file(&'static self, id: &str) {
        let id = id.to_owned();
        self.thread_apps.update(|v| v.retain(|a| a.app != id));
        spawn_detached(async move {
            let _ = client::post("/artifact/delete", serde_json::json!({ "id": id })).await;
            self.refresh_thread_apps();
            self.refresh_turns();
        });
    }

    /// The Versions picker of a Library row, opened at (`x`, `y`).
    pub fn library_versions(&'static self, id: &str, x: f32, y: f32) {
        if let Some(a) = self.thread_apps.with_untracked(|v| v.iter().find(|a| a.app == id).cloned()) {
            self.vers_pop.set(Some(VersPop { app: a.app, x, y, cur: a.v, versions: a.versions }));
        }
    }

    /// A group's artifact in its own chat, at the message that made it.
    pub fn show_group_item(&'static self, a: &RoxApp) {
        // an attached server app: each chat it's attached in has its card
        if a.kind == "mcp-app" && !a.token.is_empty() {
            let here = (!a.thread_uid.is_empty()).then(|| (a.thread_uid.clone(), a.turn_id.clone()));
            return crate::slash::show_in_chats(a.token.clone(), here);
        }
        if a.turn_id.is_empty() {
            self.go_to(&a.thread_uid, (a.i >= 0).then_some(a.i));
        } else {
            self.go_to_key(&a.thread_uid, Some(a.turn_id.clone()));
        }
    }

    /// Keeps each chat's composer its own (Grok Bot's per-agent drafts):
    /// on a switch, the chat left keeps what the composer held and the
    /// chat opened gets its own back; the open chat's draft is saved as
    /// it changes (debounced).
    fn track_drafts(&'static self) {
        self.owner.run(|| {
            let prev = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
            effect(move || {
                let id = self.active_id.get();
                let from = prev.replace(id.clone());
                if from == id {
                    return;
                }
                untracked(|| {
                    let current = self.composer_draft();
                    if from.is_empty() {
                        // Typed before any chat was open (a new chat): it
                        // is this chat's draft.
                        if !current.is_empty() {
                            return;
                        }
                    } else {
                        self.keep_draft(&from, current);
                    }
                    let next = self.drafts.borrow().get(&id).cloned().unwrap_or_default();
                    self.draft.set(next.text);
                    self.reply_to.set(next.reply);
                    self.attachments.set(next.attachments);
                });
            });
            let epoch = std::rc::Rc::new(std::cell::Cell::new(0u64));
            effect(move || {
                // Subscribe to all three (read without cloning).
                self.draft.with(|_| ());
                self.reply_to.with(|_| ());
                self.attachments.with(|_| ());
                let id = self.active_id.get_untracked();
                if id.is_empty() {
                    return;
                }
                untracked(|| self.keep_draft(&id, self.composer_draft()));
                let e = epoch.get().wrapping_add(1);
                epoch.set(e);
                let epoch = epoch.clone();
                Owner::detached().run(|| {
                    spawn(async move {
                        timer(std::time::Duration::from_millis(700)).await;
                        if epoch.get() == e {
                            self.save_drafts();
                        }
                    })
                });
            });
        });
    }

    /// What the composer holds now.
    fn composer_draft(&'static self) -> Draft {
        Draft {
            text: self.draft.get_untracked(),
            reply: self.reply_to.get_untracked(),
            attachments: self.attachments.get_untracked(),
        }
    }

    fn keep_draft(&'static self, thread: &str, draft: Draft) {
        let mut drafts = self.drafts.borrow_mut();
        if draft.is_empty() {
            drafts.remove(thread);
        } else {
            drafts.insert(thread.to_owned(), draft);
        }
    }

    /// `/state/rox-drafts`, without the drafts of chats that are gone.
    fn save_drafts(&'static self) {
        let live: std::collections::BTreeSet<String> =
            self.threads.get_untracked().iter().map(|t| t.id.clone()).collect();
        if !live.is_empty() {
            self.drafts.borrow_mut().retain(|k, _| live.contains(k));
        }
        let drafts: serde_json::Map<String, serde_json::Value> = self
            .drafts
            .borrow()
            .iter()
            .map(|(k, d)| (k.clone(), d.to_json()))
            .collect();
        client::post_bg(
            "/state/rox-drafts".to_owned(),
            serde_json::json!({ "drafts": drafts }),
        );
    }

    /// Opens chat `thread` (an agent's session or a plain chat) on turn
    /// `i`, if given: in the open chat the transcript jumps there; any
    /// other chat opens already on the turn — its transcript mounts at it
    /// (`take_jump`), centered among its neighbours — instead of opening
    /// at the end and scrolling back. The turn is banded for a moment.
    pub fn go_to(&'static self, thread: &str, i: Option<i64>) {
        self.go_to_key(thread, i.map(|i| i.to_string()));
    }

    /// [`go_to`] by a turn key: its `id` (a link's UUID) or its index.
    pub fn go_to_key(&'static self, thread: &str, key: Option<String>) {
        // A link names the chat by its uid; the store by its id.
        let known = self.threads.with_untracked(|t| {
            t.iter().find(|t| t.uid == thread || t.id == thread).map(|t| t.id.clone())
        });
        let Some(thread) = known else {
            // A chat this window hasn't listed yet (made elsewhere a moment
            // ago): list the chats again, then go — an unknown uid taken as
            // the id would leave the transcript on the old chat for good.
            let (thread, key) = (thread.to_owned(), key.clone());
            spawn_detached(async move {
                if let Some(v) = client::get("/threads").await {
                    self.set_threads(v);
                }
                if self.threads.with_untracked(|t| t.iter().any(|t| t.uid == thread || t.id == thread)) {
                    self.go_to_key(&thread, key);
                }
            });
            return;
        };
        let thread = thread.as_str();
        self.nav_epoch.set(self.nav_epoch.get().wrapping_add(1));
        self.tab.set("chat".to_owned());
        let open = self.turns_of.get_untracked() == thread
            && self.active_id.get_untracked() == thread
            && !self.turns.with_untracked(|t| t.is_empty());
        if open {
            if let Some(key) = key {
                let hit = self.turns.with_untracked(|t| {
                    t.iter().position(|t| turn_is(t, &key)).map(|p| (p, t[p].i))
                });
                let list = *self.chat_list.borrow();
                if let (Some((pos, i)), Some(list)) = (hit, list) {
                    list.scroll_to_index(pos as u32, percentage(0.5), px(0.0), false);
                    self.flash(i);
                    // Once more as the rows around it finish measuring,
                    // unless something else navigated meanwhile.
                    let epoch = self.nav_epoch.get();
                    Owner::detached().run(|| {
                        spawn(async move {
                            timer(std::time::Duration::from_millis(250)).await;
                            if self.nav_epoch.get() == epoch {
                                list.scroll_to_index(pos as u32, percentage(0.5), px(0.0), false);
                            }
                        })
                    });
                }
            }
            return;
        }
        *self.jump.borrow_mut() = key.map(|k| (thread.to_owned(), k));
        self.open_thread(thread.to_owned());
    }

    /// The pending jump if it is for the open chat and its turn is
    /// loaded: (row, turn i). Taken once, by the transcript as it mounts.
    pub fn take_jump(&'static self) -> Option<(usize, i64)> {
        let (thread, key) = self.jump.borrow().clone()?;
        if self.turns_of.get_untracked() != thread {
            return None;
        }
        let (pos, i) = self.turns.with_untracked(|t| {
            t.iter().position(|t| turn_is(t, &key)).map(|p| (p, t[p].i))
        })?;
        self.jump.borrow_mut().take();
        Some((pos, i))
    }

    /// Rings turn `i` (Grok Bot's `data-highlighted` row): 1.3s, then a
    /// 0.4s fade.
    pub fn flash(&'static self, i: i64) {
        // a link's message is where the keys start from next
        self.set_sel(i, false);
        self.highlight.set(Some(i));
        self.highlight_on.set(true);
        Owner::detached().run(|| {
            spawn(async move {
                timer(std::time::Duration::from_millis(1300)).await;
                if self.highlight.get_untracked() != Some(i) {
                    return;
                }
                self.highlight_on.set(false);
                timer(std::time::Duration::from_millis(450)).await;
                if self.highlight.get_untracked() == Some(i) {
                    self.highlight.set(None);
                }
            })
        });
    }

    /// A link to turn `i` of the open chat, by permanent ids:
    /// `buwiz://chat/<thread uid>?m=<turn id>` (the old `t<n>` / index when
    /// a server has none yet).
    pub fn turn_link(&'static self, i: i64) -> String {
        let id = self.active_id.get_untracked();
        let thread = self
            .threads
            .with_untracked(|t| t.iter().find(|t| t.id == id).map(|t| t.uid.clone()))
            .filter(|u| !u.is_empty())
            .unwrap_or(id);
        let turn = self
            .turns
            .with_untracked(|t| t.iter().find(|t| t.i == i).map(|t| t.id.clone()))
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| i.to_string());
        format!("buwiz://chat/{thread}?m={turn}")
    }

    /// A link pressed in Rox: `buwiz://chat/<thread>[?m=<i>]` goes to that
    /// chat (and turn); an outside URL opens outside Rox as
    /// [`crate::links::outside`] decides.
    pub fn open_link(&'static self, url: &str) {
        self.route_link(url, false);
    }

    /// A `buwiz://` link another app or a web page opened (`lib.rs`, a launch
    /// argument): routed like [`open_link`], except that what would make
    /// something (a new chat) or open a mini app waits on the person in Rox.
    pub fn open_os_link(&'static self, url: &str) {
        self.route_link(url, true);
    }

    fn route_link(&'static self, url: &str, os: bool) {
        if !self.booted.get() && url.starts_with("buwiz://") {
            *self.early_link.borrow_mut() = Some((url.to_owned(), os));
            return;
        }
        // a v1 plugin's `buwiz://app/` link (9.4); v0 links go on as before
        if url.starts_with("buwiz://app/") && self.v1_link(url, os) {
            return;
        }
        self.route_link_v0(url, os);
    }

    /// [`route_link`] for everything but v1 plugins' links.
    pub(crate) fn route_link_v0(&'static self, url: &str, os: bool) {
        use crate::links::{AppOpener, LinkAsk, Outside};
        if let Some(key) = mini_link(url) {
            let token = link_token(url);
            let pass = link_pass(url);
            if os && !pass.is_empty() {
                // a link the app server opened for a mini app carries a
                // one-time pass: redeemed, it opens like one from Rox
                spawn(async move {
                    let ok = client::post("/mcp/open/pass", serde_json::json!({ "pass": pass }))
                        .await
                        .is_some_and(|v| v.get("ok").and_then(|b| b.as_bool()) == Some(true));
                    let from = if ok { AppOpener::Rox } else { AppOpener::Outside };
                    self.open_mini_from(from, &key, "", &token);
                });
                return;
            }
            let from = if os { AppOpener::Outside } else { AppOpener::Rox };
            self.open_mini_from(from, &key, "", &token);
            return;
        }
        // `buwiz://view?path=<abs path>&title=<…>`: a file a mini app made (a
        // print preview), in the viewer over the full-screen app. Only a
        // preview in the account's temporary folder (`links::viewable`),
        // however the path is spelled.
        if let Some(q) = url.strip_prefix("buwiz://view").filter(|q| q.is_empty() || q.starts_with(['?', '/'])) {
            let arg = |k: &str| {
                q.trim_start_matches(['/', '?'])
                    .split('&')
                    .find_map(|kv| kv.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
                    .map(client::urldec)
                    .unwrap_or_default()
            };
            let path = arg("path");
            if path.is_empty() {
                return;
            }
            let Some(path) = crate::links::viewable(&path, &crate::links::view_root()) else {
                log::warn!("rox: buwiz://view of a file that is no preview in {} refused", crate::links::view_root().display());
                return crate::toast::show("Buwiz Bot shows only previews from its temporary folder");
            };
            let path = path.to_string_lossy().into_owned();
            let file = path.rsplit('/').next().unwrap_or_default().to_owned();
            let ext = crate::files::ext_of(&file, "");
            let kind = match ext.as_str() {
                "html" | "htm" => "html",
                "pdf" => "pdf",
                "md" | "markdown" => "markdown",
                "json" => "json",
                "png" | "jpg" | "jpeg" | "gif" | "webp" => "image",
                "mp4" | "mov" | "m4v" => "video",
                "txt" | "csv" | "log" | "xml" => "text",
                _ => "file",
            };
            let title = arg("title");
            self.open_file(&Attach {
                kind: kind.to_owned(),
                name: if title.is_empty() { file } else { title },
                path,
                ext,
                ..Default::default()
            });
            return;
        }
        // `buwiz://chat/new?draft=<text>`: a new chat, its composer holding
        // the text (a mini app handing work to the assistant). From another
        // app (a mini app's link comes through the system too), the chat is
        // made only after Start chat in Rox's sheet.
        if let Some(q) = url.strip_prefix("buwiz://chat/new").filter(|q| q.is_empty() || q.starts_with('?') || q == &"/") {
            let draft = q
                .trim_start_matches(['/', '?'])
                .split('&')
                .find_map(|kv| kv.strip_prefix("draft="))
                .map(client::urldec)
                .unwrap_or_default();
            if os {
                return self.link_ask.set(Some(LinkAsk::NewChat(draft)));
            }
            self.mini_full.set(Vec::new());
            Owner::detached().run(|| self.new_thread_with(draft));
            return;
        }
        if let Some(name) = url.strip_prefix("buwiz://plugin/") {
            let name = name.trim_end_matches('/').to_owned();
            self.open_plugins();
            if !name.is_empty() {
                self.push_nav("plugin", &name, "");
            }
            return;
        }
        match parse_chat_link(url) {
            Some((thread, key)) => self.go_to_key(&thread, key),
            None if url.starts_with('#') => {}
            None => match crate::links::outside(url) {
                Outside::Open => drop(open_url(url.to_owned())),
                Outside::Ask => self.link_ask.set(Some(LinkAsk::Open(url.to_owned()))),
                Outside::Refuse => log::warn!("rox: not opening {url}"),
            },
        }
    }

    /// Open (`yes`) or Cancel in the link sheet: the waiting link goes
    /// ahead, or is dropped.
    pub fn answer_link_ask(&'static self, yes: bool) {
        let Some(ask) = self.link_ask.get_untracked() else { return };
        self.link_ask.set(None);
        if !yes {
            // a v1 question hears the no (9.7: declining changes nothing)
            return self.v1_answered(ask, false);
        }
        match ask {
            crate::links::LinkAsk::Open(url) => drop(open_url(url)),
            crate::links::LinkAsk::NewChat(draft) => {
                self.mini_full.set(Vec::new());
                Owner::detached().run(|| self.new_thread_with(draft));
            }
            crate::links::LinkAsk::App(app) => self.open_mini_token(&app.key, &app.title, &app.token),
            v1 => return self.v1_answered(v1, true),
        }
        self.v1_next_ask();
    }

    /// A mini app full screen for `from`: at once when the person opened it
    /// in Rox, else after Open in the link sheet
    /// ([`crate::links::app_opens_unasked`]).
    pub fn open_mini_from(&'static self, from: crate::links::AppOpener, key: &str, title: &str, token: &str) {
        if crate::links::app_opens_unasked(from) {
            return self.open_mini_token(key, title, token);
        }
        let ask = crate::links::AppAsk { key: key.to_owned(), title: title.to_owned(), token: token.to_owned(), from };
        self.link_ask.set(Some(crate::links::LinkAsk::App(ask)));
    }

    fn mini_title(&'static self, app: &str) -> Signal<String> {
        self.mini_source_title(app)
    }

    /// Shows a server's mini app full screen, over the one open (✕ comes
    /// back to it). The module is fetched fresh, so a server's newer build
    /// is what opens, and every open mounts a new app (its number
    /// changes): the app on top opened again reloads in its place, and
    /// drops its inputs' text changes.
    pub fn open_mini_full(&'static self, key: &str, title: &str) {
        // a v1 app's pin opens as the app declares: inline in the chat,
        // mounted at once (Q9), or full screen
        if crate::mini_v1::is_v1(key) {
            return self.open_v1_pin(key, title);
        }
        self.open_mini_token(key, title, "");
    }

    /// [`open_mini_full`] for what a token resolved to: the token rides
    /// along, so the app can be attached to a chat.
    pub fn open_mini_token(&'static self, key: &str, title: &str, token: &str) {
        // a new open drops what ⌘] would have reopened, as a browser's does
        crate::mini::forget_forward();
        self.minis.borrow_mut().remove(key);
        self.mini_err(key).set(String::new());
        if !title.is_empty() {
            self.mini_source_title(key).set(title.to_owned());
        }
        let n = self.mini_seq.get() + 1;
        self.mini_seq.set(n);
        let open = MiniOpen { key: key.to_owned(), title: title.to_owned(), n, token: token.to_owned() };
        self.mini_full.update(|v| {
            if v.last().is_some_and(|top| top.key == open.key) {
                v.pop();
            }
            v.push(open);
        });
        // opened from a link with no title: its token's resolved title
        // names it (the title bar, Pin and Attach), not the app's own name
        if title.is_empty()
            && let Some((server, scope, value)) = crate::slash::token(token)
        {
            spawn_detached(async move {
                let body = serde_json::json!({ "server": server, "name": scope, "args": value, "complete": false });
                let Some(v) = client::post("/mcp/resolve", body).await else { return };
                let t = v.get("title").and_then(|t| t.as_str()).unwrap_or_default().to_owned();
                if v.get("ok").and_then(|x| x.as_bool()) == Some(true) && !t.is_empty() {
                    crate::live::store().mini_full.update(|v| {
                        if let Some(o) = v.iter_mut().find(|o| o.n == n) {
                            o.title = t;
                        }
                    });
                }
            });
        }
    }

    /// ✕ on the full-screen app: back to the one under it.
    pub fn close_mini(&'static self) {
        let mut top = None;
        self.mini_full.update(|v| top = v.pop());
        // ⌘] opens it again (`mini::step`)
        if let Some(top) = top {
            crate::mini::left(top);
        }
    }

    /// ⌘W over the full-screen apps: a v1 app's print preview closes
    /// first, then the file viewer over them, then the top app.
    pub fn mini_close_top(&'static self) {
        if self.v1.preview.with_untracked(Option::is_some) {
            self.v1_close_preview();
        } else if self.viewing.get_untracked().is_some() {
            if crate::office::may_close() {
                self.viewing.set(None);
            }
        } else {
            self.close_mini();
        }
    }
}

/// One full-screen mini app: its module name (`mcp:<server>/<app>[|<props>]`),
/// title, the open's number and the token it was resolved from ("" when
/// a link opened it).
#[derive(Clone, PartialEq, Default)]
pub struct MiniOpen {
    pub key: String,
    pub title: String,
    pub n: u64,
    pub token: String,
}

/// The token a `buwiz://app/` link names (`&token=`), "" when none: the app
/// it opens can be attached from the title bar then.
pub(crate) fn link_token(url: &str) -> String {
    url.split_once('?')
        .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("token=")))
        .map(client::urldec)
        .unwrap_or_default()
}

/// The one-time pass the app server adds to a `buwiz://app/` link it opens
/// for a mini app (`&buwiz_pass=`), "" when none.
pub(crate) fn link_pass(url: &str) -> String {
    url.split_once('?')
        .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("buwiz_pass=")))
        .unwrap_or_default()
        .to_owned()
}

/// `buwiz://app/<server>/<app>[?props=<JSON>]` → the mini app's module name
/// (`mcp:<server>/<app>[|<props>]`).
pub(crate) fn mini_link(url: &str) -> Option<String> {
    let rest = url.strip_prefix("buwiz://app/")?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (server, app) = path.trim_end_matches('/').split_once('/')?;
    if server.is_empty() || app.is_empty() {
        return None;
    }
    let props = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("props="))
        .map(client::urldec)
        .filter(|p| p.trim_start().starts_with('{') && p.trim() != "{}");
    Some(match props {
        Some(p) => format!("mcp:{server}/{app}|{p}"),
        None => format!("mcp:{server}/{app}"),
    })
}

/// `prettyModel` (modelpicker.tsx:38): "gpt-6.1-sol" → "GPT-6.1 Sol",
/// "xai/grok-4.6" → "Grok 4.6"; other providers' routes show as-is.
pub(crate) fn pretty_model(id: &str) -> String {
    if id.is_empty() {
        return "Default".to_owned();
    }
    let (ns, rest) = match id.split_once('/') {
        Some((n, r)) => (n, r),
        None => ("", id),
    };
    if !ns.is_empty() && !ns.eq_ignore_ascii_case("xai") {
        return id.to_owned();
    }
    let title = |w: &str| {
        let mut c = w.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => String::new(),
        }
    };
    let parts: Vec<&str> = rest.split('-').collect();
    if parts.first() == Some(&"gpt") {
        let mut s = format!("GPT-{}", parts.get(1).copied().unwrap_or(""));
        for p in &parts[2.min(parts.len())..] {
            s.push(' ');
            s.push_str(&title(p));
        }
        return s;
    }
    parts.iter().map(|p| title(p)).collect::<Vec<_>>().join(" ")
}

/// Whether chat `thread` may send now: unless it is busy itself (a message
/// on its way or being answered). Other chats' runs don't block it.
pub(crate) fn may_send(busy: &std::collections::BTreeSet<String>, thread: &str) -> bool {
    !busy.contains(thread)
}

/// `spawn`, but owned by no layer: for work started from a menu or a modal
/// that closes right after (a task under that owner dies before it runs).
pub(crate) fn spawn_detached(f: impl std::future::Future<Output = ()> + 'static) {
    Owner::detached().run(|| {
        spawn(f);
    });
}

/// The connected names in order: v1 plugins as `(id, title)` (the title,
/// else the id), installed catalog plugins, then standalone MCP servers
/// (the name, else the id).
fn conn_names(v1: &[(String, String)], inst: &[PluginRow], standalone: &[McpServer]) -> Vec<String> {
    let or = |name: &str, id: &str| if name.is_empty() { id.to_owned() } else { name.to_owned() };
    v1.iter()
        .map(|(id, title)| or(title, id))
        .chain(inst.iter().map(|p| p.name.clone()))
        .chain(standalone.iter().map(|s| or(&s.name, &s.id)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{link_pass, link_token, mini_link};
    use crate::links::{app_key_parts, app_opens_unasked, AppOpener};

    #[test]
    fn connected_counts_v1_plugins_first_then_catalog_then_servers() {
        use super::{conn_names, McpServer, PluginRow};
        let v1 = [("app.buwiz.ebirforms".to_owned(), "BIR".to_owned()), ("app.buwiz.books".to_owned(), String::new())];
        let inst = [PluginRow { name: "github".into(), ..Default::default() }];
        let standalone = [McpServer { id: "cua-driver".into(), ..Default::default() }, McpServer { id: "x".into(), name: "Everything".into(), ..Default::default() }];
        assert_eq!(conn_names(&v1, &inst, &standalone), ["BIR", "app.buwiz.books", "github", "cua-driver", "Everything"]);
        assert!(conn_names(&[], &[], &[]).is_empty());
    }

    #[test]
    fn chats_send_in_parallel_and_only_a_busy_chat_waits() {
        let mut busy = std::collections::BTreeSet::new();
        assert!(super::may_send(&busy, "t1"));
        busy.insert("t1".to_owned());
        assert!(!super::may_send(&busy, "t1"), "its own run is going");
        assert!(super::may_send(&busy, "t2"), "another chat's run does not block");
        busy.insert("t2".to_owned());
        busy.remove("t1");
        assert!(super::may_send(&busy, "t1") && !super::may_send(&busy, "t2"));
    }

    #[test]
    fn a_reply_quotes_a_title_and_never_an_empty_text() {
        use super::{DriveLink, Turn, reply_label};
        let text = Turn { text: "what are the due forms".into(), ..Default::default() };
        assert_eq!(reply_label(&text), "what are the due forms");
        // a card with no text quotes what it links (a v1 app card's title)
        let card = Turn {
            links: vec![DriveLink { provider: "buwiz-app".into(), id: "x".into(), name: "Buwiz Sign-in".into(), ..Default::default() }],
            ..Default::default()
        };
        assert_eq!(reply_label(&card), "Buwiz Sign-in");
        assert_eq!(reply_label(&Turn { text: "  ".into(), ..Default::default() }), "", "no chip then");
        // the attachment message keeps its canonical token: the reply links it
        let t = "tkt_0123456789ABCDEFGHJKMNPQRS";
        let attached = Turn { text: format!("Attached @bir:taxprofile:{t}"), ..Default::default() };
        assert!(reply_label(&attached).contains(t));
    }

    #[test]
    fn a_routine_notice_turn_parses_and_reads_as_one_line() {
        use super::{RoutineNotice, Turn};
        let v = serde_json::json!({
            "i": 7, "id": "n1", "role": "notice", "text": "BIR payments ran · 2 new items",
            "notice": {"kind": "routine", "plugin": "dev.example.bir", "routine": "payments", "name": "BIR payments",
                "thread": "t9", "thread_uid": "uid-t9", "run": "run_1", "seq": 4, "turn": "turn-a", "items": 2}
        });
        let n = Turn::of(&v).notice.expect("a notice");
        assert_eq!(n, RoutineNotice {
            name: "BIR payments".into(), plugin: "dev.example.bir".into(), routine: "payments".into(),
            thread: "uid-t9".into(), turn: "turn-a".into(), items: 2, error: String::new(),
        });
        assert_eq!(n.line(), "BIR payments ran · 2 new items");
        assert_eq!(RoutineNotice { items: 1, ..n.clone() }.line(), "BIR payments ran · 1 new item");
        // a failure before any run: no thread to open, the error shown
        let f = Turn::of(&serde_json::json!({"role": "notice",
            "notice": {"kind": "routine", "name": "BIR payments", "thread": null, "items": 0, "error": "fx_query: down"}}));
        let f = f.notice.expect("a failure notice");
        assert_eq!((f.thread.as_str(), f.line().as_str()), ("", "BIR payments failed: fx_query: down"));
        // an unknown kind, or `notice` on another role, is no routine notice
        assert!(Turn::of(&serde_json::json!({"role": "notice", "notice": {"kind": "other"}})).notice.is_none());
        assert!(Turn::of(&serde_json::json!({"role": "ai", "notice": {"kind": "routine"}})).notice.is_none());
    }

    #[test]
    fn a_routine_threads_item_turn_is_an_instruction_and_its_row_names_the_routine() {
        use super::{RoutineOf, Turn};
        let mark = serde_json::json!({"plugin": "dev.example.bir", "id": "payments", "name": "BIR payments", "run": "run_1"});
        let u = Turn::of(&serde_json::json!({"role": "user", "text": "New item", "routine": mark}));
        assert_eq!(u.instruction_from, "BIR payments");
        assert_eq!(Turn::of(&serde_json::json!({"role": "user", "text": "hi"})).instruction_from, "");
        let row = serde_json::json!({"plugin": "dev.example.bir", "id": "payments", "name": "BIR payments", "thread": "t5"});
        assert_eq!(RoutineOf::of(&row), Some(RoutineOf {
            plugin: "dev.example.bir".into(), id: "payments".into(), instance: String::new(), name: "BIR payments".into(), thread: "t5".into(),
        }));
        assert_eq!(RoutineOf::of(&serde_json::Value::Null), None);
        assert_eq!(RoutineOf::of(&serde_json::json!({"name": "x"})), None);
    }

    #[test]
    fn a_consent_notice_is_a_card_until_answered() {
        use super::Turn;
        let t = Turn::of(&serde_json::json!({"role": "notice", "text": "Enable Payments (Taxpayer B) here? It may change BIR data without asking: bir_record_payment.",
            "notice": {"kind": "routine_consent", "plugin": "dev.example.bir", "routine": "payments", "instance": "ri_abc123", "name": "Payments", "state": "pending"}}));
        assert!(t.notice.is_none(), "no run line");
        let c = t.consent.expect("a consent card");
        assert_eq!((c.instance.as_str(), c.routine.as_str(), c.state.as_str()), ("ri_abc123", "payments", "pending"));
        assert!(c.text.starts_with("Enable Payments (Taxpayer B) here?"));
        assert_eq!(c.answered(None), None, "it still asks");
        assert_eq!(c.answered(Some("enabled")), Some("Enabled"), "the tap shows before the chat reloads");
        let done = super::ConsentNotice { state: "declined".into(), ..c };
        assert_eq!(done.answered(None), Some("Not enabled"));
        assert!(Turn::of(&serde_json::json!({"role": "notice", "notice": {"kind": "routine_consent"}})).consent.is_none(), "no instance, no card");
    }

    #[test]
    fn app_links_name_the_module_with_their_props() {
        assert_eq!(mini_link("buwiz://app/bir/forms").as_deref(), Some("mcp:bir/forms"));
        assert_eq!(mini_link("buwiz://app/bir/forms/").as_deref(), Some("mcp:bir/forms"));
        let props = "%7B%22form%22%3A%221601C%22%2C%22tin%22%3A%22123%2F00000%22%7D";
        let url = format!("buwiz://app/bir/forms?props={props}&token=%40bir%3Ataxprofile%3A123");
        let key = Some(r#"mcp:bir/forms|{"form":"1601C","tin":"123/00000"}"#);
        assert_eq!(mini_link(&url).as_deref(), key);
        assert_eq!(mini_link(&format!("buwiz://app/bir/forms?props={props}")).as_deref(), key);
        // props that are no object, or an empty one, are left off
        assert_eq!(mini_link("buwiz://app/bir/forms?props=%7B%7D").as_deref(), Some("mcp:bir/forms"));
        assert_eq!(mini_link("buwiz://app/bir/forms?props=5").as_deref(), Some("mcp:bir/forms"));
        assert_eq!(mini_link("buwiz://app/bir/forms?props=").as_deref(), Some("mcp:bir/forms"));
        for url in ["buwiz://app/", "buwiz://app/bir", "buwiz://app/bir/", "buwiz://app//forms", "buwiz://chat/new", "https://x/app/bir/forms"] {
            assert_eq!(mini_link(url), None, "{url:?}");
        }
    }

    #[test]
    fn app_links_carry_their_token() {
        assert_eq!(link_token("buwiz://app/bir/forms?props=%7B%7D&token=%40bir%3Ataxprofile%3A123"), "@bir:taxprofile:123");
        assert_eq!(link_token("buwiz://app/bir/forms?token=%40bir%3Ataxprofile%3A123%2F00001"), "@bir:taxprofile:123/00001");
        assert_eq!(link_token("buwiz://app/bir/forms?props=%7B%7D"), "");
        assert_eq!(link_token("buwiz://app/bir/forms"), "");
    }

    #[test]
    fn app_links_carry_the_servers_one_time_pass() {
        let url = "buwiz://app/bir/forms?props=%7B%22form%22%3A%221601C%22%7D&buwiz_pass=AbC-d_e";
        assert_eq!(link_pass(url), "AbC-d_e");
        assert_eq!(link_pass("buwiz://app/bir/forms?buwiz_pass=x1"), "x1");
        assert_eq!(link_pass("buwiz://app/bir/forms?props=%7B%7D"), "");
        assert_eq!(link_pass("buwiz://app/bir/forms"), "");
        // the pass changes neither the module nor the token
        assert_eq!(mini_link(url), mini_link("buwiz://app/bir/forms?props=%7B%22form%22%3A%221601C%22%7D"));
        assert_eq!(link_token(url), "");
    }

    #[test]
    fn an_app_link_routes_to_its_module_and_asks_unless_rox_opened_it() {
        let key = mini_link("buwiz://app/bir/forms?props=%7B%22form%22%3A%221601C%22%7D").expect("an app link");
        assert_eq!(app_key_parts(&key), Some(("bir", "forms")));
        // `route_link`: a link card in Rox opens it; the system's (`os`) asks
        assert!(app_opens_unasked(AppOpener::Rox));
        assert!(!app_opens_unasked(AppOpener::Outside));
    }
}
