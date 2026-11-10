//! Computer homes: where a Bot's new computer is made. Docker on this Mac, AWS EC2, E2B and
//! boat.dev run on the person's own keys; a remote computer is connected from the Computer tab as
//! before. There's one default home, and a Bot can have its own, which applies only to its next
//! new computer: a Bot keeps the computer it has.
//!
//! The broker holds the keys and checks them before it saves one (`/v1/host/computer/providers`).
//! What a person types into a key form ([`KeyForm`]) stays in its signals until it's sent, and is
//! cleared once it is, or when the form closes. A saved secret comes back only as whether it's
//! set and maybe its last four characters: it's never painted, logged or put in the store.
//!
//! Here: the homes as the broker lists them, the calls that change them, the one key form drawn
//! from a home's fields, Settings › Computers, and the Bot Details picker. The chat's cards are in
//! `cards/computer_setup.rs`.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use nsh_base::input;
use nsh_core::prelude::*;
use serde_json::{Value, json};

use crate::client;
use crate::live::keep;
use crate::store::spawn_detached;
use crate::theme;

/// The homes, in the broker's order, for when it hasn't said yet: (kind, label).
pub(crate) use buwiz_protocol::homes::HOMES as KINDS;

const PROVIDERS: &str = "/v1/host/computer/providers";
const NO_ANSWER: &str = "The broker didn't answer.";
const NEXT_ONLY: &str = "Applies to this Bot's next new computer";

/// One field of a home's form. A secret one never carries its value: only whether it's set,
/// and maybe its last four characters.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Field {
    pub key: String,
    pub label: String,
    pub placeholder: String,
    pub hint: String,
    pub secret: bool,
    pub required: bool,
    /// a plain field's current value; always "" for a secret
    pub value: String,
    pub set: bool,
    pub last4: String,
}

impl Field {
    fn parse(v: &Value) -> Option<Self> {
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        let secret = flag("secret");
        let key = text("key");
        (!key.is_empty()).then(|| Self {
            key,
            label: text("label"),
            placeholder: text("placeholder"),
            hint: text("hint"),
            secret,
            required: flag("required"),
            // a secret's value is never taken, whatever comes
            value: if secret { String::new() } else { text("value") },
            set: flag("set"),
            last4: if secret { text("last4") } else { String::new() },
        })
    }
}

/// A home as the broker lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Home {
    pub kind: String,
    pub label: String,
    /// this build can make computers there
    pub built: bool,
    pub configured: bool,
    pub can_default: bool,
    pub is_default: bool,
    pub fields: Vec<Field>,
    pub notes: Vec<String>,
    /// the computers on it: (id, the Bots at it)
    pub computers: Vec<(String, Vec<String>)>,
    /// the ones on it being deleted, apart: (id, where a person looks for what's left of it)
    pub leaving: Vec<(String, String)>,
    /// the ones found with their record lost and nothing of them found, apart: (id, where a person
    /// looks). Nothing deletes one until a person presses Delete
    pub lost: Vec<(String, String)>,
    /// a limited policy to copy, as JSON text; "" for none
    pub policy: String,
    /// the cloud's own page that sets the home up (AWS's Launch Stack); "" for none
    pub launch: String,
    /// what's left on this Mac of computers that are gone: Docker's volume names, listed only
    pub leftovers: Vec<String>,
}

impl Home {
    pub(crate) fn parse(v: &Value) -> Option<Self> {
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        let list = |k: &str| v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
        let kind = text("kind");
        if kind.is_empty() {
            return None;
        }
        let (mut computers, mut leaving, mut lost) = (Vec::new(), Vec::new(), Vec::new());
        for c in list("computers") {
            let Some(id) = c.get("id").and_then(Value::as_str).map(str::to_owned) else { continue };
            let look = || c.get("look").and_then(Value::as_str).unwrap_or_default().to_owned();
            if c.get("lost").and_then(Value::as_bool) == Some(true) {
                lost.push((id, look()));
            } else if c.get("leaving").and_then(Value::as_bool) == Some(true) {
                leaving.push((id, look()));
            } else {
                let bots = c.get("bots").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
                computers.push((id, bots));
            }
        }
        Some(Self {
            label: Some(text("label")).filter(|l| !l.is_empty()).unwrap_or_else(|| label_of(&kind)),
            built: v.get("built").and_then(Value::as_bool).unwrap_or(true),
            configured: flag("configured"),
            can_default: flag("canDefault"),
            is_default: flag("isDefault"),
            fields: list("fields").iter().filter_map(Field::parse).collect(),
            notes: list("notes").iter().filter_map(Value::as_str).filter(|n| !n.is_empty()).map(str::to_owned).collect(),
            computers,
            leaving,
            lost,
            policy: text("policy"),
            // only a web page: it's opened in the browser
            launch: Some(text("launch")).filter(|u| u.starts_with("https://")).unwrap_or_default(),
            leftovers: list("leftovers").iter().filter_map(|l| l.get("name")?.as_str()).map(str::to_owned).collect(),
            kind,
        })
    }

    /// The sentence naming what's left of computers that are gone; "" when nothing is.
    pub(crate) fn leftovers_words(&self) -> String {
        if self.leftovers.is_empty() {
            return String::new();
        }
        format!("Files from a computer that's gone are still on this Mac, in these Docker volumes: {}. Nothing here deletes them.", self.leftovers.join(", "))
    }

    /// Whether it takes a key of its own.
    pub(crate) fn keyed(&self) -> bool {
        self.fields.iter().any(|f| f.secret)
    }

    /// Whether a computer can be made there now: Docker needs no key.
    pub(crate) fn ready(&self) -> bool {
        self.built && (self.kind == "docker" || self.configured)
    }

    /// "Key set ••x9Qa", "Key set" or "No key"; `None` for a home with no key.
    pub(crate) fn key_words(&self) -> Option<String> {
        if !self.keyed() {
            return None;
        }
        let set: Vec<&Field> = self.fields.iter().filter(|f| f.secret && f.set).collect();
        Some(match set.iter().find(|f| !f.last4.is_empty()) {
            Some(f) => format!("Key set ••{}", f.last4),
            None if !set.is_empty() => "Key set".to_owned(),
            None => "No key".to_owned(),
        })
    }

    /// "Computers here: cmp_ab12 (Ann)"; "" for none.
    pub(crate) fn computers_words(&self) -> String {
        if self.computers.is_empty() {
            return String::new();
        }
        let each: Vec<String> = self.computers.iter().map(|(id, bots)| if bots.is_empty() { id.clone() } else { format!("{id} ({})", bots.join(", ")) }).collect();
        format!("Computers here: {}", each.join(", "))
    }

    /// Where it stands, in a word or two, for a list.
    pub(crate) fn state_words(&self) -> &'static str {
        match () {
            _ if !self.built => "Not available yet",
            _ if self.kind == "remote" => "Connect one in the Computer tab",
            _ if self.ready() => "Ready",
            _ => "Needs a key",
        }
    }
}

/// Where a new computer would be made: the default, or one Bot's. `source` is `saved`, `env` or
/// `builtin` for the default, `bot` or `default` for a Bot's.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Next {
    pub kind: String,
    pub label: String,
    pub source: String,
}

impl Next {
    pub(crate) fn parse(v: &Value) -> Option<Self> {
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        let kind = text("kind");
        (!kind.is_empty()).then(|| Self { label: Some(text("label")).filter(|l| !l.is_empty()).unwrap_or_else(|| label_of(&kind)), source: text("source"), kind })
    }
}

/// `GET /v1/host/computer/providers`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Homes {
    pub default: Next,
    pub list: Vec<Home>,
}

impl Homes {
    pub(crate) fn parse(v: &Value) -> Self {
        let list = v.get("providers").and_then(Value::as_array).into_iter().flatten().filter_map(Home::parse).collect();
        let default = v.get("default").and_then(Next::parse).unwrap_or_else(|| Next { kind: "docker".to_owned(), label: label_of("docker"), source: "builtin".to_owned() });
        Self { default, list }
    }

    pub(crate) fn get(&self, kind: &str) -> Option<&Home> {
        self.list.iter().find(|h| h.kind == kind)
    }

    /// The home a person named: its kind or its label, ignoring case.
    pub(crate) fn named(&self, word: &str) -> Option<&Home> {
        let w = word.trim().to_lowercase();
        self.list.iter().find(|h| h.kind == w || h.label.to_lowercase() == w)
    }

    /// The homes a default can be: Docker, and any other ready one that may.
    pub(crate) fn default_choices(&self) -> Vec<&Home> {
        self.list.iter().filter(|h| h.can_default && h.ready()).collect()
    }

    /// The homes one Bot's next computer can be made on.
    pub(crate) fn bot_choices(&self) -> Vec<&Home> {
        self.list.iter().filter(|h| h.kind != "remote" && h.ready()).collect()
    }

    fn replace(&mut self, mut home: Home) {
        match self.list.iter_mut().find(|h| h.kind == home.kind) {
            Some(h) => {
                // only the full list looks for leftovers: one home's reply doesn't say they went
                if home.leftovers.is_empty() {
                    home.leftovers = std::mem::take(&mut h.leftovers);
                }
                *h = home;
            }
            None => self.list.push(home),
        }
    }
}

/// A home's label when the broker hasn't said it.
pub(crate) fn label_of(kind: &str) -> String {
    KINDS.iter().find(|(k, _)| *k == kind).map(|(_, l)| (*l).to_owned()).unwrap_or_else(|| kind.to_owned())
}

/// The homes before the broker has said: the five kinds, none ready but Docker.
pub(crate) fn fallback() -> Homes {
    let list = KINDS.iter().map(|(k, l)| Home { kind: (*k).to_owned(), label: (*l).to_owned(), built: true, can_default: *k != "remote", ..Home::default() }).collect();
    Homes { default: Next { kind: "docker".to_owned(), label: label_of("docker"), source: "builtin".to_owned() }, list }
}

thread_local! {
    static HOMES: Signal<Option<Homes>> = Owner::detached().run(|| keep("homes.list", || None));
    /// What the last press on a home said, by kind (and `default`): (it worked, the sentence).
    static NOTES: Signal<HashMap<String, (bool, String)>> = Owner::detached().run(|| keep("homes.notes", HashMap::new));
    /// The Settings page's form.
    static PAGE_FORM: KeyForm = Owner::detached().run(KeyForm::new);
    /// Each Bot's next home, as its computer's status last said.
    static BOT_HOME: Signal<HashMap<String, Next>> = Owner::detached().run(|| keep("homes.bot", HashMap::new));
    /// The Bot Details picker: open, and what its last press said.
    static PICKER: (Signal<bool>, Signal<String>) = Owner::detached().run(|| (signal(false), signal(String::new())));
    /// When a view last asked the broker for each thing it shows: a view built again soon after
    /// doesn't ask again
    static ASKED: std::cell::RefCell<HashMap<String, std::time::Instant>> = std::cell::RefCell::new(HashMap::new());
}

/// How soon a view that's built again may ask the broker again. A press always asks.
const ASK_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// Whether a view may ask the broker for `what` now; if so, that's noted.
fn due(what: &str) -> bool {
    ASKED.with(|a| {
        let mut a = a.borrow_mut();
        let now = std::time::Instant::now();
        let due = a.get(what).is_none_or(|t| now.duration_since(*t) >= ASK_EVERY);
        if due {
            a.insert(what.to_owned(), now);
        }
        due
    })
}

/// The homes as last heard (tracked); `None` before the broker has said.
pub(crate) fn homes() -> Option<Homes> {
    HOMES.with(|s| s.get())
}

/// Whether the broker has listed the homes yet (untracked).
pub(crate) fn known() -> bool {
    HOMES.with(|s| s.with_untracked(Option::is_some))
}

/// The homes as last heard, else [`fallback`] (untracked).
pub(crate) fn homes_now() -> Homes {
    HOMES.with(|s| s.get_untracked()).unwrap_or_else(fallback)
}

fn set_homes(f: impl FnOnce(&mut Homes)) {
    HOMES.with(|s| {
        s.update(|h| {
            let mut now = h.take().unwrap_or_else(fallback);
            f(&mut now);
            *h = Some(now);
        })
    });
}

fn set_note(kind: &str, ok: bool, words: String) {
    NOTES.with(|s| {
        s.update(|m| {
            m.insert(kind.to_owned(), (ok, words));
        })
    });
}

fn clear_note(kind: &str) {
    NOTES.with(|s| {
        s.update(|m| {
            m.remove(kind);
        })
    });
}

/// What the broker said to a change: yes with its answer, or its sentence.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Said {
    Yes(Value),
    No(String),
}

/// A reply as [`Said`]: a 2xx not saying `ok: false` is a yes; anything else shows its
/// `message` as it is.
pub(crate) fn said(reply: Option<(u16, Value)>) -> Said {
    let Some((status, v)) = reply else { return Said::No(NO_ANSWER.to_owned()) };
    if (200..300).contains(&status) && v.get("ok").and_then(Value::as_bool) != Some(false) {
        return Said::Yes(v);
    }
    Said::No(v.get("message").and_then(Value::as_str).filter(|m| !m.is_empty()).unwrap_or("The broker said no.").to_owned())
}

async fn ask(method: &str, path: &str, body: Option<Value>) -> Said {
    said(client::send_status(method, path, body).await)
}

fn provider_path(kind: &str, rest: &str) -> String {
    format!("{PROVIDERS}/{}{rest}", client::urlenc(kind))
}

fn bot_path(bot: &str) -> String {
    format!("/v1/host/computer/bot/{}", client::urlenc(bot))
}

/// Asks the broker for the homes; whether it answered.
pub(crate) async fn load() -> bool {
    let Some(v) = client::get_any(PROVIDERS).await.filter(|v| v.get("providers").is_some()) else { return false };
    let homes = Homes::parse(&v);
    HOMES.with(|s| s.set(Some(homes)));
    true
}

/// [`load`], in the background, unless a view asked a moment ago.
pub(crate) fn refresh() {
    if !due("providers") {
        return;
    }
    spawn_detached(async {
        load().await;
    });
}

/// The homes, asked for first if they never were.
pub(crate) async fn loaded() -> Homes {
    if !known() {
        load().await;
    }
    homes_now()
}

/// Saves a home's key: `PUT providers/{kind}`. The broker checks it first, and a yes carries the
/// home as it is now.
pub(crate) async fn save(kind: &str, body: Value) -> Said {
    let reply = ask("PUT", &provider_path(kind, ""), Some(body)).await;
    if let Said::Yes(v) = &reply
        && let Some(home) = v.get("provider").and_then(Home::parse)
    {
        set_homes(|h| h.replace(home));
    }
    reply
}

/// Checks a home's saved key: `POST providers/{kind}/check`. Ok is the sentence to show, Err why not.
pub(crate) async fn check(kind: &str) -> Result<String, String> {
    match ask("POST", &provider_path(kind, "/check"), None).await {
        Said::Yes(v) => Ok(v.get("message").and_then(Value::as_str).unwrap_or("It works.").to_owned()),
        Said::No(why) => Err(why),
    }
}

/// Forgets a home's key: `POST providers/{kind}/remove`. The broker refuses while computers are on it.
pub(crate) async fn remove(kind: &str) -> Said {
    let reply = ask("POST", &provider_path(kind, "/remove"), None).await;
    if let Said::Yes(v) = &reply
        && let Some(home) = v.get("provider").and_then(Home::parse)
    {
        set_homes(|h| h.replace(home));
    }
    reply
}

/// Forgets computer `id`, being deleted, once a person checked nothing of it is left:
/// `POST providers/{kind}/forget`. The broker tries the keys in use once more first.
pub(crate) async fn forget(kind: &str, id: &str) -> Said {
    let reply = ask("POST", &provider_path(kind, "/forget"), Some(json!({"id": id}))).await;
    if let Said::Yes(v) = &reply
        && let Some(home) = v.get("provider").and_then(Home::parse)
    {
        set_homes(|h| h.replace(home));
    }
    reply
}

/// Keeps computer `id`, found with its record lost: `POST providers/{kind}/keep`. The broker looks
/// for it once more; found, it's a computer again with no Bot at it. Ok is the sentence to show.
pub(crate) async fn keep_lost(kind: &str, id: &str) -> Result<String, String> {
    match ask("POST", &provider_path(kind, "/keep"), Some(json!({"id": id}))).await {
        Said::Yes(v) => {
            if let Some(home) = v.get("provider").and_then(Home::parse) {
                set_homes(|h| h.replace(home));
            }
            Ok(v.get("message").and_then(Value::as_str).map_or_else(|| format!("{id} is kept."), str::to_owned))
        }
        Said::No(why) => Err(why),
    }
}

/// Deletes computer `id`, found with its record lost, on its platform: `POST providers/{kind}/delete`.
pub(crate) async fn delete_lost(kind: &str, id: &str) -> Said {
    let reply = ask("POST", &provider_path(kind, "/delete"), Some(json!({"id": id}))).await;
    if let Said::Yes(v) = &reply
        && let Some(home) = v.get("provider").and_then(Home::parse)
    {
        set_homes(|h| h.replace(home));
    }
    reply
}

/// Makes `kind` the default home: `PUT providers/default`.
pub(crate) async fn set_default(kind: &str) -> Said {
    let reply = ask("PUT", &provider_path("default", ""), Some(json!({"kind": kind}))).await;
    if let Said::Yes(v) = &reply {
        let next = v.get("default").and_then(Next::parse);
        let kind = next.as_ref().map(|n| n.kind.clone()).unwrap_or_else(|| kind.to_owned());
        set_homes(|h| {
            if let Some(n) = next {
                h.default = n;
            }
            for home in &mut h.list {
                home.is_default = home.kind == kind;
            }
        });
    }
    reply
}

/// Where `bot`'s next new computer is made: `kind`, or the default for `None`. A yes carries
/// the Bot's computer status.
pub(crate) async fn set_for_bot(bot: &str, kind: Option<&str>) -> Said {
    let reply = ask("PUT", &format!("{}/provider", bot_path(bot)), Some(json!({"kind": kind}))).await;
    if let Said::Yes(v) = &reply
        && let Some(next) = v.get("home").and_then(Next::parse)
    {
        BOT_HOME.with(|s| {
            s.update(|m| {
                m.insert(bot.to_owned(), next);
            })
        });
    }
    reply
}

/// `bot`'s next home as its computer's status says; `None` when the broker didn't.
pub(crate) async fn bot_home(bot: &str) -> Option<Next> {
    let next = client::get_any(&bot_path(bot)).await.and_then(|v| v.get("home").and_then(Next::parse))?;
    BOT_HOME.with(|s| {
        s.update(|m| {
            m.insert(bot.to_owned(), next.clone());
        })
    });
    Some(next)
}

/// What a Bot's next home reads as: its own home's label, or "Default (…)".
pub(crate) fn next_words(next: &Next, default_label: &str) -> String {
    if next.source == "bot" { next.label.clone() } else { format!("Default ({})", if default_label.is_empty() { &next.label } else { default_label }) }
}

// ---------------------------------------------------------------------------
// The key form
// ---------------------------------------------------------------------------

/// A key form's state. What's typed is in `typed` while the form is open, and nowhere else.
#[derive(Clone, Copy)]
pub(crate) struct KeyForm {
    /// the home it's open for; "" when closed
    pub kind: Signal<String>,
    typed: Signal<BTreeMap<String, String>>,
    pub note: Signal<String>,
    busy: Signal<bool>,
    /// how many times it opened or closed: an answer for an older one is no answer
    turn: Signal<u64>,
}

impl KeyForm {
    pub(crate) fn new() -> Self {
        Self { kind: signal(String::new()), typed: signal(BTreeMap::new()), note: signal(String::new()), busy: signal(false), turn: signal(0) }
    }

    pub(crate) fn open(&self, kind: &str) {
        self.close();
        self.kind.set(kind.to_owned());
    }

    /// Closes it, and no secret stays.
    pub(crate) fn close(&self) {
        self.turn.update(|n| *n += 1);
        self.typed.set(BTreeMap::new());
        self.note.set(String::new());
        self.busy.set(false);
        self.kind.set(String::new());
    }

    /// Whether anything typed is still held.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.typed.with(BTreeMap::is_empty)
    }

    fn typed(&self, key: &str, text: &str) {
        self.typed.update(|m| {
            m.insert(key.to_owned(), text.to_owned());
        });
    }

    /// The PUT's body for `home`: each plain field as it reads now, each secret typed into (a
    /// blank one keeps the saved one), and in `clear` the plain fields emptied.
    pub(crate) fn body(&self, home: &Home) -> Value {
        let typed = self.typed.get_untracked();
        let mut values = serde_json::Map::new();
        let mut clear = Vec::new();
        for f in &home.fields {
            let now = typed.get(&f.key);
            if f.secret {
                if let Some(t) = now.filter(|t| !t.trim().is_empty()) {
                    values.insert(f.key.clone(), json!(t));
                }
                continue;
            }
            let text = now.cloned().unwrap_or_else(|| f.value.clone());
            if !text.trim().is_empty() {
                values.insert(f.key.clone(), json!(text.trim()));
            } else if !f.value.is_empty() {
                clear.push(f.key.clone());
            }
        }
        json!({"values": values, "clear": clear})
    }

    /// Sends what's typed for `home`. A yes clears the form and goes to `then`; a no keeps it
    /// open with the broker's sentence.
    pub(crate) fn submit(&self, home: &Home, then: Rc<dyn Fn()>) {
        self.submit_clearing(home, &[], then);
    }

    /// [`KeyForm::submit`], emptying the saved `fields` too: how a saved key is let go on purpose,
    /// since a blank box keeps it.
    pub(crate) fn submit_clearing(&self, home: &Home, fields: &[&str], then: Rc<dyn Fn()>) {
        let f = *self;
        if f.busy.get_untracked() {
            return;
        }
        let kind = home.kind.clone();
        let mut body = f.body(home);
        if let Some(clear) = body["clear"].as_array_mut() {
            clear.extend(fields.iter().map(|k| json!(k)));
        }
        let turn = f.turn.get_untracked();
        f.note.set(String::new());
        f.busy.set(true);
        spawn_detached(async move {
            let reply = save(&kind, body).await;
            if f.turn.get_untracked() != turn {
                return;
            }
            match reply {
                Said::Yes(_) => {
                    f.close();
                    then();
                }
                Said::No(why) => {
                    f.busy.set(false);
                    f.note.set(why);
                }
            }
        });
    }
}

/// The page's form, which the slash command can open.
pub(crate) fn page_form() -> KeyForm {
    PAGE_FORM.with(|f| *f)
}

/// A small rounded button: `words` on it, `label` for a screen reader. The Computer tab uses it too.
pub(crate) fn small_button(label: impl Into<String>, words: impl Into<String>, press: impl Fn() + 'static) -> View {
    let (label, words) = (label.into(), words.into());
    view()
        .label(label)
        .h(px(28.0))
        .px(px(12.0))
        .rounded(px(14.0))
        .items_center()
        .justify_center()
        .bg(theme::HOVER)
        .child(text(words).text_size(px(12.0)).text_color(theme::TEXT))
        .on_press(press)
}

/// A muted 12-point line, for a page or a tab. A card's line is `cards::note`, which is a little brighter.
pub(crate) fn line(words: String) -> Text {
    text(words).text_size(px(12.0)).line_height(px(16.0)).text_color(theme::MUTED)
}

/// One field: its label, the box, and its hint. A secret's box is masked, never filled from
/// the broker, and shows dots when one is saved.
fn field_row(form: KeyForm, f: &Field) -> impl IntoElement {
    let typed = form.typed;
    let (k1, k2) = (f.key.clone(), f.key.clone());
    let stored = f.value.clone();
    let placeholder = if f.secret && f.set { "••••••••".to_owned() } else { f.placeholder.clone() };
    let title = if f.required { f.label.clone() } else { format!("{} (optional)", f.label) };
    view()
        .gap(px(4.0))
        .child(text(title).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT))
        .child(
            view().h(px(32.0)).rounded(px(8.0)).bg(theme::HOVER).px(px(10.0)).justify_center().child(
                input()
                    .test_id(&format!("home-field-{}", f.key))
                    .label(f.label.clone())
                    .value(move || typed.with(|m| m.get(&k1).cloned()).unwrap_or_else(|| stored.clone()))
                    .placeholder(placeholder)
                    .placeholder_color(|| color(theme::GHOST))
                    .masked(f.secret)
                    .text_size(px(12.0))
                    .text_color(theme::TEXT)
                    .h(px(32.0))
                    .on_change(move |t: &str| form.typed(&k2, t)),
            ),
        )
        .when(!f.hint.is_empty(), |v| v.child(line(f.hint.clone())))
}

/// The form for `home`, drawn from its fields: Save checks the key and saves it, and `saved`
/// follows a yes. Cancel closes it, and `cancelled` follows.
pub(crate) fn key_form(form: KeyForm, home: Home, saved: Rc<dyn Fn()>, cancelled: Rc<dyn Fn()>) -> impl IntoElement {
    let mut col = view().test_id(&format!("home-form-{}", home.kind)).gap_2().pt(px(6.0));
    if home.fields.iter().any(|f| f.secret && f.set) {
        col = col.child(line("A saved key shows as dots. Leave it blank to keep it.".to_owned()));
    }
    for f in &home.fields {
        col = col.child(field_row(form, f));
    }
    let busy = form.busy;
    let mut buttons = view().flex_row().flex_wrap().gap_2().pt(px(4.0));
    let h = home.clone();
    buttons = buttons.child(
        view()
            .label(format!("Save the key for {}", home.label))
            .h(px(28.0))
            .px(px(12.0))
            .rounded(px(14.0))
            .items_center()
            .justify_center()
            .bg(theme::TEXT)
            .child(text(move || if busy.get() { "Checking…".to_owned() } else { "Check and save".to_owned() }).text_size(px(12.0)).text_color(theme::MAIN))
            .on_press({
                let saved = saved.clone();
                move || form.submit(&h, saved.clone())
            }),
    );
    // an AWS home with a pasted key: the way to sign with a profile instead, said as it is
    if home.kind == "ec2" && home.fields.iter().any(|f| f.secret && f.set) {
        let (h, saved) = (home.clone(), saved.clone());
        buttons = buttons.child(small_button("Use the AWS profile instead of the saved key".to_owned(), "Use the profile instead".to_owned(), move || {
            form.submit_clearing(&h, &["access_key_id", "secret_access_key"], saved.clone());
        }));
    }
    if !home.launch.is_empty() {
        let launch = home.launch.clone();
        buttons = buttons.child(small_button("Launch the AWS stack".to_owned(), "Launch the AWS stack".to_owned(), move || {
            crate::live::store().open_link(&launch);
        }));
    }
    if !home.policy.is_empty() {
        let policy = home.policy.clone();
        buttons = buttons.child(small_button("Copy the limited policy".to_owned(), "Copy the limited policy".to_owned(), move || {
            crate::live::store().copy_text(&policy);
            crate::toast::show("Copied the limited policy. Put your 12-digit AWS account number where it says <ACCOUNT_ID>.");
        }));
    }
    buttons = buttons.child(small_button(format!("Cancel the key for {}", home.label), "Cancel".to_owned(), move || {
        form.close();
        cancelled();
    }));
    col.child(view().test_id("home-form-note").child(text(move || form.note.get()).number_of_lines(6).text_size(px(12.0)).line_height(px(16.0)).text_color(theme::DOWN))).child(buttons)
}

// ---------------------------------------------------------------------------
// Settings › Computers
// ---------------------------------------------------------------------------

/// Opens Settings on Computers; with `kind`, its key form open.
pub(crate) fn open_page(kind: Option<&str>) {
    let st = crate::live::store();
    st.settings_page.set("computers".to_owned());
    st.settings_open.set(true);
    if let Some(kind) = kind {
        page_form().open(kind);
    }
}

fn run_check(kind: String) {
    set_note(&kind, true, "Checking…".to_owned());
    spawn_detached(async move {
        match check(&kind).await {
            Ok(words) => set_note(&kind, true, words),
            Err(why) => set_note(&kind, false, why),
        }
    });
}

fn run_remove(kind: String) {
    clear_note(&kind);
    spawn_detached(async move {
        match remove(&kind).await {
            Said::Yes(_) => set_note(&kind, true, "The key is removed.".to_owned()),
            Said::No(why) => set_note(&kind, false, why),
        }
    });
}

fn run_forget(kind: String, id: String) {
    clear_note(&kind);
    spawn_detached(async move {
        match forget(&kind, &id).await {
            Said::Yes(_) => set_note(&kind, true, format!("{id} is forgotten.")),
            Said::No(why) => set_note(&kind, false, why),
        }
    });
}

fn run_keep(kind: String, id: String) {
    clear_note(&kind);
    spawn_detached(async move {
        match keep_lost(&kind, &id).await {
            Ok(words) => set_note(&kind, true, words),
            Err(why) => set_note(&kind, false, why),
        }
    });
}

fn run_delete_lost(kind: String, id: String) {
    clear_note(&kind);
    spawn_detached(async move {
        match delete_lost(&kind, &id).await {
            Said::Yes(_) => set_note(&kind, true, format!("{id} is deleted.")),
            Said::No(why) => set_note(&kind, false, why),
        }
    });
}

fn run_default(kind: String) {
    clear_note("default");
    spawn_detached(async move {
        if let Said::No(why) = set_default(&kind).await {
            set_note("default", false, why);
        }
    });
}

fn note_of(kind: &str) -> impl IntoElement {
    let kind = kind.to_owned();
    dynamic(move || {
        let Some((ok, words)) = NOTES.with(|s| s.with(|m| m.get(&kind).cloned())) else { return view().into_element() };
        view().test_id(&format!("home-note-{kind}")).child(text(words).number_of_lines(6).text_size(px(12.0)).line_height(px(16.0)).text_color(if ok { theme::MUTED } else { theme::DOWN })).into_element()
    })
}

/// The default picker: what new computers are made on, and the homes it can be.
fn default_part(h: &Homes) -> impl IntoElement {
    let from = match h.default.source.as_str() {
        "env" => " (set by this Mac's environment)",
        "builtin" => " (the built-in choice)",
        _ => "",
    };
    let mut choices = view().flex_row().flex_wrap().gap_2();
    for home in h.default_choices() {
        let (kind, on) = (home.kind.clone(), home.kind == h.default.kind);
        choices = choices.child(
            view()
                .label(format!("Make {} the default", home.label))
                .h(px(28.0))
                .px(px(12.0))
                .rounded(px(14.0))
                .items_center()
                .justify_center()
                .bg(if on { theme::ACTIVE } else { theme::SUBTLE })
                .border_1()
                .border_color(if on { theme::ACCENT } else { theme::BORDER_F })
                .child(text(if on { format!("{} ✓", home.label) } else { home.label.clone() }).text_size(px(12.0)).text_color(theme::TEXT))
                .on_press(move || {
                    if !on {
                        run_default(kind.clone());
                    }
                }),
        );
    }
    view()
        .test_id("homes-default")
        .gap_2()
        .p_3()
        .rounded(px(10.0))
        .bg(theme::SUBTLE)
        .child(text(format!("New computers are made on {}{from}.", h.default.label)).text_size(px(13.0)).text_color(theme::TEXT))
        .child(line("A Bot can have its own in its Details. Either applies to its next new computer: a Bot keeps the one it has.".to_owned()))
        .child(choices)
        .child(note_of("default"))
}

/// One home on the page: its name and key, its notes and computers, and what can be done.
fn home_part(home: &Home, form: KeyForm) -> impl IntoElement {
    let mut col = view().test_id(&format!("home-{}", home.kind)).gap(px(6.0)).p_3().rounded(px(10.0)).bg(theme::SUBTLE);
    let mut head = view().flex_row().items_center().gap_2().child(text(home.label.clone()).text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT));
    if let Some(words) = home.key_words() {
        head = head.child(text(words).text_size(px(12.0)).text_color(theme::MUTED));
    }
    if home.is_default {
        head = head.child(text("Default").text_size(px(11.0)).text_color(theme::ACCENT));
    }
    col = col.child(head);
    if !home.built {
        col = col.child(line("Not available in this build yet.".to_owned()));
    }
    for n in &home.notes {
        col = col.child(line(n.clone()));
    }
    let on = home.computers_words();
    if !on.is_empty() {
        col = col.child(line(on));
    }
    let left = home.leftovers_words();
    if !left.is_empty() {
        col = col.child(view().test_id(&format!("home-{}-leftovers", home.kind)).child(line(left)));
    }
    for (id, look) in &home.leaving {
        let mut words = format!("{id} is being deleted. If it stays listed, the keys in use can't reach it, and it may still be there and costing.");
        if !look.is_empty() {
            words.push_str(&format!(" {look}, then press Forget {id}."));
        }
        let (kind, id) = (home.kind.clone(), id.clone());
        col = col.child(line(words)).child(view().flex_row().child(small_button(format!("Forget {id} on {}", home.label), format!("Forget {id}"), move || run_forget(kind.clone(), id.clone()))));
    }
    for (id, look) in &home.lost {
        let mut words = format!("{id} was found with its record lost, and couldn't be reached. It may still be there, with its files, and costing. Its Bots moved to a new computer. Keep looks for it again; Delete deletes it there for good. Nothing happens to it until you choose.");
        if !look.is_empty() {
            words.push_str(&format!(" To check by hand: {look}."));
        }
        let (keep_kind, keep_id, delete_kind, delete_id) = (home.kind.clone(), id.clone(), home.kind.clone(), id.clone());
        let buttons = view()
            .flex_row()
            .gap_2()
            .child(small_button(format!("Keep {id} on {}", home.label), "Keep".to_owned(), move || run_keep(keep_kind.clone(), keep_id.clone())))
            .child(small_button(format!("Delete {id} on {}", home.label), "Delete".to_owned(), move || run_delete_lost(delete_kind.clone(), delete_id.clone())));
        col = col.child(line(words)).child(buttons);
    }
    if home.built && home.kind != "remote" {
        let mut buttons = view().flex_row().flex_wrap().gap_2().pt(px(4.0));
        if !home.fields.is_empty() {
            let kind = home.kind.clone();
            let words = if home.fields.iter().any(|f| f.secret && f.set) { "Change key" } else { "Add key" };
            buttons = buttons.child(small_button(format!("{words} for {}", home.label), words.to_owned(), move || {
                clear_note(&kind);
                form.open(&kind);
            }));
        }
        if home.ready() {
            let kind = home.kind.clone();
            buttons = buttons.child(small_button(format!("Check {}", home.label), "Check".to_owned(), move || run_check(kind.clone())));
        }
        if home.fields.iter().any(|f| f.secret && f.set) {
            let kind = home.kind.clone();
            buttons = buttons.child(small_button(format!("Remove the key for {}", home.label), "Remove key".to_owned(), move || run_remove(kind.clone())));
        }
        col = col.child(buttons);
    }
    col = col.child(note_of(&home.kind));
    if form.kind.get() == home.kind {
        let kind = home.kind.clone();
        let saved: Rc<dyn Fn()> = Rc::new(move || set_note(&kind, true, "Saved. The key works.".to_owned()));
        col = col.child(key_form(form, home.clone(), saved, Rc::new(|| {})));
    }
    col
}

/// Settings › Computers: every home, the default, and each one's key.
pub(crate) fn settings_page() -> impl IntoElement {
    refresh();
    let form = page_form();
    view().flex_1().h_full().min_w(px(0.0)).child(
        scroll().flex_1().child(
            view()
                .test_id("homes-page")
                .p_4()
                .gap_3()
                .child(text("Computers").text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT))
                .child(line("Where a Bot's new computer is made. Keys stay with the broker on this Mac, and a saved key never shows again.".to_owned()))
                .child(dynamic(move || {
                    let Some(h) = homes() else { return line("Asking the broker…".to_owned()).into_element() };
                    let mut col = view().gap_3().child(default_part(&h));
                    for home in &h.list {
                        col = col.child(home_part(home, form));
                    }
                    col.into_element()
                })),
        ),
    )
}

// ---------------------------------------------------------------------------
// Bot Details › Computer
// ---------------------------------------------------------------------------

/// Bot Details' Computer line: where this Bot's next new computer is made, and a picker.
/// `edit` false: Bot Details is open to read, so the line only says where, with no picker.
pub(crate) fn bot_row(bot: &str, edit: bool) -> impl IntoElement {
    let (open, note) = PICKER.with(|p| *p);
    let b = bot.to_owned();
    let (list, home) = (due("providers"), due(&format!("bot:{bot}")));
    if list || home {
        spawn_detached(async move {
            if list {
                load().await;
            }
            if home {
                bot_home(&b).await;
            }
        });
    }
    let bot = bot.to_owned();
    dynamic(move || {
        let h = homes().unwrap_or_else(fallback);
        let next = BOT_HOME.with(|s| s.with(|m| m.get(&bot).cloned()));
        let words = next.as_ref().map(|n| next_words(n, &h.default.label)).unwrap_or_else(|| format!("Default ({})", h.default.label));
        let mut col = view().test_id("bot-computer").child(
            view()
                .label("Computer")
                .flex_row()
                .items_center()
                .gap_2()
                .px(px(16.0))
                .py(px(12.0))
                .child(text("Computer").text_size(px(15.0)).text_color(theme::TEXT).flex_1())
                .child(text(if edit { format!("{words} ▾") } else { words }).number_of_lines(1).text_size(px(14.0)).text_color(theme::DIM))
                .on_press(move || {
                    if edit {
                        open.update(|o| *o = !*o);
                    }
                }),
        );
        if edit && open.get() {
            let on = next.as_ref().filter(|n| n.source == "bot").map(|n| n.kind.clone());
            let mut picks = view().px(px(16.0)).pb(px(12.0)).gap(px(4.0)).child(bot_choice(&bot, format!("Default ({})", h.default.label), None, on.is_none()));
            for home in h.bot_choices() {
                picks = picks.child(bot_choice(&bot, home.label.clone(), Some(home.kind.clone()), on.as_deref() == Some(home.kind.as_str())));
            }
            col = col.child(picks);
        }
        col.child(text(NEXT_ONLY).text_size(px(12.0)).text_color(theme::MUTED).px(px(16.0)).pb(px(10.0)))
            .child(when(move || !note.with(String::is_empty), move || text(move || note.get()).number_of_lines(4).text_size(px(12.0)).text_color(theme::DOWN).px(px(16.0)).pb(px(10.0)).into_element()))
            .into_element()
    })
}

/// Bot Details opened: its picker starts shut, with nothing said.
pub(crate) fn reset_picker() {
    let (open, note) = PICKER.with(|p| *p);
    open.set(false);
    note.set(String::new());
}

/// One row of the picker: `kind` `None` follows the default.
fn bot_choice(bot: &str, label: String, kind: Option<String>, chosen: bool) -> impl IntoElement {
    let (open, note) = PICKER.with(|p| *p);
    let bot = bot.to_owned();
    view()
        .label(label.clone())
        .flex_row()
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .bg(if chosen { theme::ACTIVE } else { AdaptiveColor::TRANSPARENT })
        .child(text(if chosen { format!("{label} ✓") } else { label }).text_size(px(14.0)).text_color(theme::TEXT))
        .on_press(move || {
            let (bot, kind) = (bot.clone(), kind.clone());
            note.set(String::new());
            spawn_detached(async move {
                match set_for_bot(&bot, kind.as_deref()).await {
                    Said::Yes(_) => open.set(false),
                    Said::No(why) => note.set(why),
                }
            });
        })
}

// ---------------------------------------------------------------------------
// `/computer`
// ---------------------------------------------------------------------------

/// The sub-commands, with what each does.
const SUBS: [(&str, &str); 5] = [
    ("setup", "Enter a home's key"),
    ("use", "Where this Bot's next new computer is made"),
    ("default", "Where new computers are made"),
    ("check", "Check a home's key"),
    ("status", "This Bot's computer, and where its next one is made"),
];

const USAGE: &str = "/computer setup <home> | use <home> | default <home> | check [<home>] | status";

/// What follows `/computer`, read whole against the homes there are.
#[derive(Debug, PartialEq)]
pub(crate) enum Line {
    Page,
    Status,
    /// a card: pick a home, or a home's key
    Setup(Option<String>),
    /// this Bot's next home; `None` follows the default
    Use(Option<String>),
    Default(String),
    /// "" checks the default home
    Check(String),
    Say(String),
}

fn unknown(word: &str, h: &Homes) -> Line {
    let kinds: Vec<&str> = h.list.iter().map(|h| h.kind.as_str()).collect();
    Line::Say(format!("No home is called {word}. The homes: {}.", kinds.join(", ")))
}

pub(crate) fn parse(rest: &str, h: &Homes) -> Line {
    let line = rest.trim();
    let (sub, arg) = line.split_once(char::is_whitespace).map(|(s, a)| (s, a.trim())).unwrap_or((line, ""));
    let home = |arg: &str| h.named(arg).map(|h| h.kind.clone());
    match (sub.to_lowercase().as_str(), arg) {
        ("", _) => Line::Page,
        ("status", "") => Line::Status,
        ("setup", "") => Line::Setup(None),
        ("check", "") => Line::Check(String::new()),
        ("use", "") => Line::Say("Use which? /computer use <home>, or /computer use default".to_owned()),
        ("default", "") => Line::Say("Which home? /computer default <home>".to_owned()),
        ("use", a) if a.eq_ignore_ascii_case("default") => Line::Use(None),
        ("setup" | "use" | "default" | "check", a) => match (sub.to_lowercase().as_str(), home(a)) {
            (_, None) => unknown(a, h),
            ("setup", k) => Line::Setup(k),
            ("use", k) => Line::Use(k),
            ("check", Some(k)) if k == "remote" => Line::Say("A remote computer is checked when it's connected, in the Computer tab.".to_owned()),
            ("check", Some(k)) => Line::Check(k),
            (_, Some(k)) => match h.get(&k) {
                Some(home) if !home.can_default => Line::Say(format!("{} can't be the default.", home.label)),
                _ => Line::Default(k),
            },
        },
        _ => Line::Say(USAGE.to_owned()),
    }
}

/// A row that fills the box with `fill`.
fn fill_row(fill: String, label: &str, detail: &str) -> crate::slash::PopRow {
    crate::slash::PopRow { pick: crate::slash::Pick::Fill(fill), label: label.to_owned(), hint: String::new(), detail: detail.to_owned(), bot: String::new() }
}

/// The rows past `/computer ` for `rest`, with the homes in `h`.
pub(crate) fn rows(rest: &str, h: &Homes, in_bot: bool) -> (Vec<crate::slash::PopRow>, String) {
    let line = rest.trim_start();
    let starts = |word: &str, q: &str| word.to_lowercase().starts_with(&q.to_lowercase());
    let Some((sub, q)) = line.split_once(char::is_whitespace) else {
        let rows = SUBS.iter().filter(|(s, _)| starts(s, line)).map(|(s, d)| fill_row(if *s == "status" { "/computer status".to_owned() } else { format!("/computer {s} ") }, s, d)).collect();
        return (rows, String::new());
    };
    let sub = sub.to_lowercase();
    let q = q.trim_start();
    let mut out = Vec::new();
    if sub == "use" && starts("default", q) {
        out.push(fill_row("/computer use default".to_owned(), "default", &format!("Follow the default: {}", h.default.label)));
    }
    let fits = |home: &Home| match sub.as_str() {
        "setup" | "use" => true,
        "default" => home.can_default,
        "check" => home.kind != "remote",
        _ => false,
    };
    out.extend(h.list.iter().filter(|home| fits(home) && (starts(&home.kind, q) || starts(&home.label, q))).map(|home| fill_row(format!("/computer {sub} {}", home.kind), &home.label, home.state_words())));
    let head = if sub == "use" && !in_bot { "Open a Bot's chat first.".to_owned() } else { String::new() };
    (out, head)
}

/// [`crate::slash`]'s `menu`: the list past `/computer `.
pub(crate) fn menu(rest: &str) -> (Vec<crate::slash::PopRow>, String) {
    if !known() {
        refresh();
    }
    rows(rest, &homes_now(), crate::live::store().active_bot_untracked().is_some())
}

/// [`crate::slash`]'s `run`: `rest` is what follows `/computer`. Nothing reaches the model.
pub(crate) fn run(rest: &str) {
    let rest = rest.to_owned();
    spawn_detached(async move {
        let h = loaded().await;
        act(parse(&rest, &h), &h).await;
    });
}

/// A card in the open chat, starting at `step` for `home`; with no chat open, the page.
fn local_card(step: &str, home: &str) {
    let st = crate::live::store();
    if st.active_id.get_untracked().is_empty() {
        return open_page((step == "key").then_some(home));
    }
    let (bot, name) = st.active_bot_untracked().map(|b| (b.id, b.name)).unwrap_or_default();
    let at = crate::tools::now_ms();
    st.add_local_wire_turn(&json!({"role": "notice", "text": "", "notice": {
        "kind": "computer_setup", "id": format!("cs_local_{at}"), "bot": bot, "botName": name, "chat": st.active_id.get_untracked(),
        "reason": "", "state": "pending", "at": at, "step": step, "home": home,
    }}));
}

async fn act(line: Line, h: &Homes) {
    let st = crate::live::store();
    let bot = st.active_bot_untracked();
    let ready = |k: &str| h.get(k).is_some_and(Home::ready);
    let say = |s: String| crate::toast::show(s);
    match line {
        Line::Page => open_page(None),
        Line::Say(s) => say(s),
        Line::Setup(None) => local_card("", ""),
        Line::Setup(Some(k)) | Line::Use(Some(k)) if k == "remote" => match bot {
            Some(b) => crate::computer::open_remote(&b.id),
            None => say("Open a Bot's chat first.".to_owned()),
        },
        Line::Setup(Some(k)) => match h.get(&k) {
            Some(home) if !home.built => say(format!("{} isn't available in this build yet.", home.label)),
            // nothing to type for it: what's left is to see that it works
            Some(home) if home.fields.is_empty() => local_card("check", &k),
            _ => local_card("key", &k),
        },
        Line::Check(k) => local_card("check", &k),
        Line::Status => {
            let Some(b) = bot else { return open_page(None) };
            let v = client::get_any(&bot_path(&b.id)).await;
            let status = v.as_ref().map(crate::computer::Status::parse).unwrap_or_default();
            let next = status.home.as_ref().map(|n| next_words(n, &h.default.label)).unwrap_or_else(|| format!("Default ({})", h.default.label));
            say(format!("{}: {}. New computers for {}: {next}.", b.name, status.headline(), b.name));
        }
        Line::Use(_) if bot.is_none() => say("Open a Bot's chat first.".to_owned()),
        Line::Use(Some(k)) if !ready(&k) => local_card("key", &k),
        Line::Use(kind) => {
            let b = bot.unwrap_or_default();
            match set_for_bot(&b.id, kind.as_deref()).await {
                Said::Yes(_) => {
                    let label = kind.as_deref().map(label_in(h)).unwrap_or_else(|| format!("Default ({})", h.default.label));
                    say(format!("New computers for {}: {label}. {NEXT_ONLY}.", b.name));
                }
                Said::No(why) => say(why),
            }
        }
        Line::Default(k) if !ready(&k) => local_card("key", &k),
        Line::Default(k) => match set_default(&k).await {
            Said::Yes(_) => say(format!("New computers are now made on {}.", label_in(h)(&k))),
            Said::No(why) => say(why),
        },
    }
}

/// A home's label in `h`.
fn label_in(h: &Homes) -> impl Fn(&str) -> String + '_ {
    move |k| h.get(k).map(|h| h.label.clone()).unwrap_or_else(|| label_of(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ec2() -> Value {
        json!({"kind": "ec2", "label": "AWS EC2", "built": true, "configured": true, "canDefault": true, "isDefault": false,
            "fields": [
                {"key": "region", "label": "Region", "placeholder": "ap-southeast-1", "secret": false, "required": true, "hint": "Where it runs", "value": "ap-southeast-1"},
                {"key": "access_key_id", "label": "Access key ID", "secret": false, "required": true, "value": "AKIA1"},
                {"key": "secret_access_key", "label": "Secret access key", "secret": true, "required": false, "set": true, "last4": "x9Qa", "value": "leaked-if-taken"}
            ],
            "notes": ["Billed to your account."], "computers": [{"id": "cmp_ab12", "bots": ["Ann"]}], "policy": "{\"Version\":\"2012-10-17\"}"})
    }

    fn listed() -> Homes {
        Homes::parse(&json!({"default": {"kind": "docker", "label": "Docker on this Mac", "source": "builtin"}, "providers": [
            {"kind": "docker", "label": "Docker on this Mac", "canDefault": true, "fields": []},
            ec2(),
            {"kind": "e2b", "label": "E2B", "configured": true, "canDefault": true, "fields": [{"key": "api_key", "label": "API key", "secret": true, "set": true}]},
            {"kind": "boat", "label": "boat.dev", "configured": false, "canDefault": true, "fields": [{"key": "token", "label": "Token", "secret": true}]},
            {"kind": "remote", "label": "A remote computer", "canDefault": false, "fields": []}
        ]}))
    }

    #[test]
    fn files_left_by_a_gone_computer_are_named_and_only_named() {
        let v = json!({"kind": "docker", "label": "Docker on this Mac", "fields": [], "leftovers": [{"name": "buwiz-computer-0a1b2c3d-workspace", "id": "cmp_1"}, {"name": "buwiz-computer-0a1b2c3d-home"}]});
        let home = Home::parse(&v).unwrap();
        assert_eq!(home.leftovers, ["buwiz-computer-0a1b2c3d-workspace", "buwiz-computer-0a1b2c3d-home"]);
        assert_eq!(home.leftovers_words(), "Files from a computer that's gone are still on this Mac, in these Docker volumes: buwiz-computer-0a1b2c3d-workspace, buwiz-computer-0a1b2c3d-home. Nothing here deletes them.");
        assert_eq!(Home::parse(&json!({"kind": "docker"})).unwrap().leftovers_words(), "");
        // one home's reply has no list of them: what the full list said stays
        let mut h = listed();
        h.replace(home.clone());
        h.replace(Home::parse(&json!({"kind": "docker", "label": "Docker on this Mac"})).unwrap());
        assert_eq!(h.get("docker").unwrap().leftovers, home.leftovers);
    }

    #[test]
    fn a_launch_page_is_kept_only_when_it_is_a_web_page() {
        let mut v = ec2();
        v["launch"] = json!("https://console.aws.amazon.com/cloudformation/home?region=ap-southeast-1");
        assert!(Home::parse(&v).unwrap().launch.starts_with("https://console.aws.amazon.com/"));
        v["launch"] = json!("file:///etc/passwd");
        assert_eq!(Home::parse(&v).unwrap().launch, "");
    }

    #[test]
    fn the_command_reads_its_sub_command_and_a_home_by_kind_or_name() {
        let h = listed();
        assert_eq!(parse("", &h), Line::Page);
        assert_eq!(parse(" status", &h), Line::Status);
        assert_eq!(parse(" setup", &h), Line::Setup(None));
        assert_eq!(parse(" setup AWS EC2", &h), Line::Setup(Some("ec2".into())));
        assert_eq!(parse(" use default", &h), Line::Use(None));
        assert_eq!(parse(" use e2b", &h), Line::Use(Some("e2b".into())));
        assert_eq!(parse(" use remote", &h), Line::Use(Some("remote".into())));
        assert_eq!(parse(" default E2B", &h), Line::Default("e2b".into()));
        assert_eq!(parse(" check", &h), Line::Check(String::new()));
        assert_eq!(parse(" check boat", &h), Line::Check("boat".into()));
        assert_eq!(parse(" default remote", &h), Line::Say("A remote computer can't be the default.".into()));
        assert_eq!(parse(" use mars", &h), Line::Say("No home is called mars. The homes: docker, ec2, e2b, boat, remote.".into()));
        assert_eq!(parse(" fly", &h), Line::Say(USAGE.into()));
    }

    #[test]
    fn the_menu_lists_the_sub_commands_then_the_homes_each_takes() {
        let h = listed();
        let labels = |rest: &str| rows(rest, &h, true).0.into_iter().map(|r| r.label).collect::<Vec<_>>();
        assert_eq!(labels(""), ["setup", "use", "default", "check", "status"]);
        assert_eq!(labels("s"), ["setup", "status"]);
        assert_eq!(labels("default "), ["Docker on this Mac", "AWS EC2", "E2B", "boat.dev"], "not remote");
        assert_eq!(labels("use "), ["default", "Docker on this Mac", "AWS EC2", "E2B", "boat.dev", "A remote computer"]);
        assert_eq!(labels("setup aw"), ["AWS EC2"]);
        let fills: Vec<_> = rows("default e", &h, true).0.into_iter().map(|r| r.pick).collect();
        assert!(fills == [crate::slash::Pick::Fill("/computer default ec2".into()), crate::slash::Pick::Fill("/computer default e2b".into())]);
        assert_eq!(rows("use ", &h, false).1, "Open a Bot's chat first.");
    }

    #[test]
    fn a_computer_being_deleted_is_listed_with_where_to_look() {
        let mut v = ec2();
        v["computers"] = json!([{"id": "cmp_ab12", "bots": ["Ann"]}, {"id": "cmp_cd34", "bots": [], "leaving": true, "look": "Terminate any instance tagged buwiz:computer-id=cmp_cd34"}]);
        let h = Home::parse(&v).unwrap();
        assert_eq!(h.leaving, vec![("cmp_cd34".to_owned(), "Terminate any instance tagged buwiz:computer-id=cmp_cd34".to_owned())]);
        assert_eq!(h.computers_words(), "Computers here: cmp_ab12 (Ann)", "listed once, as being deleted");
    }

    #[test]
    fn a_lost_computer_is_listed_apart_with_where_to_look() {
        let mut v = ec2();
        v["computers"] = json!([{"id": "cmp_ab12", "bots": ["Ann"]}, {"id": "cmp_ef56", "bots": [], "lost": true, "look": "Terminate any instance tagged buwiz:computer-id=cmp_ef56"}]);
        let h = Home::parse(&v).unwrap();
        assert_eq!(h.lost, vec![("cmp_ef56".to_owned(), "Terminate any instance tagged buwiz:computer-id=cmp_ef56".to_owned())]);
        assert!(h.leaving.is_empty());
        assert_eq!(h.computers_words(), "Computers here: cmp_ab12 (Ann)", "listed once, as lost");
    }

    #[test]
    fn a_secret_fields_value_is_never_taken() {
        let h = Home::parse(&ec2()).unwrap();
        let secret = h.fields.iter().find(|f| f.secret).unwrap();
        assert_eq!((secret.value.as_str(), secret.set, secret.last4.as_str()), ("", true, "x9Qa"));
        assert!(!format!("{h:?}").contains("leaked-if-taken"));
        assert_eq!(h.key_words().as_deref(), Some("Key set ••x9Qa"));
        assert_eq!(h.computers_words(), "Computers here: cmp_ab12 (Ann)");
        assert!(h.ready() && !h.policy.is_empty());
    }

    #[test]
    fn a_home_says_its_key_and_where_it_stands() {
        let none = Home::parse(&json!({"kind": "e2b", "label": "E2B", "configured": false, "canDefault": true, "fields": [{"key": "api_key", "label": "API key", "secret": true, "set": false}]})).unwrap();
        assert_eq!((none.key_words().as_deref(), none.state_words()), (Some("No key"), "Needs a key"));
        let docker = Home::parse(&json!({"kind": "docker", "label": "Docker on this Mac", "canDefault": true, "fields": []})).unwrap();
        assert_eq!((docker.key_words(), docker.ready(), docker.state_words()), (None, true, "Ready"), "Docker needs no key");
        let off = Home::parse(&json!({"kind": "boat", "built": false, "configured": true})).unwrap();
        assert_eq!((off.label.as_str(), off.ready(), off.state_words()), ("boat.dev", false, "Not available yet"));
    }

    #[test]
    fn the_list_knows_its_default_and_its_choices() {
        let v = json!({"default": {"kind": "e2b", "label": "E2B", "source": "saved"}, "providers": [
            {"kind": "docker", "label": "Docker on this Mac", "canDefault": true, "fields": []},
            ec2(),
            {"kind": "e2b", "label": "E2B", "configured": false, "canDefault": true, "fields": [{"key": "api_key", "label": "API key", "secret": true}]},
            {"kind": "remote", "label": "A remote computer", "canDefault": false, "configured": true, "fields": []}
        ]});
        let h = Homes::parse(&v);
        assert_eq!(h.default, Next { kind: "e2b".into(), label: "E2B".into(), source: "saved".into() });
        assert_eq!(h.default_choices().iter().map(|h| h.kind.as_str()).collect::<Vec<_>>(), ["docker", "ec2"], "not a home with no key, nor remote");
        assert_eq!(h.bot_choices().iter().map(|h| h.kind.as_str()).collect::<Vec<_>>(), ["docker", "ec2"]);
        assert_eq!(h.named("aws ec2").map(|h| h.kind.as_str()), Some("ec2"));
        assert_eq!(h.named("E2B").map(|h| h.kind.as_str()), Some("e2b"));
        assert!(h.named("e2").is_none());
    }

    #[test]
    fn a_refusal_is_its_sentence_as_it_is() {
        assert_eq!(said(Some((409, json!({"ok": false, "code": "bad_key", "message": "AWS says that key is wrong."})))), Said::No("AWS says that key is wrong.".into()));
        assert_eq!(said(None), Said::No(NO_ANSWER.into()));
        assert_eq!(said(Some((200, json!({"ok": true})))), Said::Yes(json!({"ok": true})));
        assert_eq!(said(Some((200, json!({"state": "absent"})))), Said::Yes(json!({"state": "absent"})), "a status is a yes");
        assert!(matches!(said(Some((500, Value::Null))), Said::No(_)));
    }

    #[test]
    fn a_bots_next_home_reads_as_its_own_or_the_default() {
        let own = Next { kind: "e2b".into(), label: "E2B".into(), source: "bot".into() };
        assert_eq!(next_words(&own, "Docker on this Mac"), "E2B");
        let follows = Next { kind: "docker".into(), label: "Docker on this Mac".into(), source: "default".into() };
        assert_eq!(next_words(&follows, "Docker on this Mac"), "Default (Docker on this Mac)");
    }
}
