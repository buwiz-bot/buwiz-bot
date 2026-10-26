//! Settings (⌘,): a modal with its pages down the left. Keyboard lists
//! every command by where it applies, with the keys it answers to: press a
//! key chip and then the new keys to change it, × to drop one, + to add
//! one, Reset to go back to the defaults. A key another command in the
//! same place used moves over (the note under the list says from where).
//!
//! Routines lists what the Rox server runs on a timer (`GET /routines`):
//! each with its chat, interval, last run and error, an on/off switch and
//! Delete. The agent makes them; here they're watched and stopped. Above
//! them, Suggested lists the templates connected servers announce (`GET
//! /routines/templates`): Enable picks a chat or bot in ⌘K and makes one
//! there (`POST /routine/from-template`).

use nsh_core::ui::prelude::*;

use crate::keys::{self, COMMANDS, Scope};
use crate::live::store;
use crate::theme;

const PAGES: [(&str, &str); 3] = [("keyboard", "Keyboard"), ("routines", "Routines"), ("logins", "Logins")];

/// The order Keyboard lists the places in: the broad ones first.
const LIST_ORDER: [Scope; 10] = [
    Scope::Global,
    Scope::Chat,
    Scope::Menu,
    Scope::Viewer,
    Scope::ViewerTable,
    Scope::Lightbox,
    Scope::Palette,
    Scope::Plugins,
    Scope::PluginsSearch,
    Scope::Settings,
];

/// ⌘, — opens Settings, or closes it.
pub(crate) fn toggle() {
    let st = store();
    if !st.settings_open.get_untracked() {
        st.settings_page.set("keyboard".to_owned());
    }
    st.settings_open.update(|v| *v = !*v);
    st.key_record.set(None);
}

/// What the last change said ("⌘H moved from Hide Rox").
fn note() -> Signal<String> {
    store().settings_note
}

/// The search over the keyboard list.
fn query() -> Signal<String> {
    store().settings_query
}

/// A key the recorder caught (canonical keystroke text): Escape cancels;
/// any other key goes in the recording slot.
pub(crate) fn recorded(key: &str) {
    let st = store();
    let Some((id, slot)) = st.key_record.get_untracked() else { return };
    st.key_record.set(None);
    let Some(k) = keys::parse(key) else { return };
    if key == "escape" {
        return;
    }
    // Find by keys: the pressed keys become the search
    if id == "__search" {
        query().set(k.to_string());
        st.settings_sel.set(-1);
        return;
    }
    if k.reserved(nsh_core::platform::Os::MacOs) == Some("it quits the app") {
        note().set(format!("{} stays Quit", key_label(key)));
        return;
    }
    let Some(c) = COMMANDS.iter().find(|c| c.id == id) else { return };
    let canon = k.to_string();
    // the key leaves the commands it would collide with
    let taken = keys::conflicts(c, &canon);
    for o in &taken {
        let rest: Vec<String> = keys::keys_of(o).into_iter().filter(|ok| keys::parse(ok) != Some(k)).collect();
        keys::set_keys(o.id, rest);
    }
    let mut mine = keys::keys_of(c);
    if slot < mine.len() {
        mine[slot] = canon;
    } else {
        mine.push(canon);
    }
    let mut seen = Vec::new();
    mine.retain(|m| {
        let p = keys::parse(m);
        let fresh = !seen.contains(&p);
        seen.push(p);
        fresh
    });
    keys::set_keys(c.id, mine);
    note().set(if taken.is_empty() {
        String::new()
    } else {
        format!(
            "{} moved here from {}",
            key_label(key),
            taken.iter().map(|o| o.title).collect::<Vec<_>>().join(", ")
        )
    });
}

/// The Settings modal — mount in the root stack.
pub(crate) fn layer() -> impl IntoElement {
    dynamic(|| {
        if !store().settings_open.get() {
            return view().into_element();
        }
        view()
            .size_full()
            .absolute()
            .items_center()
            .justify_center()
            .child(
                view()
                    .size_full()
                    .absolute()
                    .bg(hsla(0.0, 0.0, 0.0, 0.4))
                    .on_press(|| {
                        let st = store();
                        st.settings_open.set(false);
                        st.key_record.set(None);
                    }),
            )
            .child(
                view()
                    .w(px(860.0))
                    .h(px(620.0))
                    .max_w(percentage(0.92))
                    .max_h(percentage(0.9))
                    .flex_row()
                    .bg(theme::MENU_BG)
                    .rounded(px(14.0))
                    .border_1()
                    .border_color(theme::BORDER_S)
                    .overflow_hidden()
                    .child(nav())
                    .child(dynamic(|| {
                        let page = store().settings_page.get();
                        if page == "routines" {
                            routines_page().into_element()
                        } else if page == "logins" {
                            crate::vault::settings_page().into_element()
                        } else {
                            keyboard_page().into_element()
                        }
                    })),
            )
            .into_element()
    })
}

fn nav() -> impl IntoElement {
    view()
        .w(px(190.0))
        .h_full()
        .p_3()
        .gap_1()
        .bg(theme::SIDEBAR)
        .child(
            text("Settings")
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::MUTED)
                .px_2()
                .pb_2(),
        )
        .children(PAGES.iter().map(|(id, title)| {
            let id = *id;
            text(*title)
                .text_size(px(13.0))
                .text_color(theme::TEXT)
                .px_2()
                .py(px(6.0))
                .rounded(px(7.0))
                .bg(move || if store().settings_page.get() == id { theme::ACTIVE } else { AdaptiveColor::TRANSPARENT })
                .on_press(move || {
                    store().settings_page.set(id.to_owned());
                    if id == "routines" {
                        load_page();
                    }
                })
                .into_element()
        }))
}

fn keyboard_page() -> impl IntoElement {
    view()
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_4()
                .pt_4()
                .pb_2()
                .child(
                    text("Keyboard shortcuts")
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::TEXT)
                        .flex_1(),
                )
                .child(
                    text("Reset all")
                        .text_size(px(12.0))
                        .text_color(theme::MUTED)
                        .px_2()
                        .py_1()
                        .rounded(px(6.0))
                        .bg(theme::HOVER)
                        .on_press(|| {
                            keys::reset_all();
                            note().set("Every shortcut is back to its default".to_owned());
                        }),
                ),
        )
        .child(search_box())
        .child(dynamic(|| {
            let n = note().get();
            if n.is_empty() {
                return view().into_element();
            }
            text(n).text_size(px(12.0)).text_color(theme::ACCENT).px_4().pb_2().into_element()
        }))
        .child({
            // One line per place heading or shortcut, virtualized: only the
            // lines near the viewport are built, and each rebuilds alone
            // when the keymap or the recorder changes.
            let lines = computed(|| lines(&query().get()));
            virtual_list(
                move || lines.with(|l| l.len()),
                move |at| {
                    dynamic(move || match lines.with(|l| l.get(at.index()).cloned()) {
                        Some(Line::Head(title)) => text(title)
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::MUTED)
                            .pt_3()
                            .pb_1()
                            .into_element(),
                        Some(Line::Cmd(c, n)) => {
                            let _ = store().keymap.get();
                            let _ = store().key_record.get();
                            row(c, n).into_element()
                        }
                        None => view().into_element(),
                    })
                },
            )
            .estimated_item_size(px(38.0))
            .row_gap(px(4.0))
            .content_padding_x(px(16.0))
            .content_padding_bottom(px(16.0))
            .empty(text("No shortcuts match").text_size(px(13.0)).text_color(theme::MUTED).px_4().pt_3())
            .flex_1()
        })
}

/// A line of the keyboard list: a place's heading, or a shortcut with its
/// position among the shortcuts (the arrow keys' highlight).
#[derive(Clone)]
enum Line {
    Head(&'static str),
    Cmd(&'static keys::Command, usize),
}

impl PartialEq for Line {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Line::Head(a), Line::Head(b)) => a == b,
            (Line::Cmd(a, i), Line::Cmd(b, j)) => std::ptr::eq(*a, *b) && i == j,
            _ => false,
        }
    }
}

/// `visible(q)` as lines, in list order.
fn lines(q: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut n = 0;
    for (s, rows) in visible(q) {
        out.push(Line::Head(s.title()));
        for c in rows {
            out.push(Line::Cmd(c, n));
            n += 1;
        }
    }
    out
}

// ——————————————— the search: groups, keys, words ———————————————

/// `$name` filters: the places each name stands for. A file type means the
/// file viewer (a spreadsheet's own keys too).
const GROUPS: [(&str, &[Scope]); 17] = [
    ("global", &[Scope::Global]),
    ("chat", &[Scope::Chat]),
    ("menu", &[Scope::Menu]),
    ("viewer", &[Scope::Viewer, Scope::ViewerTable]),
    ("docx", &[Scope::Viewer]),
    ("pptx", &[Scope::Viewer]),
    ("xlsx", &[Scope::Viewer, Scope::ViewerTable]),
    ("csv", &[Scope::Viewer, Scope::ViewerTable]),
    ("pdf", &[Scope::Viewer]),
    ("code", &[Scope::Viewer]),
    ("image", &[Scope::Viewer, Scope::Lightbox]),
    ("lightbox", &[Scope::Lightbox]),
    ("palette", &[Scope::Palette]),
    ("plugins", &[Scope::Plugins, Scope::PluginsSearch]),
    ("plugins-search", &[Scope::PluginsSearch]),
    ("settings", &[Scope::Settings]),
    ("spreadsheet", &[Scope::ViewerTable]),
];

/// A key pattern: `cmd+` (any key with ⌘), `cmd+shift+`, `c+s+`, `cmd+k`
/// (exactly ⌘K), `fn` (the function keys). Modifiers: `cmd`/`command`/`⌘`,
/// `ctrl`/`ctl`/`c`, `shift`/`s`, `opt`/`option`/`alt`/`o`; a one-letter
/// name is a modifier only before a `+`.
struct KeyPattern {
    cmd: bool,
    ctrl: bool,
    shift: bool,
    alt: bool,
    function: bool,
    /// The key itself (exact match); none: any key with those modifiers.
    key: Option<String>,
}

fn key_pattern(tok: &str) -> Option<KeyPattern> {
    if tok == "fn" {
        return Some(KeyPattern { cmd: false, ctrl: false, shift: false, alt: false, function: true, key: None });
    }
    if !tok.contains('+') {
        return None;
    }
    let parts: Vec<&str> = tok.split('+').collect();
    let mut p = KeyPattern { cmd: false, ctrl: false, shift: false, alt: false, function: false, key: None };
    for (i, part) in parts.iter().enumerate() {
        let last = i == parts.len() - 1;
        if last && part.is_empty() {
            break;
        }
        let modifier = match *part {
            "cmd" | "command" | "⌘" | "mod" => Some(&mut p.cmd),
            "ctrl" | "ctl" | "control" | "⌃" => Some(&mut p.ctrl),
            "shift" | "⇧" => Some(&mut p.shift),
            "opt" | "option" | "alt" | "⌥" => Some(&mut p.alt),
            "c" if !last => Some(&mut p.ctrl),
            "s" if !last => Some(&mut p.shift),
            "o" if !last => Some(&mut p.alt),
            "fn" => {
                p.function = true;
                None
            }
            _ => None,
        };
        match modifier {
            Some(m) => *m = true,
            None if part == &"fn" => {}
            None if last => p.key = Some((*part).to_owned()),
            None => return None,
        }
    }
    Some(p)
}

impl KeyPattern {
    fn matches(&self, key: &str) -> bool {
        let Some(k) = keys::parse(key) else { return false };
        let m = k.modifiers;
        if self.function && !matches!(k.key, nsh_core::keymap::Key::F(_)) {
            return false;
        }
        match &self.key {
            // exactly these modifiers and this key
            Some(name) => {
                let mut text = String::new();
                for (on, n) in [(self.ctrl, "ctrl+"), (self.alt, "alt+"), (self.shift, "shift+"), (self.cmd, "cmd+")] {
                    if on {
                        text.push_str(n);
                    }
                }
                text.push_str(name);
                keys::parse(&text) == Some(k)
            }
            // at least these modifiers
            None => (!self.cmd || m.cmd) && (!self.ctrl || m.ctrl) && (!self.shift || m.shift) && (!self.alt || m.alt),
        }
    }
}

/// The groups and their commands that `q` keeps, in list order: `$group`
/// tokens narrow the places, key patterns the keys, other words the
/// titles (or the keys' labels).
fn visible(q: &str) -> Vec<(Scope, Vec<&'static keys::Command>)> {
    let q = q.trim().to_lowercase();
    let mut places: Vec<Scope> = Vec::new();
    let mut patterns = Vec::new();
    let mut words = Vec::new();
    for tok in q.split_whitespace() {
        if let Some(g) = tok.strip_prefix('$') {
            let hits: Vec<Scope> = GROUPS.iter().filter(|(n, _)| n.starts_with(g)).flat_map(|(_, s)| s.iter().copied()).collect();
            places.extend(hits);
        } else if let Some(p) = key_pattern(tok) {
            patterns.push(p);
        } else {
            words.push(tok.to_owned());
        }
    }
    let mut out = Vec::new();
    for s in LIST_ORDER {
        if !places.is_empty() && !places.contains(&s) {
            continue;
        }
        let rows: Vec<_> = COMMANDS
            .iter()
            .filter(|c| c.scope == s)
            .filter(|c| {
                let ks = keys::keys_of(c);
                patterns.iter().all(|p| ks.iter().any(|k| p.matches(k)))
                    && words.iter().all(|w| {
                        c.title.to_lowercase().contains(w.as_str())
                            || c.id.contains(w.as_str())
                            || ks.iter().any(|k| k.contains(w.as_str()) || key_label(k).to_lowercase().contains(w.as_str()))
                    })
            })
            .collect();
        if !rows.is_empty() {
            out.push((s, rows));
        }
    }
    out
}

/// The `$group` suggestions for the word being typed (its last token).
fn suggestions(q: &str) -> Vec<&'static str> {
    if q.ends_with(char::is_whitespace) {
        return Vec::new();
    }
    let Some(last) = q.split_whitespace().last() else { return Vec::new() };
    let Some(g) = last.strip_prefix('$') else { return Vec::new() };
    let g = g.to_lowercase();
    GROUPS.iter().map(|(n, _)| *n).filter(|n| n.starts_with(&g) && *n != g).collect()
}

/// Completes the typed `$…` word with suggestion `name`.
fn complete(name: &str) {
    let q = query().get_untracked();
    let cut = q.rfind(char::is_whitespace).map_or(0, |i| i + 1);
    query().set(format!("{}${name} ", &q[..cut]));
    store().settings_sug.set(0);
}

/// ↑ / ↓ in Settings: through the `$` suggestions while they show, else
/// through the shortcuts.
pub(crate) fn step(d: i64) {
    let st = store();
    let sugs = suggestions(&query().get_untracked());
    if !sugs.is_empty() {
        let n = sugs.len() as i64;
        st.settings_sug.update(|i| *i = (*i + d).rem_euclid(n));
        return;
    }
    let n: usize = visible(&query().get_untracked()).iter().map(|(_, r)| r.len()).sum();
    if n == 0 {
        return;
    }
    st.settings_sel.update(|i| *i = if *i < 0 { if d > 0 { 0 } else { n as i64 - 1 } } else { (*i + d).rem_euclid(n as i64) });
}

/// ↩ in Settings: takes the highlighted suggestion, else records a new key
/// for the highlighted shortcut.
pub(crate) fn enter() {
    let st = store();
    let sugs = suggestions(&query().get_untracked());
    if let Some(name) = sugs.get(st.settings_sug.get_untracked().max(0) as usize) {
        complete(name);
        return;
    }
    let sel = st.settings_sel.get_untracked();
    if sel < 0 {
        return;
    }
    let all: Vec<&keys::Command> = visible(&query().get_untracked()).into_iter().flat_map(|(_, r)| r).collect();
    if let Some(c) = all.get(sel as usize) {
        let slot = keys::keys_of(c).len().min(1).saturating_sub(1);
        st.key_record.set(Some((c.id.to_owned(), slot)));
    }
}

/// The search: a field with the `$` suggestions under it, and a button
/// that finds a shortcut by pressing it.
fn search_box() -> impl IntoElement {
    view()
        .px_4()
        .pb_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    input()
                        .auto_focus(true)
                        .value(move || query().get())
                        .placeholder("Search — $docx $global, cmd+ c+s+ fn, or words")
                        .placeholder_color(|| color(theme::GHOST))
                        .text_size(px(13.0))
                        .text_color(theme::TEXT)
                        .on_change(move |v: &str| {
                            query().set(v.to_owned());
                            store().settings_sel.set(-1);
                            store().settings_sug.set(0);
                        })
                        .flex_1()
                        .h(px(30.0))
                        .px_2()
                        .rounded(px(7.0))
                        .bg(theme::HOVER),
                )
                .child(
                    text(move || {
                        if store().key_record.get().is_some_and(|(id, _)| id == "__search") {
                            "Press keys…"
                        } else {
                            "⌨ Find by keys"
                        }
                    })
                    .text_size(px(12.0))
                    .text_color(theme::TEXT)
                    .px_2()
                    .py(px(6.0))
                    .rounded(px(7.0))
                    .bg(theme::HOVER)
                    .on_press(|| store().key_record.set(Some(("__search".to_owned(), 0)))),
                ),
        )
        .child(dynamic(|| {
            let sugs = suggestions(&query().get());
            if sugs.is_empty() {
                return view().into_element();
            }
            let sel = store().settings_sug.get();
            view()
                .mt_1()
                .rounded(px(8.0))
                .border_1()
                .border_color(theme::BORDER_S)
                .bg(theme::MENU_BG)
                .py_1()
                .children(sugs.into_iter().enumerate().map(|(i, name)| {
                    let places: Vec<&str> = GROUPS
                        .iter()
                        .find(|(n, _)| *n == name)
                        .map(|(_, s)| s.iter().map(|s| s.title()).collect())
                        .unwrap_or_default();
                    view()
                        .flex_row()
                        .gap_2()
                        .px_3()
                        .py(px(5.0))
                        .bg(if i as i64 == sel { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                        .child(text(format!("${name}")).text_size(px(13.0)).text_color(theme::TEXT))
                        .child(text(places.join(", ")).text_size(px(12.0)).text_color(theme::MUTED))
                        .on_press(move || complete(name))
                        .into_element()
                }))
                .into_element()
        }))
}

fn row(c: &'static keys::Command, n: usize) -> impl IntoElement {
    let keys_now = keys::keys_of(c);
    let changed = keys_now != c.keys.iter().map(|k| (*k).to_owned()).collect::<Vec<_>>();
    let recording = store().key_record.get_untracked();
    let menu_note = c.menu.is_some() && !changed;
    let mut chips = view().flex_row().items_center().gap(px(6.0));
    for (slot, k) in keys_now.iter().enumerate() {
        let live = recording.as_ref() == Some(&(c.id.to_owned(), slot));
        chips = chips.child(chip(c.id, slot, if live { "Press keys…".to_owned() } else { key_label(k) }, live, true));
    }
    let adding = recording.as_ref() == Some(&(c.id.to_owned(), keys_now.len()));
    chips = chips.child(chip(c.id, keys_now.len(), if adding { "Press keys…".to_owned() } else { "+".to_owned() }, adding, false));
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .min_h(px(34.0))
        .px_2()
        .rounded(px(7.0))
        .bg(theme::SUBTLE)
        .border_1()
        // The arrow keys' highlight, bound: a step redraws two borders, not
        // the list. Unselected, all but transparent: a bound border that
        // starts transparent never took the accent.
        .border_color(move || if store().settings_sel.get() == n as i64 { theme::ACCENT } else { theme::tc("#00000001") })
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(text(c.title).text_size(px(13.0)).text_color(theme::TEXT).number_of_lines(1))
                .when(menu_note, |v| {
                    v.child(text("macOS menu key").text_size(px(11.0)).text_color(theme::FAINT))
                }),
        )
        .when(changed, |v| {
            v.child(
                text("Reset")
                    .text_size(px(11.0))
                    .text_color(theme::MUTED)
                    .px_2()
                    .on_press(move || {
                        keys::set_keys(c.id, c.keys.iter().map(|k| (*k).to_owned()).collect());
                    }),
            )
        })
        .child(chips)
}

/// One key of a command (press: record a new one in its place; ×: drop
/// it), or the + that records another.
fn chip(id: &'static str, slot: usize, label: String, live: bool, removable: bool) -> impl IntoElement {
    let hov = signal(false);
    view()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .h(px(24.0))
        .px_2()
        .rounded(px(6.0))
        .border_1()
        .border_color(if live { theme::ACCENT } else { theme::BORDER_S })
        .bg(if live { theme::ACCENT_TINT } else { theme::CARD })
        .on_hover_in(move || hov.set(true))
        .on_hover_out(move || hov.set(false))
        .child(
            text(label)
                .text_size(px(12.0))
                .text_color(theme::TEXT)
                .on_press(move || store().key_record.set(Some((id.to_owned(), slot)))),
        )
        .when(removable && !live, |v| {
            v.child(
                text("×")
                    .text_size(px(12.0))
                    .text_color(theme::MUTED)
                    .opacity(move || if hov.get() { 1.0f32 } else { 0.0 })
                    .on_press(move || {
                        let Some(c) = COMMANDS.iter().find(|c| c.id == id) else { return };
                        let mut ks = keys::keys_of(c);
                        if slot < ks.len() {
                            ks.remove(slot);
                        }
                        keys::set_keys(id, ks);
                    }),
            )
        })
}

// ——————————————————————————— Routines ———————————————————————————

/// One routine the server runs (`GET /routines`): every `every_s`
/// seconds it checks a tool (`check`, cheap) and wakes the agent in its
/// chat only for what's new — or runs the agent every time without one.
#[derive(Clone, PartialEq, Default)]
pub struct Routine {
    pub id: String,
    pub name: String,
    pub thread: String,
    pub thread_title: String,
    pub bot_name: String,
    pub every_s: f64,
    pub enabled: bool,
    /// The tool it checks: `server › tool`, or "".
    pub check: String,
    /// Epoch ms of the last run (0 never).
    pub last_run: f64,
    pub last_error: String,
}

impl Routine {
    fn of(v: &serde_json::Value) -> Self {
        let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned();
        let n = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
        let check = v.get("check").filter(|c| c.is_object()).map(|c| format!("{} › {}", s(c, "server"), s(c, "tool")));
        // a time in seconds reads as ms
        let last = n("last_run");
        Routine {
            id: v.get("id").map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).unwrap_or_default(),
            name: s(v, "name"),
            thread: s(v, "thread"),
            thread_title: s(v, "threadTitle"),
            bot_name: s(v, "botName"),
            every_s: n("every_s"),
            enabled: v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false),
            check: check.unwrap_or_default(),
            last_run: if last > 0.0 && last < 1e11 { last * 1000.0 } else { last },
            last_error: s(v, "last_error"),
        }
    }

    fn interval(&self) -> String {
        interval(self.every_s)
    }
}

/// "every minute", "every 5 min", "every 2 h"
fn interval(every_s: f64) -> String {
    let s = every_s.max(0.0).round() as i64;
    match s {
        0..=60 => "every minute".to_owned(),
        _ if s % 3600 == 0 => format!("every {} h", s / 3600),
        _ if s < 3600 => format!("every {} min", (s + 30) / 60),
        _ => format!("every {:.1} h", s as f64 / 3600.0),
    }
}

/// A routine a connected server suggests (`GET /routines/templates`).
/// One that can't run yet says why (`reason`).
#[derive(Clone, PartialEq, Default)]
pub struct RoutineTemplate {
    pub server: String,
    pub id: String,
    pub name: String,
    pub description: String,
    pub every_s: f64,
    pub available: bool,
    pub reason: String,
    /// Enable asks for the user's Google address first.
    pub needs_email: bool,
}

impl RoutineTemplate {
    fn of(v: &serde_json::Value) -> Self {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned();
        let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        RoutineTemplate {
            server: s("server"),
            id: s("id"),
            name: s("name"),
            description: s("description"),
            every_s: v.get("every_s").and_then(|x| x.as_f64()).unwrap_or(0.0),
            available: b("available"),
            reason: s("reason"),
            needs_email: b("needs_email"),
        }
    }
}

/// `GET /routines` into the page.
fn load_routines() {
    Owner::detached().run(|| {
        spawn(async move {
            if let Some(v) = crate::client::get("/routines").await {
                let list = v
                    .get("routines")
                    .and_then(|x| x.as_array())
                    .map(|l| l.iter().map(Routine::of).collect())
                    .unwrap_or_default();
                store().routines.set(list);
            }
        })
    });
}

/// `GET /routines/templates` into Suggested — on its own, as it may wait
/// on servers starting.
fn load_templates() {
    Owner::detached().run(|| {
        spawn(async move {
            if let Some(v) = crate::client::get("/routines/templates").await {
                let list = v.as_array().map(|l| l.iter().map(RoutineTemplate::of).collect()).unwrap_or_default();
                store().routine_templates.set(list);
            }
        })
    });
}

/// The page: the routines and the suggestions.
fn load_page() {
    load_routines();
    load_templates();
}

/// Enable: ⌘K over the bots and chats (Settings steps aside for it); a
/// bot's routine runs in its main chat. A template that needs the Google
/// address asks for it in its card first.
fn pick_chat(t: &RoutineTemplate) {
    use crate::store::{PaletteAct, PaletteItem};
    let st = store();
    let email = st.routine_email.get_untracked().trim().to_owned();
    if t.needs_email && email.is_empty() {
        st.routine_ask_email.set(Some((t.server.clone(), t.id.clone())));
        return;
    }
    let row = |title: String, subtitle: String, thread: String| PaletteItem {
        title: title.clone(),
        subtitle,
        icon: String::new(),
        checked: false,
        act: PaletteAct::EnableRoutine { server: t.server.clone(), id: t.id.clone(), thread, place: title, email: email.clone() },
        bot: None,
        letter_of: String::new(),
        app_of: String::new(),
    };
    let mut items = Vec::new();
    for b in st.bots.get_untracked().into_iter().filter(|b| !b.hidden && !b.thread_id.is_empty()) {
        items.push(PaletteItem { bot: Some(b.clone()), ..row(b.name.clone(), "Bot".into(), b.thread_id.clone()) });
    }
    for c in untracked(|| st.chats()) {
        let name = if c.name.is_empty() { "New chat".to_owned() } else { c.name.clone() };
        items.push(PaletteItem { letter_of: name.clone(), ..row(name, "Chat".into(), c.id.clone()) });
    }
    st.settings_open.set(false);
    st.open_palette_pick(format!("Run {} in…", t.name), items);
}

/// The chat picked for a template (`POST /routine/from-template`); then
/// Settings comes back on Routines with it in the list. The server keeps
/// the Google address for the next one.
pub(crate) fn enable_template(server: String, id: String, thread: String, place: String, email: String) {
    Owner::detached().run(|| {
        spawn(async move {
            let mut body = serde_json::json!({ "server": server, "id": id, "thread": thread });
            if !email.is_empty() {
                body["user_google_email"] = email.into();
            }
            let v = crate::client::post_any("/routine/from-template", body).await;
            let st = store();
            let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned();
            match v {
                Some(v) if v.get("ok").and_then(|x| x.as_bool()) == Some(true) => {
                    let name = v.get("routine").map(|r| s(r, "name")).unwrap_or_default();
                    crate::toast::show(format!("{name} runs in {place}"));
                    st.routine_ask_email.set(None);
                    st.routine_email.set(String::new());
                }
                Some(v) => {
                    if s(&v, "need") == "user_google_email" {
                        st.routine_ask_email.set(Some((server, id)));
                    }
                    let error = s(&v, "error");
                    crate::toast::show(if error.is_empty() { "Couldn't enable the routine".to_owned() } else { error });
                }
                None => crate::toast::show("The app server didn't answer".to_owned()),
            }
            st.settings_page.set("routines".to_owned());
            st.settings_open.set(true);
            load_page();
        })
    });
}

/// On / off (`POST /routine/update {id, enabled}`), shown at once.
fn set_routine_enabled(id: String, enabled: bool) {
    store().routines.update(|l| {
        for r in l.iter_mut().filter(|r| r.id == id) {
            r.enabled = enabled;
        }
    });
    Owner::detached().run(|| {
        spawn(async move {
            let v = crate::client::post("/routine/update", serde_json::json!({ "id": id, "enabled": enabled })).await;
            if !v.as_ref().is_some_and(|v| v.get("ok").and_then(|x| x.as_bool()) == Some(true)) {
                crate::toast::show("Couldn't change the routine".to_owned());
            }
            load_routines();
        })
    });
}

/// Run now (`POST /routine/run {id}`): a tick at once; its turn posts in
/// its chat.
fn run_routine(id: String) {
    Owner::detached().run(|| {
        spawn(async move {
            let v = crate::client::post_any("/routine/run", serde_json::json!({ "id": id })).await;
            match v {
                Some(v) if v.get("ok").and_then(|x| x.as_bool()) == Some(true) => crate::toast::show("Running the routine".to_owned()),
                v => {
                    let error = v.and_then(|v| v.get("error").and_then(|x| x.as_str()).map(str::to_owned)).unwrap_or_default();
                    crate::toast::show(if error.is_empty() { "Couldn't run the routine".to_owned() } else { error });
                }
            }
            load_routines();
        })
    });
}

/// Delete (`POST /routine/delete {id}`).
fn delete_routine(id: String) {
    store().routines.update(|l| l.retain(|r| r.id != id));
    Owner::detached().run(|| {
        spawn(async move {
            let v = crate::client::post("/routine/delete", serde_json::json!({ "id": id })).await;
            if !v.as_ref().is_some_and(|v| v.get("ok").and_then(|x| x.as_bool()) == Some(true)) {
                crate::toast::show("Couldn't delete the routine".to_owned());
            }
            load_routines();
        })
    });
}

fn routines_page() -> impl IntoElement {
    view()
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_4()
                .pt_4()
                .pb_2()
                .child(
                    text("Routines")
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::TEXT)
                        .flex_1(),
                )
                .child(
                    text("Refresh")
                        .text_size(px(12.0))
                        .text_color(theme::MUTED)
                        .px_2()
                        .py_1()
                        .rounded(px(6.0))
                        .bg(theme::HOVER)
                        .on_press(load_page),
                ),
        )
        .child(
            text("Ask the agent to make one: \"check my BIR confirmation emails every minute\".")
                .text_size(px(12.0))
                .text_color(theme::MUTED)
                .px_4()
                .pb_2(),
        )
        .child(scroll().flex_1().child(view().child(suggested()).child(dynamic(|| {
            let list = store().routines.get();
            if list.is_empty() {
                return text("No routines yet")
                    .text_size(px(13.0))
                    .text_color(theme::GHOST)
                    .px_4()
                    .py_3()
                    .into_element();
            }
            view().px_4().pb_4().gap(px(8.0)).children(list.into_iter().map(routine_row)).into_element()
        }))))
}

/// Suggested: the servers' templates, none when there are none.
fn suggested() -> impl IntoElement {
    dynamic(|| {
        let list = store().routine_templates.get();
        if list.is_empty() {
            return view().into_element();
        }
        view()
            .px_4()
            .pb_4()
            .gap(px(8.0))
            .child(text("Suggested").text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::MUTED))
            .children(list.into_iter().map(template_row))
            .into_element()
    })
}

/// A template: its name and Enable; its server and interval; what it
/// does; why it can't run yet; the Google address when Enable asks.
fn template_row(t: RoutineTemplate) -> impl IntoElement {
    let (server, id) = (t.server.clone(), t.id.clone());
    let asking = move || store().routine_ask_email.get().is_some_and(|(s, i)| s == server && i == id);
    let (t1, t2, t3) = (t.clone(), t.clone(), t.clone());
    let hov = signal(false);
    view()
        .rounded(px(10.0))
        .border_1()
        .border_color(theme::BORDER_S)
        .px(px(12.0))
        .py(px(10.0))
        .gap(px(4.0))
        .label(t.name.clone())
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    text(t.name.clone())
                        .flex_1()
                        .min_w(px(0.0))
                        .number_of_lines(1)
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::TEXT),
                )
                .when(t.available, |v| {
                    v.child(
                        text("Enable")
                            .text_size(px(12.0))
                            .text_color(move || if hov.get() { theme::TEXT } else { theme::MUTED })
                            .px_2()
                            .py_1()
                            .rounded(px(6.0))
                            .bg(theme::HOVER)
                            .on_hover_in(move || hov.set(true))
                            .on_hover_out(move || hov.set(false))
                            .on_press(move || pick_chat(&t1))
                            .label("Enable routine"),
                    )
                }),
        )
        .child(
            text(format!("{} · {}", t.server, interval(t.every_s)))
                .number_of_lines(1)
                .text_size(px(12.0))
                .text_color(theme::MUTED),
        )
        .when(!t.description.is_empty(), |v| {
            v.child(text(t.description.clone()).number_of_lines(3).text_size(px(12.0)).text_color(theme::MUTED))
        })
        .when(!t.available && !t.reason.is_empty(), |v| {
            v.child(text(t.reason.clone()).number_of_lines(3).text_size(px(12.0)).text_color(theme::DOWN))
        })
        .child(dynamic(move || {
            if !asking() {
                return view().into_element();
            }
            let (t2, t3) = (t2.clone(), t3.clone());
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .pt_1()
                .child(
                    input()
                        .auto_focus(true)
                        .value(move || store().routine_email.get())
                        .placeholder("Your Google address, for the Gmail search")
                        .placeholder_color(|| color(theme::GHOST))
                        .text_size(px(13.0))
                        .text_color(theme::TEXT)
                        .on_change(|v: &str| store().routine_email.set(v.to_owned()))
                        .on_submit(move |_: &str| pick_chat(&t2))
                        .flex_1()
                        .h(px(30.0))
                        .px_2()
                        .rounded(px(7.0))
                        .bg(theme::HOVER),
                )
                .child(
                    text("Continue")
                        .text_size(px(12.0))
                        .text_color(theme::TEXT)
                        .px_2()
                        .py(px(6.0))
                        .rounded(px(7.0))
                        .bg(theme::HOVER)
                        .on_press(move || pick_chat(&t3)),
                )
                .into_element()
        }))
}

/// A routine: its name and switch; its chat, interval and last run; what
/// it checks; its last error; Delete.
fn routine_row(r: Routine) -> impl IntoElement {
    let st = store();
    let chat = if !r.thread_title.is_empty() {
        r.thread_title.clone()
    } else {
        st.threads
            .with_untracked(|t| t.iter().find(|t| t.id == r.thread || t.uid == r.thread).map(|t| t.name.clone()))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "A chat".to_owned())
    };
    let chat = if r.bot_name.is_empty() { chat } else { format!("{} · {chat}", r.bot_name) };
    let last = if r.last_run > 0.0 {
        format!("last run {} ago", crate::store::Store::ago(r.last_run))
    } else {
        "not run yet".to_owned()
    };
    let (id, id2, id3, id4, on) = (r.id.clone(), r.id.clone(), r.id.clone(), r.id.clone(), r.enabled);
    let run_hov = signal(false);
    let enabled = move || store().routines.with(|l| l.iter().any(|x| x.id == id && x.enabled));
    let hov = signal(false);
    view()
        .rounded(px(10.0))
        .border_1()
        .border_color(theme::BORDER_S)
        .px(px(12.0))
        .py(px(10.0))
        .gap(px(4.0))
        .label(r.name.clone())
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    text(if r.name.is_empty() { "Routine".to_owned() } else { r.name.clone() })
                        .flex_1()
                        .min_w(px(0.0))
                        .number_of_lines(1)
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::TEXT),
                )
                .child(
                    view()
                        .label(if on { "Turn off" } else { "Turn on" })
                        .child(crate::plugins::tool_switch(enabled, move || set_routine_enabled(id2.clone(), !on))),
                ),
        )
        .child(
            text(format!("{chat} · {} · {last}", r.interval()))
                .number_of_lines(1)
                .text_size(px(12.0))
                .text_color(theme::MUTED),
        )
        .when(!r.check.is_empty(), |v| {
            v.child(
                text(format!("Checks {} — the agent wakes only for new items", r.check))
                    .number_of_lines(1)
                    .text_size(px(12.0))
                    .text_color(theme::MUTED),
            )
        })
        .when(!r.last_error.is_empty(), |v| {
            v.child(text(r.last_error.clone()).number_of_lines(3).text_size(px(12.0)).text_color(theme::DOWN))
        })
        .child(
            view().flex_row().justify_end().gap_1().child(
                text("Run now")
                    .text_size(px(12.0))
                    .text_color(move || if run_hov.get() { theme::TEXT } else { theme::MUTED })
                    .px_2()
                    .py_1()
                    .rounded(px(6.0))
                    .on_hover_in(move || run_hov.set(true))
                    .on_hover_out(move || run_hov.set(false))
                    .on_press(move || run_routine(id4.clone()))
                    .label("Run routine now"),
            )
            .child(
                text("Delete")
                    .text_size(px(12.0))
                    .text_color(move || if hov.get() { theme::DOWN } else { theme::MUTED })
                    .px_2()
                    .py_1()
                    .rounded(px(6.0))
                    .on_hover_in(move || hov.set(true))
                    .on_hover_out(move || hov.set(false))
                    .on_press(move || delete_routine(id3.clone()))
                    .label("Delete routine"),
            ),
        )
}
