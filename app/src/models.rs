//! The model picker (`modelpicker.tsx`): `model_pill` (used in the pane
//! and the composer) opens a root-anchored popover — the effort card
//! (⚡ fast toggle, effort name, "model ›", reset, a stepped slider that
//! turns violet at Ultra) or the model list. The catalog is opencodex's
//! (`GET /models`).

use std::str::FromStr;
use std::time::Duration;

use nsh_core::ui::prelude::*;

use crate::live::store;
use crate::store::ModelsOpen;
use crate::theme;

const W: f32 = 222.0;
const TRACK: f32 = 200.0;
const KNOB: f32 = 22.0;
const TOP_TINT: &str = "#a78bfa";

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

fn model_of(id: &str) -> Option<crate::store::ModelDef> {
    store()
        .models
        .get()
        .into_iter()
        .find(|m| m.id == id)
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

/// `ModelPill(choice)` — model name + effort, a chevron; pressing measures
/// the pill and opens the popover at it (modelpicker.tsx:60).
pub(crate) fn model_pill() -> impl IntoElement {
    let node = element_ref();
    view()
        .element_ref(&node)
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .h(px(28.0))
        .px(px(10.0))
        .rounded(px(14.0))
        .label("Select model")
        .bg(move || {
            if mp_open().get().is_some() {
                tc(0.08)
            } else {
                tc(0.04)
            }
        })
        .child(
            text(move || {
                let (m, _, _) = choice();
                crate::store::pretty_model(&m)
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
                    .text_color(move || {
                        if is_top() {
                            hx(TOP_TINT)
                        } else {
                            tc(0.54)
                        }
                    })
            },
        ))
        .child(
            text("⌄")
                .text_size(px(10.0))
                .text_color(tc(0.54))
                .ml(px(1.0)),
        )
        .on_press(move || {
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
fn pos(o: &ModelsOpen, h: f32) -> (f32, f32) {
    let win = window();
    let left = (o.x + o.w / 2.0 - W / 2.0).clamp(8.0, win.width.0 - W - 8.0);
    let below = o.y + o.h + 6.0;
    let top = if below + h > win.height.0 - 8.0 {
        (o.y - h - 6.0).max(8.0)
    } else {
        below
    };
    (left, top)
}

fn card(o: &ModelsOpen, h: f32) -> View {
    let (left, top) = pos(o, h);
    view()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(W))
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
    // top-effort effects: a 2.4s shimmer sweep + twinkling sparkles, one
    // 60fps clock driving both (the TSX's Animated loops).
    let clock = signal(0.0f64);
    set_interval(Duration::from_millis(16), {
        let clock = clock.clone();
        move || clock.update(|c| *c += 0.016)
    });
    let hover = signal(String::new());
    // untracked: an eager `efforts()` read here would make the layer's
    // `dynamic` rebuild the card (and remount the slider mid-drag) on
    // every `pick`.
    let h = if untracked(|| efforts().is_empty()) {
        56.0
    } else {
        92.0
    };
    card(o, h)
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
                                    } else {
                                        tc(0.96)
                                    }
                                }),
                            )
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(2.0))
                                    .child(
                                        text(move || {
                                            let (m, _, _) = choice();
                                            crate::store::pretty_model(&m)
                                        })
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
fn slider(clock: Signal<f64>) -> impl IntoElement {
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
                    let clock = clock.clone();
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
                                let p = (clock.get() % 2.4) / 2.4;
                                px(-50.0 + (TRACK + 50.0) * p as f32)
                            })
                    }
                }))
                // drifting sparkles, twinkling out of phase.
                .child(when(move || is_top(), {
                    let clock = clock.clone();
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
                                        let t = clock.get();
                                        // 600+k*170 up / 700+k*130 down, k*260
                                        // offset — as a sine in the same period.
                                        let per = (600.0 + kk * 170.0 + 700.0 + kk * 130.0)
                                            / 1000.0;
                                        let ph = ((t * 1000.0 + kk * 260.0) / (per * 1000.0))
                                            * std::f64::consts::TAU;
                                        (0.15 + 0.85 * (0.5 + 0.5 * ph.sin())) as f32
                                    })
                                    .translate_x(move || {
                                        let p = (clock.get() % 2.4) / 2.4;
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
        .on_responder_release(move |_| drag3.set(None))
        .on_responder_terminate(move |_| drag4.set(None))
}

// ————— model list (modelpicker.tsx:350) —————

fn list_card(o: &ModelsOpen) -> impl IntoElement {
    card(o, 286.0)
        .py(px(6.0))
        .child(
            text("Select model")
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(tc(0.54))
                .px_3()
                .py(px(4.0)),
        )
        .child(
            scroll().max_h(px(256.0)).child(
                view()
                    .px_1()
                    // Default row — `Recommended · <default>`.
                    .child(
                        view()
                            .px_2()
                            .py(px(5.0))
                            .rounded(px(8.0))
                            .bg(move || {
                                if choice().0.is_empty() {
                                    tc(0.06)
                                } else {
                                    AdaptiveColor::TRANSPARENT
                                }
                            })
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
                                            .text_color(tc(0.96)),
                                    )
                                    .child(when(
                                        || choice().0.is_empty(),
                                        || {
                                            text("✓")
                                                .text_size(px(12.0))
                                                .text_color(tc(0.96))
                                        },
                                    )),
                            )
                            .child(
                                text(move || {
                                    format!(
                                        "Recommended · {}",
                                        store().default_model.get()
                                    )
                                })
                                .text_size(px(12.0))
                                .line_height(px(16.0))
                                .text_color(tc(0.54)),
                            ),
                    )
                    .child(each(
                        || store().models.get(),
                        |m| m.id.clone(),
                        |m, _| {
                            let id = m.get().id.clone();
                            let id2 = m.get().id.clone();
                            let def = m.get().default_effort.clone();
                            view()
                                .flex_row()
                                .items_center()
                                .h(px(25.0))
                                .px_2()
                                .rounded(px(8.0))
                                .bg(move || {
                                    if choice().0 == id {
                                        tc(0.06)
                                    } else {
                                        AdaptiveColor::TRANSPARENT
                                    }
                                })
                                .on_press(move || {
                                    store().set_choice(&id2, &def, false);
                                    mp_open().update(|o| {
                                        if let Some(o) = o {
                                            o.mode = "effort".into();
                                        }
                                    });
                                })
                                .child(
                                    text(crate::store::pretty_model(&m.get().id))
                                        .number_of_lines(1)
                                        .flex_1()
                                        .text_size(px(13.0))
                                        .line_height(px(18.0))
                                        .text_color(tc(0.96)),
                                )
                                .child(when(
                                    move || {
                                        let (c, _, _) = choice();
                                        c == m.get().id
                                    },
                                    || {
                                        text("✓")
                                            .text_size(px(12.0))
                                            .text_color(tc(0.96))
                                    },
                                ))
                        },
                    ))
                    .child(when(
                        || store().models.get().is_empty(),
                        || {
                            text("opencodex isn't reachable on :8080")
                                .text_size(px(12.0))
                                .text_color(tc(0.54))
                                .p_2()
                        },
                    )),
            ),
        )
}
