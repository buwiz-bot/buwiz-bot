//! The person-facing side of v1 plugins in Rox: what the store keeps for
//! them ([`V1`], `Store::v1`), the consent sheet's v1 questions (9.7), the
//! chat seam attachments go through (10.3), `rox://app/` links (9.4), the
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
    cards::{self, OpenCard, Opener, Placement},
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
    /// Opens a new version left on old source: a Reload banner (12.2).
    pub stale: Signal<HashSet<String>>,
    /// Opens whose plugin restarted, by key: their plugin (13.7).
    reconnecting: RefCell<HashMap<String, String>>,
    /// Inline cards shown (their ids).
    pub shown: Signal<HashSet<String>>,
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
    /// The opens running, with the fingerprint each loaded.
    running: RefCell<Vec<(u64, String, String)>>,
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
            records: signal(Records::default()),
            review: signal(None),
            busy: signal(String::new()),
            form: signal(String::new()),
            page: signal(String::new()),
            queue: RefCell::new(VecDeque::new()),
            running: RefCell::new(Vec::new()),
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
    #[cfg(target_os = "macos")]
    super::native::on_main(move || store().v1.records.set(r));
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
            super::native::on_main(move || crate::toast::show(format!("Attached {title} to this chat")));
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
                super::native::on_main(move || store().v1_consent(ask, answer));
            }));
            super::native::set_chat(Arc::new(OpenChat));
            super::session::set_event_listener(Arc::new(|v: &Value| {
                let v = v.clone();
                super::native::on_main(move || store().v1_plugin_event(&v));
            }));
            super::session::listen_for_generations();
        }
        self.in_store(|| {
            effect(move || {
                let id = self.active_id.get();
                *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = id;
            })
        });
        self.v1_refresh(true);
    }

    /// Lists the v1 plugins again; `migrate` runs the one-time v0
    /// migration once BIR's v1 plugin runs.
    pub fn v1_refresh(&'static self, migrate: bool) {
        Owner::detached().run(|| {
            spawn(async move {
                let got = crate::vault::off_main(|| host_api::plugins(&broker())).await;
                let Some(Ok(list)) = got else {
                    self.v1.loaded.set(true);
                    return;
                };
                let entries: Vec<PluginEntry> = list.iter().filter_map(PluginEntry::from_json).collect();
                *ENTRIES.lock().unwrap_or_else(|e| e.into_inner()) = entries.clone();
                let running: Vec<(String, String)> =
                    self.v1.running.borrow().iter().map(|(_, k, f)| (k.clone(), f.clone())).collect();
                let stale = lifecycle::stale(&running, &entries);
                batch(|| {
                    self.v1.listed.set(list.iter().map(connect::Listed::of).collect());
                    self.v1.entries.set(entries.clone());
                    self.v1.loaded.set(true);
                    if !stale.is_empty() {
                        self.v1.stale.update(|s| s.extend(stale));
                    }
                });
                let bir = list.iter().find(|p| p.get("id").and_then(Value::as_str) == Some(links::BIR));
                let runs = bir.is_some_and(|p| p.get("state").and_then(Value::as_str) == Some("running"));
                if migrate && runs && self.v1.records.with_untracked(|r| r.migrated_at == 0) {
                    let version = bir.and_then(|p| p.get("version")).and_then(Value::as_str).unwrap_or("1.0.0").to_owned();
                    let done = crate::vault::off_main(move || migrate_now(&version, super::migrate::Mode::Apply)).await;
                    if let Some(Ok(report)) = done {
                        log::info!("mini v1: {report}");
                    }
                    self.v1.records.set(Records::load(&data_dir()));
                }
            })
        });
    }

    /// A broker `plugin` event: the list again; the opens that wait on a
    /// restarted plugin reopen on its new generation (13.7).
    pub fn v1_plugin_event(&'static self, event: &Value) {
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
        }
        self.v1_refresh(false);
    }

    /// A question from the bridge (an app's `rox.open({url})` or
    /// `rox.attach`), asked in the sheet; `answer` hears the person.
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

    /// A `rox://app/` link of a v1 plugin (9.4): true when it was one
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
                    provider: "rox-app".to_owned(),
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
                self.v1_resolved.set(true);
                self.send();
            })
        });
        true
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
    /// popup, a token (opened as its scope's), an error as a toast.
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
                        self.open_v1_placed(OpenCard { plugin, app, title, placement, ticket, props }, Opener::User);
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
                    tokens::Answer::Token { ticket, .. } => {
                        // a scope's ticket: what the scope's resolve opens
                        if kind == "command" {
                            crate::toast::show("Resolved");
                        } else if ticket != value {
                            self.v1_resolve(plugin, "scope", name, ticket);
                        }
                    }
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
                crate::slash::show_rows(rows);
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

    /// Asks once for canonical `token`'s title (7.4) and caches it.
    fn v1_title(&'static self, token: &str) {
        if !self.v1.asked.borrow_mut().insert(token.to_owned()) {
            return;
        }
        let Some((short, _, ticket)) = tokens::canonical(token) else { return };
        let Some(p) = self.v1.by_short(short) else { return };
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
                self.v1_refresh(false);
            })
        });
    }

    /// Install: the plugin's command line in the form, inspected; its
    /// review shows next (nothing is installed yet).
    pub fn v1_inspect(&'static self) {
        let line = self.v1.form.get_untracked();
        let mut words = line.split_whitespace().map(str::to_owned);
        let Some(command) = words.next() else { return crate::toast::show("Enter the plugin's command") };
        let args: Vec<String> = words.collect();
        self.v1_job("Checking the plugin…", move || connect::inspect(&broker(), &command, &args), move |r| {
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

    /// A routine's approve, enable (in the open chat) or disable (5.9).
    pub fn v1_routine(&'static self, plugin: &str, routine: &str, action: host_api::RoutineAction) {
        let (p, r) = (plugin.to_owned(), routine.to_owned());
        let thread = self.active_id.get_untracked();
        if action == host_api::RoutineAction::Enable && thread.is_empty() {
            return crate::toast::show("Open the chat the routine should post in");
        }
        self.v1_job("", move || host_api::routine_action(&broker(), &p, &r, action, Some(thread.as_str())), |_| {});
    }

    /// Closes every open of `plugin`: the full-screen stack and the inline
    /// cards (13.4, 13.6 step 2).
    pub fn v1_close_opens(&'static self, plugin: &str) {
        let prefix = format!("v1:{plugin}/");
        self.mini_full.update(|v| v.retain(|o| !o.key.starts_with(&prefix)));
        let link = format!("rox://app/{plugin}/");
        self.v1.shown.update(|s| s.retain(|id| !id.contains(&link)));
    }

    /// Uninstall (13.6): opens close, sessions end, the broker uninstalls,
    /// then the plugin's attachments, pins (and their sidebar items) and
    /// storage go.
    pub fn v1_uninstall(&'static self, plugin: &str) {
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
            },
        );
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

/// Whether a `rox://app/` link's first segment is a reverse-DNS plugin id
/// (a v1 link even when that plugin is not listed yet).
fn url_names_plugin_id(url: &str) -> bool {
    url.strip_prefix("rox://app/")
        .and_then(|r| r.split(['/', '?']).next())
        .is_some_and(super::perms::plugin_id_ok)
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

/// Runs the v0 migration against the configured data dirs and BIR's v1
/// plugin through the broker (the dry run asks nothing).
pub fn migrate_now(version: &str, mode: super::migrate::Mode) -> Result<super::migrate::Report, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let token = crate::client::host_token_path(std::env::var("ROX_HOST_TOKEN_FILE").ok().as_deref(), &home);
    let broker_dir = token.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    migrate_dirs(&data_dir(), &broker_dir, version, mode)
}

/// [`migrate_now`] over the given dirs (a rehearsal on a copy).
pub fn migrate_dirs(
    rox_dir: &std::path::Path,
    broker_dir: &std::path::Path,
    version: &str,
    mode: super::migrate::Mode,
) -> Result<super::migrate::Report, String> {
    let b = broker();
    let mut resolve = |scope: &str, value: &str| {
        let ask = host_api::ResolveAsk {
            plugin: links::BIR.to_owned(),
            kind: "scope",
            name: scope.to_owned(),
            value: value.to_owned(),
            complete: false,
            context: json!({"attached": []}),
        };
        match host_api::resolve(&b, &ask).map(|v| tokens::answer(&v)) {
            Ok(tokens::Answer::Token { ticket, title }) => Ok((ticket, title)),
            Ok(tokens::Answer::Error(m)) => Err(m),
            Ok(_) => Err("it did not resolve to a token".to_owned()),
            Err(e) => Err(e.code),
        }
    };
    super::migrate::run(rox_dir, broker_dir, version, mode, &mut resolve)
}

/// `rox --mini-v1-migrate [--dry-run] [--rox-dir <dir>] [--broker-dir <dir>]
/// [--version <BIR's v1 version>]`: runs the v0 migration (Q11) and prints
/// its report; `None` when the arguments ask for something else (the app
/// runs). The dirs default to Rox's (`ROX_DATA_DIR`) and the broker's (the
/// host token's folder); point them at a copy to rehearse. A dry run asks
/// the plugin nothing and changes nothing.
pub fn migrate_cli(args: &[String]) -> Option<i32> {
    if !args.iter().any(|a| a == "--mini-v1-migrate") {
        return None;
    }
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let mode = if args.iter().any(|a| a == "--dry-run") { super::migrate::Mode::DryRun } else { super::migrate::Mode::Apply };
    let rox_dir = arg("--rox-dir").map(std::path::PathBuf::from).unwrap_or_else(data_dir);
    let home = std::env::var("HOME").unwrap_or_default();
    let token = crate::client::host_token_path(std::env::var("ROX_HOST_TOKEN_FILE").ok().as_deref(), &home);
    let broker_dir = arg("--broker-dir")
        .map(std::path::PathBuf::from)
        .or_else(|| token.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_default();
    let version = match (arg("--version"), mode) {
        (Some(v), _) => v,
        (None, super::migrate::Mode::DryRun) => "1.0.0".to_owned(),
        (None, super::migrate::Mode::Apply) => {
            let listed = host_api::plugins(&broker()).ok().unwrap_or_default();
            match listed.iter().find(|p| p.get("id").and_then(Value::as_str) == Some(links::BIR)) {
                Some(p) => p.get("version").and_then(Value::as_str).unwrap_or("1.0.0").to_owned(),
                None => {
                    eprintln!("{} is not installed as a v1 plugin: nothing can issue tickets.", links::BIR);
                    return Some(1);
                }
            }
        }
    };
    println!("Rox data dir: {}\nApp server data dir: {}", rox_dir.display(), broker_dir.display());
    match migrate_dirs(&rox_dir, &broker_dir, &version, mode) {
        Ok(report) => {
            print!("{report}");
            Some(0)
        }
        Err(e) => {
            eprintln!("The migration stopped: {e}");
            Some(1)
        }
    }
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

/// A tool result's card (9.6): its title, the plugin, and Open (full) or
/// Show (inline). Nothing opens or mounts before the press (INV-08). A
/// shown inline card mounts below it.
pub fn open_card(link: &crate::store::DriveLink, card_id: String) -> AnyElement {
    let Some(card) = link.card.as_ref().and_then(|c| OpenCard::from_link(&json!({"provider": "rox-app", "card": c}))) else {
        return view().into_element();
    };
    let plugin_title = store().v1.entry(&card.plugin).map(|p| p.title).unwrap_or_else(|| card.plugin.clone());
    let (c1, c2, id1, id2) = (card.clone(), card.clone(), card_id.clone(), card_id.clone());
    let shown = move || store().v1.shown.with(|s| s.contains(&id1));
    view()
        .gap_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(64.0))
                .px(px(12.0))
                .rounded(px(12.0))
                .bg(tc("#1f1f1f"))
                .child(crate::appdetails::named_tile("", &card.title, 36.0))
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
                .child(small_button(card.button(), true, move || match c1.placement {
                    Placement::Full => store().open_v1(c1.key(), &c1.title, Opener::ToolResult),
                    Placement::Inline => store().v1.shown.update(|s| {
                        if !s.remove(&id2) {
                            s.insert(id2.clone());
                        }
                    }),
                })),
        )
        .child(when(shown, move || inline_card(c2.clone()).into_element()))
        .into_element()
}

/// An inline app (6.5): a title bar with Open (full screen) over the app
/// at its declared size, clamped to the range and the column.
pub fn inline_card(card: OpenCard) -> impl IntoElement {
    let key = card.key().to_key();
    let app = store().v1.entry(&card.plugin).and_then(|p| p.app(&card.app).cloned()).unwrap_or(Value::Null);
    let (w, h) = cards::inline_size(&app, COLUMN);
    let c = card.clone();
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
                .h(px(36.0))
                .px(px(10.0))
                .bg(tc("#1f1f1f"))
                .child(text(card.title.clone()).flex_1().number_of_lines(1).text_size(px(13.0)).text_color(tc("#f5f5f5")))
                .child(small_button("Open", false, move || store().open_v1(c.key(), &c.title, Opener::User))),
        )
        .child(
            view()
                .h(px(h))
                .child(banners(&key))
                .child(crate::mini::mini_app_inline(&key, w, h))
                .child(error_overlay(&key)),
        )
}

/// The banners over an open (`key`): its notice ("This item is no longer
/// available", "reconnecting") and a stale open's Reload (12.2).
pub fn banners(key: &str) -> impl IntoElement {
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
            move || store().v1.stale.with(|s| s.contains(&k3)),
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
