//! Settings (⌘,): a modal with its pages down the left. Keyboard lists
//! every command by where it applies, with the keys it answers to: press a
//! key chip and then the new keys to change it, × to drop one, + to add
//! one, Reset to go back to the defaults. A key another command in the
//! same place used moves over (the note under the list says from where).
//!
//! Routines aren't here: a routine belongs to a bot, so they're managed in
//! the bot chat's Details pane (`routines.rs`, `pane.rs`).

use nsh_base::input;
use nsh_core::prelude::*;

use crate::keys::{self, COMMANDS, Scope};
use crate::live::store;
use crate::theme;

const PAGES: [(&str, &str); 2] = [("keyboard", "Keyboard"), ("logins", "Logins")];

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
                        if page == "logins" {
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
                .on_press(move || store().settings_page.set(id.to_owned()))
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
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
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
