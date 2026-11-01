//! The person-facing side of v1 plugins in Rox: what the store keeps for
//! them ([`V1`], `Store::v1`), the consent sheet's v1 questions (9.7), the
//! chat seam attachments go through (10.3), `buwiz://app/` links (9.4), the
//! composer's tokens and commands (9.2, 9.3), tool-result and inline cards
//! (6.5, 9.6), chips, the running app's banners and errors (6.7, 12.2,
//! 13.7), and Connect Apps' actions (13). The decisions are in the pure
//! modules beside this one; this file only acts on them.
//!
//! Work that waits on the broker runs off the main thread
//! (`vault::off_main`); signals are only touched on the main thread.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use nsh_core::prelude::*;
use serde_json::{Value, json};

use super::{
    RoxError, V1Key,
    cards::{self, CardAction, CardMeta, OpenCard, Opener, Placement},
    connect, consent,
    host_api::{self, HttpBroker},
    lifecycle, links,
    load::PluginEntry,
    records::{self, Records},
    tokens,
};
use crate::{links::LinkAsk, live::store, store::Store, theme};

/// The chat column an inline card fits in (the reply's width).
pub const COLUMN: f32 = 560.0;

/// What the store keeps for v1 plugins.
pub struct V1 {
    /// The installed v1 plugins as the broker lists them.
    pub entries: Signal<Vec<PluginEntry>>,
    /// The same, as Connect Apps shows them.
    pub listed: Signal<Vec<connect::Listed>>,
    pub loaded: Signal<bool>,
    /// Ticket titles for chips, by canonical token (7.4, cached).
    pub titles: Signal<HashMap<String, String>>,
    asked: RefCell<HashSet<String>>,
    /// Per open (module key): a notice over the app ("no longer
    /// available", "reconnecting").
    pub notice: Signal<HashMap<String, String>>,
    /// Per open: a running app's error (6.7), with Reload and Close.
    pub error: Signal<HashMap<String, String>>,
    /// Full-screen opens a new version left on old source: a Reload
    /// banner (12.2). Inline cards reload instead.
    pub stale: Signal<HashSet<String>>,
    /// Opens whose plugin restarted, by key: their plugin (13.7).
    reconnecting: RefCell<HashMap<String, String>>,
    /// Inline cards shown (their ids).
    pub shown: Signal<HashSet<String>>,
    /// Answer cards that mount at once (first-party, 9.8) the person hid.
    pub hidden: Signal<HashSet<String>>,
    /// Cards refreshed: they read live instead of their snapshot (9.8).
    pub live: Signal<HashSet<String>>,
    /// Per inline open (module key): the card height its `buwiz.fit` set
    /// (clamped); the declared height until then.
    pub fit: Signal<HashMap<String, f32>>,
    /// Per inline open (module key): the number of the open whose app
    /// called `buwiz.ready()`, which ends its card's hold
    /// (`mini_v1::settle`).
    pub ready: Signal<HashMap<String, u64>>,
    /// Per inline open (module key): its footer buttons (`buwiz.actions`).
    pub footer: Signal<HashMap<String, Footer>>,
    /// Per inline open: its header status (`buwiz.meta`), with the number
    /// of the open that set it.
    pub meta: Signal<HashMap<String, (u64, CardMeta)>>,
    /// Inline cards settled (`buwiz.settle`), by module key: the one line
    /// each shows instead of its app, for the rest of this run.
    pub settled: Signal<HashMap<String, String>>,
    /// Rox's records (attachments, pins), as last saved.
    pub records: Signal<Records>,
    /// Connect Apps: the review on screen, what is running, the install
    /// form, the plugin page shown.
    pub review: Signal<Option<connect::Review>>,
    pub busy: Signal<String>,
    pub form: Signal<String>,
    pub page: Signal<String>,
    /// Asks waiting behind the one the sheet shows.
    queue: RefCell<VecDeque<LinkAsk>>,
    /// Which plugin generations were prewarmed (14.6).
    prewarm: RefCell<super::cache::Prewarm>,
    /// The opens running, with the fingerprint each loaded.
    running: RefCell<Vec<(u64, String, String)>>,
    /// The document a `buwiz.preview` shows over the apps (7.10).
    pub preview: Signal<Option<super::bridge::Preview>>,
    /// Account sign-ins waiting on the browser ([`signing_key`]).
    pub signing: Signal<HashSet<String>>,
    /// A refused sign-in's message, by [`signing_key`], under its row.
    pub account_err: Signal<HashMap<String, String>>,
}

/// What a refused sign-in says: `needs_setup` is the setup it lacks.
pub fn signin_error(e: &RoxError) -> String {
    match (e.code.as_str(), e.message.is_empty()) {
        ("needs_setup", true) => "Add your Google client in Setup".to_owned(),
        _ => e.message.clone(),
    }
}

/// `<plugin>|<account>/<label>`: a sign-in in progress.
pub fn signing_key(plugin: &str, account: &str, label: &str) -> String {
    format!("{plugin}|{}", connect::account_key(account, label))
}

impl V1 {
    pub fn new() -> V1 {
        V1 {
            entries: signal(Vec::new()),
            listed: signal(Vec::new()),
            loaded: signal(false),
            titles: signal(HashMap::new()),
            asked: RefCell::new(HashSet::new()),
            notice: signal(HashMap::new()),
            error: signal(HashMap::new()),
            stale: signal(HashSet::new()),
            reconnecting: RefCell::new(HashMap::new()),
            shown: signal(HashSet::new()),
            hidden: signal(HashSet::new()),
            live: signal(HashSet::new()),
            fit: signal(HashMap::new()),
            ready: signal(HashMap::new()),
            footer: signal(HashMap::new()),
            meta: signal(HashMap::new()),
            settled: signal(HashMap::new()),
            records: signal(Records::default()),
            review: signal(None),
            busy: signal(String::new()),
            form: signal(String::new()),
            page: signal(String::new()),
            queue: RefCell::new(VecDeque::new()),
            prewarm: RefCell::new(Default::default()),
            running: RefCell::new(Vec::new()),
            preview: signal(None),
            signing: signal(HashSet::new()),
            account_err: signal(HashMap::new()),
        }
    }

    /// The installed plugin `id`.
    pub fn entry(&self, id: &str) -> Option<PluginEntry> {
        self.entries.with_untracked(|e| e.iter().find(|p| p.id == id).cloned())
    }

    /// The enabled plugin of a short.
    pub fn by_short(&self, short: &str) -> Option<PluginEntry> {
        self.entries.with_untracked(|e| e.iter().find(|p| p.short == short && p.enabled).cloned())
    }

    /// Whether `short` names an installed v1 plugin (its tokens are v1's).
    pub fn is_short(&self, short: &str) -> bool {
        self.entries.with_untracked(|e| e.iter().any(|p| p.short == short))
    }
}

/// An inline open's footer (`buwiz.actions`): its buttons, how a press
/// reaches the app, and the number of the open that set them.
#[derive(Clone)]
pub struct Footer {
    pub owner: u64,
    pub actions: Vec<CardAction>,
    pub press: super::bridge::Press,
}

impl PartialEq for Footer {
    fn eq(&self, other: &Footer) -> bool {
        self.owner == other.owner && self.actions == other.actions && Arc::ptr_eq(&self.press, &other.press)
    }
}

impl Default for V1 {
    fn default() -> Self {
        V1::new()
    }
}

fn broker() -> HttpBroker {
    HttpBroker::configured()
}

/// Rox's data dir (records live there).
fn data_dir() -> std::path::PathBuf {
    crate::vault::data_dir()
}

// ---------------------------------------------------------------------------
// Consent (9.7) and the chat seam (10.3), for the bridge's other threads
// ---------------------------------------------------------------------------

type Answer = Box<dyn FnOnce(bool) + Send>;

static PENDING: Mutex<Vec<(u64, Answer)>> = Mutex::new(Vec::new());
static NEXT: AtomicU64 = AtomicU64::new(1);
/// The open chat, for the bridge's threads.
static ACTIVE: Mutex<String> = Mutex::new(String::new());
/// The plugins listed, for the bridge's threads.
static ENTRIES: Mutex<Vec<PluginEntry>> = Mutex::new(Vec::new());
/// Records are changed under this lock (the bridge and the main thread).
static RECORDS: Mutex<()> = Mutex::new(());

/// Answers ask `id` once (later answers are dropped).
fn answer(id: u64, yes: bool) {
    let f = {
        let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        p.iter().position(|(i, _)| *i == id).map(|at| p.remove(at).1)
    };
    if let Some(f) = f {
        f(yes);
    }
}

/// Changes the records in Rox's data dir under the lock, then shows them.
fn change_records<R>(f: impl FnOnce(&mut Records) -> R) -> R {
    let _held = RECORDS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = data_dir();
    let mut r = Records::load(&dir);
    let out = f(&mut r);
    if let Err(e) = r.save(&dir) {
        log::warn!("mini v1: records not saved: {e}");
    }
    super::to_store(move |st| st.v1.records.set(r));
    out
}

/// The chat a v1 app attaches to: the open one.
struct OpenChat;

#[cfg(target_os = "macos")]
impl super::native::Chat for OpenChat {
    fn has_chat(&self, _: &str) -> bool {
        !ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    fn attach(&self, key: &str, token: &str, title: &str) -> bool {
        let thread = ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Ok(k) = V1Key::parse(key) else { return false };
        let entry = ENTRIES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|p| p.id == k.plugin).cloned();
        let Some(entry) = entry else { return false };
        let (token, title) = (token.to_owned(), title.to_owned());
        let added = change_records(|r| r.attach(&thread, &token, &title, &entry));
        if added {
            // the attachment is a message of the person's in the chat
            // (an id, a reply target, in order for the model)
            super::to_store(move |st| st.v1_attach_message(&thread, &token, &title));
        }
        added
    }
}

impl Store {
    /// At start: the records, the plugins, the consent dialogs and the
    /// chat the bridge uses, the broker's events, the v0 migration.
    pub fn v1_boot(&'static self) {
        self.v1.records.set(Records::load(&data_dir()));
        #[cfg(target_os = "macos")]
        {
            super::native::set_consent(Arc::new(|ask: super::bridge::Consent, answer: Answer| {
                super::to_store(move |st| st.v1_consent(ask, answer));
            }));
            super::native::set_chat(Arc::new(OpenChat));
            super::session::set_event_listener(Arc::new(|v: &Value| {
                let v = v.clone();
                super::to_store(move |st| st.v1_plugin_event(&v));
            }));
            super::session::listen_for_generations();
        }
        // work from the main thread and the bridge reaches the store here
        self.in_store(|| set_interval(std::time::Duration::from_millis(40), move || super::run_store_jobs(self)));
        self.in_store(|| {
            effect(move || {
                let id = self.active_id.get();
                *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = id;
            })
        });
        self.v1_refresh();
    }

    /// Lists the v1 plugins again.
    pub fn v1_refresh(&'static self) {
        Owner::detached().run(|| {
            spawn(async move {
                let got = crate::vault::off_main(|| host_api::plugins(&broker())).await;
                let Some(Ok(list)) = got else {
                    self.v1.loaded.set(true);
                    return;
                };
                let entries: Vec<PluginEntry> = list.iter().filter_map(PluginEntry::from_json).collect();
                *ENTRIES.lock().unwrap_or_else(|e| e.into_inner()) = entries.clone();
                self.v1_prewarm(&list);
                // each plugin's version, so its page can say it was updated
                // (saved only when one is new or changed)
                let seen: Vec<(String, String)> = entries.iter().map(|p| (p.id.clone(), p.version.clone())).collect();
                let now = super::session::now_ms();
                if self.v1.records.with_untracked(|r| r.clone().saw_versions(&seen, now)) {
                    change_records(move |r| r.saw_versions(&seen, now));
                }
                let running: Vec<(String, String)> =
                    self.v1.running.borrow().iter().map(|(_, k, f)| (k.clone(), f.clone())).collect();
                // a full-screen open keeps its source and offers Reload
                // (12.2); an inline card reloads in place, as its Refresh
                // does: an update is the plugin page's news, not the card's
                let full: HashSet<String> = self.mini_full.with_untracked(|f| f.iter().map(|o| o.key.clone()).collect());
                let (stale, reload): (Vec<String>, Vec<String>) =
                    lifecycle::stale(&running, &entries).into_iter().partition(|k| full.contains(k));
                batch(|| {
                    self.v1.listed.set(list.iter().map(connect::Listed::of).collect());
                    self.v1.entries.set(entries.clone());
                    self.v1.loaded.set(true);
                    if !stale.is_empty() {
                        self.v1.stale.update(|s| s.extend(stale));
                    }
                });
                for key in reload {
                    self.v1_reload(&key);
                }
            })
        });
    }

    /// Prewarm (14.6): for each running plugin at a generation not warmed
    /// yet, compile its inline apps (answer cards) into the cache, one at a
    /// time on a background thread; and evict entries of fingerprints the
    /// manifest no longer has.
    fn v1_prewarm(&'static self, list: &[Value]) {
        if !super::cache::enabled() {
            return;
        }
        let mut due = Vec::new();
        for p in list {
            let Some(entry) = PluginEntry::from_json(p) else { continue };
            let current: Vec<(String, String)> = entry
                .apps
                .iter()
                .filter_map(|a| Some((a.get("id")?.as_str()?.to_owned(), a.get("fingerprint")?.as_str()?.to_owned())))
                .collect();
            let root = super::plugins_dir();
            super::cache::evict(&root, &entry.id, &current);
            let running = p.get("state").and_then(Value::as_str) == Some("running");
            if running && self.v1.prewarm.borrow_mut().due(&entry.id, entry.generation.unwrap_or(0)) {
                due.push(entry);
            }
        }
        if due.is_empty() {
            return;
        }
        std::thread::Builder::new()
            .name("rox-mini-prewarm".to_owned())
            .spawn(move || {
                // after start-up's own work, one app at a time
                std::thread::sleep(std::time::Duration::from_secs(2));
                let (b, root) = (broker(), super::plugins_dir());
                for p in due {
                    for (app, _) in super::cache::warm_apps(&p.apps) {
                        let t0 = std::time::Instant::now();
                        match super::load::warm(&b, &p, &app, &root) {
                            Ok(true) => log::info!("mini v1 timing: {}/{app} prewarmed in {} ms", p.id, t0.elapsed().as_millis()),
                            Ok(false) => {}
                            Err(e) => log::info!("mini v1: prewarm of {}/{app}: {e}", p.id),
                        }
                    }
                }
            })
            .ok();
    }

    /// Opens the session of an answer card about to mount (14.6), in the
    /// background, while its module is prepared; the open takes it.
    pub fn v1_preopen(&'static self, plugin: &str, app: &str) {
        let Some(entry) = self.v1.entry(plugin) else { return };
        let Some(perms) = entry.app(app).map(|a| a.get("permissions").cloned()) else { return };
        let (plugin, app, generation) = (plugin.to_owned(), app.to_owned(), entry.generation);
        std::thread::spawn(move || {
            let b = broker();
            if let Err(e) = super::session::preopen(&b, &plugin, &app, perms.as_ref(), generation) {
                log::info!("mini v1: pre-open of {plugin}/{app}: {e}");
            }
            // anything unused for a minute is closed
            std::thread::sleep(super::session::PREOPEN_TTL + std::time::Duration::from_secs(1));
            super::session::sweep_preopened(Arc::new(b));
        });
    }

    /// A broker `plugin` event: the list again; the opens that wait on a
    /// restarted plugin reopen on its new generation (13.7).
    pub fn v1_plugin_event(&'static self, event: &Value) {
        // an event with nothing new (a chatty broker) is not a restart:
        // reloading on it would open the cards' sessions again, each of
        // which another broker may echo as an event
        static LAST: std::sync::Mutex<Option<HashMap<String, String>>> = std::sync::Mutex::new(None);
        if lifecycle::repeated(LAST.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new), event) {
            return;
        }
        let plugin = event.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
        if lifecycle::running_again(event, &plugin) {
            let keys: Vec<String> = {
                let mut r = self.v1.reconnecting.borrow_mut();
                let keys: Vec<String> = r.iter().filter(|(_, p)| **p == plugin).map(|(k, _)| k.clone()).collect();
                r.retain(|_, p| *p != plugin);
                keys
            };
            for key in keys {
                self.v1.notice.update(|n| {
                    n.remove(&key);
                });
                self.reload_mini(&key);
            }
            // a card out of view keeps its source in `minis`: it fetches
            // the plugin's new code now, before it shows again
            let running: Vec<String> = self.v1.running.borrow().iter().map(|(_, k, _)| k.clone()).collect();
            let idle = lifecycle::idle_sources(self.minis.borrow().keys(), &running, &plugin);
            for key in idle {
                self.reload_mini(&key);
            }
        }
        self.v1_refresh();
    }

    /// A question from the bridge (an app's `buwiz.open({url})` or
    /// `buwiz.attach`), asked in the sheet; `answer` hears the person.
    pub fn v1_consent(&'static self, ask: super::bridge::Consent, answer: Answer) {
        let id = NEXT.fetch_add(1, Ordering::SeqCst);
        PENDING.lock().unwrap_or_else(|e| e.into_inner()).push((id, answer));
        let ask = match ask {
            super::bridge::Consent::OpenUrl { url, app } => LinkAsk::V1Url { url, app, id },
            super::bridge::Consent::Attach { title, app } => LinkAsk::V1Attach { title, app, id },
        };
        self.v1_ask(ask);
    }

    /// Shows `ask` in the sheet, or after the one it shows.
    pub fn v1_ask(&'static self, ask: LinkAsk) {
        if self.link_ask.get_untracked().is_some() {
            self.v1.queue.borrow_mut().push_back(ask);
        } else {
            self.link_ask.set(Some(ask));
        }
    }

    /// The sheet's answer to a v1 ask, then the next one waiting.
    pub fn v1_answered(&'static self, ask: LinkAsk, yes: bool) {
        match ask {
            LinkAsk::V1Url { id, .. } | LinkAsk::V1Attach { id, .. } => answer(id, yes),
            LinkAsk::V1App { key, title } if yes => {
                if let Ok(k) = V1Key::parse(&key) {
                    self.open_v1(k, &title, Opener::Link);
                }
            }
            _ => {}
        }
        self.v1_next_ask();
    }

    /// The next question waiting for the sheet, once it is free.
    pub fn v1_next_ask(&'static self) {
        if self.link_ask.get_untracked().is_none()
            && let Some(next) = self.v1.queue.borrow_mut().pop_front()
        {
            self.link_ask.set(Some(next));
        }
    }

    /// A `buwiz://app/` link of a v1 plugin (9.4): true when it was one
    /// (opened, asked about, or refused); false for a v0 link, which goes
    /// the v0 way. From outside Rox (`os`) it always asks (9.7).
    pub fn v1_link(&'static self, url: &str, os: bool) -> bool {
        let quick = self.v1.entries.with_untracked(|e| links::route(url, e));
        if quick == links::Route::NotV1 && !url_names_plugin_id(url) {
            return false;
        }
        let url = url.to_owned();
        Owner::detached().run(|| {
            spawn(async move {
                // the list as it is now: a link may name a plugin just installed
                let fresh = crate::vault::off_main(|| host_api::plugins(&broker())).await;
                let entries: Vec<PluginEntry> = match fresh {
                    Some(Ok(l)) => l.iter().filter_map(PluginEntry::from_json).collect(),
                    _ => self.v1.entries.get_untracked(),
                };
                match links::route(&url, &entries) {
                    links::Route::NotV1 => self.open_link_v0(&url, os),
                    links::Route::Refused(why) => {
                        log::warn!("mini v1: link refused: {why}");
                        crate::toast::show(format!("This link can't be opened: {why}"));
                    }
                    links::Route::NotInstalled(p) => crate::toast::show(links::not_installed(&p)),
                    links::Route::Disabled(title) => crate::toast::show(format!("{title} is disabled")),
                    links::Route::Open(key) => {
                        let title = entries
                            .iter()
                            .find(|p| p.id == key.plugin)
                            .and_then(|p| p.app(&key.app))
                            .and_then(|a| a.get("title"))
                            .and_then(Value::as_str)
                            .unwrap_or(&key.app)
                            .to_owned();
                        match consent::ask(if os { consent::Trigger::OsLink } else { consent::Trigger::AttachmentOrPin }) {
                            consent::Ask::Dialog => self.v1_ask(LinkAsk::V1App { key: key.to_key(), title }),
                            _ => self.open_v1(key, &title, Opener::Link),
                        }
                    }
                }
            })
        });
        true
    }

    /// Opens v1 app `key` full screen for `opener`, with the notice its
    /// ticket needs (INV-06).
    pub fn open_v1(&'static self, key: V1Key, title: &str, opener: Opener) {
        let text = key.to_key();
        self.v1.error.update(|e| {
            e.remove(&text);
        });
        self.v1.stale.update(|s| {
            s.remove(&text);
        });
        self.v1.notice.update(|n| {
            n.remove(&text);
        });
        self.open_mini_token(&text, title, "");
        if key.ticket.is_some() && opener != Opener::User {
            Owner::detached().run(|| {
                spawn(async move {
                    let k = key.clone();
                    let notice = crate::vault::off_main(move || links::ticket_notice(&broker(), &k)).await.flatten();
                    if let Some(n) = notice {
                        self.v1.notice.update(|m| {
                            m.insert(key.to_key(), n.to_owned());
                        });
                    }
                })
            });
        }
    }

    /// Opens `card` as its placement and opener want: full screen, or an
    /// inline card in the chat (mounted at once for the person's own
    /// opens, Q9).
    pub fn open_v1_placed(&'static self, card: OpenCard, opener: Opener) {
        match card.placement {
            Placement::Full => self.open_v1(card.key(), &card.title, opener),
            Placement::Inline => {
                // a card at the end of the chat, as a tool result's is
                let link = links::link_of(&card.key());
                let id = self.add_local_link(crate::store::DriveLink {
                    provider: "buwiz-app".to_owned(),
                    id: link.clone(),
                    name: card.title.clone(),
                    link,
                    action: "show".to_owned(),
                    kind: "app".to_owned(),
                    file_type: "Mini app".to_owned(),
                    card: Some(json!({
                        "plugin": card.plugin, "app": card.app, "title": card.title, "placement": "inline",
                        "ticket": card.ticket, "props": card.props,
                    })),
                    ..Default::default()
                });
                if !id.is_empty() && cards::mounts_at_once(opener) {
                    self.v1.shown.update(|s| {
                        s.insert(id);
                    });
                }
            }
        }
    }

    /// `token` (titled `title`) was attached to chat `thread`: it is sent
    /// as the person's message "Attached <token>" (shown with the ticket's
    /// title), so it has a place in the transcript, can be replied to, and
    /// reaches the model in order with the attachment.
    pub fn v1_attach_message(&'static self, thread: &str, token: &str, title: &str) {
        let token = token.to_owned();
        self.v1.titles.update(|t| {
            t.insert(token.clone(), title.to_owned());
        });
        if self.active_id.get_untracked() != thread {
            return crate::toast::show(format!("Attached {title}"));
        }
        // the records as just saved: the message goes with the attachment
        self.v1.records.set(Records::load(&data_dir()));
        // sent as it is: no resolve, and not held back as a token-only message
        self.v1_resolved.set(true);
        self.send_text(attach_message(&token));
    }

    /// A pin (or a library item) of v1 app `key` was opened (12.3, 9.7: no
    /// consent): its declared placement, an inline app mounted at once in
    /// the open chat (Q9).
    pub fn open_v1_pin(&'static self, key: &str, title: &str) {
        let Ok(k) = V1Key::parse(key) else { return };
        let app = self.v1.entry(&k.plugin).and_then(|p| p.app(&k.app).cloned());
        let placement = app.as_ref().map(cards::declared).unwrap_or(Placement::Full);
        let title = match app.as_ref().and_then(|a| a.get("title")).and_then(Value::as_str) {
            Some(t) if title.is_empty() => t.to_owned(),
            _ => title.to_owned(),
        };
        let placement = if placement == Placement::Inline && self.active_id.get_untracked().is_empty() {
            Placement::Full
        } else {
            placement
        };
        let card = OpenCard { plugin: k.plugin, app: k.app, title, placement, ticket: k.ticket, props: k.props, size: Default::default() };
        self.open_v1_placed(card, Opener::Pin);
    }

    /// An open of `key` started running with `fingerprint` (its number
    /// `n`): kept for 12.2's banner, and `app_changed` logged when the app
    /// changed since it last opened.
    pub fn v1_running(&'static self, n: u64, key: &str, fingerprint: &str) {
        self.v1.running.borrow_mut().push((n, key.to_owned(), fingerprint.to_owned()));
        if let Ok(k) = V1Key::parse(key) {
            let fp = fingerprint.to_owned();
            if let Some(old) = change_records(|r| r.opened(&k.plugin, &k.app, &fp)) {
                log::info!("mini v1: app_changed {}/{} {old} -> {fingerprint}", k.plugin, k.app);
            }
        }
    }

    /// The inline open `key` settled (`buwiz.settle`): its card is `line`
    /// from now on, and its app unmounts with its buttons and status.
    pub fn v1_settle(&'static self, key: &str, line: String) {
        let k = key.to_owned();
        batch(|| {
            self.v1.footer.update(|f| {
                f.remove(&k);
            });
            self.v1.meta.update(|m| {
                m.remove(&k);
            });
            self.v1.settled.update(|s| {
                s.insert(k.clone(), line);
            });
        });
    }

    /// Open `n` of `key` ended: the buttons and status it set go (a newer
    /// open of the same card keeps its own).
    pub fn v1_card_ended(&'static self, key: &str, n: u64) {
        let mine = |owner: u64| owner == n;
        if self.v1.footer.with_untracked(|f| f.get(key).is_some_and(|x| mine(x.owner))) {
            self.v1.footer.update(|f| {
                f.remove(key);
            });
        }
        if self.v1.meta.with_untracked(|m| m.get(key).is_some_and(|x| mine(x.0))) {
            self.v1.meta.update(|m| {
                m.remove(key);
            });
        }
        if self.v1.ready.with_untracked(|r| r.get(key).is_some_and(|x| mine(*x))) {
            self.v1.ready.update(|r| {
                r.remove(key);
            });
        }
    }

    /// Open `n` ended.
    pub fn v1_stopped(&'static self, n: u64) {
        self.v1.running.borrow_mut().retain(|(i, _, _)| *i != n);
    }

    /// The open `key`'s app hit an error (6.7).
    pub fn v1_error(&'static self, key: &str, message: String) {
        let key = key.to_owned();
        self.v1.error.update(|e| {
            e.insert(key, message);
        });
    }

    /// The open's plugin restarted: "Reconnecting" until it runs again.
    pub fn v1_reconnecting(&'static self, key: &str, title: &str) {
        let Ok(k) = V1Key::parse(key) else { return };
        self.v1.reconnecting.borrow_mut().insert(key.to_owned(), k.plugin);
        let (key, text) = (key.to_owned(), lifecycle::reconnecting(title));
        self.v1.notice.update(|n| {
            n.insert(key, text);
        });
    }

    /// Reload on an error or a stale open: the app's current source, a new
    /// session.
    pub fn v1_reload(&'static self, key: &str) {
        let k = key.to_owned();
        batch(|| {
            self.v1.error.update(|e| {
                e.remove(&k);
            });
            self.v1.stale.update(|s| {
                s.remove(&k);
            });
        });
        self.mini_err(key).set(String::new());
        if self.minis.borrow().contains_key(key) {
            self.reload_mini(key);
        } else {
            self.retry_mini(key);
        }
    }

    /// Shows `doc`, a `buwiz.preview` document, over the apps (one at a
    /// time: a newer one replaces it).
    pub fn v1_preview(&'static self, doc: super::bridge::Preview) {
        self.v1.preview.set(Some(doc));
    }

    /// Close (or ⌘W) on the preview: back to the app.
    pub fn v1_close_preview(&'static self) {
        self.v1.preview.set(None);
    }

    /// Close on an error: the full-screen open goes, an inline card hides.
    pub fn v1_close(&'static self, key: &str) {
        let k = key.to_owned();
        self.v1.error.update(|e| {
            e.remove(&k);
        });
        if self.mini_full.with_untracked(|v| v.last().is_some_and(|o| o.key == k)) {
            self.close_mini();
        }
        if let Ok(key) = V1Key::parse(&k) {
            let link = links::link_of(&key);
            self.v1.shown.update(|s| s.retain(|id| !id.ends_with(&link)));
        }
    }

    /// The composer's send for v1 plugins (9.3): true when it took the
    /// message. Typed tokens of v1 plugins are resolved and rewritten to
    /// canonical tokens first; a message of only tokens or one command is
    /// resolved and not sent to the model; anything else is sent as
    /// rewritten. Unresolved tokens stay as typed, with a warning.
    pub fn v1_send(&'static self, text: &str) -> bool {
        let text = text.trim().to_owned();
        let entries = self.v1.entries.get_untracked();
        if entries.is_empty() {
            return false;
        }
        let shorts = |s: &str| entries.iter().any(|p| p.short == s && p.enabled);
        // `@short:scope:value … attach`: the tokens attached to this chat
        if let Some(toks) = attach_words(&text, &|s| shorts(s)) {
            self.draft.set(String::new());
            crate::slash::close();
            self.v1_attach_tokens(toks);
            return true;
        }
        let ours: Vec<bool> = tokens::typed_tokens(&text).iter().filter(|t| shorts(t.short)).map(|t| super::perms::ticket_ok(t.value)).collect();
        let command = tokens::slash(&text).and_then(|c| match tokens::route(&c, &commands(&entries)) {
            tokens::Route::Plugin { plugin, .. } => Some((plugin, c.name.to_owned(), c.args.to_owned())),
            tokens::Route::Ambiguous(shorts) => {
                let forms: Vec<String> = shorts.iter().map(|s| format!("/{s}:{}", c.name)).collect();
                crate::toast::show(format!("/{} is ambiguous: use {}", c.name, forms.join(" or ")));
                Some((String::new(), String::new(), String::new()))
            }
            _ => None,
        });
        if let Some((plugin, name, args)) = command {
            if !plugin.is_empty() {
                self.draft.set(String::new());
                crate::slash::close();
                self.v1_resolve_command(plugin, name, args);
            }
            return true;
        }
        let only = tokens::only_tokens_or_command(&text)
            && self.attachments.get_untracked().is_empty()
            && self.reply_to.get_untracked().is_none();
        // no v1 token, or only canonical ones in a message for the model:
        // nothing to resolve
        if ours.is_empty() || (!only && ours.iter().all(|canonical| *canonical)) {
            return false;
        }
        let thread = self.active_id.get_untracked();
        let attached: Vec<Value> = self.v1.records.with_untracked(|r| r.model_tokens(&thread, &entries));
        // the completion popup closes with the send (a late answer is dropped)
        crate::slash::close();
        Owner::detached().run(|| {
            spawn(async move {
                let entries2 = entries.clone();
                let text2 = text.clone();
                let rewrite = crate::vault::off_main(move || {
                    let plugin_of = |s: &str| entries2.iter().find(|p| p.short == s && p.enabled).map(|p| p.id.clone());
                    let b = broker();
                    let mut resolve = |plugin: &str, scope: &str, value: &str| {
                        let ask = host_api::ResolveAsk {
                            plugin: plugin.to_owned(),
                            kind: "scope",
                            name: scope.to_owned(),
                            value: value.to_owned(),
                            complete: false,
                            context: json!({"attached": attached.iter().filter(|a| a["plugin"] == plugin).collect::<Vec<_>>()}),
                        };
                        match host_api::resolve(&b, &ask).map(|v| tokens::answer(&v)) {
                            Ok(tokens::Answer::Token { ticket, title }) => Ok((ticket, title)),
                            Ok(tokens::Answer::Error(m)) => Err(m),
                            Ok(_) => Err("it did not resolve to a token".to_owned()),
                            Err(e) => Err(e.message),
                        }
                    };
                    tokens::rewrite(&text2, &plugin_of, &mut resolve)
                })
                .await;
                let Some(r) = rewrite else { return };
                for (tok, why) in &r.unresolved {
                    let scope = tokens::typed_tokens(tok).first().map(|t| format!("@{}:{}", t.short, t.scope)).unwrap_or_default();
                    crate::toast::show(format!("{scope} did not resolve: {why}"));
                }
                self.v1.titles.update(|t| {
                    for (tok, title) in &r.tokens {
                        if !title.is_empty() {
                            t.insert(tok.clone(), title.clone());
                        }
                    }
                });
                crate::slash::close();
                // the person typed on meanwhile: their new draft stays
                if self.draft.get_untracked().trim() != text.trim() {
                    return;
                }
                if only {
                    self.draft.set(String::new());
                    for (tok, _) in r.tokens {
                        self.v1_open_token(&tok);
                    }
                    return;
                }
                self.draft.set(r.text);
                // sent from the store's own timer (as a press would be): this
                // task ends now, and the send's run must outlive it
                super::to_store(|st| {
                    st.v1_resolved.set(true);
                    st.send();
                });
            })
        });
        true
    }

    /// Attaches typed or canonical `toks` to the open chat (10.3): each is
    /// resolved to its ticket (`complete: false`), its scope must allow
    /// attaching, and the attachments go out as one message of the
    /// person's ("Attached …").
    fn v1_attach_tokens(&'static self, toks: Vec<String>) {
        let thread = self.active_id.get_untracked();
        if thread.is_empty() {
            return crate::toast::show("Open a chat to attach to");
        }
        let entries = self.v1.entries.get_untracked();
        Owner::detached().run(|| {
            spawn(async move {
                let got = crate::vault::off_main(move || {
                    let b = broker();
                    let mut out = Vec::new();
                    for tok in toks {
                        let Some(t) = tokens::typed_tokens(&tok).into_iter().next() else { continue };
                        let Some(p) = entries.iter().find(|p| p.short == t.short && p.enabled) else { continue };
                        if !p.scope_attaches(t.scope) {
                            out.push(Err(format!("@{}:{} can't be attached", t.short, t.scope)));
                            continue;
                        }
                        let ask = host_api::ResolveAsk {
                            plugin: p.id.clone(),
                            kind: "scope",
                            name: t.scope.to_owned(),
                            value: t.value.to_owned(),
                            complete: false,
                            context: json!({"attached": []}),
                        };
                        out.push(match host_api::resolve(&b, &ask).map(|v| tokens::answer(&v)) {
                            Ok(tokens::Answer::Token { ticket, title }) => {
                                Ok((p.clone(), format!("@{}:{}:{ticket}", t.short, t.scope), title))
                            }
                            Ok(tokens::Answer::Error(m)) => Err(m),
                            Ok(_) => Err(format!("@{}:{} did not resolve to a token", t.short, t.scope)),
                            Err(e) => Err(e.message),
                        });
                    }
                    out
                })
                .await;
                let mut attached = Vec::new();
                for r in got.unwrap_or_default() {
                    match r {
                        Ok((plugin, token, title)) => {
                            let (th, tk, ti) = (thread.clone(), token.clone(), title.clone());
                            if change_records(move |r| r.attach(&th, &tk, &ti, &plugin)) {
                                attached.push((token, title));
                            }
                        }
                        Err(m) => crate::toast::show(m),
                    }
                }
                if attached.is_empty() {
                    return;
                }
                self.v1.titles.update(|t| t.extend(attached.iter().cloned()));
                // the records as just saved: the message goes with them
                self.v1.records.set(Records::load(&data_dir()));
                let message = attach_message(&attached.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join(" "));
                super::to_store(move |st| {
                    st.v1_resolved.set(true);
                    st.send_text(message);
                });
            })
        });
    }

    /// A canonical token sent alone: its scope's resolve (`complete:
    /// false`) opens what it answers (user-typed: no consent, 9.7).
    fn v1_open_token(&'static self, tok: &str) {
        let Some((short, scope, ticket)) = tokens::canonical(tok) else { return };
        let Some(p) = self.v1.by_short(short) else { return };
        self.v1_resolve(p.id, "scope", scope.to_owned(), ticket.to_owned());
    }

    fn v1_resolve_command(&'static self, plugin: String, name: String, args: String) {
        self.v1_resolve(plugin, "command", name, args);
    }

    /// Runs a resolve the person asked for and acts on its answer (9.2):
    /// open (no consent; placement as the app declares), choices in the
    /// popup, a token (opened as its scope's), a command's prompt in the
    /// composer for them to edit and send (never sent here), an error as a
    /// toast.
    fn v1_resolve(&'static self, plugin: String, kind: &'static str, name: String, value: String) {
        let thread = self.active_id.get_untracked();
        let entries = self.v1.entries.get_untracked();
        let attached: Vec<Value> = self.v1.records.with_untracked(|r| r.model_tokens(&thread, &entries));
        Owner::detached().run(|| {
            spawn(async move {
                let ask = host_api::ResolveAsk {
                    plugin: plugin.clone(),
                    kind,
                    name: name.clone(),
                    value: value.clone(),
                    complete: false,
                    context: json!({"attached": attached.into_iter().filter(|a| a["plugin"] == plugin.as_str()).collect::<Vec<_>>()}),
                };
                let got = crate::vault::off_main(move || host_api::resolve(&broker(), &ask)).await;
                let v = match got {
                    Some(Ok(v)) => v,
                    Some(Err(e)) => return crate::toast::show(e.message),
                    None => return,
                };
                match tokens::answer(&v) {
                    tokens::Answer::Open { app, props, ticket, title, placement } => {
                        let entry = self.v1.entry(&plugin);
                        let declared = entry.as_ref().and_then(|p| p.app(&app)).map(cards::declared).unwrap_or(Placement::Full);
                        let placement = placement.as_deref().and_then(Placement::parse).unwrap_or(declared);
                        let title = if title.is_empty() {
                            entry.as_ref().and_then(|p| p.app(&app)).and_then(|a| a.get("title")).and_then(Value::as_str).unwrap_or(&app).to_owned()
                        } else {
                            title
                        };
                        self.open_v1_placed(OpenCard { plugin, app, title, placement, ticket, props, size: Default::default() }, Opener::User);
                    }
                    tokens::Answer::Choices(choices) => {
                        let rows = choices
                            .into_iter()
                            .map(|c| crate::slash::PopRow {
                                pick: crate::slash::Pick::V1Choice(plugin.clone(), kind, name.clone(), c.value.clone()),
                                label: c.label,
                                hint: String::new(),
                                detail: c.detail,
                            })
                            .collect();
                        crate::slash::show_rows(rows);
                    }
                    tokens::Answer::Token { ticket, title } => {
                        // a token sent alone: its scope gives a ticket, not
                        // an open; the plugin's command of the same name
                        // opens it (BIR: /taxprofile), else its title shows
                        let command = self.v1.entry(&plugin).is_some_and(|p| {
                            p.commands.iter().any(|c| c.get("name").and_then(Value::as_str) == Some(name.as_str()))
                        });
                        if kind == "scope" && command {
                            self.v1_resolve(plugin, "command", name, ticket);
                        } else {
                            crate::toast::show(if title.is_empty() { "Resolved".to_owned() } else { title });
                        }
                    }
                    tokens::Answer::Prompt(text) if kind == "command" => {
                        let draft = tokens::prompt_draft(&self.draft.get_untracked(), &text);
                        self.draft.set(draft);
                        crate::composer::focus();
                    }
                    tokens::Answer::Prompt(_) => crate::toast::show("The plugin's answer could not be read."),
                    tokens::Answer::Error(m) => crate::toast::show(m),
                }
            })
        });
    }

    /// A resolve's choice was picked in the popup: its value resolves as
    /// the command or scope asked.
    pub fn v1_pick(&'static self, plugin: String, kind: &'static str, name: String, value: String, label: String) {
        crate::slash::close();
        if kind == "scope" && let Some(p) = self.v1.entry(&plugin) {
            // a token being typed: the value completes it in the draft
            let draft = self.draft.get_untracked();
            let tail = format!("@{}:{name}:", p.short);
            if super::perms::ticket_ok(&value) {
                let token = format!("{tail}{value}");
                self.v1.titles.update(|t| {
                    t.insert(token, label);
                });
            }
            if let Some(at) = draft.rfind(&tail) {
                let end = draft[at..].find(char::is_whitespace).map_or(draft.len(), |e| at + e);
                let next = format!("{}{tail}{value} {}", &draft[..at], &draft[end..]).trim_end().to_owned() + " ";
                self.draft.set(next);
                return;
            }
        }
        self.v1_resolve(plugin, kind, name, value);
    }

    /// The composer's choices for `@short:scope:<value>` being typed
    /// (`complete: true`, 9.3): true when `short` is a v1 plugin's.
    pub fn v1_complete(&'static self, short: &str, scope: &str, value: &str) -> bool {
        let Some(p) = self.v1.by_short(short) else { return false };
        if !p.scopes.iter().any(|s| s.get("name").and_then(Value::as_str) == Some(scope)) {
            crate::slash::show_rows(Vec::new());
            return true;
        }
        let ask = host_api::ResolveAsk {
            plugin: p.id.clone(),
            kind: "scope",
            name: scope.to_owned(),
            value: value.to_owned(),
            complete: true,
            context: json!({"attached": []}),
        };
        let n = crate::slash::next_ask();
        let query = self.draft.get_untracked();
        Owner::detached().run(|| {
            spawn(async move {
                let plugin = ask.plugin.clone();
                let name = ask.name.clone();
                let got = crate::vault::off_main(move || host_api::resolve(&broker(), &ask)).await;
                if self.complete_seq.get() != n {
                    return;
                }
                let rows = match got.and_then(Result::ok).map(|v| tokens::answer(&v)) {
                    Some(tokens::Answer::Choices(c)) => c
                        .into_iter()
                        .map(|c| crate::slash::PopRow {
                            pick: crate::slash::Pick::V1Choice(plugin.clone(), "scope", name.clone(), c.value),
                            label: c.label,
                            hint: String::new(),
                            detail: c.detail,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                crate::slash::show_rows_for(query, rows);
            })
        });
        true
    }

    /// `text` as shown: each canonical token as its ticket's title (a chip
    /// in code style, 9.3 Q5), each unresolved v1 token marked. Tracked:
    /// titles fill in as they arrive.
    pub fn v1_display(&'static self, text: &str) -> String {
        if !text.contains('@') {
            return text.to_owned();
        }
        let toks = tokens::typed_tokens(text);
        if toks.is_empty() {
            return text.to_owned();
        }
        let titles = self.v1.titles.get();
        self.v1.entries.with(|_| ());
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        for t in toks {
            let typed = &text[t.start..t.end];
            out.push_str(&text[at..t.start]);
            at = t.end;
            if super::perms::ticket_ok(t.value) {
                match titles.get(typed) {
                    Some(title) if !title.is_empty() => out.push_str(&format!("`{}`", title.replace('`', "'"))),
                    _ => {
                        self.v1_title(typed);
                        out.push_str(&format!("`@{}:{}`", t.short, t.scope));
                    }
                }
            } else if self.v1.is_short(t.short) {
                out.push_str(&format!("{typed} (unresolved)"));
            } else {
                out.push_str(typed);
            }
        }
        out.push_str(&text[at..]);
        out
    }

    /// `text` for a title (a chat's name, the window): each canonical token
    /// as its ticket's title, or `@short:scope` until that is known, never
    /// the ticket. Tracked: titles fill in as they arrive.
    pub fn v1_plain(&'static self, text: &str) -> String {
        if !text.contains("tkt_") {
            return text.to_owned();
        }
        let titles = self.v1.titles.get();
        // again once the plugins are listed: their titles can be asked then
        self.v1.entries.with(|_| ());
        let (out, missing) = tokens::plain(text, &|t| {
            titles.get(t).cloned().or_else(|| titles.iter().find(|(k, _)| k.starts_with(t)).map(|(_, v)| v.clone()))
        });
        for token in missing {
            self.v1_title(&token);
        }
        out
    }

    /// Asks once for canonical `token`'s title (7.4) and caches it.
    fn v1_title(&'static self, token: &str) {
        let Some((short, _, ticket)) = tokens::canonical(token) else { return };
        // asked once the plugin is listed (a title may need the list first)
        let Some(p) = self.v1.by_short(short) else { return };
        if !self.v1.asked.borrow_mut().insert(token.to_owned()) {
            return;
        }
        let (plugin, ticket, token) = (p.id, ticket.to_owned(), token.to_owned());
        Owner::detached().run(|| {
            spawn(async move {
                let got = crate::vault::off_main(move || host_api::ticket_title(&broker(), &plugin, &ticket)).await;
                let title = match got {
                    Some(Ok(Some(t))) => t,
                    Some(Ok(None)) => links::NO_LONGER_AVAILABLE.to_owned(),
                    _ => return,
                };
                self.v1.titles.update(|t| {
                    t.insert(token, title);
                });
            })
        });
    }

    /// Remembers what message `text` went to the model with (shown on its
    /// bubble).
    pub fn v1_note_sent(&'static self, text: &str, attachments: &[Value]) {
        let tokens: Vec<(String, String)> = attachments
            .iter()
            .filter_map(|a| Some((a.get("token")?.as_str()?.to_owned(), a.get("title").and_then(Value::as_str).unwrap_or("").to_owned())))
            .collect();
        if tokens.is_empty() {
            return;
        }
        let (thread, text) = (self.active_id.get_untracked(), text.to_owned());
        change_records(move |r| r.sent(&thread, &text, tokens));
    }

    /// What user message `text` of `thread` went to the model with.
    pub fn v1_sent_with(&'static self, thread: &str, text: &str) -> Vec<(String, String)> {
        self.v1.records.with(|r| r.sent_with(thread, text))
    }

    /// The canonical tokens the model may see with the next message of
    /// the open chat (10.3, AGT-06).
    pub fn v1_model_attachments(&'static self) -> Vec<Value> {
        let thread = self.active_id.get_untracked();
        let entries = self.v1.entries.get_untracked();
        self.v1.records.with_untracked(|r| r.model_tokens(&thread, &entries))
    }

    /// Detaches `token` from the open chat.
    pub fn v1_detach(&'static self, token: &str) {
        let (thread, token) = (self.active_id.get_untracked(), token.to_owned());
        change_records(move |r| r.detach(&thread, &token));
        self.v1.records.set(Records::load(&data_dir()));
    }

    /// A pin of `key` was added or removed (a group's app item): kept as
    /// `{plugin, app, ticket?, placement}` (12.3).
    pub fn v1_pinned(&'static self, key: &str, title: &str, add: bool) {
        let Ok(k) = V1Key::parse(key) else { return };
        let placement = self.v1.entry(&k.plugin).and_then(|p| p.app(&k.app).map(cards::declared)).unwrap_or(Placement::Full);
        let title = title.to_owned();
        change_records(move |r| {
            if add {
                r.pin(records::Pin {
                    plugin: k.plugin,
                    app: k.app,
                    ticket: k.ticket,
                    placement: placement.as_str().to_owned(),
                    title,
                    needs_update: false,
                });
            } else {
                r.unpin(&k.plugin, &k.app, k.ticket.as_deref());
            }
        });
    }

    // ————— Connect Apps (13) —————

    /// Runs `work` off the main thread with `label` as the busy text, then
    /// lists the plugins again; an error is a toast.
    fn v1_job<T: Send + 'static>(
        &'static self,
        label: &str,
        work: impl FnOnce() -> Result<T, RoxError> + Send + 'static,
        then: impl FnOnce(T) + 'static,
    ) {
        self.v1.busy.set(label.to_owned());
        Owner::detached().run(|| {
            spawn(async move {
                let r = crate::vault::off_main(work).await;
                self.v1.busy.set(String::new());
                match r {
                    Some(Ok(v)) => then(v),
                    Some(Err(e)) => crate::toast::show(e.message),
                    None => {}
                }
                self.v1_refresh();
            })
        });
    }

    /// Install: the plugin's command line in the form, inspected; its
    /// review shows next (nothing is installed yet).
    pub fn v1_inspect(&'static self) {
        let line = self.v1.form.get_untracked();
        let Some((env, command, args)) = connect::command_line(&line) else {
            return crate::toast::show("Enter the plugin's command");
        };
        self.v1_job("Checking the plugin…", move || connect::inspect(&broker(), &command, &args, env), move |r| {
            store().v1.review.set(Some(r));
        });
    }

    /// Cancel on the review: nothing changes.
    pub fn v1_cancel_review(&'static self) {
        self.v1.review.set(None);
    }

    /// The review's main button (Install, Update, Approve and update,
    /// Install older version).
    pub fn v1_install(&'static self) {
        let Some(review) = self.v1.review.get_untracked() else { return };
        self.v1_job(
            "Installing…",
            move || connect::install(&broker(), &review).map(|r| (r, review)),
            move |(r, review)| match r {
                connect::Installed::Done => {
                    store().v1.review.set(None);
                    store().v1.form.set(String::new());
                    crate::toast::show(format!("{} {} is installed", review.title, review.version));
                }
                connect::Installed::NeedsApproval(_) => {
                    crate::toast::show("This update adds permissions: review and approve them");
                }
                connect::Installed::NeedsDowngrade => {
                    let mut older = review;
                    older.downgrade = true;
                    store().v1.review.set(Some(older));
                }
            },
        );
    }

    /// Disable or enable (13.4, 13.5); Retry of a failed plugin enables it.
    pub fn v1_set_enabled(&'static self, plugin: &str, on: bool) {
        let p = plugin.to_owned();
        if !on {
            self.v1_close_opens(plugin);
        }
        self.v1_job(if on { "Starting…" } else { "Stopping…" }, move || connect::set_enabled(&broker(), &p, on), |_| {});
    }

    /// The per-tool switch (8.3).
    pub fn v1_set_tool(&'static self, plugin: &str, tool: &str, disabled: bool) {
        let (p, t) = (plugin.to_owned(), tool.to_owned());
        self.v1_job("", move || host_api::set_tool_disabled(&broker(), &p, &t, disabled), |_| {});
    }

    /// A 2.1 account's Sign out or Remove (spec 2.1 §4).
    pub fn v1_account(&'static self, plugin: &str, account: &str, label: &str, action: host_api::AccountAction) {
        if action == host_api::AccountAction::SignIn {
            let key = signing_key(plugin, account, label.trim());
            return self.v1_signin(plugin, account, label, move |e| {
                store().v1.account_err.update(|m| {
                    m.insert(key, signin_error(&e));
                })
            });
        }
        let (p, a, l) = (plugin.to_owned(), account.to_owned(), label.to_owned());
        self.v1_job("", move || host_api::account_action(&broker(), &p, &a, &l, action), |_| {});
    }

    /// Sign in: the broker opens the browser; the row says "Signing in…"
    /// until the list has it connected (checked every 2 s, 5 min at most)
    /// or Cancel. `failed` hears a refusal.
    pub fn v1_signin(&'static self, plugin: &str, account: &str, label: &str, failed: impl FnOnce(RoxError) + 'static) {
        let key = signing_key(plugin, account, label.trim());
        if self.v1.signing.with_untracked(|s| s.contains(&key)) {
            return;
        }
        self.v1.account_err.update(|m| {
            m.remove(&key);
        });
        self.v1.signing.update(|s| {
            s.insert(key.clone());
        });
        let (p, a, l) = (plugin.to_owned(), account.to_owned(), label.trim().to_owned());
        Owner::detached().run(|| {
            spawn(async move {
                let (p2, a2, l2) = (p.clone(), a.clone(), l.clone());
                let r = crate::vault::off_main(move || host_api::account_action(&broker(), &p2, &a2, &l2, host_api::AccountAction::SignIn)).await;
                if let Some(Err(e)) = r {
                    self.v1.signing.update(|s| {
                        s.remove(&key);
                    });
                    return failed(e);
                }
                for _ in 0..150 {
                    if !self.v1.signing.with_untracked(|s| s.contains(&key)) {
                        break;
                    }
                    let (p2, a2, l2) = (p.clone(), a.clone(), l.clone());
                    let done = crate::vault::off_main(move || {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        let list = host_api::plugins(&broker()).ok()?;
                        let p = list.iter().map(connect::Listed::of).find(|x| x.id == p2)?;
                        Some(p.accounts.iter().any(|x| x.account == a2 && x.label == l2 && x.connected()))
                    })
                    .await
                    .flatten();
                    if done == Some(true) {
                        break;
                    }
                }
                self.v1.signing.update(|s| {
                    s.remove(&key);
                });
                self.v1_refresh();
            })
        });
    }

    /// Stops waiting on a sign-in (the browser was closed).
    pub fn v1_signin_cancel(&'static self, key: &str) {
        self.v1.signing.update(|s| {
            s.remove(key);
        });
    }

    /// A routine's approve, enable (in the open chat) or disable (5.9).
    pub fn v1_routine(&'static self, plugin: &str, routine: &str, action: host_api::RoutineAction) {
        let (p, r) = (plugin.to_owned(), routine.to_owned());
        let thread = self.active_id.get_untracked();
        if action == host_api::RoutineAction::Enable && thread.is_empty() {
            return crate::toast::show("Open the chat the routine should post in");
        }
        self.v1_job("", move || host_api::routine_action(&broker(), &p, &r, action, Some(thread.as_str())), |_| {});
    }

    /// An instance's Enable, Pause, edit or Delete (5.9); `instance` ""
    /// for a broker before instances. Resume passes the chat it reports to
    /// (`thread`), wherever it is resumed from (its own thread included).
    pub fn v1_instance(&'static self, plugin: &str, routine: &str, instance: &str, action: host_api::InstanceAction, extra: Value) {
        let (p, r, i) = (plugin.to_owned(), routine.to_owned(), instance.to_owned());
        self.v1_job("", move || host_api::instance_action(&broker(), &p, &r, &i, action, extra), |_| {});
    }

    /// A webhook instance's new URL: shown once on its row with Copy.
    pub fn v1_instance_hook(&'static self, plugin: &str, routine: &str, instance: &str) {
        let (p, r, i) = (plugin.to_owned(), routine.to_owned(), instance.to_owned());
        let key = instance.to_owned();
        self.v1_job("", move || host_api::instance_action(&broker(), &p, &r, &i, host_api::InstanceAction::Hook, json!({})), move |v| {
            let path = v.get("path").and_then(Value::as_str).unwrap_or_default();
            if !path.is_empty() {
                store().routine_hook.set(Some((key, format!("{}{path}", crate::client::backend()))));
            }
        });
    }

    /// Run now (`POST /v1/host/routines/run`): "Running…" on its rows until
    /// the plugin list says the run is done (checked every 2 s, 15 min at
    /// most), then the run's result line.
    pub fn v1_routine_run(&'static self, plugin: &str, routine: &str, instance: &str) {
        let key = crate::routines::v1_key(plugin, if instance.is_empty() { routine } else { instance });
        if crate::routines::is_running(&key) {
            return;
        }
        crate::routines::set_run(&key, crate::routines::RunState::Running);
        let (p, r, i) = (plugin.to_owned(), routine.to_owned(), instance.to_owned());
        Owner::detached().run(|| {
            spawn(async move {
                let (p2, r2, i2) = (p.clone(), r.clone(), i.clone());
                match crate::vault::off_main(move || host_api::instance_action(&broker(), &p2, &r2, &i2, host_api::InstanceAction::Run, json!({}))).await {
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return crate::routines::finish(key, crate::routines::refused(&e.message)),
                    None => return crate::routines::finish(key, crate::routines::refused("")),
                }
                for _ in 0..450 {
                    let (p2, r2, i2) = (p.clone(), r.clone(), i.clone());
                    let now = crate::vault::off_main(move || {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        let list = host_api::plugins(&broker()).ok()?;
                        let routine = list.iter().map(connect::Listed::of).find(|x| x.id == p2)?.routines.into_iter().find(|x| x.id == r2)?;
                        if i2.is_empty() {
                            return Some(routine);
                        }
                        routine.instances.iter().find(|x| x.instance == i2).map(|x| x.as_routine(&routine))
                    })
                    .await
                    .flatten();
                    if let Some(now) = now.filter(|x| !x.running) {
                        crate::routines::finish(key, crate::routines::v1_outcome(&now));
                        return self.v1_refresh();
                    }
                }
                crate::routines::finish(key, ("Still running; its thread shows what it finds".to_owned(), false));
            })
        });
    }

    /// Closes every open of `plugin`: the full-screen stack and the inline
    /// cards (13.4, 13.6 step 2).
    pub fn v1_close_opens(&'static self, plugin: &str) {
        let prefix = format!("v1:{plugin}/");
        self.mini_full.update(|v| v.retain(|o| !o.key.starts_with(&prefix)));
        let link = format!("buwiz://app/{plugin}/");
        self.v1.shown.update(|s| s.retain(|id| !id.contains(&link)));
    }

    /// Uninstall (13.6): opens close, sessions end, the broker uninstalls,
    /// then the plugin's attachments, pins (and their sidebar items) and
    /// storage go.
    pub fn v1_uninstall(&'static self, plugin: &str) {
        self.v1_uninstall_then(plugin, || {});
    }

    /// [`Self::v1_uninstall`], then `after` once it is done.
    fn v1_uninstall_then(&'static self, plugin: &str, after: impl FnOnce() + 'static) {
        let p = plugin.to_owned();
        self.v1_close_opens(plugin);
        let prefix = format!("v1:{plugin}/");
        let pinned: Vec<(String, String)> = self.groups.with_untracked(|gs| {
            gs.iter()
                .flat_map(|g| g.items.iter().filter(|it| it.kind == "app" && it.id.starts_with(&prefix)).map(|it| (g.id.clone(), it.id.clone())))
                .collect()
        });
        self.v1_job(
            "Uninstalling…",
            move || {
                let _held = RECORDS.lock().unwrap_or_else(|e| e.into_inner());
                connect::uninstall(&broker(), &p, &data_dir(), &super::plugins_dir(), &|_| {})
            },
            move |removed| {
                let st = store();
                for (gid, key) in pinned {
                    st.group_item(&gid, "app", &key, "", false);
                }
                st.v1.records.set(Records::load(&data_dir()));
                st.v1.page.set(String::new());
                crate::toast::show(format!(
                    "Uninstalled. Removed {} attachment(s) and {} pin(s).",
                    removed.attachments, removed.pins
                ));
                after();
            },
        );
    }

    // ————— App install and uninstall cards in a chat —————

    /// Card `i` of chat `chat`'s local state, changed by `f`.
    pub fn app_card<T>(&'static self, chat: &str, i: i64, f: impl FnOnce(&mut crate::store::AppCard) -> T) -> T {
        let mut out = None;
        self.app_cards.update(|l| {
            let at = match l.iter().position(|c| c.chat == chat && c.i == i) {
                Some(at) => at,
                None => {
                    l.push(crate::store::AppCard { chat: chat.to_owned(), i, ..Default::default() });
                    l.len() - 1
                }
            };
            out = Some(f(&mut l[at]));
        });
        out.expect("updated")
    }

    /// What is typed into card `i`'s fields, taken out of the store.
    fn app_take_typed(&'static self, chat: &str, i: i64) -> Vec<(String, String)> {
        let mut out = Vec::new();
        self.app_typed.update(|l| {
            let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(l).into_iter().partition(|(c, at, _, _)| c == chat && *at == i);
            *l = rest;
            out = mine.into_iter().map(|(_, _, k, v)| (k, v)).collect();
        });
        out
    }

    /// An install card's main button: installs what it reviewed with the
    /// values typed into it (or the settings of the plugin chosen), off
    /// the main thread; the typed values go with the call and nowhere else.
    pub fn app_install(&'static self, chat: &str, i: i64, n: &crate::store::AppInstallNotice) {
        let (chat, n) = (chat.to_owned(), n.clone());
        let Some(picked) = self.app_card(&chat, i, |c| (!c.busy).then(|| {
            c.start();
            c.picked.clone()
        })) else {
            return;
        };
        let typed = self.app_take_typed(&chat, i);
        let body = n.install_body(&typed, n.settings_from(picked.as_deref()));
        drop(typed);
        Owner::detached().run(|| {
            spawn(async move {
                let r = crate::vault::off_main(move || connect::install_with(&broker(), body)).await;
                let r = match r {
                    Some(Ok(connect::Installed::Done)) => Ok("installed"),
                    Some(Ok(connect::Installed::NeedsApproval(_))) => {
                        Err("This update adds permissions; ask for the install again to review them".to_owned())
                    }
                    Some(Ok(connect::Installed::NeedsDowngrade)) => {
                        Err(format!("This is older than the installed {}; ask for the install again to review it", n.title))
                    }
                    Some(Err(e)) => Err(e.message),
                    None => Err("The install did not finish".to_owned()),
                };
                self.app_card(&chat, i, |c| c.finish(r));
                self.v1_refresh();
            })
        });
    }

    /// An app card's other answers: Not now (`declined`), Cancel
    /// (`cancelled`), or Uninstall (`uninstalled`, after the uninstall
    /// Connect Apps runs); the broker's notice is settled with it.
    pub fn app_answer(&'static self, chat: &str, i: i64, kind: &'static str, id: &str, state: &'static str) {
        let (chat, plugin, id) = (chat.to_owned(), id.to_owned(), id.to_owned());
        let settle = move || {
            drop(self.app_take_typed(&chat, i));
            self.app_card(&chat, i, |c| c.finish(Ok(state)));
            let (chat, id) = (chat.clone(), id.clone());
            Owner::detached().run(|| {
                spawn(async move {
                    let r = crate::vault::off_main(move || host_api::settle_notice(&broker(), &chat, kind, &id, state)).await;
                    if let Some(Err(e)) = r {
                        log::warn!("app notice settle: {}", e.message);
                    }
                })
            });
        };
        if state == "uninstalled" {
            self.v1_uninstall_then(&plugin, settle);
        } else {
            settle();
        }
    }

    /// An `account_required` card's Sign in: the account's first label
    /// (or `default`); a refusal stays on the card.
    pub fn account_card_signin(&'static self, chat: &str, i: i64, n: &crate::store::AccountRequiredNotice) {
        let label = self.v1.listed.with_untracked(|l| l.iter().find(|p| p.id == n.plugin).map(|p| p.signin_label(&n.account)));
        let label = label.unwrap_or_else(|| "default".to_owned());
        self.app_card(chat, i, |c| c.error.clear());
        let ch = chat.to_owned();
        self.v1_signin(&n.plugin, &n.account, &label, move |e| store().app_card(&ch, i, |c| c.finish(Err(signin_error(&e)))));
    }

    /// Open logs of a plugin (14.5): its log file in the Finder.
    pub fn v1_open_logs(&'static self, plugin: &str) {
        match connect::log_file(plugin) {
            Some(path) if path.exists() => self.open_externally(&path.to_string_lossy()),
            _ => crate::toast::show("This plugin has written no log yet"),
        }
    }
}

/// The v0 link router's half: what `route_link` does with a link that is
/// not v1's.
impl Store {
    fn open_link_v0(&'static self, url: &str, os: bool) {
        self.route_link_v0(url, os);
    }
}

/// Whether a `buwiz://app/` link's first segment is a reverse-DNS plugin id
/// (a v1 link even when that plugin is not listed yet).
fn url_names_plugin_id(url: &str) -> bool {
    url.strip_prefix("buwiz://app/")
        .and_then(|r| r.split(['/', '?']).next())
        .is_some_and(super::perms::plugin_id_ok)
}

/// The message an attachment is sent as: "Attached <canonical token>".
pub fn attach_message(token: &str) -> String {
    format!("Attached {token}")
}

/// `@short:scope:value … attach` (one `attach` word, every other word a
/// token of a v1 plugin `is_short` names): the tokens to attach.
pub fn attach_words(text: &str, is_short: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 2 || words.iter().filter(|w| w.eq_ignore_ascii_case("attach")).count() != 1 {
        return None;
    }
    let toks: Vec<String> = words.into_iter().filter(|w| !w.eq_ignore_ascii_case("attach")).map(str::to_owned).collect();
    let ours = |w: &String| {
        let t = tokens::typed_tokens(w);
        t.len() == 1 && t[0].start == 0 && t[0].end == w.len() && is_short(t[0].short)
    };
    toks.iter().all(ours).then_some(toks)
}

/// The composer's view of the plugins' commands (3.1).
pub fn commands(entries: &[PluginEntry]) -> Vec<tokens::Commands> {
    entries
        .iter()
        .map(|p| tokens::Commands {
            plugin: p.id.clone(),
            short: p.short.clone(),
            title: p.title.clone(),
            enabled: p.enabled,
            commands: p.commands.iter().filter_map(|c| c.get("name").and_then(Value::as_str).map(str::to_owned)).collect(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Elements
// ---------------------------------------------------------------------------

use crate::theme::tc;

fn small_button(label: &'static str, primary: bool, on: impl Fn() + 'static) -> impl IntoElement {
    view()
        .h(px(28.0))
        .px_3()
        .rounded(px(14.0))
        .items_center()
        .justify_center()
        .bg(if primary { theme::TEXT } else { theme::SUBTLE })
        .label(label)
        .child(text(label).text_size(px(13.0)).text_color(if primary { theme::POPOVER } else { theme::TEXT }))
        .on_press(on)
}

/// A title-bar icon button, `label` its accessible name.
fn icon_button(icon: &'static str, label: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    view()
        .size(px(28.0))
        .rounded(px(14.0))
        .items_center()
        .justify_center()
        .bg(theme::SUBTLE)
        .label(label)
        .child(crate::icons::icon_ink(icon, 15.0, || tc("#f5f5f5ad")))
        .on_press(on)
}

/// A full app card's button (9.6): "Open <title>" (a plugin's sign-in
/// card reads "Open Buwiz Sign-in"), only Open when the title is long (the
/// row shows it: "BIR Form 2551Q · Q3 2026 · …"). An inline card has none:
/// its header's chevron shows it ([`inline_card`]).
pub fn button_label(card: &OpenCard) -> String {
    if card.title.chars().count() > 20 { "Open".to_owned() } else { format!("Open {}", card.title) }
}

/// Whether inline card `id` is shown: an answer card that mounts at once
/// (`auto`, a first-party plugin's, 9.8) unless the person collapsed it;
/// any other once they expanded it.
fn card_shown(id: &str, auto: bool) -> bool {
    if auto { !store().v1.hidden.with(|h| h.contains(id)) } else { store().v1.shown.with(|s| s.contains(id)) }
}

/// Show or hide card `id`.
fn toggle_card(id: &str, auto: bool) {
    let set = if auto { store().v1.hidden } else { store().v1.shown };
    let id = id.to_owned();
    set.update(|s| {
        if !s.remove(&id) {
            s.insert(id);
        }
    });
}

/// A full app card's Open.
fn open_button(label: String, on: impl Fn() + 'static) -> impl IntoElement {
    view()
        .h(px(28.0))
        .px_3()
        .rounded(px(14.0))
        .items_center()
        .justify_center()
        .bg(theme::TEXT)
        .label(label.clone())
        .child(text(label).text_size(px(13.0)).text_color(theme::POPOVER))
        .on_press(on)
}

/// An answer's cards (9.6, 9.8), from the reply's `buwiz-app` links: one
/// main card and up to two small ones, the rest collapsed. A
/// first-party plugin's inline card mounts at once; any other shows its
/// header until its chevron expands it (9.7). Once `user_after()` of the
/// person's messages came since, the cards collapse: an inline one to its
/// header, a full one to its row with Open.
pub fn answer_cards(turn: &str, links: Vec<crate::store::DriveLink>, user_after: impl Fn() -> usize + Clone + 'static) -> AnyElement {
    let cards: Vec<(String, OpenCard)> = links
        .iter()
        .filter_map(|l| {
            let c = OpenCard::from_link(&json!({"provider": "buwiz-app", "card": l.card.clone()?}))?;
            Some((format!("{turn}:{}", l.id), c))
        })
        .collect();
    let slots = cards::arrange(&cards.iter().map(|(_, c)| c.clone()).collect::<Vec<_>>());
    // a fresh answer's cards that mount at once: their sessions open now,
    // alongside their modules (14.6)
    if !cards::aged(untracked(&user_after)) {
        for ((_, c), slot) in cards.iter().zip(&slots) {
            let first_party = store().v1.entry(&c.plugin).is_some_and(|p| p.first_party);
            if *slot != cards::Slot::Link && c.placement == Placement::Inline && first_party {
                store().v1_preopen(&c.plugin, &c.app);
            }
        }
    }
    view()
        .gap_2()
        .w_full()
        .children(cards.into_iter().zip(slots).map(move |((id, card), slot)| {
            let aged = user_after.clone();
            match slot {
                cards::Slot::Link => card_row(card, id, true),
                _ => dynamic(move || card_row(card.clone(), id.clone(), cards::aged(aged()))).into_element(),
            }
        }))
        .into_element()
}

/// A tool result's card (9.6): a full app's row with its title, the
/// plugin and Open, or an inline app's one card, whose header's chevron
/// shows the app under it ([`inline_card`]).
pub fn open_card(card: OpenCard, card_id: String) -> AnyElement {
    card_row(card, card_id, false)
}

/// A card of an answer or a tool result. An inline app is one card (its
/// header shows or hides the app under it: [`inline_card`]); a full app is
/// a row with Open. A `collapsed` one (aged, or past an answer's one main
/// and two small ones) shows its header only until it is opened.
fn card_row(card: OpenCard, card_id: String, collapsed: bool) -> AnyElement {
    let entry = store().v1.entry(&card.plugin);
    let plugin_title = entry.as_ref().map(|p| p.title.clone()).unwrap_or_else(|| card.plugin.clone());
    let auto = !collapsed
        && card.placement == Placement::Inline
        && consent::ask(consent::inline_card(entry.as_ref().is_some_and(|p| p.first_party))) == consent::Ask::Nothing;
    if card.placement == Placement::Inline {
        return dynamic(move || {
            let live = store().v1.live.with(|l| l.contains(&card_id));
            inline_card(card.clone(), card_id.clone(), live, auto, plugin_title.clone()).into_element()
        })
        .into_element();
    }
    let c1 = card.clone();
    view()
        .gap_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(if card.size == cards::Size::Small { 48.0 } else { 64.0 }))
                .px(px(12.0))
                .rounded(px(12.0))
                .bg(tc("#1f1f1f"))
                .child(crate::appdetails::named_tile("", &card.title, if card.size == cards::Size::Small { 28.0 } else { 36.0 }))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(text(card.title.clone()).number_of_lines(1).text_size(px(14.0)).text_color(tc("#f5f5f5")))
                        .child(
                            text(format!("Mini app · {plugin_title}"))
                                .number_of_lines(1)
                                .text_size(px(13.0))
                                .text_color(tc("#f5f5f5ad")),
                        ),
                )
                .child(open_button(button_label(&card), move || store().open_v1(c1.key(), &c1.title, Opener::ToolResult))),
        )
        .into_element()
}

/// The full-screen v1 app's Pin: in Rox's records, `{plugin, app,
/// ticket?, placement}` and its title (the sidebar lists them).
pub fn pin_button(key: &str, n: u64) -> impl IntoElement {
    let (k1, k2) = (key.to_owned(), key.to_owned());
    let pinned = move || {
        let Ok(k) = V1Key::parse(&k1) else { return false };
        store().v1.records.with(|r| r.pins.iter().any(|p| p.plugin == k.plugin && p.app == k.app && p.ticket == k.ticket))
    };
    let p2 = pinned.clone();
    view()
        .label("Pin")
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .h(px(32.0))
        .px(px(12.0))
        .rounded(px(16.0))
        .bg(tc("#f5f5f50f"))
        .child(dynamic(move || crate::icons::icon_ink(if pinned() { "pinFilled" } else { "pin" }, 14.0, || tc("#f5f5f5a8"))))
        .child(text(move || if p2() { "Pinned".to_owned() } else { "Pin".to_owned() }).text_size(px(13.0)).text_color(tc("#f5f5f5a8")))
        .on_press(move || {
            let st = store();
            let title = st.mini_full.with_untracked(|v| v.iter().find(|o| o.n == n).map(|o| o.title.clone())).unwrap_or_default();
            let title = if title.is_empty() { st.mini_source_title(&k2).get_untracked() } else { title };
            let Ok(k) = V1Key::parse(&k2) else { return };
            let on = st.v1.records.with_untracked(|r| r.pins.iter().any(|p| p.plugin == k.plugin && p.app == k.app && p.ticket == k.ticket));
            st.v1_pinned(&k2, &title, !on);
        })
}

/// The pinned v1 apps in the sidebar (12.3): a press opens each as its app
/// declares (inline in the open chat, or full screen); ✕ unpins.
pub fn pins_section() -> impl IntoElement {
    dynamic(|| {
        // a pin a sidebar group already shows (a migrated v0 pin) is left out
        let grouped: Vec<V1Key> = store().groups.with(|gs| {
            gs.iter().flat_map(|g| &g.items).filter(|it| it.kind == "app").filter_map(|it| V1Key::parse(&it.id).ok()).collect()
        });
        let pins: Vec<_> = store()
            .v1
            .records
            .with(|r| r.pins.clone())
            .into_iter()
            .filter(|p| !grouped.iter().any(|k| k.plugin == p.plugin && k.app == p.app && k.ticket == p.ticket))
            .collect();
        if pins.is_empty() {
            return view().into_element();
        }
        view()
            .px_3()
            .pb_2()
            .gap(px(2.0))
            .child(text("Pinned apps").text_size(px(12.0)).text_color(theme::DIM).px_2().py_1())
            .children(pins.into_iter().map(|p| {
                let key = V1Key { plugin: p.plugin.clone(), app: p.app.clone(), ticket: p.ticket.clone(), props: Default::default() }.to_key();
                let (k1, k2, title) = (key.clone(), key, p.title.clone());
                let label = if p.needs_update { format!("{} · needs update", p.title) } else { p.title.clone() };
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .label(label.clone())
                    .on_press(move || store().open_mini_full(&k1, &title))
                    .child(crate::appdetails::named_tile("", &p.title, 24.0))
                    .child(text(label).flex_1().number_of_lines(1).text_size(px(13.0)).text_color(theme::TEXT))
                    .child(
                        view()
                            .size(px(18.0))
                            .items_center()
                            .justify_center()
                            .label("Unpin")
                            .on_press(move || store().v1_pinned(&k2, "", false))
                            .child(text("✕").text_size(px(10.0)).text_color(theme::MUTED)),
                    )
                    .into_element()
            }))
            .into_element()
    })
}

/// An answer's next steps (9.8, `_meta["buwiz/suggest"]`): chips under it;
/// a tap sends the prompt as the person's message.
pub fn suggest_chips(items: Vec<(String, String)>) -> impl IntoElement {
    view()
        .flex_row()
        .flex_wrap()
        .gap(px(6.0))
        .max_w(px(COLUMN))
        .children(items.into_iter().map(|(label, prompt)| {
            view()
                .h(px(28.0))
                .px(px(12.0))
                .rounded(px(14.0))
                .items_center()
                .justify_center()
                .border_1()
                .border_color(theme::BORDER_S)
                .label(label.clone())
                .on_press(move || store().send_text(prompt.clone()))
                .child(text(label).number_of_lines(1).text_size(px(13.0)).text_color(theme::TEXT))
                .into_element()
        }))
}

/// The v1 attachments the model got with a sent message (10.3), as chips
/// above the person's bubble: each ticket's title.
pub fn sent_attachments(items: Vec<(String, String)>) -> impl IntoElement {
    view()
        .flex_row()
        .flex_wrap()
        .justify_end()
        .gap(px(6.0))
        .max_w(px(560.0))
        .children(items.into_iter().map(|(token, title)| {
            let tok = token.clone();
            view()
                .flex_row()
                .items_center()
                .h(px(22.0))
                .px(px(8.0))
                .rounded(px(11.0))
                .bg(theme::ACCENT_TINT)
                .label(format!("Attached: {title}"))
                .child(
                    text(move || {
                        let shown = store().v1_plain(&tok);
                        if title.is_empty() { shown } else { title.clone() }
                    })
                    .number_of_lines(1)
                    .text_size(px(12.0))
                    .text_color(theme::ACCENT),
                )
                .into_element()
        }))
}

/// An inline app (6.5, 9.6) is one card. Its header carries the app's
/// tile and title, Refresh, a chevron that shows or hides the app, and
/// maximize (full screen); the app sits under it at its declared size,
/// clamped to the range and the column, then follows its `buwiz.fit`. A card
/// that does not mount at once (`auto` false: a third party's, 9.7, or an
/// aged one) shows its header only, with the plugin's name, until the
/// chevron opens it; opening it is the consent.
///
/// The app may hand Rox its card's chrome (7.12 to 7.14): a status in the
/// header (`buwiz.meta`), footer buttons under the app (`buwiz.actions`), and
/// the one line the card settles to (`buwiz.settle`), which replaces the
/// whole card and unmounts the app, as an answered approval card does.
pub fn inline_card(card: OpenCard, id: String, live: bool, auto: bool, plugin: String) -> impl IntoElement {
    // Refresh (9.8): the same open without its snapshot reads live
    let key = if live { card.live_key().to_key() } else { card.key().to_key() };
    let k = key.clone();
    dynamic(move || match store().v1.settled.with(|s| s.get(&k).cloned()) {
        Some(line) => crate::cards::outcome_line(line).into_element(),
        None => {
            let (card, id, key, plugin) = (card.clone(), id.clone(), key.clone(), plugin.clone());
            untracked(move || open_inline_card(card, id, key, live, auto, plugin).into_element())
        }
    })
}

/// [`inline_card`] until it settles.
fn open_inline_card(card: OpenCard, id: String, key: String, live: bool, auto: bool, plugin: String) -> impl IntoElement {
    let as_of = card.as_of().filter(|_| !live);
    let app = store().v1.entry(&card.plugin).and_then(|p| p.app(&card.app).cloned()).unwrap_or(Value::Null);
    let (w, declared) = cards::inline_size(&app, COLUMN);
    let wait_ready = cards::wait_ready(&app);
    let key_h = key.clone();
    let h = move || store().v1.fit.with(|f| f.get(&key_h).copied()).unwrap_or(declared);
    let c = card.clone();
    let title = card.title.clone();
    let key_r = key.clone();
    let (id_shown, id_toggle) = (id.clone(), id.clone());
    let shown = move || card_shown(&id_shown, auto);
    let open = shown.clone();
    view()
        .w(px(w))
        .rounded(px(12.0))
        .overflow_hidden()
        .border_1()
        .border_color(theme::BORDER_S)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(40.0))
                .px(px(10.0))
                .bg(tc("#1f1f1f"))
                .child(crate::appdetails::named_tile("", &card.title, 22.0))
                // the title and the plugin's name give way to the buttons,
                // cut to one line
                .child(text(card.title.clone()).flex_auto().min_w(px(0.0)).number_of_lines(1).text_size(px(13.0)).text_color(tc("#f5f5f5")))
                .child(card_meta(&key))
                // until it mounts, whose app this is (9.7)
                .when(!auto, |v| {
                    v.child(text(plugin).flex_shrink(1.0).min_w(px(0.0)).number_of_lines(1).text_size(px(12.0)).text_color(tc("#f5f5f5ad")))
                })
                .when(as_of.is_some(), |v| v.child(text(cards::as_of_label(as_of.unwrap_or_default())).text_size(px(12.0)).text_color(tc("#f5f5f5ad"))))
                // a snapshot goes live; a live card reads again. An action
                // card (no `as_of`: its tool changed something) has none.
                .when(as_of.is_some() || live, |v| {
                    v.child(icon_button("refresh", "Refresh", move || {
                        if as_of.is_some() {
                            let id = id.clone();
                            store().v1.live.update(|l| {
                                l.insert(id);
                            })
                        } else {
                            store().v1_reload(&key_r)
                        }
                    }))
                })
                .child(dynamic(move || {
                    let id = id_toggle.clone();
                    if shown() {
                        icon_button("chevronDown", "Collapse", move || toggle_card(&id, auto)).into_element()
                    } else {
                        icon_button("chevronRight", "Expand", move || toggle_card(&id, auto)).into_element()
                    }
                }))
                .child(icon_button("maximize", "Open full screen", move || store().open_v1(c.key(), &c.title, Opener::User))),
        )
        .child(when(open, move || {
            let h = h.clone();
            let h2 = h.clone();
            view()
                .child(
                    view()
                        .h(move || px(h()))
                        .child(banners(&key, false))
                        .child(crate::mini::mini_app_inline(&key, w, h2, title.clone(), wait_ready))
                        .child(error_overlay(&key)),
                )
                .child(card_footer(&key))
                .into_element()
        }))
}

/// The status an inline app put in its card's header (`buwiz.meta`): dim
/// text, or a pill as a settled card's.
fn card_meta(key: &str) -> impl IntoElement {
    let key = key.to_owned();
    dynamic(move || {
        let meta = store().v1.meta.with(|m| m.get(&key).map(|(_, m)| m.clone()))?;
        Some(if meta.pill {
            crate::cards::pill(&meta.text).into_element()
        } else {
            text(meta.text).number_of_lines(1).text_size(px(12.0)).text_color(tc("#f5f5f5ad")).into_element()
        })
    })
}

/// An inline app's footer buttons (`buwiz.actions`), as Rox's own cards
/// draw theirs: 28-point pills, right-aligned, the primary first. A
/// disabled one is dimmed and does nothing; a press goes to the app.
fn card_footer(key: &str) -> impl IntoElement {
    let key = key.to_owned();
    dynamic(move || {
        let footer = store().v1.footer.with(|f| f.get(&key).cloned()).filter(|f| !f.actions.is_empty())?;
        let mut row = view().flex_row().flex_wrap().justify_end().gap_2().px(px(10.0)).py(px(8.0)).bg(theme::CARD);
        for a in cards::footer_order(&footer.actions) {
            let kind = if a.primary { crate::cards::Btn::Primary } else { crate::cards::Btn::Secondary };
            let (press, id, off) = (footer.press.clone(), a.id.clone(), a.disabled);
            let button = crate::cards::btn(kind, &a.label, move || {
                if !off {
                    press(&id)
                }
            });
            row = row.child(view().opacity(if off { 0.4f32 } else { 1.0 }).child(button));
        }
        Some(row.into_element())
    })
}

/// The banners over an open (`key`): its notice ("This item is no longer
/// available", "reconnecting") and, `full` screen, a stale open's Reload
/// (12.2). An inline card has no Reload strip: it reloads in place.
pub fn banners(key: &str, full: bool) -> impl IntoElement {
    let (k1, k2, k3, k4) = (key.to_owned(), key.to_owned(), key.to_owned(), key.to_owned());
    view()
        .child(when(
            move || store().v1.notice.with(|n| n.contains_key(&k1)),
            move || {
                let k = k2.clone();
                view()
                    .px_3()
                    .py_2()
                    .bg(tc("#f5f5f514"))
                    .child(
                        text(move || store().v1.notice.with(|n| n.get(&k).cloned().unwrap_or_default()))
                            .text_size(px(13.0))
                            .text_color(tc("#f5f5f5")),
                    )
                    .into_element()
            },
        ))
        .child(when(
            move || full && store().v1.stale.with(|s| s.contains(&k3)),
            move || {
                let k = k4.clone();
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(tc("#f5f5f514"))
                    .child(
                        text("A newer version of this app is available.")
                            .flex_1()
                            .text_size(px(13.0))
                            .text_color(tc("#f5f5f5")),
                    )
                    .child(small_button("Reload", true, move || store().v1_reload(&k)))
                    .into_element()
            },
        ))
}

/// A running app's error (6.7) over it, with Reload and Close.
pub fn error_overlay(key: &str) -> impl IntoElement {
    let (k1, k2, k3, k4) = (key.to_owned(), key.to_owned(), key.to_owned(), key.to_owned());
    when(
        move || store().v1.error.with(|e| e.contains_key(&k1)),
        move || {
            let (k, kr, kc) = (k2.clone(), k3.clone(), k4.clone());
            view()
                .absolute()
                .size_full()
                .items_center()
                .justify_center()
                .bg(tc("#161616f0"))
                .child(
                    view()
                        .max_w(px(480.0))
                        .gap_3()
                        .px_6()
                        .child(
                            text(move || store().v1.error.with(|e| e.get(&k).cloned().unwrap_or_default()))
                                .number_of_lines(6)
                                .text_size(px(14.0))
                                .text_color(tc("#f5f5f5")),
                        )
                        .child(
                            view()
                                .flex_row()
                                .gap_2()
                                .child(small_button("Reload", true, move || store().v1_reload(&kr)))
                                .child(small_button("Close", false, move || store().v1_close(&kc))),
                        ),
                )
                .into_element()
        },
    )
}

/// The preview a v1 app asked for (`buwiz.preview`, 7.10), over the
/// full-screen apps: a title bar with the document's title, Print (the
/// print panel, which also saves a PDF) and Close, and the page. The page
/// is the plugin's static HTML: its web view runs no script, has no base
/// URL, carries the sealed page's no-load policy ([`super::bridge::seal`])
/// and goes nowhere: a navigation that starts is stopped, and one that
/// lands anyway closes the preview.
pub fn preview_layer() -> impl IntoElement {
    when(
        || store().v1.preview.with(Option::is_some),
        || {
            let doc = || store().v1.preview.with(|p| p.clone().unwrap_or(super::bridge::Preview { title: String::new(), html: String::new() }));
            let page = element_ref();
            let stop = page;
            // a string loaded with no base URL is about:blank
            let own = |url: &str| url.is_empty() || url.starts_with("about:");
            view()
                .absolute()
                .size_full()
                .bg(tc("#161616"))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .h(px(52.0))
                        .pl(px(84.0))
                        .pr(px(12.0))
                        .border_b(px(0.5))
                        .border_b_color(tc("#f5f5f51a"))
                        .child(
                            text(move || doc().title)
                                .flex_1()
                                .number_of_lines(1)
                                .text_size(px(14.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(tc("#f5f5f5")),
                        )
                        .child(small_button("Print", true, move || {
                            let d = doc();
                            crate::keys::print("html", d.html, &d.title);
                        }))
                        .child(small_button("Close", false, || store().v1_close_preview())),
                )
                .child(
                    web_view_html(move || doc().html, None)
                        .element_ref(&page)
                        .java_script_enabled(false)
                        .flex_1()
                        .bg(AdaptiveColor::from(Hsla { h: 0.0, s: 0.0, l: 1.0, a: 1.0 }))
                        .on_load_start(move |n: &WebNavigation| {
                            if !own(&n.url) {
                                log::warn!("mini v1: preview stopped a navigation");
                                stop.stop_loading();
                            }
                        })
                        .on_load(move |n: &WebNavigation| {
                            if !own(&n.url) {
                                store().v1_close_preview();
                            }
                        }),
                )
                .into_element()
        },
    )
}

/// The open chat's v1 attachments (10.3) above the composer: each its
/// title, "needs update" (or disabled, not installed) when it is not sent
/// to the model, and ✕ to detach.
pub fn attachments_strip() -> impl IntoElement {
    let items = computed(|| {
        let thread = store().active_id.get();
        let entries = store().v1.entries.get();
        store().v1.records.with(|r| {
            r.of_thread(&thread)
                .into_iter()
                .map(|a| {
                    let status = records::status(&a, &entries).label();
                    (a.token, a.title, status)
                })
                .collect::<Vec<_>>()
        })
    });
    dynamic(move || {
        let items = items.get();
        if items.is_empty() {
            return view().into_element();
        }
        view()
            .flex_row()
            .flex_wrap()
            .pt(px(8.0))
            .px_3()
            .gap(px(6.0))
            .children(items.into_iter().map(|(token, title, status)| {
                let tok = token.clone();
                let label = if status.is_empty() { title } else { format!("{title} · {status}") };
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .h(px(22.0))
                    .pl(px(8.0))
                    .pr(px(4.0))
                    .rounded(px(11.0))
                    .bg(if status.is_empty() { theme::ACCENT_TINT } else { theme::SUBTLE })
                    .label(label.clone())
                    .child(
                        text(label)
                            .number_of_lines(1)
                            .text_size(px(12.0))
                            .text_color(if status.is_empty() { theme::ACCENT } else { theme::MUTED }),
                    )
                    .child(
                        view()
                            .size(px(16.0))
                            .items_center()
                            .justify_center()
                            .label("Detach")
                            .on_press(move || store().v1_detach(&tok))
                            .child(text("✕").text_size(px(10.0)).text_color(theme::MUTED)),
                    )
                    .into_element()
            }))
            .into_element()
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const T: &str = "tkt_0123456789ABCDEFGHJKMNPQRS";

    #[test]
    fn a_sign_in_that_needs_setup_says_what_is_missing() {
        assert_eq!(signin_error(&RoxError::new("needs_setup", "")), "Add your Google client in Setup");
        assert_eq!(signin_error(&RoxError::new("needs_setup", "Add your Google client in Setup first")), "Add your Google client in Setup first");
        assert_eq!(signin_error(&RoxError::new("internal", "the broker answered 500")), "the broker answered 500");
        assert_eq!(signing_key("app.buwiz.gws", "google", "work"), "app.buwiz.gws|google/work");
    }

    #[test]
    fn an_attachment_is_sent_as_a_message_carrying_its_canonical_token() {
        let token = format!("@bir:taxprofile:{T}");
        let m = attach_message(&token);
        assert_eq!(m, format!("Attached {token}"));
        // a message, not tokens alone: it reaches the model, with the ticket
        assert!(!tokens::only_tokens_or_command(&m));
        assert_eq!(tokens::canonical_tokens(&m).len(), 1);
        // shown with the ticket's title, never the ticket
        let (shown, _) = tokens::plain(&m, &|_| Some("Galang Alejandro Pascua".to_owned()));
        assert_eq!(shown, "Attached Galang Alejandro Pascua");
    }

    #[test]
    fn tokens_and_attach_attach_them() {
        let bir = |s: &str| s == "bir";
        assert_eq!(attach_words("@bir:taxprofile:123456788 attach", &bir), Some(vec!["@bir:taxprofile:123456788".to_owned()]));
        assert_eq!(attach_words("Attach @bir:taxprofile:1 @bir:taxform:x", &bir).map(|t| t.len()), Some(2));
        assert_eq!(attach_words("@bir:taxprofile:1", &bir), None);
        assert_eq!(attach_words("please attach @bir:taxprofile:1", &bir), None);
        assert_eq!(attach_words("@gws:mail:1 attach", &bir), None, "v0 servers keep v0's attach");
        assert_eq!(attach_words("attach attach @bir:taxprofile:1", &bir), None);
    }

    #[test]
    fn a_plugins_sign_in_card_reads_open_and_its_app() {
        let link = json!({"provider": "buwiz-app", "card": {"plugin": "app.buwiz.ebirforms", "app": "register", "title": "Buwiz Sign-in", "placement": "full"}});
        let card = OpenCard::from_link(&link).unwrap();
        assert_eq!(button_label(&card), "Open Buwiz Sign-in");
        let long = OpenCard { title: "BIR Form 2551Q · Q3 2026 · Galang Alejandro Pascua".into(), ..card.clone() };
        assert_eq!(button_label(&long), "Open");
    }
}
