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
    pub app: String,
    pub file: String,
    pub title: String,
    pub kind: String,
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
            art: v
                .get("widget")
                .and_then(|w| w.as_bool())
                .unwrap_or(false)
                .then(|| Art {
                app: s(v, "app"),
                file: s(v, "file"),
                title: s(v, "title"),
                kind: s(v, "kind"),
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
        }
    }

    // -- boot --------------------------------------------------------------

    /// The initial loads: threads, bots, models, prefs.
    pub fn boot(&'static self) {
        spawn(async move {
            if let Some(v) = client::get("/threads").await {
                self.set_threads(v);
            }
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
        let rows: Vec<ThreadMeta> = list(&v, "threads")
            .iter()
            .map(|t| ThreadMeta { id: s(t, "id"), name: s(t, "name"), created: f64of(t, "created") })
            .collect();
        if !rows.is_empty() && self.active_id.get_untracked().is_empty() {
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

    /// Open a thread: load its turns and start watching its status.
    pub fn open_thread(&'static self, id: String) {
        if self.active_id.get_untracked() == id {
            return;
        }
        self.active_id.set(id.clone());
        self.turns.set(Vec::new());
        *self.turns_json.borrow_mut() = String::new();
        self.refresh_turns();
        self.refresh_thread_apps();
    }

    /// One `/turns` fetch for the open thread; dedupes on raw JSON so a
    /// quiet poll doesn't re-render.
    pub fn refresh_turns(&'static self) {
        let id = self.active_id.get_untracked();
        if id.is_empty() {
            return;
        }
        spawn(async move {
            let Some(v) = client::get(&format!("/turns?thread={id}")).await else { return };
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
        let thread = self.active_id.get_untracked();
        let reply_to = self.reply_to.get_untracked().map(|r| r.i);
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
            let body = serde_json::json!({
                "thread": thread,
                "text": text,
                "model": self.model.get_untracked(),
                "effort": self.effort.get_untracked(),
                "fast": self.fast.get_untracked(),
                "reply": reply_to,
                "files": files,
            });
            if let Some(v) = client::post("/chat", body).await {
                if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                    self.watch_run(id.to_owned()).await;
                }
            }
            self.sending.set(false);
            self.run_status.set(String::new());
            self.refresh_turns();
        });
    }

    /// Poll `/status?id=` until the run settles; each tick refreshes turns
    /// so streamed text lands without waiting for the end.
    async fn watch_run(&'static self, job: String) {
        for _ in 0..600 {
            let Some(v) = client::get(&format!("/status?id={job}")).await else { break };
            let status = s(&v, "status");
            self.run_status.set(status.clone());
            self.refresh_turns();
            if status == "done" || status == "error" || status == "failed" {
                self.settled.set(Some(Settle {
                    name: s(&v, "name"),
                    ok: status == "done",
                }));
                break;
            }
            timer(std::time::Duration::from_millis(700)).await;
        }
    }

    /// `POST /threads` — a fresh chat.
    pub fn new_thread(&'static self) {
        spawn(async move {
            if let Some(v) = client::post("/threads", serde_json::json!({})).await {
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
        client::post_bg(format!("/threads/{id}"), serde_json::json!({"name": name}));
    }

    pub fn delete_thread(&'static self, id: String) {
        spawn(async move {
            let _ = client::del(&format!("/threads/{id}")).await;
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
