//! The model picker: `model_pill` (used in the pane and the composer) —
//! the maker's logo, the short name and the effort, with the full name,
//! route and id on hover — opens a root-anchored popover: the effort card
//! (⚡ fast toggle, effort name, "model ›", reset, a stepped slider that
//! turns violet at the top rung) or the model combobox (search; Default and
//! the subscription models ungrouped, API connections grouped). The
//! catalog is opencodex's (`GET /models`).

use std::str::FromStr;

use nsh_base::input;
use nsh_core::prelude::*;

use crate::live::store;
use crate::store::ModelsOpen;
use crate::theme;

const W: f32 = 222.0;
const TRACK: f32 = 200.0;
const KNOB: f32 = 22.0;
const TOP_TINT: &str = "#a78bfa";
/// The effort's color below the top rung.
const EFFORT_TINT: &str = "#1e86fc";
/// The model combobox's width.
const LIST_W: f32 = 268.0;

/// `tc("#f5f5f5<a>")`.
fn tc(a: f32) -> AdaptiveColor {
    theme::tone(theme::hsla(0.0, 0.0, 0.11, a), theme::hsla(0.0, 0.0, 0.96, a))
}

fn hx(hex: &str) -> AdaptiveColor {
    AdaptiveColor::from(Hsla::from_str(hex).unwrap_or(theme::ACCENT.dark))
}

fn mp_open() -> Signal<Option<ModelsOpen>> {
    store().models_pop
}

fn choice() -> (String, String, bool) {
    store().choice()
}

/// A model's catalog row. "" is the Default choice (the server's default
/// model); a bare id matches its connection-prefixed row
/// (`claude-haiku-5-5` → `anthropic-apikey/claude-haiku-5-5`).
fn model_of(id: &str) -> Option<crate::store::ModelDef> {
    let st = store();
    let id = if id.is_empty() { st.default_model.get() } else { id.to_owned() };
    let models = st.models.get();
    let suffix = format!("/{id}");
    models
        .iter()
        .find(|m| m.id == id)
        .or_else(|| models.iter().find(|m| m.id.ends_with(&suffix)))
        .cloned()
}

/// The model's maker, as an icon of `icons.rs` (its logo).
pub(crate) fn maker_logo(id: &str) -> Option<&'static str> {
    let m = id.rsplit('/').next().unwrap_or(id).to_ascii_lowercase();
    let has = |p: &[&str]| p.iter().any(|x| m.starts_with(x));
    Some(if has(&["gpt", "o1", "o3", "o4", "chatgpt", "codex", "davinci"]) {
        "logoOpenai"
    } else if has(&["claude"]) {
        "logoAnthropic"
    } else if has(&["grok"]) {
        "logoXai"
    } else if has(&["gemini", "gemma"]) {
        "logoGemini"
    } else if has(&["deepseek"]) {
        "logoDeepseek"
    } else if has(&["kimi", "moonshot"]) {
        "logoKimi"
    } else if has(&["qwen", "qwq"]) {
        "logoQwen"
    } else if has(&["mistral", "codestral", "devstral", "magistral", "ministral"]) {
        "logoMistral"
    } else if has(&["llama"]) {
        "logoMeta"
    } else if has(&["glm", "chatglm"]) {
        "logoZhipu"
    } else if has(&["minimax", "abab"]) {
        "logoMinimax"
    } else {
        return None;
    })
}

/// The name a row and the pill show: Claude's family and version without
/// the "Claude" ("Opus 5.5"), dated snapshots without the date.
pub(crate) fn short_name(id: &str) -> String {
    let base = id.rsplit('/').next().unwrap_or(id);
    let base = base.trim_end_matches("--fast");
    // drop a trailing -YYYYMMDD snapshot date
    let base = match base.rsplit_once('-') {
        Some((head, tail)) if tail.len() == 8 && tail.chars().all(|c| c.is_ascii_digit()) => head,
        _ => base,
    };
    if let Some(rest) = base.strip_prefix("claude-") {
        let mut parts = rest.split('-');
        let family = parts.next().unwrap_or("");
        let version: Vec<&str> = parts.collect();
        let mut c = family.chars();
        let fam = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
        return if version.is_empty() { fam } else { format!("{fam} {}", version.join(".")) };
    }
    crate::store::pretty_model(base)
}

/// The full name of the hover card ("Claude Opus 5.5").
pub(crate) fn full_name(id: &str) -> String {
    let short = short_name(id);
    if id.rsplit('/').next().unwrap_or(id).starts_with("claude-") {
        format!("Claude {short}")
    } else {
        short
    }
}

/// `id`'s logo at `size` in `ink`; an empty seat when the maker is unknown.
fn logo(id: &str, size: f32, ink: impl Fn() -> AdaptiveColor + 'static) -> AnyElement {
    match maker_logo(id) {
        Some(name) => crate::icons::icon_ink(name, size, ink),
        None => view().size(px(size)).into_element(),
    }
}

/// The model the choice runs: the picked one, else the default.
fn shown_model() -> String {
    let (m, _, _) = choice();
    if m.is_empty() { store().default_model.get() } else { m }
}

fn effort_label() -> String {
    let (m, e, _) = choice();
    model_of(&m)
        .and_then(|d| {
            d.efforts
                .iter()
                .find(|x| x.0 == e)
                .map(|x| x.1.clone())
        })
        .unwrap_or_default()
}

/// `isTop` — the model's last rung (Ultra / Max) is the special one.
fn is_top() -> bool {
    let (m, e, _) = choice();
    model_of(&m)
        .map(|d| d.efforts.len() > 1 && d.efforts.last().map(|x| x.0.as_str()) == Some(e.as_str()))
        .unwrap_or(false)
}

fn efforts() -> Vec<(String, String)> {
    let (m, _, _) = choice();
    model_of(&m).map(|d| d.efforts).unwrap_or_default()
}

fn idx() -> i32 {
    let (_, e, _) = choice();
    efforts()
        .iter()
        .position(|x| x.0 == e)
        .map(|i| i as i32)
        .unwrap_or(0)
        .max(0)
}

fn step() -> f32 {
    (TRACK - KNOB) / (efforts().len() as f32 - 1.0).max(1.0)
}

fn pick(i: i32) {
    let es = efforts();
    let i = i.clamp(0, es.len() as i32 - 1);
    if i < 0 {
        return;
    }
    if let Some(e) = es.get(i as usize) {
        let (m, _, f) = choice();
        store().set_choice(&m, &e.0, f);
    }
}

/// Palette/command entry — the list card centered under the top bar
/// (no pill to anchor to).
pub(crate) fn open_list() {
    let st = store();
    if st.models.get_untracked().is_empty() {
        st.load_models();
    }
    let win = window();
    mp_open().set(Some(ModelsOpen {
        mode: "list".into(),
        x: (win.width.0 - W) / 2.0,
        y: 48.0,
        w: W,
        h: 286.0,
    }));
}

pub(crate) fn close() {
    mp_open().set(None);
}

/// `ROX_MODELS=effort|list` — boots with the popover open mid-window
/// (the dev knob, like `ROX_PANE`/`ROX_PLUGINS`).
pub(crate) fn dev_open(mode: &str) {
    let st = store();
    if st.models.get_untracked().is_empty() {
        st.load_models();
    }
    let win = window();
    mp_open().set(Some(ModelsOpen {
        mode: mode.to_owned(),
        x: (win.width.0 - 28.0) / 2.0,
        y: win.height.0 - 120.0,
        w: 28.0,
        h: 28.0,
    }));
}

/// The model pill: the maker's logo, the short name, the effort (blue,
/// violet at the top rung) and a chevron; pressing measures the pill and
/// opens the popover at it. Hovering shows the full name, the route, the
/// id and the effort.
pub(crate) fn model_pill() -> impl IntoElement {
    let node = element_ref();
    let hovered = signal(false);
    let show_tip = signal(false);
    view()
        .element_ref(&node)
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .h(px(28.0))
        .px(px(10.0))
        .rounded(px(14.0))
        .label("Select model")
        .bg(move || {
            if mp_open().get().is_some() || hovered.get() {
                tc(0.08)
            } else {
                tc(0.04)
            }
        })
        .child(dynamic(|| logo(&shown_model(), 14.0, || tc(0.9))))
        .child(
            text(move || {
                let (m, _, _) = choice();
                if m.is_empty() { "Default".to_owned() } else { short_name(&m) }
            })
            .number_of_lines(1)
            .text_size(px(12.0))
            .line_height(px(16.0))
            .text_color(tc(0.9)),
        )
        .child(when(
            || !effort_label().is_empty(),
            || {
                text(move || effort_label())
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(move || if is_top() { hx(TOP_TINT) } else { hx(EFFORT_TINT) })
            },
        ))
        .child(crate::icons::icon_ink("chevronDown", 10.0, || tc(0.54)))
        // The details card, above the pill after a beat of hovering.
        .child(when(
            move || show_tip.get() && mp_open().get().is_none(),
            || pill_tip().into_element(),
        ))
        .on_hover_in(move || {
            hovered.set(true);
            spawn(async move {
                timer(std::time::Duration::from_millis(450)).await;
                if hovered.get_untracked() {
                    show_tip.set(true);
                }
            });
        })
        .on_hover_out(move || {
            hovered.set(false);
            show_tip.set(false);
        })
        .on_press(move || {
            show_tip.set(false);
            spawn(async move {
                let st = store();
                if st.models.get_untracked().is_empty() {
                    st.load_models();
                }
                if let Some(m) = node.measure().await {
                    let (model, _, _) = st.choice();
                    mp_open().set(Some(ModelsOpen {
                        mode: if model.is_empty() { "list" } else { "effort" }.to_owned(),
                        x: m.page.x.0,
                        y: m.page.y.0,
                        w: m.frame.size.width.0,
                        h: m.frame.size.height.0,
                    }));
                }
            });
        })
}

/// The pill's hover card: full name, route, id, effort.
fn pill_tip() -> impl IntoElement {
    let (m, _, _) = choice();
    let id = if m.is_empty() { store().default_model.get_untracked() } else { m.clone() };
    let def = model_of(&id);
    let route = def
        .as_ref()
        .map(|d| {
            if d.subscription { format!("{} subscription", d.group) } else { format!("{} API", d.group) }
        })
        .unwrap_or_default();
    let effort = effort_label();
    let mut lines: Vec<String> = Vec::new();
    if m.is_empty() {
        lines.push("Default · recommended".to_owned());
    }
    if !route.is_empty() {
        lines.push(format!("Connection: {route}"));
    }
    lines.push(id.clone());
    if !effort.is_empty() {
        lines.push(format!("Effort: {effort}"));
    }
    view()
        .absolute()
        .bottom(px(36.0))
        .left(px(0.0))
        .w(px(250.0))
        .z_index(40)
        .pointer_events(PointerEvents::None)
        .p(px(10.0))
        .gap(px(2.0))
        .rounded(px(10.0))
        .bg(theme::POPOVER)
        .border(px(0.5))
        .border_color(tc(0.12))
        .shadow_lg()
        .child(
            text(full_name(&id))
                .text_size(px(13.0))
                .line_height(px(18.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tc(0.96)),
        )
        .children(lines.into_iter().map(|l| {
            text(l)
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(tc(0.6))
                .number_of_lines(1)
                .into_element()
        }))
}

/// The popover layer — `ModelPopover` mounted at the root
/// (modelpicker.tsx:104).
pub(crate) fn layer() -> impl IntoElement {
    dynamic(|| {
        let Some(o) = mp_open().get() else {
            return view().into_element();
        };
        view()
            .absolute()
            .size_full()
            // scrim — press closes.
            .child(
                view()
                    .absolute()
                    .size_full()
                    .on_press(|| mp_open().set(None)),
            )
            .child(if o.mode == "effort" { effort_card(&o).into_element() } else { list_card(&o).into_element() })
            .into_element()
    })
}

/// `pos(h)` — centered under the trigger, flipped above when it would run
/// off the window (modelpicker.tsx:155).
fn pos(o: &ModelsOpen, w: f32, h: f32) -> (f32, f32) {
    let win = window();
    let left = (o.x + o.w / 2.0 - w / 2.0).clamp(8.0, win.width.0 - w - 8.0);
    let below = o.y + o.h + 6.0;
    let top = if below + h > win.height.0 - 8.0 {
        (o.y - h - 6.0).max(8.0)
    } else {
        below
    };
    (left, top)
}

fn card(o: &ModelsOpen, w: f32, h: f32) -> View {
    let (left, top) = pos(o, w, h);
    view()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(w))
        .rounded(px(12.0))
        .bg(theme::tone(
            theme::hsla(0.0, 0.0, 1.0, 1.0),
            theme::hsla(0.0, 0.0, 0.122, 1.0),
        ))
        // `0 10px 24px -6px rgba(0,0,0,0.45)` (modelpicker.tsx:191).
        .shadow([BoxShadow {
            offset_x: px(0.0),
            offset_y: px(10.0),
            blur: px(24.0),
            spread: px(-6.0),
            color: hsla(0.0, 0.0, 0.0, 0.45).into(),
            inset: false,
        }])
        .border(px(0.5))
        .border_color(tc(0.10))
        .on_press(|| {})
}

/// A card-level tooltip (modelpicker.tsx:161).
fn tip(left: Option<f32>, right: Option<f32>, top: f32, label: &str) -> impl IntoElement {
    let label = label.to_owned();
    let v = view()
        .absolute()
        .top(px(top))
        .py(px(5.0))
        .px_2()
        .rounded(px(8.0))
        .z_index(3)
        .bg(theme::tone(
            theme::hsla(0.0, 0.0, 1.0, 1.0),
            theme::hsla(0.0, 0.0, 0.149, 1.0),
        ))
        .border(px(0.5))
        .border_color(tc(0.10))
        .child(
            text(label)
                .number_of_lines(1)
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(tc(0.96)),
        );
    let v = if let Some(l) = left { v.left(px(l)) } else { v };
    if let Some(r) = right { v.right(px(r)) } else { v }
}

// ————— effort card (modelpicker.tsx:197) —————

fn effort_card(o: &ModelsOpen) -> impl IntoElement {
    // top-effort effects: a 2.4s shimmer sweep + twinkling sparkles on the
    // store's shared ~30fps clock (the TSX's Animated loops).
    let clock = || crate::live::store().tick();
    let hover = signal(String::new());
    // untracked: an eager `efforts()` read here would make the layer's
    // `dynamic` rebuild the card (and remount the slider mid-drag) on
    // every `pick`.
    let h = if untracked(|| efforts().is_empty()) {
        56.0
    } else {
        92.0
    };
    card(o, W, h)
        .p_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .h(px(36.0))
                // fast button — only on models with a "--fast" twin;
                // elsewhere an empty seat.
                .child(
                    when(
                        move || model_of(&choice().0).map(|m| m.fast).unwrap_or(false),
                        {
                            let hover = hover.clone();
                            move || {
                                let h1 = hover.clone();
                                let h2 = hover.clone();
                                let h3 = hover.clone();
                                view()
                                    .size(px(28.0))
                                    .rounded(px(8.0))
                                    .items_center()
                                    .justify_center()
                                    .label("Fast mode")
                                    .bg(move || {
                                        let (_, _, f) = choice();
                                        if f {
                                            hx("#f5b4002e")
                                        } else if h1.get() == "fast" {
                                            tc(0.08)
                                        } else {
                                            AdaptiveColor::TRANSPARENT
                                        }
                                    })
                                    .on_hover_in(move || h2.set("fast".into()))
                                    .on_hover_out(move || h3.set(String::new()))
                                    .on_press(|| store().toggle_fast())
                                    .child(
                                        text("ϟ")
                                            .text_size(px(14.0))
                                            .text_color(move || {
                                                let (_, _, f) = choice();
                                                if f {
                                                    hx("#f5b400")
                                                } else {
                                                    tc(0.54)
                                                }
                                            }),
                                    )
                            }
                        },
                    )
                    .otherwise(|| view().size(px(28.0))),
                )
                // centered: effort name + "model ›" → list mode.
                .child(
                    view().flex_1().items_center().child({
                        let h1 = hover.clone();
                        let h2 = hover.clone();
                        let h3 = hover.clone();
                        view()
                            .items_center()
                            .px(px(10.0))
                            .py(px(2.0))
                            .rounded(px(8.0))
                            .label("Change model")
                            .bg(move || {
                                if h1.get() == "model" {
                                    tc(0.08)
                                } else {
                                    AdaptiveColor::TRANSPARENT
                                }
                            })
                            .on_hover_in(move || h2.set("model".into()))
                            .on_hover_out(move || h3.set(String::new()))
                            .on_press(|| {
                                mp_open().update(|o| {
                                    if let Some(o) = o {
                                        o.mode = "list".into();
                                    }
                                })
                            })
                            .child(
                                text(move || {
                                    let l = effort_label();
                                    if l.is_empty() { "Default effort".into() } else { l }
                                })
                                .text_size(px(13.0))
                                .line_height(px(18.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || {
                                    if is_top() {
                                        hx(TOP_TINT)
                                    } else if effort_label().is_empty() {
                                        tc(0.96)
                                    } else {
                                        hx(EFFORT_TINT)
                                    }
                                }),
                            )
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(dynamic(|| logo(&shown_model(), 12.0, || tc(0.85))))
                                    .child(
                                        text(move || short_name(&shown_model()))
                                        .number_of_lines(1)
                                        .text_size(px(12.0))
                                        .line_height(px(16.0))
                                        .text_color(tc(0.66)),
                                    )
                                    .child(
                                        text("›")
                                            .text_size(px(11.0))
                                            .text_color(tc(0.66)),
                                    ),
                            )
                    }),
                )
                // reset — model's default effort, fast off.
                .child({
                    let h1 = hover.clone();
                    let h2 = hover.clone();
                    let h3 = hover.clone();
                    view()
                        .size(px(28.0))
                        .rounded(px(8.0))
                        .items_center()
                        .justify_center()
                        .label("Reset to default")
                        .bg(move || {
                            if h1.get() == "reset" {
                                tc(0.08)
                            } else {
                                AdaptiveColor::TRANSPARENT
                            }
                        })
                        .on_hover_in(move || h2.set("reset".into()))
                        .on_hover_out(move || h3.set(String::new()))
                        .on_press(|| {
                            let (m, _, _) = choice();
                            let def = model_of(&m)
                                .map(|d| d.default_effort.clone())
                                .unwrap_or_default();
                            store().set_choice(&m, &def, false);
                        })
                        .child(text("↺").text_size(px(14.0)).text_color(tc(0.54)))
                }),
        )
        // tooltips at card level, above their buttons (Codex).
        .child(when(
            {
                let hover = hover.clone();
                move || {
                    hover.get() == "fast"
                        && model_of(&choice().0).map(|m| m.fast).unwrap_or(false)
                }
            },
            || {
                let (_, _, f) = choice();
                tip(
                    Some(-4.0),
                    None,
                    -34.0,
                    if f { "Fast mode" } else { "Normal mode" },
                )
            },
        ))
        .child(when(
            {
                let hover = hover.clone();
                move || hover.get() == "reset"
            },
            || tip(None, Some(-10.0), -34.0, "Reset to default"),
        ))
        // stepped slider: a dot per effort, fill to the knob, violet at
        // Ultra.
        .child(when(
            || !efforts().is_empty(),
            move || slider(clock),
        ))
}

/// The stepped slider — `drag.panHandlers` on a 200×22 track
/// (modelpicker.tsx:277).
fn slider(clock: impl Fn() -> f64 + Copy + 'static) -> impl IntoElement {
    // the step the grant landed on (`from`), while the drag is held.
    let drag = signal(None::<i32>);
    let drag2 = drag.clone();
    let drag3 = drag.clone();
    let drag4 = drag.clone();
    const SPARKS: [(f32, f32, f32); 10] = [
        (0.06, 0.3, 2.0), (0.14, 0.7, 1.5), (0.22, 0.4, 2.5), (0.31, 0.75, 1.5),
        (0.38, 0.25, 2.0), (0.47, 0.6, 1.5), (0.55, 0.35, 2.5), (0.63, 0.72, 2.0),
        (0.71, 0.3, 1.5), (0.8, 0.55, 2.0),
    ];
    view()
        .w(px(TRACK))
        .h(px(KNOB))
        .self_center()
        .justify_center()
        // the children (fill, dots, knob) are visuals only: `locationX`
        // stays track-local no matter where the grab lands.
        .pointer_events(PointerEvents::BoxOnly)
        // track
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
                .h(px(KNOB))
                .rounded(px(KNOB / 2.0))
                .bg(tc(0.06)),
        )
        // fill — blue, or the violet gradient at Ultra.
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .w(move || px(idx() as f32 * step() + KNOB))
                .h(px(KNOB))
                .rounded(px(KNOB / 2.0))
                .overflow_hidden()
                // solid blue, with the violet `backgroundImage` over it —
                // reactive stops (transparent until Ultra) so the fill
                // view itself never remounts mid-drag.
                .bg(hx("#1e86fc"))
                .bg_linear(deg(90.0))
                .gradient_from(move || {
                    if is_top() {
                        hx("#4f6bff")
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .gradient_via(move || {
                    if is_top() {
                        hx("#7c5cf0")
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .gradient_to(move || {
                    if is_top() {
                        hx("#a26bfa")
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                // the light band sweeping across (2400ms loop).
                .child(when(move || is_top(), {
                    let clock = clock;
                    move || {
                        view()
                            .absolute()
                            .top(px(0.0))
                            .bottom(px(0.0))
                            .w(px(46.0))
                            .bg_linear(deg(90.0))
                            .gradient_from(AdaptiveColor::TRANSPARENT)
                            .gradient_via(
                                AdaptiveColor::from(Hsla::from_str("#ffffff47")
                                    .unwrap_or(theme::ACCENT.dark)),
                            )
                            .gradient_to(AdaptiveColor::TRANSPARENT)
                            .translate_x(move || {
                                let p = (clock() % 2.4) / 2.4;
                                px(-50.0 + (TRACK + 50.0) * p as f32)
                            })
                    }
                }))
                // drifting sparkles, twinkling out of phase.
                .child(when(move || is_top(), {
                    let clock = clock;
                    move || {
                        view().size_full().children(SPARKS.iter().enumerate().map(
                            |(k, (x, y, r))| {
                                let r = *r;
                                let kk = k as f64;
                                view()
                                    .absolute()
                                    .left(px(x * (TRACK - KNOB)))
                                    .top(px(y * KNOB - r / 2.0))
                                    .size(px(r))
                                    .rounded(px(r / 2.0))
                                    .bg(Hsla::from_str("#ffffff").unwrap_or(theme::ACCENT.dark))
                                    .opacity(move || {
                                        let t = clock();
                                        // 600+k*170 up / 700+k*130 down, k*260
                                        // offset — as a sine in the same period.
                                        let per = (600.0 + kk * 170.0 + 700.0 + kk * 130.0)
                                            / 1000.0;
                                        let ph = ((t * 1000.0 + kk * 260.0) / (per * 1000.0))
                                            * std::f64::consts::TAU;
                                        (0.15 + 0.85 * (0.5 + 0.5 * ph.sin())) as f32
                                    })
                                    .translate_x(move || {
                                        let p = (clock() % 2.4) / 2.4;
                                        px(6.0 * p as f32)
                                    })
                            },
                        ))
                    }
                })),
        )
        // the step dots.
        .child(each(
            || efforts(),
            |e| e.0.clone(),
            |_e, i| {
                view()
                    .absolute()
                    .left(px(i.get() as f32 * step() + KNOB / 2.0 - 2.0))
                    .size(px(4.0))
                    .rounded(px(2.0))
                    .bg(move || {
                        if i.get() <= idx() as usize {
                            hx("#ffffffb3")
                        } else {
                            tc(0.30)
                        }
                    })
                    .into_element()
            },
        ))
        // the knob.
        .child(
            view()
                .absolute()
                .left(move || px(idx() as f32 * step()))
                .size(px(KNOB))
                .rounded(px(KNOB / 2.0))
                .bg(Hsla::from_str("#ffffff").unwrap_or(theme::ACCENT.dark))
                // `0 1px 3px rgba(0,0,0,0.35)` (modelpicker.tsx:342).
                .shadow([BoxShadow {
                    offset_x: px(0.0),
                    offset_y: px(1.0),
                    blur: px(3.0),
                    spread: px(0.0),
                    color: hsla(0.0, 0.0, 0.0, 0.35).into(),
                    inset: false,
                }]),
        )
        // pan — grant snaps to the tapped step by `locationX`; moves
        // pick by `dx` (modelpicker.tsx:143).
        .claim_touches(TouchClaim::on_start())
        .keep_touches(true)
        .on_responder_grant(move |e: &ResponderEvent| {
            let from = ((e.location.x.0 - KNOB / 2.0) / step()).round() as i32;
            drag.set(Some(from));
            pick(from);
        })
        .on_responder_move(move |e: &ResponderEvent| {
            if let Some(from) = drag2.get() {
                pick((from as f32 + e.gesture.delta.x.0 / step()).round() as i32);
            }
        })
        .on_responder_release(move |_: &ResponderEvent| drag3.set(None))
        .on_responder_terminate(move |_: &ResponderEvent| drag4.set(None))
}

// ————— model combobox —————

/// The rows of the combobox under `q`: (group, models) — the subscription
/// models first under no heading, then each API connection's; names that
/// repeat within a section (dated snapshots, aliases) show once.
fn sections(q: &str) -> Vec<(String, Vec<crate::store::ModelDef>)> {
    let q = q.trim().to_lowercase();
    let mut out: Vec<(String, Vec<crate::store::ModelDef>)> = Vec::new();
    for m in store().models.get() {
        let name = short_name(&m.id);
        if !q.is_empty()
            && !name.to_lowercase().contains(&q)
            && !m.id.to_lowercase().contains(&q)
            && !m.group.to_lowercase().contains(&q)
        {
            continue;
        }
        let key = if m.subscription { String::new() } else { m.group.clone() };
        let at = match out.iter().position(|(k, _)| *k == key) {
            Some(i) => i,
            None => {
                out.push((key, Vec::new()));
                out.len() - 1
            }
        };
        if !out[at].1.iter().any(|x| short_name(&x.id) == name) {
            out[at].1.push(m);
        }
    }
    out.sort_by_key(|(k, _)| (!k.is_empty(), k.clone()));
    out
}

fn list_card(o: &ModelsOpen) -> impl IntoElement {
    let q = signal(String::new());
    card(o, LIST_W, 460.0)
        .p(px(6.0))
        .child(
            // search
            view()
                .flex_row()
                .items_center()
                .gap(px(7.0))
                .h(px(32.0))
                .px(px(10.0))
                .mb(px(4.0))
                .rounded(px(8.0))
                .bg(tc(0.05))
                .child(crate::icons::icon_ink("search", 13.0, || tc(0.5)))
                .child(
                    input()
                        .value(move || q.get())
                        .on_change(move |v: &str| q.set(v.to_owned()))
                        .on_key_press(|key: &str| {
                            if key == "Escape" {
                                mp_open().set(None);
                            }
                        })
                        .auto_focus(true)
                        .placeholder("Search models…")
                        .placeholder_color(|| color(tc(0.4)))
                        .text_size(px(13.0))
                        .text_color(tc(0.96))
                        .flex_1(),
                ),
        )
        .child(
            scroll().max_h(px(460.0 - 48.0)).child(
                view()
                    // Default — `Recommended · <default>` (hidden by a search).
                    .child(when(
                        move || q.get().trim().is_empty(),
                        || {
                            view()
                                .px(px(10.0))
                                .py(px(6.0))
                                .rounded(px(8.0))
                                .bg(move || if choice().0.is_empty() { hx("#1e86fc1f") } else { AdaptiveColor::TRANSPARENT })
                                .on_press(|| {
                                    store().set_choice("", "", false);
                                    mp_open().set(None);
                                })
                                .child(
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .child(
                                            text("Default")
                                                .flex_1()
                                                .text_size(px(13.0))
                                                .line_height(px(18.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(move || if choice().0.is_empty() { hx(EFFORT_TINT) } else { tc(0.96) }),
                                        )
                                        .child(when(
                                            || choice().0.is_empty(),
                                            || crate::icons::icon_ink("check", 13.0, || hx(EFFORT_TINT)).into_element(),
                                        )),
                                )
                                .child(
                                    text(move || format!("Recommended · {}", short_name(&store().default_model.get())))
                                        .text_size(px(12.0))
                                        .line_height(px(16.0))
                                        .text_color(tc(0.54)),
                                )
                                .into_element()
                        },
                    ))
                    .child(dynamic(move || {
                        let secs = sections(&q.get());
                        if secs.is_empty() {
                            return text(if store().models.get().is_empty() {
                                "opencodex isn't reachable on :8080"
                            } else {
                                "No models match"
                            })
                            .text_size(px(12.0))
                            .text_color(tc(0.54))
                            .p_2()
                            .into_element();
                        }
                        view()
                            .children(secs.into_iter().map(|(group, models)| {
                                view()
                                    .when(!group.is_empty(), |v| {
                                        v.child(view().h(px(0.5)).mx(px(6.0)).my(px(5.0)).bg(tc(0.08))).child(
                                            text(group.clone())
                                                .text_size(px(12.0))
                                                .line_height(px(16.0))
                                                .text_color(tc(0.5))
                                                .px(px(10.0))
                                                .pt(px(2.0))
                                                .pb(px(3.0)),
                                        )
                                    })
                                    .children(models.into_iter().map(model_row))
                                    .into_element()
                            }))
                            .into_element()
                    })),
            ),
        )
}

fn model_row(m: crate::store::ModelDef) -> AnyElement {
    let hovered = signal(false);
    let (id, id2, id3, id4) = (m.id.clone(), m.id.clone(), m.id.clone(), m.id.clone());
    let def = m.default_effort.clone();
    let picked = move || choice().0 == id;
    let picked2 = picked.clone();
    let picked3 = picked.clone();
    let picked4 = picked.clone();
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .h(px(32.0))
        .px(px(10.0))
        .rounded(px(8.0))
        .bg(move || {
            if picked() {
                hx("#1e86fc1f")
            } else if hovered.get() {
                tc(0.06)
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .child(logo(&id2, 16.0, move || if picked2() { hx(EFFORT_TINT) } else { tc(0.9) }))
        .child(
            text(short_name(&id3))
                .number_of_lines(1)
                .flex_1()
                .text_size(px(13.0))
                .line_height(px(18.0))
                .text_color(move || if picked3() { hx(EFFORT_TINT) } else { tc(0.96) }),
        )
        .child(when(picked4, || crate::icons::icon_ink("check", 13.0, || hx(EFFORT_TINT)).into_element()))
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || {
            store().set_choice(&id4, &def, false);
            mp_open().update(|o| {
                if let Some(o) = o {
                    o.mode = "effort".into();
                }
            });
        })
        .label(full_name(&m.id))
        .into_element()
}
