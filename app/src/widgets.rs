//! Generated-app cards (`WidgetShelf`/`TurnCell`'s widget branch in the
//! TSX). The TSX runs server-emitted TSX inside QuickJS; under a Rust
//! root there is no script engine, so the card is a native stand-in:
//! title, version, open-state, and the actions that pin it to the thread
//! or open its detail in the right pane.

use nsh_core::ui::prelude::*;

use crate::icons::icon;
use crate::live::store;
use crate::store::Art;
use crate::theme;

/// What a card turn carries into `CardShell` (shell.tsx:640): the
/// artifact, the turn's label/version/caption/reaction and its index.
#[derive(Clone, Default)]
pub(crate) struct CardShell {
    pub art: Art,
    pub label: String,
    pub v: i64,
    pub caption: String,
    pub reaction: String,
    pub i: i64,
    /// The turn's hover bar (☺ ↩ ⋯), mounted under the card.
    pub bar: Option<std::rc::Rc<dyn Fn() -> AnyElement>>,
}

/// `CardShell` — a card turn: title + `v<n>` chip, desc, caption, the
/// live mini app, then the action bar. The app is sized from its declared
/// `w×h` and scaled to fit (shell.tsx:911): `min(1, 560/w, 560/h,
/// (column-24)/w)`, so a narrow window never pushes it under the pane.
pub(crate) fn app_card(shell: CardShell) -> impl IntoElement {
    let CardShell { art, label, v, caption, reaction, i, bar } = shell;
    let dw = if art.w > 0.0 { art.w as f32 } else { 480.0 };
    let dh = if art.h > 0.0 { art.h as f32 } else { (dw * 0.75).round() };
    // the measured column width; 0 until the first layout.
    let avail = signal(0.0f32);
    // `natH` — the app's measured content height (shell.tsx:1017: the
    // frame hugs it, shorter or taller than declared), seeded from the
    // shared cache so a rebound row frames at it at once; the declared
    // height until the first measure.
    let nat = signal(store().mini_heights.borrow().get(&art.app).copied().unwrap_or(0.0));
    let h_of = move || {
        let n = nat.get();
        if n > 0.0 { n } else { dh }
    };
    let fit = move || {
        let f = 1.0f32.min(560.0 / dw).min(560.0 / h_of());
        let a = avail.get();
        if a > 0.0 { f.min(((a - 24.0) / dw).max(0.0)) } else { f }
    };
    let iw = move || (dw * fit()).round().max(140.0);
    let ih = move || (h_of() * fit()).round();
    let nat_key = art.app.clone();
    let on_natural: std::rc::Rc<dyn Fn(f32)> = std::rc::Rc::new(move |h: f32| {
        store().mini_heights.borrow_mut().insert(nat_key.clone(), h);
        nat.set(h);
    });
    let title = if !art.title.is_empty() {
        art.title.clone()
    } else if label == "image" {
        "Image".to_owned()
    } else {
        "Mini app".to_owned()
    };
    let card = view()
        .w_full()
        .bg(theme::CARD)
        .rounded(px(16.0))
        .border_1()
        .border_color(theme::BORDER_F)
        .p_3()
        .when(!reaction.is_empty(), |v| {
            v.child(crate::chat::tapback(i, reaction.clone(), true))
        })
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .min_h(px(22.0))
                .child(
                    text(title)
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::TEXT)
                        .flex_shrink(1.0)
                        .number_of_lines(1),
                )
                .when(v > 0, |row| {
                    row.child(
                        text(format!("v{v}"))
                            .text_size(px(10.5))
                            .text_color(theme::tc("#f5f5f585"))
                            .bg(theme::tc("#f5f5f514"))
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(7.0)),
                    )
                }),
        )
        .when(!art.desc.is_empty(), |c| {
            c.child(
                text(art.desc.clone())
                    .text_size(px(12.0))
                    .text_color(theme::DIM)
                    .mt(px(2.0))
                    .number_of_lines(2),
            )
        })
        .when(!caption.is_empty(), |c| {
            let caption = caption.clone();
            c.child(
                view()
                    .py_1()
                    .child(crate::md::markdown(move || caption.clone(), || false, || false)),
            )
        })
        .child(
            // Body: the live mini app (compiled TSX running in its own
            // engine inside this element) at its fitted size.
            // No margin: the app sits right under the header, as in Grok
            // (only `ArtPlaceholder`'s box has `marginTop: 10`).
            view()
                .w(move || px(iw()))
                .h(move || px(ih()))
                .rounded(px(10.0))
                .overflow_hidden()
                .child(crate::mini::mini_app(
                    &art.app,
                    0.0,
                    Some(std::rc::Rc::new(move || (dw, h_of()))),
                    Some(on_natural),
                )),
        )
        .child({
            let err_app = art.app.clone();
            let err_app2 = art.app.clone();
            view().mt_1().child(when(
                move || !store().mini_err(&err_app).get().is_empty(),
                move || {
                    let err = store().mini_err(&err_app2).get();
                    text(format!("widget: {err}"))
                        .text_size(px(10.0))
                        .text_color(theme::DOWN)
                        .number_of_lines(2)
                        .into_element()
                },
            ))
        })
        // No action row: the card's actions live in its ⋯ menu
        // (`ctx::card_items`), as Grok's do. Only a failed build offers
        // Retry in place.
        .child({
            let ret = art.app.clone();
            let ret2 = art.app.clone();
            when(
                move || !store().mini_err(&ret).get().is_empty(),
                move || {
                    let r = ret2.clone();
                    view()
                        .flex_row()
                        .mt_2()
                        .child(card_action("retry", "Retry", move || store().retry_mini(&r)))
                        .into_element()
                },
            )
        });
    // `self-stretch` + `onLayout` (shell.tsx:916): the column width the
    // fit reads, measured on a row the card's own size never changes.
    view()
        .w_full()
        .items_start()
        .on_layout(move |b: &Bounds<Points>| {
            if (b.size.width.0 - avail.get_untracked()).abs() >= 0.5 {
                avail.set(b.size.width.0);
            }
        })
        .child(
            // `self-start`, `iw + 24` wide: the card, then its bar row —
            // `mt-1.5`, `minHeight 24`, the actions at the card's right
            // edge (shell.tsx:1103).
            view()
                .w(move || px(iw() + 24.0))
                .max_w(percentage(1.0))
                .child(card)
                .when(bar.is_some(), |c| {
                    let bar = bar.clone().unwrap_or_else(|| std::rc::Rc::new(|| view().into_element()));
                    c.child(
                        view()
                            .mt(px(6.0))
                            .min_h(px(24.0))
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .child(bar()),
                    )
                }),
        )
}

fn card_action(
    name: &'static str,
    label: &'static str,
    on: impl Fn() + 'static,
) -> impl IntoElement {
    let hovered = signal(false);
    view()
        .label(label)
        .child(
            view()
                .size(px(28.0))
                .rounded(px(7.0))
                .items_center()
                .justify_center()
                .bg(move || if hovered.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                .child(icon(name)),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || on())
}
