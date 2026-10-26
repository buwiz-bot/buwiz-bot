//! The composer (Grok's, App.tsx:2153): one card — the reply chip, the
//! attachment strip, then the input with `+` at the lower-left and
//! ModelPill + dictate + send/voice at the lower-right. A short draft
//! overlays the controls on the single input row (its padding reserves
//! them); `composerTall` (a newline, 64+ chars, a reply or attachments)
//! moves them into a 40pt band under the input. The agent status row
//! floats above the card. The `+` opens the attach menu at the button.

use nsh_core::ui::prelude::*;

use crate::icons::{self, icon};
use crate::live::store;
use crate::statusfx;
use crate::store::AtItem;
use crate::theme;

fn right_w() -> Signal<f32> {
    store().composer_right_w
}
/// `dockH` — the floating composer's height; the transcript pads its
/// end by it (App.tsx:179).
pub(crate) fn dock_h() -> Signal<f32> {
    store().composer_dock_h
}
/// `plusMenu` — the + button's measured page position.
pub(crate) fn plus_menu() -> Signal<Option<(f32, f32)>> {
    store().plus_menu
}
fn plus_hover() -> Signal<String> {
    store().plus_hover
}
fn attach_hover() -> Signal<String> {
    store().attach_hover
}
fn at_items() -> Signal<Vec<AtItem>> {
    store().at_items
}
fn at_start() -> Signal<i64> {
    store().at_start
}
fn at_seq() -> Signal<u64> {
    store().at_seq
}

fn json_s(v: &serde_json::Value, k: &str) -> String {
    v.get(k)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_owned()
}

/// `composerTall` — several lines, a long line, or a reply / attachments
/// (App.tsx:491).
fn composer_tall() -> bool {
    let st = store();
    st.draft.get().contains('\n')
        || st.draft.get().len() > 64
        || st.reply_to.get().is_some()
        || !st.attachments.get().is_empty()
}

/// `sendFill` — the bot's color, or the adaptive light fill on neutral
/// bots (white / gray / plain chats) (App.tsx:254).
fn send_fill() -> AdaptiveColor {
    let bot_color = store()
        .active_bot()
        .and_then(|b| b.color.parse::<Hsla>().ok());
    let neutral = match &bot_color {
        None => true,
        Some(c) => {
            let hex = c.to_hex().to_uppercase();
            hex == "#FFFFFF" || hex == "#777777"
        }
    };
    if neutral {
        theme::TEXT
    } else {
        bot_color.map(AdaptiveColor::from).unwrap_or(theme::BUBBLE)
    }
}
/// `sendInk` (App.tsx:255).
fn send_ink() -> AdaptiveColor {
    let bot_color = store()
        .active_bot()
        .and_then(|b| b.color.parse::<Hsla>().ok());
    let neutral = match &bot_color {
        None => true,
        Some(c) => {
            let hex = c.to_hex().to_uppercase();
            hex == "#FFFFFF" || hex == "#777777"
        }
    };
    if neutral {
        theme::tone(theme::hsla(0.0, 0.0, 1.0, 1.0), theme::hsla(0.0, 0.0, 0.0706, 1.0))
    } else {
        theme::tone(theme::hsla(0.0, 0.0, 1.0, 1.0), theme::hsla(0.0, 0.0, 1.0, 1.0))
    }
}

/// `tc("#f5f5f5<a>")`.
fn tc(a: f32) -> AdaptiveColor {
    theme::tone(theme::hsla(0.0, 0.0, 0.11, a), theme::hsla(0.0, 0.0, 0.96, a))
}

/// `composerTip` — 12pt on the elevated surface, above the button
/// (App.tsx:499).
fn composer_tip(label: &str) -> impl IntoElement {
    view()
        .absolute()
        .bottom(px(34.0))
        .right(px(0.0))
        .py(px(4.0))
        .px(px(6.0))
        .rounded(px(6.0))
        .bg(theme::CARD)
        .border(px(0.5))
        .border_color(theme::BORDER_F)
        .child(
            text(label.to_owned())
                .number_of_lines(1)
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(theme::TEXT),
        )
}

/// The bottom dock (App.tsx:2157).
pub(crate) fn composer() -> impl IntoElement {
    view()
        .absolute()
        .bottom(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .pointer_events(PointerEvents::BoxNone)
        .on_layout(|b: &Bounds<Points>| dock_h().set(b.size.height.0))
        // agent status row — the mark + shimmering label (App.tsx:2164).
        .child(
            view()
                .self_center()
                .w_full()
                .max_w(px(992.0))
                .px(px(20.0))
                .pb(px(4.0))
                .child({
                    // Rebuilt only when a run starts/ends or its bot changes:
                    // a rebuild per status tick recreated the mark (its
                    // animation restarting) and the label's canvas (blank
                    // until measured again). State and label are read live
                    // inside.
                    let key = computed(|| {
                        let st = store();
                        let run = st.agent()?;
                        let bot = st.active_bot();
                        let seed = run
                            .bot
                            .or_else(|| bot.as_ref().map(|b| b.id.clone()))
                            .unwrap_or_else(|| "status".to_owned());
                        // A plain chat has no bot: Grok's own mark, white on
                        // dark and black on light.
                        let (shape, color) = bot.map(|b| (b.shape, b.color)).unwrap_or_else(|| {
                            ("blob".into(), "#FFFFFF".into())
                        });
                        Some((seed, shape, color))
                    });
                    dynamic(move || {
                        let Some((seed, shape, color)) = key.get() else {
                            return view().into_element();
                        };
                        statusfx::status_fx(
                            shape,
                            color,
                            std::rc::Rc::new(|| {
                                store().agent().map(|a| a.state).unwrap_or_default()
                            }),
                            20.0,
                            Some(seed),
                            move || store().agent().map(|a| a.label).unwrap_or_default(),
                        )
                        .into_element()
                    })
                }),
        )
        // the dock shares the transcript's column (App.tsx:2188).
        .child(
            view()
                .self_center()
                .w_full()
                .max_w(px(992.0))
                .px_4()
                .pb_4()
                .child(dock_card()),
        )
}

/// The card: radius 18, 0.5px border at 12%, a soft shadow; the reply
/// chip, attachment strip and input area live inside (App.tsx:2189).
fn dock_card() -> View {
    view()
        .bg(theme::CARD)
        .rounded(px(18.0))
        .border(px(0.5))
        .border_color(theme::BORDER_S)
        // `0 2px 8px -1px rgba(0,0,0,0.12), 0 1px 2px rgba(0,0,0,0.08)`.
        .shadow([
            BoxShadow {
                offset_x: px(0.0),
                offset_y: px(2.0),
                blur: px(8.0),
                spread: px(-1.0),
                color: hsla(0.0, 0.0, 0.0, 0.12).into(),
                inset: false,
            },
            BoxShadow {
                offset_x: px(0.0),
                offset_y: px(1.0),
                blur: px(2.0),
                spread: px(0.0),
                color: hsla(0.0, 0.0, 0.0, 0.08).into(),
                inset: false,
            },
        ])
        .overflow_hidden()
        .child(reply_chip())
        .child(attachment_strip())
        .child(token_strip())
        .child(input_area())
}

/// The reply chip inside the card (App.tsx:2199): 💬︎ + the collapsed
/// quote + a close.
fn reply_chip() -> impl IntoElement {
    dynamic(|| {
        let Some(r) = store().reply_to.get() else {
            return view().into_element();
        };
        let quote: String = r
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(120)
            .collect();
        view()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .pt(px(10.0))
            .px_3()
            .child(text("💬︎").text_size(px(12.0)).text_color(tc(0.54)))
            .child(
                text(format!("“{quote}”"))
                    .number_of_lines(1)
                    .flex_1()
                    .text_size(px(13.0))
                    .line_height(px(18.0))
                    .text_color(tc(0.54)),
            )
            .child(
                view()
                    .p_1()
                    .label("Cancel reply")
                    .on_press(|| store().reply_to.set(None))
                    .child(icon("close")),
            )
            .into_element()
    })
}

/// The attachment strip — a horizontal row of 64pt `FileTile`s, each
/// with a hover ✕ badge (App.tsx:2210).
fn attachment_strip() -> impl IntoElement {
    dynamic(|| {
        if store().attachments.get().is_empty() {
            return view().into_element();
        }
        scroll()
            .horizontal()
            .child(
                view()
                    .flex_row()
                    .py(px(6.0))
                    .px_3()
                    .gap_2()
                    .child(each(
                        || store().attachments.get(),
                        |a| format!("{}:{}", a.url, a.name),
                        |a, _| attach_tile(a),
                    )),
            )
            .into_element()
    })
}

/// One 64pt tile + the hover remove badge (App.tsx:2214).
fn attach_tile(a: ReadSignal<crate::store::Attach>) -> impl IntoElement {
    let key = {
        let a = a.get();
        if a.url.is_empty() { a.path.clone() } else { a.url.clone() }
    };
    let key_in = key.clone();
    let key_out = key.clone();
    let key_show = key.clone();
    let label = a.get().name.clone();
    view()
        .size(px(64.0))
        .label(if label.is_empty() { key.clone() } else { label })
        .on_hover_in(move || attach_hover().set(key_in.clone()))
        .on_hover_out(move || {
            if attach_hover().get_untracked() == key_out {
                attach_hover().set(String::new())
            }
        })
        .child(crate::files::file_tile(&a.get(), 64.0))
        .child(when(
            move || attach_hover().get() == key_show,
            move || {
                let key = a.get().url.clone();
                view()
                    .absolute()
                    .top(px(2.0))
                    .right(px(3.0))
                    .size(px(20.0))
                    .rounded(px(10.0))
                    .bg(theme::TEXT)
                    .items_center()
                    .justify_center()
                    .label("Remove")
                    .on_press(move || {
                        store()
                            .attachments
                            .update(|l| l.retain(|x| x.url != key))
                    })
                    .child(
                        text("✕")
                            .text_size(px(10.0))
                            .text_color(theme::tone(
                                theme::hsla(0.0, 0.0, 0.96, 1.0),
                                theme::hsla(0.0, 0.0, 0.07, 1.0),
                            )),
                    )
            },
        ))
}

/// The input area: the `/` and `@` popup, a command's error, the capped
/// input wrapper, and the absolute controls (App.tsx:2247).
fn input_area() -> impl IntoElement {
    view()
        .child(pop_picker())
        // the wrapper carries the cap: the input's box otherwise grows
        // to all its text (App.tsx:2274).
        .child(
            view()
                .pb(move || px(if composer_tall() { 40.0 } else { 0.0 }))
                .max_h(move || px(if composer_tall() { 174.0 } else { 40.0 }))
                .overflow_hidden()
                .child(the_input()),
        )
        .child(plus_button())
        .child(right_controls())
}

thread_local! {
    /// The message field, for ⌘L.
    static FIELD: std::cell::Cell<Option<ElementRef>> = const { std::cell::Cell::new(None) };
}

/// ⌘L: the keyboard into the message field (Grok Bot's).
pub(crate) fn focus() {
    if let Some(field) = FIELD.with(std::cell::Cell::get) {
        field.focus();
    }
}

/// The caret out of the message field: a press on a choice card, so its
/// letter keys answer the card (`keys::choice_keys`).
pub(crate) fn blur() {
    if let Some(field) = FIELD.with(std::cell::Cell::get) {
        field.blur();
    }
}

/// `Input` — 14/20, capped at 6 lines; short drafts keep one row
/// between + and the controls, tall ones go full width above the
/// controls band (App.tsx:2275).
fn the_input() -> Input {
    let field = element_ref();
    FIELD.with(|f| f.set(Some(field)));
    input()
        .element_ref(&field)
        .label("Message input")
        .value(move || store().draft.get())
        .multiline(true)
        .submit_behavior(if is_desktop() {
            SubmitBehavior::Submit
        } else {
            SubmitBehavior::Newline
        })
        .return_key_type(ReturnKeyType::Send)
        .placeholder(move || {
            let st = store();
            if st.reply_to.get().is_some() {
                "Reply…".to_owned()
            } else if !st.attachments.get().is_empty() {
                "Add a message, or hit send".to_owned()
            } else if let Some(b) = st.active_bot() {
                format!("Message {}", b.name)
            } else {
                // A plain chat talks to the app itself.
                format!("Message {}", crate::app_name())
            }
        })
        .placeholder_color(|| color(tc(0.54)))
        .text_size(px(14.0))
        .line_height(px(20.0))
        .text_color(theme::TEXT)
        .on_change(move |v: &str| draft_changed(v))
        .on_submit(move |_: &str| store().send())
        .pl(move || px(if composer_tall() { 12.0 } else { 44.0 }))
        .pr(move || {
            px(if composer_tall() {
                12.0
            } else {
                right_w().get() + 16.0
            })
        })
        .pt(px(10.0))
        .pb(move || px(if composer_tall() { 4.0 } else { 10.0 }))
        .min_h(move || px(if composer_tall() { 34.0 } else { 40.0 }))
        .max_h(move || px(if composer_tall() { 134.0 } else { 40.0 }))
}

/// The `+` button — measures itself and opens the menu at its page
/// position (App.tsx:2308).
fn plus_button() -> impl IntoElement {
    let node = element_ref();
    view()
        .absolute()
        .left(px(8.0))
        .bottom(px(6.0))
        .child(
            view()
                .element_ref(&node)
                .size(px(28.0))
                .rounded(px(14.0))
                .bg(tc(0.06))
                .items_center()
                .justify_center()
                .label("Composer actions")
                .on_press(move || {
                    spawn(async move {
                        if let Some(m) = node.measure().await {
                            plus_menu().set(Some((m.page.x.0, m.page.y.0)));
                        }
                    });
                })
                .child(icon("plus")),
        )
}

/// The lower-right row: `ModelPill` + dictate + send/voice
/// (App.tsx:2322). `onLayout` keeps `rightW` for the input's padding.
fn right_controls() -> impl IntoElement {
    view()
        .absolute()
        .right(px(8.0))
        .bottom(px(6.0))
        .flex_row()
        .items_center()
        .gap_2()
        .on_layout(|b: &Bounds<Points>| right_w().set(b.size.width.0))
        .child(crate::models::model_pill())
        .child(
            // dictate — a stub like ⌘D's "coming soon" run.
            view()
                .size(px(28.0))
                .rounded(px(14.0))
                .items_center()
                .justify_center()
                .label("Dictate")
                .on_hover_in(|| attach_hover().set("__dictate".into()))
                .on_hover_out(|| attach_hover().set(String::new()))
                .on_press(|| store().dictate_stub())
                .child(icon("mic"))
                .child(when(
                    || attach_hover().get() == "__dictate",
                    || composer_tip("Click or hold ⌘D to dictate"),
                )),
        )
        .child(dynamic(|| {
            let st = store();
            let has = !st.draft.get().trim().is_empty() || !st.attachments.get().is_empty();
            if has {
                view()
                    .size(px(28.0))
                    .rounded(px(14.0))
                    .bg(move || send_fill())
                    .items_center()
                    .justify_center()
                    .label("Send message")
                    .on_press(|| store().send())
                    .child(icons::icon_ink("arrowUp", 15.0, send_ink))
                    .into_element()
            } else {
                // voice chat — the waveform glyph; decorative for now.
                view()
                    .size(px(28.0))
                    .rounded(px(14.0))
                    .bg(move || send_fill())
                    .items_center()
                    .justify_center()
                    .label("Start voice chat")
                    .on_hover_in(|| attach_hover().set("__voice".into()))
                    .on_hover_out(|| attach_hover().set(String::new()))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(1.5))
                            .children([5.0f32, 9.0, 13.0, 9.0, 5.0].into_iter().map(|h| {
                                view()
                                    .w(px(1.8))
                                    .h(px(h))
                                    .rounded(px(1.0))
                                    .bg(move || send_ink())
                                    .into_element()
                            })),
                    )
                    .child(when(
                        || attach_hover().get() == "__voice",
                        || composer_tip("Start voice chat"),
                    ))
                    .into_element()
            }
        }))
}

/// `draftChanged` — the draft's signal, then the popup: `/name` lists
/// commands; `@<query>` the servers with `@` scopes and the
/// artifacts/files (App.tsx:463), `@server:…` that server's scopes and
/// values (slash.rs).
pub(crate) fn draft_changed(t: &str) {
    let st = store();
    st.draft.set(t.to_owned());
    if crate::slash::slash_menu(t) {
        at_items().set(Vec::new());
        at_start().set(-1);
        return;
    }
    // the @ the query began at, while it's still there
    let i = match at_start().get_untracked() {
        s if s >= 0 && (s as usize) < t.len() && t.as_bytes()[s as usize] == b'@' => s as usize,
        _ => t.rfind('@').unwrap_or(usize::MAX),
    };
    if i == usize::MAX {
        crate::slash::close();
        return;
    }
    let tail = &t[i + 1..];
    if tail.chars().any(|c| c.is_whitespace()) || tail.len() > 80 {
        crate::slash::close();
        return;
    }
    at_start().set(i as i64);
    let n = at_seq().get_untracked() + 1;
    at_seq().set(n);
    if crate::slash::token_menu(tail) {
        at_items().set(Vec::new());
        return;
    }
    crate::slash::server_menu(tail);
    let thread = st.active_id.get_untracked();
    let q = tail.to_owned();
    spawn(async move {
        let v = crate::client::get(&format!(
            "/artifacts?thread={}&q={}",
            crate::client::urlenc(&thread),
            crate::client::urlenc(&q)
        ))
        .await;
        if at_seq().get_untracked() != n {
            return;
        }
        let items: Vec<AtItem> = v
            .and_then(|v| {
                v.get("items").and_then(|x| x.as_array()).map(|l| {
                    l.iter()
                        .map(|x| AtItem {
                            kind: json_s(x, "kind"),
                            name: json_s(x, "name"),
                            title: json_s(x, "title"),
                            file: json_s(x, "file"),
                            v: x.get("v").and_then(|n| n.as_i64()).unwrap_or(0),
                            uri: json_s(x, "uri"),
                            scope: json_s(x, "scope"),
                        })
                        .collect()
                })
            })
            .unwrap_or_default();
        at_items().set(items);
    });
}

/// `atPick` — the @ tail becomes the item's title and its uri stages as
/// an attachment the server resolves (App.tsx:476).
pub(crate) fn at_pick(it: &AtItem) {
    let st = store();
    let t = st.draft.get_untracked();
    let i = at_start().get_untracked().max(0) as usize;
    let name = if it.title.is_empty() {
        it.name.clone()
    } else {
        it.title.clone()
    };
    st.draft.set(format!("{}{} ", &t[..i.min(t.len())], name));
    crate::slash::close();
    st.attachments.update(|a| {
        a.push(crate::store::Attach {
            kind: "file".to_owned(),
            name: name.clone(),
            url: it.uri.clone(),
            ..Default::default()
        })
    });
}

/// The `/` and `@` popup — commands, servers, scopes, values or a
/// resolve's choices, then this thread's artifacts first, then global
/// (App.tsx:2250); five rows show, the rest scroll in with the selection.
fn pop_picker() -> impl IntoElement {
    dynamic(|| {
        let rows = crate::slash::rows();
        if rows.is_empty() {
            return view().into_element();
        }
        let head = store().pop_head.get();
        let sel = store().pop_sel.get().min(rows.len() - 1);
        let start = crate::slash::first_shown(sel);
        view()
            .mb(px(6.0))
            .rounded(px(10.0))
            .bg(theme::CARD)
            .border(px(0.5))
            .border_color(theme::BORDER_F)
            .py(px(4.0))
            .overflow_hidden()
            .when(!head.is_empty(), |v| {
                v.child(
                    text(head)
                        .number_of_lines(1)
                        .px(px(10.0))
                        .py(px(4.0))
                        .text_size(px(12.0))
                        .text_color(tc(0.54)),
                )
            })
            .children(
                rows.into_iter()
                    .enumerate()
                    .skip(start)
                    .take(crate::slash::VISIBLE)
                    .map(|(i, r)| pop_row(r, i == sel, i - start + 1)),
            )
            .into_element()
    })
}

/// A popup row; `n` its number among the rows shown (⌘n picks it).
fn pop_row(r: crate::slash::PopRow, selected: bool, n: usize) -> impl IntoElement {
    use crate::slash::Pick;
    let tile = match &r.pick {
        Pick::Artifact(it) if it.kind == "app" => crate::appdetails::app_icon(&it.name, 20.0).into_element(),
        p => view()
            .size(px(20.0))
            .rounded(px(5.0))
            .items_center()
            .justify_center()
            .bg(tc(0.10))
            .child(
                text(match p {
                    Pick::Artifact(_) => "doc",
                    Pick::Builtin(_) | Pick::Command(..) => "/",
                    Pick::App(..) => "</>",
                    Pick::Choice(..) | Pick::Value(..) => "›",
                    _ => "@",
                })
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tc(0.54)),
            )
            .into_element(),
    };
    let r2 = r.clone();
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px(px(10.0))
        .py(px(6.0))
        .bg(if selected { tc(0.06) } else { AdaptiveColor::TRANSPARENT })
        .on_press(move || crate::slash::pick(&r2))
        .child(tile)
        .child(text(r.label).number_of_lines(1).text_size(px(13.0)).text_color(theme::TEXT))
        .when(!r.hint.is_empty(), |v| {
            v.child(text(r.hint).number_of_lines(1).text_size(px(12.0)).text_color(tc(0.40)))
        })
        .child(
            text(r.detail)
                .number_of_lines(1)
                .flex_1()
                .text_size(px(11.0))
                .text_color(tc(0.40)),
        )
        .child(pick_key(n))
}

/// The key that picks the popup's `n`th shown row (⌘1…).
fn pick_key(n: usize) -> impl IntoElement {
    let id = format!("popup.pick_{n}");
    let key = crate::keys::COMMANDS
        .iter()
        .find(|c| c.id == id)
        .and_then(|c| crate::keys::keys_of(c).into_iter().next());
    match key {
        Some(k) => text(key_label(&k)).text_size(px(11.0)).text_color(theme::FAINT).into_element(),
        None => view().into_element(),
    }
}

/// The draft's `@server:scope:value` tokens as chips — the token's label
/// when it was picked from a list — each with a ✕ that takes it out.
fn token_strip() -> impl IntoElement {
    // rebuilt when the tokens change, not on every keystroke
    let tokens = computed(|| {
        let st = store();
        st.mcp_manifest.with(|_| ());
        st.draft.with(|d| {
            d.split_whitespace()
                .filter(|w| crate::slash::token(w).is_some())
                .map(str::to_owned)
                .collect::<Vec<String>>()
        })
    });
    dynamic(move || {
        let tokens = tokens.get();
        if tokens.is_empty() {
            return view().into_element();
        }
        scroll()
            .horizontal()
            .child(
                view()
                    .flex_row()
                    .pt(px(8.0))
                    .px_3()
                    .gap(px(6.0))
                    .children(tokens.into_iter().map(token_chip)),
            )
            .into_element()
    })
}

fn token_chip(tok: String) -> impl IntoElement {
    let (server, scope, value) = crate::slash::token(&tok).unwrap_or_default();
    let label = crate::slash::token_label(&tok);
    let shown = if label.is_empty() { value } else { label };
    view()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .h(px(22.0))
        .pl(px(8.0))
        .pr(px(4.0))
        .rounded(px(11.0))
        .bg(theme::ACCENT_TINT)
        .label(tok.clone())
        .child(
            text(format!("@{server} · {scope}{}", if shown.is_empty() { String::new() } else { format!(" · {shown}") }))
                .number_of_lines(1)
                .text_size(px(12.0))
                .text_color(theme::ACCENT),
        )
        .child(
            view()
                .size(px(16.0))
                .items_center()
                .justify_center()
                .label("Remove")
                .on_press(move || {
                    let st = store();
                    let words: Vec<String> = st
                        .draft
                        .get_untracked()
                        .split(' ')
                        .filter(|w| *w != tok)
                        .map(str::to_owned)
                        .collect();
                    st.draft.set(words.join(" "));
                })
                .child(text("✕").text_size(px(10.0)).text_color(theme::ACCENT)),
        )
}

/// `plusMenu` — the small card just above the + button
/// (App.tsx:3368): Attach files · Teach a task. Mounted at the root.
pub(crate) fn plus_layer() -> impl IntoElement {
    dynamic(|| {
        let Some((x, y)) = plus_menu().get() else {
            return view().into_element();
        };
        view()
            .size_full()
            .absolute()
            .child(
                view()
                    .size_full()
                    .on_press(|| plus_menu().set(None)),
            )
            .child(
                view()
                    .absolute()
                    .left(px(x - 4.0))
                    .top(px(y - 6.0 - 64.0))
                    .w(px(172.0))
                    .p_1()
                    .rounded(px(10.0))
                    .bg(theme::CARD)
                    .border(px(0.5))
                    .border_color(theme::BORDER_F)
                    // `0 8px 20px -4px rgba(0,0,0,0.4)`.
                    .shadow([BoxShadow {
                        offset_x: px(0.0),
                        offset_y: px(8.0),
                        blur: px(20.0),
                        spread: px(-4.0),
                        color: hsla(0.0, 0.0, 0.0, 0.4).into(),
                        inset: false,
                    }])
                    .on_press(|| {})
                    .child(plus_row(
                        "attach",
                        "Attach files",
                        true,
                        || store().pick_attachments(),
                    ))
                    .child(plus_row("teach", "Teach a task", false, || {})),
            )
            .into_element()
    })
}

/// One `+`-menu row — the teach row is disabled at 0.5 with a red
/// record dot for an icon (App.tsx:3411).
fn plus_row(id: &str, label: &str, enabled: bool, run: impl Fn() + 'static) -> impl IntoElement {
    let id = id.to_owned();
    let id2 = id.clone();
    let id3 = id.clone();
    let label = label.to_owned();
    let is_teach = id == "teach";
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(28.0))
        .px_2()
        .rounded(px(6.0))
        .bg(move || {
            if enabled && plus_hover().get() == id {
                tc(0.06)
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .opacity(if enabled { 1.0 } else { 0.5 })
        .on_hover_in(move || plus_hover().set(id2.clone()))
        .on_hover_out(move || {
            if plus_hover().get_untracked() == id3 {
                plus_hover().set(String::new())
            }
        })
        .on_press(move || {
            if !enabled {
                return;
            }
            plus_menu().set(None);
            run();
        })
        .child(
            view()
                .w(px(14.0))
                .items_center()
                .child(if is_teach {
                    // a bordered circle with a red center (App.tsx:3413).
                    view()
                        .size(px(11.0))
                        .rounded(px(6.0))
                        .border(px(1.5))
                        .border_color(theme::tone(
                            theme::hsla(0.98, 0.82, 0.57, 1.0),
                            theme::hsla(0.98, 0.82, 0.57, 1.0),
                        ))
                        .items_center()
                        .justify_center()
                        .child(
                            view()
                                .size(px(5.0))
                                .rounded(px(3.0))
                                .bg(theme::tone(
                                    theme::hsla(0.98, 0.82, 0.57, 1.0),
                                    theme::hsla(0.98, 0.82, 0.57, 1.0),
                                )),
                        )
                        .into_element()
                } else {
                    icon("attach").into_element()
                }),
        )
        .child(
            text(label)
                .text_size(px(13.0))
                .line_height(px(18.0))
                .text_color(theme::TEXT),
        )
}
