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
}

/// A row of `GET /catalog` — an agent (bot) the shell can open.
#[derive(Clone, PartialEq, Default)]
pub struct BotMeta {
    pub id: String,
    pub name: String,
    pub emoji: String,
    pub desc: String,
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

/// An app pinned to a thread (`GET /thread/apps`).
#[derive(Clone, PartialEq, Default)]
pub struct ThreadApp {
    pub app: String,
    pub title: String,
}

/// A key/value pair of `GET /state/:app` for the detail pane.
#[derive(Clone, PartialEq)]
pub struct Kv(pub String, pub String);

/// A context-menu item (the custom card, not a native menu).
#[derive(Clone, PartialEq)]
pub struct CtxItem {
    pub label: String,
    pub icon: String,
    pub danger: bool,
    pub act: String,
}

/// The popover the shell shows on right-click / long-press / "more".
#[derive(Clone, PartialEq)]
pub struct CtxMenu {
    pub x: f32,
    pub y: f32,
    pub items: Vec<CtxItem>,
}

/// The transient "job settled" toast (agent finished / failed).
#[derive(Clone, PartialEq)]
pub struct Settle {
    pub name: String,
    pub ok: bool,
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

fn s(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned()
}
fn i64of(v: &serde_json::Value, k: &str) -> i64 {
    v.get(k).and_then(|x| x.as_i64()).unwrap_or_default()
}
fn f64of(v: &serde_json::Value, k: &str) -> f64 {
    v.get(k).and_then(|x| x.as_f64()).unwrap_or_default()
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
            art: (!s(v, "name").is_empty() && v.get("art").is_some()).then(|| {
                let a = v.get("art").unwrap();
                let name = s(v, "name");
                let ver = i64of(v, "v");
                Art {
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
    pub bots: Signal<Vec<BotMeta>>,
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
    pub run_status: Signal<String>,
    pub settled: Signal<Option<Settle>>,

    /// Models.
    pub models: Signal<Vec<ModelDef>>,
    pub model: Signal<String>,
    pub default_model: Signal<String>,
    pub effort: Signal<String>,
    pub fast: Signal<bool>,

    /// Layout.
    pub side_mode: Signal<String>,
    pub right_open: Signal<bool>,
    pub detail_tab: Signal<String>,
    pub detail: Signal<Vec<Kv>>,
    pub detail_app: Signal<String>,
    pub thread_apps: Signal<Vec<ThreadApp>>,
    pub tab: Signal<String>,

    /// Overlays.
    pub ctx: Signal<Option<CtxMenu>>,
    pub palette: Signal<bool>,
    pub viewing: Signal<Option<FileView>>,
    pub models_open: Signal<bool>,

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
            collapsed: signal(BTreeMap::new()),
            active_id: signal(String::new()),
            turns: signal(Vec::new()),
            revealed: signal(false),
            last_status: signal(String::new()),
            draft: signal(String::new()),
            attachments: signal(Vec::new()),
            reply_to: signal(None),
            at_open: signal(false),
            sending: signal(false),
            run_status: signal(String::new()),
            settled: signal(None),
            models: signal(Vec::new()),
            model: signal(String::new()),
            default_model: signal(String::new()),
            effort: signal(String::new()),
            fast: signal(false),
            side_mode: signal("full".to_owned()),
            right_open: signal(false),
            detail_tab: signal("state".to_owned()),
            detail: signal(Vec::new()),
            detail_app: signal(String::new()),
            thread_apps: signal(Vec::new()),
            tab: signal("chat".to_owned()),
            ctx: signal(None),
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
            if let Some(v) = client::get("/catalog").await {
                self.set_bots(v);
            }
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
    }

    fn set_threads(&self, v: serde_json::Value) {
        let rows: Vec<ThreadMeta> = list(&v, "list")
            .iter()
            .map(|t| ThreadMeta {
                id: s(t, "id"),
                name: s(t, "title"),
                created: f64of(t, "created"),
            })
            .collect();
        // The server names its active thread; open that one on first load.
        let active = s(&v, "active");
        if !active.is_empty() && self.active_id.get_untracked().is_empty() {
            self.active_id.set(active);
        } else if !rows.is_empty() && self.active_id.get_untracked().is_empty() {
            self.active_id.set(rows[0].id.clone());
        }
        self.threads.set(rows);
    }

    fn set_bots(&self, v: serde_json::Value) {
        self.bots.set(
            list(&v, "apps")
                .iter()
                .map(|b| BotMeta {
                    id: s(b, "id"),
                    name: s(b, "name"),
                    emoji: s(b, "emoji"),
                    desc: s(b, "desc"),
                })
                .collect(),
        );
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
        self.turns.set(Vec::new());
        *self.turns_json.borrow_mut() = String::new();
        let sel = id.clone();
        spawn(async move {
            client::post("/thread/select", serde_json::json!({"id": sel})).await;
            self.active_id.set(id.clone());
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
                        .map(|a| ThreadApp { app: s(a, "app"), title: s(a, "title") })
                        .collect(),
                );
            }
        });
    }

    /// `GET /state/:app` for the detail pane.
    pub fn load_detail(&'static self, app: String) {
        self.detail_app.set(app.clone());
        spawn(async move {
            if let Some(v) = client::get(&format!("/state/{app}")).await {
                let mut rows: Vec<Kv> = list(&v, "entries")
                    .iter()
                    .map(|e| Kv(s(e, "key"), s(e, "value")))
                    .collect();
                if rows.is_empty() {
                    if let Some(obj) = v.as_object() {
                        rows = obj
                            .iter()
                            .map(|(k, x)| Kv(k.clone(), x.to_string()))
                            .collect();
                    }
                }
                self.detail.set(rows);
            }
        });
    }

    // -- chat --------------------------------------------------------------

    /// `POST /chat` with the composer contents, then follow the run.
    pub fn send(&'static self) {
        let text = self.draft.get_untracked().trim().to_owned();
        if text.is_empty() || self.sending.get_untracked() {
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
        self.run_status.set("sending".to_owned());
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
            if let Some(v) = client::post("/chat", body).await {
                // The job id is a number in the payload.
                let id = v
                    .get("id")
                    .and_then(|x| x.as_i64().or_else(|| x.as_str().and_then(|s| s.parse().ok())));
                if let Some(id) = id {
                    self.watch_run(id.to_string()).await;
                }
            }
            self.sending.set(false);
            self.run_status.set(String::new());
            self.refresh_turns();
        });
    }

    /// Poll `/status?id=` until the run settles; each tick refreshes turns
    /// so streamed text lands without waiting for the end. The payload is
    /// `{id, seq, phase, detail, done}`.
    async fn watch_run(&'static self, job: String) {
        for _ in 0..600 {
            let Some(v) = client::get(&format!("/status?id={job}")).await else { break };
            let phase = s(&v, "phase");
            let detail = s(&v, "detail");
            let status = if detail.is_empty() { phase } else { detail };
            self.run_status.set(status.clone());
            self.refresh_turns();
            if v.get("done").and_then(|x| x.as_bool()).unwrap_or(false)
                || status == "done"
                || status == "error"
                || status == "failed"
            {
                self.settled.set(Some(Settle {
                    name: s(&v, "name"),
                    ok: status == "done",
                }));
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

    pub fn rename_thread(&'static self, id: String, name: String) {
        client::post_bg(
            "/thread/update".to_owned(),
            serde_json::json!({"id": id, "title": name}),
        );
    }

    pub fn delete_thread(&'static self, id: String) {
        spawn(async move {
            let _ = client::post("/thread/delete", serde_json::json!({"id": id})).await;
            if let Some(v) = client::get("/threads").await {
                self.set_threads(v);
            }
            if self.active_id.get_untracked() == id {
                if let Some(t) = self.threads.get_untracked().first() {
                    self.open_thread(t.id.clone());
                }
            }
        });
    }

    /// `POST /turns/:i/react` — toggle a reaction emoji.
    pub fn react(&'static self, i: i64, emoji: String) {
        client::post_bg("/react".to_owned(), serde_json::json!({"i": i, "reaction": emoji}));
        self.refresh_turns();
    }

    pub fn choose_model(&'static self, id: String) {
        self.model.set(id.clone());
        let def = self
            .models
            .get_untracked()
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.default_effort.clone())
            .unwrap_or_default();
        self.effort.set(def);
        client::post_bg("/prefs".to_owned(), serde_json::json!({"model": id}));
    }

    pub fn choose_effort(&'static self, value: String) {
        self.effort.set(value.clone());
        client::post_bg("/prefs".to_owned(), serde_json::json!({"effort": value}));
    }

    pub fn toggle_fast(&'static self) {
        let next = !self.fast.get_untracked();
        self.fast.set(next);
        client::post_bg("/prefs".to_owned(), serde_json::json!({"fast": next}));
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

    fn mini_title(&'static self, app: &str) -> Signal<String> {
        self.mini_source_title(app)
    }
}

/// `prettyModel` — a display name for a model id.
fn pretty_model(id: &str) -> String {
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
