//! The model: every piece of server state the shell shows, as signals in
//! one `Store` anchored by [`crate::live::store`]. Views read the signals;
//! the methods here move the data — fetches spawned on the engine write
//! the results back.

use std::collections::BTreeMap;

use nsh_core::ui::prelude::*;

use crate::client;

// ---------------------------------------------------------------------------
// Server payloads
// ---------------------------------------------------------------------------

/// A row of `GET /threads` — the sidebar's chat list.
#[derive(Clone, PartialEq, Default)]
pub struct ThreadMeta {
    pub id: String,
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

/// `mcpForm` — the connect tab's "Connect app" draft.
#[derive(Clone, PartialEq, Default)]
pub struct McpForm {
    pub name: String,
    pub command: String,
    pub args: String,
    pub env: String,
    pub scope: String,
}

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

/// The reply quote under a turn.
#[derive(Clone, PartialEq, Default)]
pub struct Reply {
    pub name: String,
    pub text: String,
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
}

/// An app pinned to a thread (`GET /thread/apps` — `routes/chat.mjs:410`).
#[derive(Clone, PartialEq, Default)]
pub struct RoxApp {
    pub app: String,
    pub title: String,
    pub file: String,
    /// `mtime` in epoch ms — the Library groups on it (Today / week / Older).
    pub mtime: f64,
    /// The current version pointer; 0 hides the `v<n>` chip.
    pub v: i64,
    /// `published` — the app is in Mini Apps (the menu's Promote/Remove).
    pub published: bool,
    pub versions: Vec<AppVersion>,
}

/// A key/value pair of `GET /state/:app` for the detail pane.
#[derive(Clone, PartialEq)]
pub struct Kv(pub String, pub String);

/// `DetailMeta` — the detail pane's app card (`/catalog` row or a bare
/// `{name,title}` when unpublished): kinds/tags/published/pinned.
#[derive(Clone, PartialEq, Default)]
pub struct AppMeta {
    pub name: String,
    pub title: String,
    pub kinds: Vec<String>,
    pub tags: Vec<String>,
    pub published: bool,
    pub pinned: bool,
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

/// A published mini app — one `GET /catalog` row for the work shelf
/// (`widgets/index.tsx`'s `Item`).
#[derive(Clone, PartialEq, Default)]
pub struct ShelfItem {
    pub name: String,
    pub title: String,
    pub published_at: String,
    pub kinds: Vec<String>,
    pub pinned: bool,
    pub archived: bool,
    pub tags: Vec<String>,
}

/// The composer reply target.
#[derive(Clone, PartialEq)]
pub struct ReplyTo {
    pub i: i64,
    pub name: String,
    pub text: String,
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
    fn of(v: &serde_json::Value) -> Turn {
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
            role: s(v, "role"),
            text: s(v, "text"),
            name: s(v, "name"),
            label: s(v, "label"),
            caption: s(v, "caption"),
            reaction: s(v, "reaction"),
            widget: v.get("widget").and_then(|x| x.as_bool()).unwrap_or(false),
            v: i64of(v, "v"),
            reply: v.get("reply").map(|r| Reply { name: s(r, "name"), text: s(r, "text") }),
            attachments,
            files: files.iter().map(Attach::of).collect(),
            // A turn names a module in `name` ("R15") and describes the
            // artifact in `art`; `v` pins a version (`R15@v3`). The shell's
            // rule: `tt.name` set → app card, else message.
            art: (!s(v, "name").is_empty()).then(|| {
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
    /// The bot catalog's named sections (`/threads`' top-level `sections`).
    pub sections: Signal<Vec<String>>,
    /// `collapsedSecs` — `__hidden`, `__hiddenChats`, `__archived` start
    /// collapsed; named sections start open (absent ⇒ false).
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

    /// Sending / jobs.
    pub sending: Signal<bool>,
    /// `agentRuns` — the live run per thread; a thread's status row and
    /// its bot's mark read their own entry (App.tsx:113).
    pub agent_runs: Signal<BTreeMap<String, AgentRun>>,
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
    pub tab: Signal<String>,
    /// The work shelf's `/catalog` rows (`WidgetShelf`'s `list`).
    pub shelf_list: Signal<Vec<ShelfItem>>,
    /// `q` — the shelf's filter text.
    pub shelf_q: Signal<String>,
    /// `kind` — `all|db|api|keys|mem|arch`.
    pub shelf_kind: Signal<String>,
    /// `tag` — a `#tag` filter, or empty.
    pub shelf_tag: Signal<String>,
    /// `sent` — the card whose "＋ chat" reuse just landed (flashes ✓).
    pub shelf_sent: Signal<String>,
    /// `renaming` — the card being title-edited (its `name`, "" = none).
    pub shelf_renaming: Signal<String>,
    /// `tagging` — the card being tag-edited.
    pub shelf_tagging: Signal<String>,
    /// The rename/tag inputs' drafts.
    pub shelf_rename_text: Signal<String>,
    pub shelf_tag_text: Signal<String>,

    /// Overlays.
    pub ctx: Signal<Option<CtxMenu>>,
    pub palette: Signal<bool>,
    /// `viewing` (fileviewer.tsx) — the file the fullscreen viewer shows.
    pub viewing: Signal<Option<Attach>>,
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
    pub light_diag: Signal<(f64, f64)>,
    /// (the model popover's `open` lives in `models.rs` via `live::keep` —
    /// `models_open` is gone with the old card.)
    /// `sectionPrompt` — the bot id awaiting a new section name, or None.
    pub section_prompt: Signal<Option<String>>,
    /// `sectionName` — the prompt's draft.
    pub section_name: Signal<String>,
    /// `railPop` — the bot hover card's target, or None.
    pub rail_pop: Signal<Option<RailPop>>,
    /// `popSeq` — the measure-response generation: a stale measure (the
    /// pointer already left) must not open the card.
    pub pop_seq: std::cell::Cell<u64>,
    /// `botModal` — the "Create New Bot" sheet is open.
    pub bot_modal: Signal<bool>,
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
    /// `botEditName` — the profile name field's draft.
    pub bot_edit_name: Signal<String>,
    /// The avatar draft picked/generated for the crop editor
    /// (`draft`/`draftCrop`): `{uri,name,w,h}`.
    pub avatar_draft: Signal<Option<AvatarDraft>>,
    pub draft_crop: Signal<Crop>,
    /// `mcpServers` — the connect tab's server rows.
    pub mcp_servers: Signal<Vec<McpServer>>,
    /// `mcpFormOpen`/`mcpForm`/`mcpErr`/`mcpBusy` — the "Connect app" form.
    pub mcp_form_open: Signal<bool>,
    pub mcp_form: Signal<McpForm>,
    pub mcp_err: Signal<String>,
    pub mcp_busy: Signal<bool>,
    /// `versOpen` — the library row's expanded version tree (app name).
    pub vers_open: Signal<String>,
    /// `appRename`/`appRenameText` — the rename prompt (App.tsx:1637).
    pub app_rename: Signal<Option<AppRename>>,
    pub app_rename_text: Signal<String>,
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

    /// The dedupe cache: raw JSON of the last /turns payload, so identical
    /// polls don't dirty the list.
    pub turns_json: std::cell::RefCell<String>,

    /// Compiled mini-app sources, per module name (`R<n>`): the JS the
    /// `rox/mini-app` component loads. Filled lazily by [`mini_source`].
    pub minis: std::cell::RefCell<std::collections::HashMap<String, Signal<String>>>,
    /// Display titles of mini apps (from `/module`), for card headers.
    pub mini_titles: std::cell::RefCell<std::collections::HashMap<String, Signal<String>>>,
    /// Per-module load/compile errors, shown in the card.
    pub mini_errs: std::cell::RefCell<std::collections::HashMap<String, Signal<String>>>,

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
            sections: signal(Vec::new()),
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
            models_pop: signal(None),
            composer_dock_h: signal(96.0),
            composer_right_w: signal(150.0),
            plus_menu: signal(None),
            plus_hover: signal(String::new()),
            attach_hover: signal(String::new()),
            at_items: signal(Vec::new()),
            at_start: signal(-1),
            at_seq: signal(0),
            at_open: signal(false),
            sending: signal(false),
            agent_runs: signal(BTreeMap::new()),
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
            tab: signal("chat".to_owned()),
            shelf_list: signal(Vec::new()),
            shelf_q: signal(String::new()),
            shelf_kind: signal("all".to_owned()),
            shelf_tag: signal(String::new()),
            shelf_sent: signal(String::new()),
            shelf_renaming: signal(String::new()),
            shelf_tagging: signal(String::new()),
            shelf_rename_text: signal(String::new()),
            shelf_tag_text: signal(String::new()),
            ctx: signal(None),
            section_prompt: signal(None),
            section_name: signal(String::new()),
            rail_pop: signal(None),
            pop_seq: std::cell::Cell::new(0),
            bot_modal: signal(false),
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
            avatar_draft: signal(None),
            draft_crop: signal(Crop { s: 1.0, x: 0.0, y: 0.0 }),
            mcp_servers: signal(Vec::new()),
            mcp_form_open: signal(false),
            mcp_form: signal(McpForm::default()),
            mcp_err: signal(String::new()),
            mcp_busy: signal(false),
            vers_open: signal(String::new()),
            app_rename: signal(None),
            app_rename_text: signal(String::new()),
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
            account_open: signal(false),
            avatar_gen_busy: signal(false),
            palette: signal(false),
            viewing: signal(None),
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
            light_diag: signal((0.0, 0.0)),
            // `models_open` removed — see the note on the struct.
            theme_pref: signal("dark".to_owned()),
            chat_scrolling: signal(false),
            frame_clock: signal(0.0),
            frame_started: std::cell::Cell::new(false),
            turns_json: std::cell::RefCell::new(String::new()),
            minis: std::cell::RefCell::new(std::collections::HashMap::new()),
            mini_titles: std::cell::RefCell::new(std::collections::HashMap::new()),
            mini_errs: std::cell::RefCell::new(std::collections::HashMap::new()),
            owner: Owner::current(),
        }
    }

    // -- boot --------------------------------------------------------------

    /// The initial loads: threads, bots, models, prefs, then the active
    /// thread's turns and pinned apps (`set_threads` picks the server's
    /// `active`).
    pub fn boot(&'static self) {
        spawn(async move {
            if let Some(v) = client::get("/threads").await {
                self.set_threads(v);
            }
            self.refresh_turns();
            self.refresh_thread_apps();
            // Bots ride the `/threads` payload (`bots`), not `/catalog`
            // (which is the widget shelf's data).
            if let Some(v) = client::get("/models").await {
                self.set_models(v);
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
            }
        });
        self.watch_shelf();
    }

    /// `startClock` — one shared ~30fps signal every animating canvas
    /// subscribes to (blob.tsx:91). Started lazily on first avatar/fx
    /// mount; a per-canvas `set_interval` would mean one tree commit per
    /// avatar per frame — that was the hover stutter.
    pub fn frame_clock(&'static self) -> Signal<f64> {
        if !self.frame_started.replace(true) {
            let clock = self.frame_clock;
            set_interval(std::time::Duration::from_millis(33), move || {
                clock.update(|c| *c += 0.033);
            });
        }
        self.frame_clock
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
        let rows: Vec<ThreadMeta> = list(&v, "list")
            .iter()
            .map(|t| ThreadMeta {
                id: s(t, "id"),
                name: s(t, "title"),
                count: i64of(t, "count"),
                created: f64of(t, "created"),
                hidden: b(t, "hidden"),
                archived: b(t, "archived"),
                last_app: s(t, "lastApp"),
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
                })
                .collect(),
        );
        self.sections.set(
            list(&v, "sections")
                .iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect(),
        );
        // The server names its active thread; open that one on first load.
        let active = s(&v, "active");
        if !active.is_empty() && self.active_id.get_untracked().is_empty() {
            self.active_id.set(active);
        } else if !rows.is_empty() && self.active_id.get_untracked().is_empty() {
            self.active_id.set(rows[0].id.clone());
        }
        self.threads.set(rows);
        // App.tsx's effect tracks `threads()` too — a refresh that lands
        // new app metadata (a run finishing) refetches the Library.
        self.refresh_thread_apps();
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
            .filter(|b| b.pinned && !b.hidden)
            .collect()
    }

    /// `hiddenBots()` — the collapsed "Hidden Bots" section.
    pub fn hidden_bots(&'static self) -> Vec<RoxBot> {
        self.bots
            .get()
            .into_iter()
            .filter(|b| b.hidden)
            .collect()
    }

    /// `botGroups()` — `sections()`' named groups of unpinned, unhidden
    /// bots, plus "Unassigned" for the rest.
    pub fn bot_groups(&'static self) -> Vec<(String, String, Vec<RoxBot>)> {
        let bots = self.bots.get();
        let vis: Vec<&RoxBot> = bots.iter().filter(|b| !b.hidden && !b.pinned).collect();
        let sections = self.sections.get();
        let known: std::collections::BTreeSet<&str> =
            sections.iter().map(String::as_str).collect();
        let mut groups: Vec<(String, String, Vec<RoxBot>)> = self
            .sections
            .get()
            .iter()
            .map(|n| {
                (
                    n.clone(),
                    n.clone(),
                    vis.iter()
                        .filter(|b| b.section == *n)
                        .map(|b| (*b).clone())
                        .collect::<Vec<_>>(),
                )
            })
            .filter(|g| !g.2.is_empty())
            .collect();
        let un: Vec<RoxBot> = vis
            .iter()
            .filter(|b| b.section.is_empty() || !known.contains(b.section.as_str()))
            .map(|b| (*b).clone())
            .collect();
        if !un.is_empty() {
            groups.push(("__un".to_owned(), "Unassigned".to_owned(), un));
        }
        groups
    }

    /// `chats()` — visible chats: not bot-owned, not hidden, not archived.
    pub fn chats(&'static self) -> Vec<ThreadMeta> {
        self.threads
            .get()
            .into_iter()
            .filter(|t| t.bot.is_none() && !t.hidden && !t.archived)
            .collect()
    }

    /// `hiddenChats()` / `archivedChats()` — the collapsed sections below.
    pub fn hidden_chats(&'static self) -> Vec<ThreadMeta> {
        self.threads
            .get()
            .into_iter()
            .filter(|t| t.bot.is_none() && t.hidden)
            .collect()
    }

    pub fn archived_chats(&'static self) -> Vec<ThreadMeta> {
        self.threads
            .get()
            .into_iter()
            .filter(|t| t.bot.is_none() && t.archived)
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
        self.agent_runs.update(|m| {
            if let Some((state, label)) = v {
                m.insert(thread, AgentRun { state, label, bot });
            } else {
                m.remove(&thread);
            }
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
        if let Some(st) = self.settled.get().as_ref() {
            if st.bot == id {
                return st.state.clone();
            }
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
        // `if (!agent()) emphBot.set("")` — the status row unmounts when
        // switching away from the run's thread.
        self.emph_bot.set(String::new());
        self.turns.set(Vec::new());
        *self.turns_json.borrow_mut() = String::new();
        let sel = id.clone();
        spawn(async move {
            client::post("/thread/select", serde_json::json!({"id": sel})).await;
            self.active_id.set(id.clone());
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

    /// One `/turns` fetch for the open thread; dedupes on raw JSON so a
    /// quiet poll doesn't re-render.
    pub fn refresh_turns(&'static self) {
        let id = self.active_id.get_untracked();
        if id.is_empty() {
            return;
        }
        spawn(async move {
            let Some(v) = client::get("/turns").await else { return };
            // The server may still be on the previous thread for a beat;
            // keep the stale response out of the list.
            if s(&v, "active") != id {
                return;
            }
            let raw = v.to_string();
            if raw == *self.turns_json.borrow() {
                return;
            }
            *self.turns_json.borrow_mut() = raw;
            self.turns.set(list(&v, "turns").iter().map(Turn::of).collect());
            if let Some(b) = v.get("revealed").and_then(|x| x.as_bool()) {
                self.revealed.set(b);
            }
            self.last_status.set(s(&v, "status"));
        });
    }

    pub fn refresh_thread_apps(&'static self) {
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
                self.thread_apps.set(
                    list(&v, "apps")
                        .iter()
                        .map(|a| RoxApp {
                            app: s(a, "name"),
                            title: s(a, "title"),
                            file: s(a, "file"),
                            mtime: f64of(a, "mtime"),
                            v: a.get("v").and_then(|x| x.as_i64()).unwrap_or(0),
                            published: self::b(a, "published"),
                            versions: list(a, "versions")
                                .iter()
                                .map(|vr| AppVersion {
                                    v: vr.get("v").and_then(|x| x.as_i64()).unwrap_or(0),
                                    label: s(vr, "label"),
                                    parent: vr.get("parent").and_then(|x| x.as_i64()),
                                    created_at: f64of(vr, "created_at"),
                                })
                                .collect(),
                        })
                        .collect(),
                );
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
                        pinned: self::b(&it, "pinned"),
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
                if rows.is_empty() {
                    if let Some(obj) = v.get("state").and_then(|o| o.as_object()) {
                        rows = obj
                            .iter()
                            .filter(|(k, _)| k.as_str() != "widget")
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
        for a in self.thread_apps.get().iter() {
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
    pub fn set_version(&'static self, app: String, v: i64) {
        spawn(async move {
            let _ = client::post("/app/version", serde_json::json!({"name": app, "v": v})).await;
            self.refresh_thread_apps();
            self.refresh_threads();
        });
    }

    /// `saveAppRename` — title/file via `POST /app/meta`, a branch label
    /// via `POST /app/version/label` (App.tsx:1654).
    pub fn save_app_rename(&'static self) {
        let Some(r) = self.app_rename.get() else { return };
        let v = self.app_rename_text.get_untracked().trim().to_owned();
        self.app_rename.set(None);
        if r.field == "label" {
            spawn(async move {
                let _ = client::post(
                    "/app/version/label",
                    serde_json::json!({"name": r.name, "v": r.v, "label": v}),
                )
                .await;
                self.refresh_thread_apps();
            });
        } else {
            if v.is_empty() {
                return;
            }
            let body = if r.field == "title" {
                serde_json::json!({"name": r.name, "title": v})
            } else {
                serde_json::json!({"name": r.name, "file": v})
            };
            spawn(async move {
                let _ = client::post("/app/meta", body).await;
                self.refresh_thread_apps();
            });
        }
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
                spawn(async move {
                    let _ = client::post(
                        "/app/delete",
                        serde_json::json!({"name": name, "everywhere": everywhere}),
                    )
                    .await;
                    self.refresh_thread_apps();
                    self.refresh_threads();
                });
            }
            "app-rename-title" => {
                let text = self
                    .thread_apps
                    .get_untracked()
                    .iter()
                    .find(|a| a.app == name)
                    .map(|a| a.title.clone())
                    .unwrap_or_default();
                self.app_rename_text.set(text);
                self.app_rename.set(Some(AppRename {
                    name,
                    field: "title".into(),
                    v: None,
                }));
            }
            "app-rename-file" => {
                let text = self
                    .thread_apps
                    .get_untracked()
                    .iter()
                    .find(|a| a.app == name)
                    .map(|a| a.file.clone())
                    .unwrap_or_default();
                self.app_rename_text.set(text);
                self.app_rename.set(Some(AppRename {
                    name,
                    field: "file".into(),
                    v: None,
                }));
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

    pub fn mcp_verb(&'static self, verb: &str, id: String) {
        let (route, body) = match verb {
            "mcp-enable" => ("/mcp/server/state", serde_json::json!({"id": id, "enabled": true})),
            "mcp-disable" => ("/mcp/server/state", serde_json::json!({"id": id, "enabled": false})),
            "mcp-reconnect" => ("/mcp/servers", serde_json::json!({"id": id})),
            "mcp-delete" => ("/mcp/server/delete", serde_json::json!({"id": id})),
            _ => return,
        };
        spawn(async move {
            let _ = client::post(route, body).await;
            self.fetch_mcp();
        });
    }

    /// `addMcpServer` — `POST /mcp/servers` with the parsed form
    /// (App.tsx:1611).
    pub fn add_mcp_server(&'static self) {
        if self.mcp_busy.get_untracked() {
            return;
        }
        self.mcp_err.set(String::new());
        let f = self.mcp_form.get_untracked();
        if f.name.trim().is_empty() || f.command.trim().is_empty() {
            self.mcp_err.set("name and command are required".into());
            return;
        }
        self.mcp_busy.set(true);
        let env: serde_json::Map<String, serde_json::Value> = f
            .env
            .split(';')
            .filter_map(|p| {
                let (k, v) = p.trim().split_once('=')?;
                let k = k.trim();
                if k.chars().next().map(|c| c.is_ascii_alphabetic() || c == '_').unwrap_or(false)
                    && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    Some((k.to_owned(), serde_json::Value::from(v.to_owned())))
                } else {
                    None
                }
            })
            .collect();
        let bot = if f.scope == "bot" {
            self.active_bot_untracked().map(|b| b.id)
        } else {
            None
        };
        spawn(async move {
            let r = client::post(
                "/mcp/servers",
                serde_json::json!({
                    "name": f.name.trim(), "command": f.command.trim(),
                    "args": f.args.trim(), "env": env, "bot": bot,
                }),
            )
            .await;
            match r {
                None => self.mcp_err.set("server unreachable".into()),
                Some(d) if d.get("ok") == Some(&serde_json::Value::from(false)) => {
                    self.mcp_err.set(s(&d, "error"))
                }
                Some(d) if s(&d, "status") == "error" => {
                    self.mcp_err.set(if s(&d, "err").is_empty() {
                        "server failed to start".into()
                    } else {
                        s(&d, "err")
                    })
                }
                Some(_) => {
                    self.mcp_form.set(McpForm::default());
                    self.mcp_form_open.set(false);
                }
            }
            self.fetch_mcp();
            self.mcp_busy.set(false);
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
                        if let Some(d) = self.plugin_details.get_untracked().get(&name) {
                            for m in &d.mcp {
                                if !m.id.is_empty() {
                                    self.fetch_mcp_tools(&m.id);
                                }
                            }
                        }
                    });
                }
                if top.page == "mcp" && !top.id.is_empty() {
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
        match page {
            "plugin" if !name.is_empty() => {
                self.fetch_plugin_detail(name);
                let name = name.to_owned();
                spawn(async move {
                    timer(std::time::Duration::from_millis(60)).await;
                    if let Some(d) = self.plugin_details.get_untracked().get(&name) {
                        for m in &d.mcp {
                            if !m.id.is_empty() {
                                self.fetch_mcp_tools(&m.id);
                            }
                        }
                    }
                });
            }
            "mcp" if !id.is_empty() => self.fetch_mcp_tools(id),
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

    /// `popNav`.
    pub fn pop_nav(&'static self) {
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
        spawn(async move {
            let Some(v) = client::get("/plugins/catalog").await else { return };
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
                })
                .collect();
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
            };
            self.plugin_details.update(|m| {
                m.insert(name, d);
            });
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

    /// `installPlugin` (App.tsx:1407).
    pub fn install_plugin(&'static self, name: &str) {
        if !self.plugin_busy.get_untracked().is_empty() {
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
            let _ = client::post("/skills", j).await;
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
    pub fn plugin_cats(&self) -> Vec<String> {
        let mut cats: Vec<String> = vec!["All".into()];
        for p in self.plugins.get().iter() {
            let c = p.category.clone();
            let c = c
                .chars()
                .next()
                .map(|f| f.to_uppercase().collect::<String>() + &c[1..])
                .unwrap_or(c);
            if !c.is_empty() && !cats.iter().any(|x| x.eq_ignore_ascii_case(&c)) {
                cats.push(c);
            }
        }
        cats
    }

    /// `pluginList` — category + query filters (App.tsx:1451).
    pub fn plugin_list(&self) -> Vec<PluginRow> {
        let q = self.plugin_query.get().trim().to_lowercase();
        let cat = self.plugin_cat.get();
        let mut list = self.plugins.get();
        if cat != "All" {
            list.retain(|p| p.category.eq_ignore_ascii_case(&cat));
        }
        if !q.is_empty() {
            list.retain(|p| {
                format!("{} {} {}", p.name, p.description, p.keywords.join(" "))
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
                    .map(|d| d.meta_mcp.clone())
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

    /// `mockStatus(arg)` — the `/status:` slash command (App.tsx:143):
    /// `/status:demo` cycles every state, `/status:<name>` sets one,
    /// `/status:off|idle|clear` stops. Local only — never hits `/chat`.
    pub fn mock_status(&'static self, arg: &str) {
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
                    let label = crate::statusfx::status_label(name);
                    self.agent_set(
                        thread.clone(),
                        bot.clone(),
                        Some((name.to_string(), format!("{name} — {label}"))),
                    );
                }
                timer(std::time::Duration::from_millis(1700)).await;
                self.agent_set(thread, bot, None);
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
        let text = self.draft.get_untracked().trim().to_owned();
        // `/status:` is a local command — the TSX never posts it.
        if let Some(arg) = text.strip_prefix("/status:") {
            self.draft.set(String::new());
            self.mock_status(arg.trim());
            return;
        }
        let reply_to = self.reply_to.get_untracked();
        // The attachments go over verbatim — the server forwards them to
        // the model (picked files carry `uri`, reply files `path`).
        let files: Vec<serde_json::Value> = self
            .attachments
            .get_untracked()
            .iter()
            .map(|a| {
                serde_json::json!({
                    "uri": a.url, "path": a.path, "name": a.name, "mime": a.mime,
                    "kind": a.kind, "ext": a.ext, "size": a.size,
                    "thumb": a.thumb, "feed": a.feed, "poster": a.poster,
                    "width": a.width, "height": a.height,
                    "duration": a.duration, "animated": a.animated,
                })
            })
            .collect();
        if (text.is_empty() && files.is_empty()) || self.sending.get_untracked() {
            return;
        }
        self.draft.set(String::new());
        self.attachments.set(Vec::new());
        self.reply_to.set(None);
        self.sending.set(true);
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
        spawn(async move {
            // /chat reads {prompt, reply:{i,text,files}, attachments} and
            // runs on the server's active thread — select first so the
            // reply lands on the visible one.
            client::post(
                "/thread/select",
                serde_json::json!({"id": self.active_id.get_untracked()}),
            )
            .await;
            let body = serde_json::json!({
                "prompt": text,
                "reply": reply_to.map(|r| serde_json::json!({"i": r.i, "text": r.text})),
                "attachments": files,
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
            self.sending.set(false);
            // Grok: `if (!agent()) emphBot.set("")` — the row is gone.
            self.emph_bot.set(String::new());
            self.refresh_turns();
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
    pub fn pick_attachments(&'static self) {
        spawn(async move {
            if let Some(v) = client::post("/attach/pick", serde_json::json!({})).await {
                let files: Vec<Attach> = v
                    .get("files")
                    .and_then(|x| x.as_array())
                    .map(|l| l.iter().map(Attach::of).collect())
                    .unwrap_or_default();
                if !files.is_empty() {
                    self.attachments.update(|a| a.extend(files));
                }
            }
        });
    }

    /// Poll `/status?id=` until the run settles; each tick refreshes turns
    /// so streamed text lands without waiting for the end. The payload is
    /// `{id, seq, phase, detail, done}`. `thread`/`bot` are the run's
    /// `agentRuns` key and credit.
    async fn watch_run(&'static self, job: String, thread: String, bot: Option<String>) {
        for _ in 0..600 {
            let Some(v) = client::get(&format!("/status?id={job}")).await else { break };
            let phase = s(&v, "phase");
            let detail = s(&v, "detail");
            let status = if detail.is_empty() { phase.clone() } else { detail };
            // `agentSet(thread, bot, {state, label})` every tick.
            self.agent_set(
                thread.clone(),
                bot.clone(),
                Some((phase_state(&phase).to_owned(), status.clone())),
            );
            self.refresh_turns();
            self.refresh_threads();
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
            timer(std::time::Duration::from_millis(700)).await;
        }
    }

    /// `POST /thread/new` — a fresh chat.
    pub fn new_thread(&'static self) {
        spawn(async move {
            if let Some(v) = client::post("/thread/new", serde_json::json!({})).await {
                if let Some(v) = client::get("/threads").await {
                    self.set_threads(v);
                }
                if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
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
        let mut body = patch;
        body["id"] = id.into();
        client::post_bg("/bot/update".to_owned(), body);
    }

    // ——————————————————— files (fileviewer.tsx / lightbox.tsx) ———————————————————

    /// `raw(f)` — the server's bytes of a reply file.
    pub fn file_raw_url(path: &str) -> String {
        let enc = path
            .split('/')
            .map(|p| crate::client::urlenc(p))
            .collect::<Vec<_>>()
            .join("/");
        format!("{}/file/raw/{enc}", crate::client::GEN)
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
        self.light_diag.set((0.0, 0.0));
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
        self.sheet_idx.set(0);
        self.html_mode.set("preview".to_owned());
        self.sel_a.set(None);
        self.sel_b.set(None);
        let path = f.path.clone();
        spawn(async move {
            let url = format!(
                "{}/file/render?path={}",
                crate::client::GEN,
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

    /// `moveToSection` — group a bot under a named section (creating it)
    /// and clear its pin (App.tsx:822).
    pub fn move_to_section(&'static self, id: &str, name: &str) {
        if !name.is_empty() && !self.sections.get_untracked().iter().any(|s| s == name) {
            self.sections.update(|v| v.push(name.to_owned()));
        }
        self.update_bot(id, serde_json::json!({"section": name, "pinned": false}));
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

    /// `POST /turns/:i/react` — toggle a reaction emoji.
    pub fn react(&'static self, i: i64, emoji: String) {
        client::post_bg("/react".to_owned(), serde_json::json!({"i": i, "reaction": emoji}));
        self.refresh_turns();
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
        let name = app.to_owned();
        spawn(async move {
            let Some(v) = client::get(&format!("/module?name={name}")).await else {
                self.mini_err(&name).set("server unreachable".to_owned());
                return;
            };
            let title = v.get("title").and_then(|x| x.as_str()).unwrap_or_default();
            self.mini_title(&name).set(title.to_owned());
            let src = s(&v, "source");
            if src.is_empty() {
                self.mini_err(&name).set("empty module".to_owned());
                return;
            }
            match crate::mini::compile_widget(&name, &src) {
                Ok(js) => {
                    self.mini_err(&name).set(String::new());
                    sig.set(js);
                }
                Err(e) => {
                    log::warn!("mini {name}: compile failed: {e}");
                    let first = e.lines().next().unwrap_or("compile failed");
                    self.mini_err(&name).set(first.to_owned());
                }
            }
        });
        sig
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
    pub fn popout(&'static self, app: &str) {
        client::post_bg(
            "/popout".to_owned(),
            serde_json::json!({"name": app}),
        );
    }

    /// `POST /popout {name, mode:"desktop"}` — the card's Pin-to-desktop.
    pub fn popout_desktop(&'static self, app: &str) {
        client::post_bg(
            "/popout".to_owned(),
            serde_json::json!({"name": app, "mode": "desktop"}),
        );
    }

    // -- work shelf --------------------------------------------------------

    /// `GET /catalog` → `shelf_list` (`WidgetShelf`'s `load`).
    pub fn load_shelf(&'static self) {
        spawn(async move {
            let Some(v) = client::get("/catalog").await else { return };
            let rows: Vec<ShelfItem> = list(&v, "list")
                .iter()
                .map(|i| ShelfItem {
                    name: s(i, "name"),
                    title: s(i, "title"),
                    published_at: s(i, "publishedAt"),
                    kinds: list(i, "kinds")
                        .iter()
                        .filter_map(|k| k.as_str().map(|x| x.to_owned()))
                        .collect(),
                    pinned: b(i, "pinned"),
                    archived: b(i, "archived"),
                    tags: list(i, "tags")
                        .iter()
                        .filter_map(|t| t.as_str().map(|x| x.to_owned()))
                        .collect(),
                })
                .collect();
            self.shelf_list.set(rows);
        });
    }

    /// The 3s catalog poll, on while the work tab is up (`WidgetShelf`'s
    /// `setInterval(load, 3000)`).
    pub fn watch_shelf(&'static self) {
        spawn(async move {
            loop {
                if self.tab.get_untracked() == "work" {
                    self.load_shelf();
                }
                timer(std::time::Duration::from_millis(3000)).await;
            }
        });
    }

    /// `POST /widget-meta` — rename (`title`), tag (`tags`), archive
    /// (`archived`); the catalog reloads after the write.
    pub fn widget_meta(&'static self, app: &str, patch: serde_json::Value) {
        let mut body = patch;
        body["name"] = serde_json::Value::String(app.to_owned());
        spawn(async move {
            if client::post("/widget-meta", body).await.is_some() {
                self.load_shelf();
            }
        });
    }

    /// `POST /pin {name,pinned}` — Nominate (★) / Unpin on the shelf card.
    pub fn pin_widget(&'static self, app: &str, pinned: bool) {
        let app = app.to_owned();
        spawn(async move {
            if client::post("/pin", serde_json::json!({"name": app, "pinned": pinned}))
                .await
                .is_some()
            {
                self.load_shelf();
            }
        });
    }

    /// `POST /unpublish` — "Delete from Mini Apps".
    pub fn unpublish(&'static self, app: &str) {
        let app = app.to_owned();
        spawn(async move {
            if client::post("/unpublish", serde_json::json!({"name": app}))
                .await
                .is_some()
            {
                self.load_shelf();
            }
        });
    }

    /// `POST /retry {name}` — Regenerate: re-prompt the model against the
    /// same request and patch the published file.
    pub fn retry_widget(&'static self, app: &str) {
        client::post_bg("/retry".to_owned(), serde_json::json!({"name": app}));
    }

    /// `POST /reuse` — "＋ chat": drops the app into a turn; the card's
    /// label flashes `＋ chat ✓` for 1.5s.
    pub fn reuse_widget(&'static self, app: &str) {
        let app = app.to_owned();
        spawn(async move {
            if client::post("/reuse", serde_json::json!({"name": app}))
                .await
                .is_some()
            {
                self.shelf_sent.set(app.to_owned());
                timer(std::time::Duration::from_millis(1500)).await;
                if self.shelf_sent.get_untracked() == app {
                    self.shelf_sent.set(String::new());
                }
            }
        });
    }

    /// The shelf's visible rows — `shown()`: text/tag match plus the
    /// kind filter (archived rows hide unless `arch` is selected).
    pub fn shelf_shown(&'static self) -> Vec<ShelfItem> {
        let q = self.shelf_q.get().to_lowercase();
        let kind = self.shelf_kind.get();
        let tag = self.shelf_tag.get();
        self.shelf_list
            .get()
            .into_iter()
            .filter(|i| {
                let hay =
                    format!("{} {} {}", i.title, i.name, i.tags.join(" ")).to_lowercase();
                let m = (tag.is_empty() || i.tags.contains(&tag))
                    && (q.is_empty() || hay.contains(&q));
                if kind == "arch" {
                    i.archived && m
                } else {
                    !i.archived && (kind == "all" || i.kinds.contains(&kind)) && m
                }
            })
            .collect()
    }

    /// `allTags` — every tag across the catalog, deduped.
    pub fn shelf_tags(&'static self) -> Vec<String> {
        let mut t: Vec<String> = self
            .shelf_list
            .get()
            .iter()
            .flat_map(|i| i.tags.clone())
            .collect();
        t.sort();
        t.dedup();
        t
    }

    fn mini_title(&'static self, app: &str) -> Signal<String> {
        self.mini_source_title(app)
    }
}

/// `prettyModel` — a display name for a model id.
pub(crate) fn pretty_model(id: &str) -> String {
    if id.is_empty() {
        return "Default".to_owned();
    }
    if !id.contains('/') && !id.starts_with("grok") && !id.starts_with("x") {
        return id.to_owned();
    }
    if let Some(rest) = id.strip_prefix("gpt-") {
        return format!("GPT-{rest}");
    }
    id.split('-')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
