//! The dialogs' shell (DR's dialog chunks: `PageSetupDialog`,
//! `TablePropertiesDialog`, `SplitCellDialog`, `WatermarkDialog`,
//! `ImagePropertiesDialog`, `ImagePositionDialog` share it): a dimmed
//! window, a card with the title over a hairline, the body, and the
//! buttons right-aligned over a hairline. A press on the dimmed window or
//! Escape cancels, Return confirms, and Tab moves between the dialog's
//! fields only.

use std::rc::Rc;

use nsh_core::keymap::When;
use nsh_core::prelude::*;

use super::palette as p;

/// The card (`Q`): radius 8, `0 4px 20px var(--doc-shadow)`, 20 from the
/// window's edges; each dialog sets its min and max width.
pub(crate) const RADIUS: f32 = 8.0;
pub(crate) const MARGIN: f32 = 20.0;
/// The title (`V`): `padding 16px 20px 12px`, 16px semibold.
pub(crate) const TITLE_PAD: (f32, f32, f32) = (16.0, 20.0, 12.0);
pub(crate) const TITLE_SIZE: f32 = 16.0;
/// The body (`X`): `padding 16px 20px`, a column.
pub(crate) const BODY_PAD: (f32, f32) = (16.0, 20.0);
/// The buttons (`Y`, `A`): `padding 12px 20px 16px`, 8 apart; each
/// `padding 6px 16px`, 13px, radius 4.
pub(crate) const FOOTER_PAD: (f32, f32, f32) = (12.0, 20.0, 16.0);
pub(crate) const BUTTON_PAD: (f32, f32) = (16.0, 6.0);
/// The roomier shell (`HyperlinkDialog`'s `Z`, `ee`, `re`, `ae`, `C`): a
/// header of `padding 16px 20px` with an 18px title and a ×, the body
/// `padding 20px`, the footer 12 apart, buttons `padding 10px 20px` at
/// 14px medium.
pub(crate) const ROOMY_TITLE_SIZE: f32 = 18.0;
pub(crate) const ROOMY_PAD: (f32, f32) = (16.0, 20.0);
pub(crate) const ROOMY_BUTTON_PAD: (f32, f32) = (20.0, 10.0);

/// The fields a dialog's Tab cycles through, in the order they joined.
#[derive(Clone, Copy)]
struct Trap {
    fields: Stored<Vec<ElementRef>>,
    at: Signal<usize>,
}

/// Joins `field` to the Tab order of the dialog around it, if any.
pub(crate) fn join_trap(field: &ElementRef) {
    if let Some(t) = use_context::<Trap>() {
        let f = *field;
        t.fields.with(|v| v.push(f));
    }
}

/// A footer button.
#[derive(Clone)]
pub(crate) struct Action {
    pub label: SharedString,
    pub primary: bool,
    /// While it doesn't hold the button is dimmed and takes no press.
    pub enabled: Rc<dyn Fn() -> bool>,
    pub run: Rc<dyn Fn()>,
    /// Red, at the footer's left (Remove Link).
    pub danger: bool,
}

/// A dialog, shown while `open`.
pub(crate) struct Modal {
    open: Signal<bool>,
    title: SharedString,
    min_w: f32,
    max_w: f32,
    gap: f32,
    body: Rc<dyn Fn() -> AnyElement>,
    actions: Vec<Action>,
    on_cancel: Rc<dyn Fn()>,
    on_confirm: Rc<dyn Fn()>,
    at: Option<Signal<usize>>,
    roomy: bool,
}

/// A dialog titled `title` (Page Setup's widths, 400–480, and body gap,
/// 14, until set).
pub(crate) fn modal(open: Signal<bool>, title: impl Into<SharedString>) -> Modal {
    Modal {
        open,
        title: title.into(),
        min_w: 400.0,
        max_w: 480.0,
        gap: 14.0,
        body: Rc::new(|| view().into_element()),
        actions: Vec::new(),
        on_cancel: Rc::new(|| {}),
        on_confirm: Rc::new(|| {}),
        at: None,
        roomy: false,
    }
}

impl Modal {
    pub(crate) fn width(mut self, min: f32, max: f32) -> Self {
        self.min_w = min;
        self.max_w = max;
        self
    }
    /// The space between the body's rows (12, 14 or 16 by dialog).
    pub(crate) fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }
    pub(crate) fn body(mut self, body: impl Fn() -> AnyElement + 'static) -> Self {
        self.body = Rc::new(body);
        self
    }
    /// A footer button; `primary` is the filled one.
    pub(crate) fn action(mut self, label: impl Into<SharedString>, primary: bool, run: impl Fn() + 'static) -> Self {
        self.actions.push(Action { label: label.into(), primary, enabled: Rc::new(|| true), run: Rc::new(run), danger: false });
        self
    }
    /// A red button at the footer's left, while `shown` holds.
    pub(crate) fn danger_when(mut self, label: impl Into<SharedString>, shown: impl Fn() -> bool + 'static, run: impl Fn() + 'static) -> Self {
        self.actions.push(Action { label: label.into(), primary: false, enabled: Rc::new(shown), run: Rc::new(run), danger: true });
        self
    }
    /// The roomier shell: a header with a ×, larger type and buttons.
    pub(crate) fn roomy(mut self) -> Self {
        self.roomy = true;
        self
    }
    /// A footer button enabled while `enabled` holds (Watermark's Apply
    /// without a picture: `opacity: .5`, disabled).
    pub(crate) fn action_when(mut self, label: impl Into<SharedString>, primary: bool, enabled: impl Fn() -> bool + 'static, run: impl Fn() + 'static) -> Self {
        self.actions.push(Action { label: label.into(), primary, enabled: Rc::new(enabled), run: Rc::new(run), danger: false });
        self
    }
    /// Escape, a press outside the card (the dialog closes after it).
    pub(crate) fn on_cancel(mut self, run: impl Fn() + 'static) -> Self {
        self.on_cancel = Rc::new(run);
        self
    }
    /// The field Tab is on, kept in `at` (its index in the Tab order).
    pub(crate) fn tab_index(mut self, at: Signal<usize>) -> Self {
        self.at = Some(at);
        self
    }
    /// Return (the dialog stays open unless it closes it).
    pub(crate) fn on_confirm(mut self, run: impl Fn() + 'static) -> Self {
        self.on_confirm = Rc::new(run);
        self
    }
}

/// A roomy footer's button (`se`, `pe`, `de`, `ce`): the link colour,
/// the hover grey with an input border, red, or greyed while it can't be
/// pressed.
fn roomy_button(a: Action) -> AnyElement {
    let run = a.run.clone();
    let (on, on2, on3, on4) = (a.enabled.clone(), a.enabled.clone(), a.enabled.clone(), a.enabled.clone());
    let (primary, danger) = (a.primary, a.danger);
    text(a.label.clone())
        .px(px(ROOMY_BUTTON_PAD.0))
        .py(px(ROOMY_BUTTON_PAD.1))
        .text_size(px(14.0))
        .font_weight(FontWeight::MEDIUM)
        .rounded(px(4.0))
        .border(px(if primary || danger { 0.0 } else { 1.0 }))
        .border_color(p::BORDER_INPUT)
        .bg(move || match (primary, danger, on()) {
            (true, _, false) => p::BORDER_INPUT,
            (true, _, true) => p::LINK,
            (_, true, _) => p::ERROR,
            _ => p::BG_HOVER,
        })
        .text_color(move || match (primary || danger, on2()) {
            (true, false) => p::TEXT_MUTED,
            (true, true) => p::ON_PRIMARY,
            _ => p::TEXT,
        })
        .label(a.label)
        .when(danger, |v| v.opacity(move || if on3() { 1.0f32 } else { 0.0 }))
        .on_press(move || {
            if on4() {
                run()
            }
        })
        .into_element()
}

fn footer_button(a: Action) -> AnyElement {
    let run = a.run.clone();
    let (on, on2) = (a.enabled.clone(), a.enabled.clone());
    text(a.label.clone())
        .px(px(BUTTON_PAD.0))
        .py(px(BUTTON_PAD.1))
        .text_size(px(13.0))
        .border_1()
        .border_color(if a.primary { p::PRIMARY } else { p::BORDER })
        .rounded(px(4.0))
        // the host page resets a button's background; the primary one
        // sets its own
        .bg(if a.primary { p::PRIMARY } else { AdaptiveColor::TRANSPARENT })
        .text_color(if a.primary { p::ON_PRIMARY } else { p::TEXT })
        .label(a.label)
        .opacity(move || if on() { 1.0f32 } else { 0.5 })
        .on_press(move || {
            if on2() {
                run()
            }
        })
        .into_element()
}

impl IntoElement for Modal {
    fn into_element(self) -> AnyElement {
        let Modal { open, title, min_w, max_w, gap, body, actions, on_cancel, on_confirm, at, roomy } = self;
        let cancel: Rc<dyn Fn()> = Rc::new(move || {
            if open.get_untracked() {
                open.set(false);
                on_cancel()
            }
        });
        when(
            move || open.get(),
            move || {
                let trap = Trap { fields: stored(Vec::new()), at: at.unwrap_or_else(|| signal(0)) };
                trap.at.set(0);
                provide_context(trap);
                let confirm = on_confirm.clone();
                on_key_with("enter", KeyOptions { when: When::Always, repeat: false }, move |_| confirm());
                for (key, step) in [("tab", 1isize), ("shift+tab", -1)] {
                    on_key_with(key, KeyOptions { when: When::Always, repeat: true }, move |_| {
                        let n = trap.fields.with(|f| f.len());
                        if n == 0 {
                            return;
                        }
                        let i = (trap.at.get_untracked() as isize + step).rem_euclid(n as isize) as usize;
                        trap.at.set(i);
                        trap.fields.with(|f| f[i].focus());
                    });
                }
                let (c1, c2, cancel) = (cancel.clone(), cancel.clone(), cancel.clone());
                overlay()
                    .inset_0()
                    .items_center()
                    .justify_center()
                    .label(title.clone())
                    .on_cancel(move || c1())
                    .child(view().absolute().inset_0().bg(p::OVERLAY).label("Dialog backdrop").on_press(move || c2()))
                    .child(
                        view()
                            .w_full()
                            .min_w(px(min_w))
                            .max_w(px(max_w))
                            .m(px(MARGIN))
                            .rounded(px(RADIUS))
                            .bg(p::SURFACE)
                            .shadow([BoxShadow {
                                offset_x: px(0.0),
                                offset_y: px(4.0),
                                blur: px(20.0),
                                spread: px(0.0),
                                color: color(p::SHADOW).into(),
                                inset: false,
                            }])
                            .label(format!("{title} dialog"))
                            .child(if roomy {
                                let c3 = cancel.clone();
                                view()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .py(px(ROOMY_PAD.0))
                                    .px(px(ROOMY_PAD.1))
                                    .border_b(px(1.0))
                                    .border_color(p::BORDER)
                                    .child(text(title.clone()).text_size(px(ROOMY_TITLE_SIZE)).font_weight(FontWeight::SEMIBOLD).text_color(p::TEXT))
                                    .child(text("\u{d7}").px(px(8.0)).py(px(4.0)).text_size(px(20.0)).line_height(px(20.0)).text_color(p::TEXT_MUTED).label("Close dialog").on_press(move || c3()))
                                    .into_element()
                            } else {
                                text(title.clone())
                                    .pt(px(TITLE_PAD.0))
                                    .px(px(TITLE_PAD.1))
                                    .pb(px(TITLE_PAD.2))
                                    .border_b(px(1.0))
                                    .border_color(p::BORDER)
                                    .text_size(px(TITLE_SIZE))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p::TEXT)
                                    .into_element()
                            })
                            .child(if roomy { view().p(px(ROOMY_PAD.1)).gap(px(gap)).child(body()) } else { view().py(px(BODY_PAD.0)).px(px(BODY_PAD.1)).gap(px(gap)).child(body()) })
                            .child(if roomy {
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(12.0))
                                    .py(px(ROOMY_PAD.0))
                                    .px(px(ROOMY_PAD.1))
                                    .border_t(px(1.0))
                                    .border_color(p::BORDER)
                                    .children(actions.iter().filter(|a| a.danger).cloned().map(roomy_button))
                                    .child(view().flex_1())
                                    .children(actions.iter().filter(|a| !a.danger).cloned().map(roomy_button))
                            } else {
                                view()
                                    .flex_row()
                                    .justify_end()
                                    .gap(px(8.0))
                                    .pt(px(FOOTER_PAD.0))
                                    .px(px(FOOTER_PAD.1))
                                    .pb(px(FOOTER_PAD.2))
                                    .border_t(px(1.0))
                                    .border_color(p::BORDER)
                                    .children(actions.iter().cloned().map(footer_button))
                            }),
                    )
            },
        )
        .into_element()
    }
}
