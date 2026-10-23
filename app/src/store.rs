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

/// What the lightbox / file viewer shows.
#[derive(Clone, PartialEq)]
pub struct FileView {
    pub title: String,
    pub path: String,
    pub body: String,
    pub kind: String,
}

/// The composer reply target.
#[derive(Clone, PartialEq)]
pub struct ReplyTo {
    pub i: i64,
    pub name: String,
    pub text: String,
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
            url: s(v, "url"),
            thumb: s(v, "thumb"),
            ext: s(v, "ext"),
            size: f64of(v, "size"),
            feed: s(v, "feed"),
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
    pub viewing: Signal<Option<FileView>>,
    pub models_open: Signal<bool>,
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

    /// Theme.
    pub dark: Signal<bool>,

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
            avatar_gen_busy: signal(false),
            palette: signal(false),
            viewing: signal(None),
            models_open: signal(false),
            dark: signal(true),
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
            }
        });
        self.watch_shelf();
    }

    fn set_threads(&self, v: serde_json::Value) {
        let rows: Vec<ThreadMeta> = list(&v, "list")
            .iter()
            .map(|t| ThreadMeta {
                id: s(t, "id"),
                name: s(t, "title"),
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
        if text.is_empty() || self.sending.get_untracked() {
            return;
        }
        // `/status:` is a local command — the TSX never posts it.
        if let Some(arg) = text.strip_prefix("/status:") {
            self.draft.set(String::new());
            self.mock_status(arg.trim());
            return;
        }
        let reply_to = self.reply_to.get_untracked();
        let files: Vec<serde_json::Value> = self
            .attachments
            .get_untracked()
            .iter()
            .map(|a| serde_json::json!({"path": a.path, "name": a.name, "mime": a.mime}))
            .collect();
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
        let mut body = patch;
        body["id"] = id.into();
        client::post_bg("/bot/update".to_owned(), body);
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
