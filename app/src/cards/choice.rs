//! The choice card (nativechat `render_form`): the `form` frontend tool's
//! questions with their options. One question answers the moment an
//! option is picked — by a press or by its letter key (A for the first,
//! `keys::choice_keys`); several hold their picks as chips until Submit.
//! The answer goes out as the next message (`FormSpec::answer_text`); ✕
//! puts the card away without answering. Never a password: those are
//! user forms.

use std::collections::BTreeMap;

use nsh_core::prelude::*;
use serde_json::Value;

use super::{Btn, Card, CardState};
use crate::live::store;
use crate::theme;

/// Generative choice-chip form. Answers go out as chat text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FormSpec {
    pub title: Option<String>,
    pub prompt: Option<String>,
    pub fields: Vec<FormField>,
    pub submit: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FormField {
    pub id: String,
    pub label: String,
    pub options: Vec<String>,
}

/// The tool names a choice form arrives under (nativechat `is_ui_tool`).
pub(crate) fn is_form_name(name: &str) -> bool {
    matches!(normalize_name(name).as_str(), "form" | "show-form" | "render-form")
}

pub(crate) fn normalize_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

/// The letter a choice's keycap wears and the key that picks it.
pub(crate) fn choice_letter(index: usize) -> Option<char> {
    u8::try_from(index).ok().filter(|at| *at < 26).map(|at| char::from(b'A' + at))
}

/// Which choice a key picks: `a` or `A` is the first. (The letter keys
/// are bound one per option, `keys::choice_keys`; the tests read keys.)
#[cfg(test)]
pub(crate) fn choice_index(key: &str) -> Option<usize> {
    let mut chars = key.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return None;
    };
    ch.is_ascii_alphabetic().then(|| usize::from(ch.to_ascii_uppercase() as u8 - b'A'))
}

impl FormSpec {
    /// The form in a tool's arguments (an object, or JSON text) or an
    /// inline `{component:"form", …}` value.
    pub(crate) fn from_value(value: &Value) -> Option<Self> {
        if let Some(raw) = value.as_str() {
            return serde_json::from_str::<Value>(raw).ok().as_ref().and_then(Self::from_value);
        }
        let data = value.get("data").unwrap_or(value);
        let fields: Vec<FormField> = data
            .get("fields")
            .or_else(|| value.get("fields"))
            .and_then(Value::as_array)
            .map(|rows| rows.iter().enumerate().filter_map(|(i, row)| parse_field(row, i)).collect())
            .unwrap_or_default();
        if fields.is_empty() {
            return None;
        }
        Some(Self {
            title: string_field(data, "title").or_else(|| string_field(value, "title")),
            prompt: string_field(data, "prompt").or_else(|| string_field(value, "prompt")),
            fields,
            submit: string_field(data, "submit")
                .or_else(|| string_field(value, "submit"))
                .unwrap_or_else(|| "Send".to_owned()),
        })
    }

    /// One question answers on its pick; several keep their button.
    pub(crate) fn answers_on_pick(&self) -> bool {
        matches!(self.fields.as_slice(), [field] if !field.options.is_empty())
    }

    /// The words an answer is sent as: the title, then one `Label: choice`
    /// line per question answered.
    pub(crate) fn answer_text(&self, picks: &BTreeMap<String, String>) -> String {
        let mut lines = Vec::new();
        if let Some(title) = &self.title {
            lines.push(title.clone());
        }
        for field in &self.fields {
            if let Some(value) = picks.get(&field.id) {
                lines.push(format!("{}: {value}", field.label));
            }
        }
        lines.join("\n")
    }
}

fn parse_field(row: &Value, index: usize) -> Option<FormField> {
    let label = string_field(row, "label").or_else(|| string_field(row, "name"))?;
    let id = string_field(row, "id").unwrap_or_else(|| format!("field-{index}"));
    let options = row
        .get("options")
        .and_then(Value::as_array)
        .map(|opts| {
            opts.iter()
                .filter_map(|o| {
                    o.as_str()
                        .map(str::to_owned)
                        .or_else(|| string_field(o, "label"))
                        .or_else(|| string_field(o, "value"))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(FormField { id, label, options })
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

// ---------------------------------------------------------------------------
// Answering
// ---------------------------------------------------------------------------

fn picks_of(key: &str) -> BTreeMap<String, String> {
    store().card_picks.with_untracked(|m| m.get(key).cloned().unwrap_or_default())
}

/// A pick by a press or a letter: a one-question card answers there and
/// then; a card with several holds the pick (a second press takes it back).
pub(crate) fn choose(key: &str, spec: &FormSpec, field_index: usize, option_index: usize) {
    if !super::answerable(key) {
        return;
    }
    let Some(field) = spec.fields.get(field_index) else { return };
    let Some(value) = field.options.get(option_index).cloned() else { return };
    let st = store();
    if spec.answers_on_pick() {
        let picks = BTreeMap::from([(field.id.clone(), value)]);
        st.card_picks.update(|m| {
            m.insert(key.to_owned(), picks.clone());
        });
        send_answer(key, spec, &picks);
        return;
    }
    st.card_picks.update(|m| {
        let picks = m.entry(key.to_owned()).or_default();
        if picks.get(&field.id) == Some(&value) {
            picks.remove(&field.id);
        } else {
            picks.insert(field.id.clone(), value);
        }
    });
}

/// Submit on a card with several questions.
pub(crate) fn submit(key: &str, spec: &FormSpec) {
    let picks = picks_of(key);
    if picks.is_empty() || !super::answerable(key) {
        return;
    }
    send_answer(key, spec, &picks);
}

/// Answered first, then sent: the send supersedes open cards, and this one
/// is no longer open.
fn send_answer(key: &str, spec: &FormSpec, picks: &BTreeMap<String, String>) {
    if store().sending.get_untracked() {
        crate::toast::show("Wait for the reply to finish sending");
        return;
    }
    super::set_state(key, CardState::Answered(spec.answer_text(picks)));
    store().send_text(spec.answer_text(picks));
}

/// ✕: put away without answering. Nothing is sent — the run never waited
/// on the card.
pub(crate) fn dismiss(key: &str) {
    if !super::answerable(key) {
        return;
    }
    store().card_picks.update(|m| {
        m.remove(key);
    });
    super::set_state(key, CardState::Dismissed);
}

/// The newest choice card of the open chat, when it is still open and
/// answers on a pick: its key, its form (tracked).
pub(crate) fn keyed() -> Option<(String, FormSpec)> {
    let st = store();
    if st.turns_of.get() != st.active_id.get() {
        return None;
    }
    let newest = st.turns.with(|turns| {
        turns.iter().rev().find_map(|t| {
            t.cards.iter().rev().find_map(|c| match &*c.card {
                Card::Choice(spec) => Some((c.key.clone(), spec.clone())),
                _ => None,
            })
        })
    })?;
    (newest.1.answers_on_pick() && super::state_of(&newest.0).is_none()).then_some(newest)
}

/// A letter key: the newest open card's option at `index`.
pub(crate) fn answer_keyed(index: usize) {
    let Some((key, spec)) = untracked(keyed) else { return };
    if spec.fields.first().is_some_and(|f| index < f.options.len()) {
        choose(&key, &spec, 0, index);
    }
}

// ---------------------------------------------------------------------------
// The card
// ---------------------------------------------------------------------------

pub(crate) fn card(spec: FormSpec, key: String) -> impl IntoElement {
    let state = {
        let key = key.clone();
        computed(move || {
            let st = super::state_of(&key);
            let picks = store().card_picks.with(|m| m.get(&key).cloned().unwrap_or_default());
            let live = keyed().is_some_and(|(k, _)| k == key);
            (st, picks, live)
        })
    };
    dynamic(move || {
        let (st, picks, live) = state.get();
        match st {
            Some(CardState::Dismissed) => settled_pill(&spec, "Dismissed").into_element(),
            Some(CardState::Superseded) => settled_pill(&spec, "Not answered").into_element(),
            Some(CardState::Answered(_)) => answered(&spec, &picks).into_element(),
            _ => open_card(spec.clone(), key.clone(), picks, live).into_element(),
        }
    })
}

fn heading(spec: &FormSpec) -> String {
    spec.title.clone().or_else(|| spec.prompt.clone()).unwrap_or_default()
}

fn settled_pill(spec: &FormSpec, words: &'static str) -> impl IntoElement {
    super::shell("")
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(super::title(heading(spec)).flex_1().text_color(theme::DIM))
                .child(super::pill(words)),
        )
}

fn answered(spec: &FormSpec, picks: &BTreeMap<String, String>) -> impl IntoElement {
    let mut body = super::shell("").child(super::title(heading(spec)));
    for field in &spec.fields {
        let Some(value) = picks.get(&field.id) else { continue };
        let letter = spec
            .answers_on_pick()
            .then(|| field.options.iter().position(|o| o == value))
            .flatten()
            .and_then(choice_letter);
        body = body.child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .when_some(letter, |r, l| r.child(super::keycap(l, false)))
                .when(!spec.answers_on_pick(), |r| {
                    r.child(text(format!("{}:", field.label)).text_size(px(13.0)).text_color(theme::DIM))
                })
                .child(text(value.clone()).text_size(px(13.0)).text_color(theme::TEXT))
                .child(text("✓").text_size(px(13.0)).text_color(theme::ACCENT)),
        );
    }
    body
}

fn open_card(spec: FormSpec, key: String, picks: BTreeMap<String, String>, live: bool) -> impl IntoElement {
    let subtitle = spec.title.as_ref().and(spec.prompt.clone());
    let one = spec.answers_on_pick();
    // A press on the card takes the caret out of the message field, so its
    // letter keys answer it (a letter typed in a field is the person's).
    let mut body = super::shell(&format!("choice-{key}"))
        .on_press(crate::composer::blur)
        .child(
            view()
                .flex_row()
                .items_start()
                .gap_2()
                .child(
                    view()
                        .flex_1()
                        .gap_1()
                        .child(super::title(heading(&spec)))
                        .when_some(subtitle, |v, p| v.child(super::note(p))),
                )
                .child({
                    let key = key.clone();
                    super::close_x("Dismiss without answering", move || dismiss(&key))
                }),
        );
    for (fi, field) in spec.fields.iter().enumerate() {
        let selected = picks.get(&field.id).cloned();
        let mut options = if one { view().gap(px(6.0)) } else { view().flex_row().flex_wrap().gap(px(6.0)) };
        for (oi, option) in field.options.iter().enumerate() {
            let on = selected.as_deref() == Some(option.as_str());
            let letter = one.then(|| choice_letter(oi)).flatten();
            let (key, spec) = (key.clone(), spec.clone());
            options = options.child(option_row(option.clone(), letter, live, on, one, move || {
                crate::composer::blur();
                choose(&key, &spec, fi, oi);
            }));
        }
        body = body.child(
            view()
                .gap(px(6.0))
                .when(!one, |v| v.child(super::note(field.label.clone())))
                .child(options),
        );
    }
    if !one {
        let (key2, spec2) = (key.clone(), spec.clone());
        body = body.child(
            view()
                .flex_row()
                .child(super::btn(Btn::Primary, &spec.submit, move || submit(&key2, &spec2))),
        );
    }
    body
}

/// One option: a row with its keycap on a one-question card, a chip on a
/// card with several.
fn option_row(
    label: String,
    letter: Option<char>,
    live: bool,
    on: bool,
    one: bool,
    press: impl Fn() + 'static,
) -> impl IntoElement {
    let hov = signal(false);
    view()
        .label(label.clone())
        .flex_row()
        .items_center()
        .gap_2()
        .px(px(10.0))
        .py(px(if one { 6.0 } else { 4.0 }))
        .rounded(px(if one { 10.0 } else { 999.0 }))
        .border(px(0.5))
        .border_color(move || if on || hov.get() { theme::ACCENT } else { theme::BORDER_S })
        .bg(if on { theme::TEXT } else { theme::SUBTLE })
        .on_hover_in(move || hov.set(true))
        .on_hover_out(move || hov.set(false))
        .on_press(move || press())
        .when_some(letter, |r, l| r.child(super::keycap(l, live)))
        .child(
            text(label)
                .text_size(px(13.0))
                .line_height(px(18.0))
                .text_color(if on { theme::MAIN } else { theme::TEXT }),
        )
}
