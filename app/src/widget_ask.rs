//! The permission card a generated widget's data call raises
//! (`widget_ask`): "<widget> wants to use <source>", what it can read, what
//! it can also change, and the person's answer (`POST
//! /v1/host/widget/grant`). Answered, the card shows the answer in the
//! place the buttons were, and the widget's card in the chat mounts again
//! so the widget gets its data without being reopened.

use std::cell::RefCell;
use std::collections::HashMap;

use nsh_core::prelude::*;
use serde_json::{Value, json};

use crate::cards::{self, Btn};
use crate::live::store;
use crate::theme;

/// A `notice` turn the broker posts when a widget calls a source it has no
/// answer for.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct Notice {
    /// The card (`w_…`).
    pub id: String,
    pub bot: String,
    pub bot_name: String,
    /// The widget's module name (`R37`) and its title.
    pub widget: String,
    pub widget_title: String,
    /// The source's name (`reminders`) and title.
    pub source: String,
    pub source_title: String,
    /// What it can read.
    pub reads: Vec<String>,
    /// What it could change: empty for a source with no write action.
    pub writes: Vec<String>,
    /// `pending`, then `write`, `read` or `no`.
    pub state: String,
    pub at: i64,
}

fn text_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn lines_of(v: &Value, k: &str) -> Vec<String> {
    v.get(k)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).filter(|l| !l.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

impl Notice {
    pub(crate) fn of(v: &Value) -> Option<Self> {
        if !v.is_object() || text_of(v, "kind") != "widget_ask" || text_of(v, "id").is_empty() {
            return None;
        }
        let state = Some(text_of(v, "state")).filter(|s| !s.is_empty()).unwrap_or_else(|| "pending".to_owned());
        Some(Self {
            id: text_of(v, "id"),
            bot: text_of(v, "bot"),
            bot_name: text_of(v, "botName"),
            widget: text_of(v, "widget"),
            widget_title: text_of(v, "widgetTitle"),
            source: text_of(v, "source"),
            source_title: text_of(v, "sourceTitle"),
            reads: lines_of(v, "reads"),
            writes: lines_of(v, "writes"),
            state,
            at: v.get("at").and_then(Value::as_i64).unwrap_or(0),
        })
    }
}

/// The words of a card: its title, and its buttons (label, answer).
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Copy {
    pub title: String,
    pub buttons: Vec<(&'static str, &'static str)>,
}

/// "<widget title> wants to use <source title>", with the names where a
/// title is missing.
pub(crate) fn copy(n: &Notice) -> Copy {
    let or = |title: &str, name: &str| if title.is_empty() { name.to_owned() } else { title.to_owned() };
    let title = format!("{} wants to use {}", or(&n.widget_title, &n.widget), or(&n.source_title, &n.source));
    let buttons = if n.writes.is_empty() {
        vec![("Not now", "no"), ("Allow", "write")]
    } else {
        vec![("Not now", "no"), ("Read only", "read"), ("Allow", "write")]
    };
    Copy { title, buttons }
}

/// What an answered card says in place of its buttons; `None` while it is
/// pending (or in a state this build does not know).
pub(crate) fn answered_words(n: &Notice, state: &str) -> Option<&'static str> {
    match state {
        "write" => Some("Allowed"),
        // a source with no write action has nothing "read only" would hold back
        "read" if n.writes.is_empty() => Some("Allowed"),
        "read" => Some("Read only"),
        "no" => Some("Not allowed"),
        _ => None,
    }
}

/// A card's state here, before the chat reloads.
#[derive(Clone, PartialEq, Default, Debug)]
pub(crate) struct Local {
    pub busy: bool,
    pub error: String,
    /// The state the person's answer left (`write`, `read`, `no`).
    pub answered: Option<String>,
}

fn local(id: &str) -> Option<Local> {
    store().widget_asks.with(|m| m.get(id).cloned())
}

fn set_local(id: &str, f: impl FnOnce(&mut Local)) {
    store().widget_asks.update(|m| f(m.entry(id.to_owned()).or_default()));
}

/// The state `n` shows now: the answer given here, else the broker's.
pub(crate) fn state_now(n: &Notice, local: Option<&Local>) -> String {
    local.and_then(|l| l.answered.clone()).unwrap_or_else(|| n.state.clone())
}

thread_local! {
    /// Each card's state as last drawn, so a card that moves off `pending`
    /// is noticed once, whoever answered it.
    static SEEN: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// Records `state` for card `id`; whether it just left `pending`.
fn moved(id: &str, state: &str) -> bool {
    let before = SEEN.with(|m| m.borrow_mut().insert(id.to_owned(), state.to_owned()));
    before.as_deref() == Some("pending") && state != "pending"
}

/// Mounts the cards of widget `widget` again (every version of it), so a
/// widget that was told "not granted" asks for its data afresh.
pub(crate) fn remount(widget: &str) {
    let keys: Vec<String> = store().minis.borrow().keys().filter(|k| crate::store::app_name(k) == widget).cloned().collect();
    for key in keys {
        store().reload_mini(&key);
    }
}

/// The row `chat.rs` draws for a `widget_ask` notice.
pub(crate) fn row(n: Notice) -> impl IntoElement {
    view().flex_row().w_full().justify_center().child(dynamic(move || {
        let state = state_now(&n, local(&n.id).as_ref());
        // answered here or elsewhere (another window, a reload): the widget
        // on screen asks for its data again
        if moved(&n.id, &state) {
            remount(&n.widget);
        }
        card(&n, &state).into_element()
    }))
}

/// The height of the card's last row, the buttons' (28) or the answer's:
/// the same either way, so the card does not move when one replaces the other.
const ANSWER_ROW: f32 = 28.0;

fn card(n: &Notice, state: &str) -> impl IntoElement {
    let copy = copy(n);
    let local = local(&n.id).unwrap_or_default();
    let mut c = cards::shell(&format!("widget-ask-{}", n.id)).label(copy.title.clone()).child(cards::title(copy.title.clone()));
    for l in &n.reads {
        c = c.child(cards::note(l.clone()));
    }
    if !n.writes.is_empty() {
        c = c.child(cards::note("It can also change things:".to_owned()).text_color(theme::TEXT));
        for l in &n.writes {
            c = c.child(cards::note(l.clone()));
        }
    }
    let mut last = view().flex_row().h(px(ANSWER_ROW)).items_center().justify_end().gap_2();
    if let Some(words) = answered_words(n, state) {
        last = last.child(text(words).text_size(px(12.5)).line_height(px(16.0)).text_color(theme::MUTED));
    } else if local.busy {
        last = last.child(text("Sending…").text_size(px(12.5)).line_height(px(16.0)).text_color(theme::MUTED));
    } else {
        let end = copy.buttons.len().saturating_sub(1);
        for (i, (label, answer)) in copy.buttons.iter().enumerate() {
            let (n1, answer) = (n.clone(), *answer);
            let kind = if i == end { Btn::Primary } else { Btn::Secondary };
            last = last.child(cards::btn(kind, label, move || answer_ask(&n1, answer)));
        }
    }
    c = c.child(last);
    if !local.error.is_empty() {
        c = c.child(cards::error_line(local.error.clone()));
    }
    c.into_element()
}

/// What `POST /v1/host/widget/grant` answers: the state it left, or why not.
pub(crate) fn grant_result(answer: Option<&Value>, asked: &str) -> Result<String, String> {
    match answer {
        Some(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => Ok(asked.to_owned()),
        Some(v) => {
            let why = text_of(v, "error");
            Err(if why.is_empty() { "The server refused it".to_owned() } else { why })
        }
        None => Err("Not connected to an agent server yet".to_owned()),
    }
}

/// `POST /v1/host/widget/grant`: the buttons go at once to "Sending…"; a
/// refusal brings them back with the words.
fn answer_ask(n: &Notice, answer: &'static str) {
    if local(&n.id).is_some_and(|l| l.busy) {
        return;
    }
    let (id, widget) = (n.id.clone(), n.widget.clone());
    set_local(&id, |l| {
        l.busy = true;
        l.error.clear();
    });
    crate::store::spawn_detached(async move {
        let r = crate::client::post_any("/v1/host/widget/grant", json!({"id": id, "answer": answer})).await;
        match grant_result(r.as_ref(), answer) {
            Ok(state) => {
                moved(&id, &state);
                set_local(&id, |l| {
                    l.busy = false;
                    l.answered = Some(state);
                });
                remount(&widget);
            }
            Err(e) => set_local(&id, |l| {
                l.busy = false;
                l.error = e;
            }),
        }
    });
}

// ---------------------------------------------------------------------------
// What a widget may use (App Details)
// ---------------------------------------------------------------------------

/// One source a widget was allowed.
#[derive(Clone, PartialEq, Default, Debug)]
pub(crate) struct Grant {
    pub source: String,
    pub title: String,
    /// `read` or `write`.
    pub mode: String,
}

/// The grants App Details shows for widget `name`.
#[derive(Clone, PartialEq, Default, Debug)]
pub(crate) struct Grants {
    pub name: String,
    /// Whether the broker answered (an older one has no such route: the
    /// section then does not show).
    pub loaded: bool,
    pub list: Vec<Grant>,
    pub error: String,
}

/// The grants out of `GET /v1/host/widget/grants`'s answer: the sources
/// the person said yes to (`read`, `write`); a `no` is no grant. The title
/// is the answer's `sourceTitle` or `title` when it has one, else the
/// source's name.
pub(crate) fn parse_grants(answer: &Value) -> Vec<Grant> {
    let Some(list) = answer.get("grants").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|g| {
            let (source, mode) = (text_of(g, "source"), text_of(g, "mode"));
            if source.is_empty() || !matches!(mode.as_str(), "read" | "write") {
                return None;
            }
            let title = Some(text_of(g, "sourceTitle")).filter(|t| !t.is_empty()).or_else(|| Some(text_of(g, "title")).filter(|t| !t.is_empty())).unwrap_or_else(|| source.clone());
            Some(Grant { source, title, mode })
        })
        .collect()
}

/// What a grant lets the widget do, in words.
pub(crate) fn grant_words(g: &Grant) -> &'static str {
    if g.mode == "write" { "Can read and change" } else { "Can read" }
}

/// Reads widget `name`'s grants into `Store::widget_grants`.
pub(crate) fn load_grants(name: &str) {
    let name = name.to_owned();
    store().widget_grants.set(Grants { name: name.clone(), ..Grants::default() });
    crate::store::spawn_detached(async move {
        let answer = crate::client::get(&format!("/v1/host/widget/grants?name={}", crate::client::urlenc(&name))).await;
        let ok = answer.as_ref().is_some_and(|v| v.get("ok").and_then(Value::as_bool) != Some(false));
        let list = answer.as_ref().filter(|_| ok).map(parse_grants).unwrap_or_default();
        store().widget_grants.update(|g| {
            if g.name == name {
                g.loaded = ok;
                g.list = list;
            }
        });
    });
}

/// `POST /v1/host/widget/revoke`: the source leaves the list; the widget's
/// next call to it asks again.
fn revoke(name: &str, source: &str) {
    let (name, source) = (name.to_owned(), source.to_owned());
    crate::store::spawn_detached(async move {
        let r = crate::client::post_any("/v1/host/widget/revoke", json!({"name": name, "source": source})).await;
        let done = grant_result(r.as_ref(), "").is_ok();
        let why = grant_result(r.as_ref(), "").err().unwrap_or_default();
        store().widget_grants.update(|g| {
            if g.name != name {
                return;
            }
            if done {
                g.list.retain(|x| x.source != source);
                g.error.clear();
            } else {
                g.error = why;
            }
        });
    });
}

/// "Data this widget can use", in App Details of generated widget `name`:
/// each source it may use, with Remove. Nothing while the broker has not
/// answered (or cannot: an older one).
pub(crate) fn data_section(name: String) -> impl IntoElement {
    load_grants(&name);
    dynamic(move || {
        let g = store().widget_grants.get();
        if g.name != name || !g.loaded {
            return view().into_element();
        }
        let mut c = view().gap(px(6.0)).test_id("widget-data").child(text("Data this widget can use").text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT));
        if g.list.is_empty() {
            c = c.child(cards::note("None yet. It asks you the first time it wants some.".to_owned()));
        }
        for grant in &g.list {
            let (n, source) = (name.clone(), grant.source.clone());
            let remove = format!("Remove {}", grant.title);
            c = c.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(text(grant.title.clone()).text_size(px(12.0)).line_height(px(17.0)).text_color(theme::TEXT))
                    .child(text(grant_words(grant)).flex_1().text_size(px(12.0)).line_height(px(17.0)).text_color(theme::MUTED))
                    .child(remove_button(remove, move || revoke(&n, &source))),
            );
        }
        if !g.error.is_empty() {
            c = c.child(cards::error_line(g.error.clone()));
        }
        c.into_element()
    })
}

/// A small "Remove" that names its source for the accessibility label.
fn remove_button(aria: String, on: impl Fn() + 'static) -> impl IntoElement {
    let hov = signal(false);
    view()
        .role(AccessibilityRole::Button)
        .label(aria)
        .px(px(8.0))
        .py(px(3.0))
        .rounded(px(7.0))
        .bg(move || if hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(on)
        .child(text("Remove").text_size(px(12.0)).text_color(theme::DOWN))
}
