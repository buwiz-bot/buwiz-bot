//! The formatting bar's buttons: an icon button with its tooltip (a
//! toggle when it has an active state), the separator, the groups, and
//! the icon dropdown (alignment, image wrap and transform: a trigger with
//! a ▾ that opens a row of icon choices).

use std::rc::Rc;

use nsh_core::prelude::*;

use super::icons::icon;
use super::palette as p;
use super::{Panel, dropdown, tip_text};

/// A toolbar button's box: `size: "icon-sm"` → `h-7 w-7` (DR `Jk`).
pub(crate) const BUTTON: f32 = 28.0;
/// Its corners: the button's `rounded-md`, `calc(var(--radius) - 2px)`
/// with `--radius: 0.5rem`.
pub(crate) const BUTTON_RADIUS: f32 = 6.0;
/// The toolbar's icons (DR `Ky = 18`).
pub(crate) const TOOLBAR_ICON: f32 = 18.0;
/// A dropdown trigger's icon (DR `k1 = 20`, `za = 20`) and its ▾ (14,
/// pulled 4 left by `-ml-1`).
pub(crate) const TRIGGER_ICON: f32 = 20.0;
pub(crate) const CARET: f32 = 14.0;
/// A disabled button's `opacity-30`.
pub(crate) const DISABLED_OPACITY: f32 = 0.3;
/// The separator (DR `xc`): `w-px h-6 mx-1.5`.
pub(crate) const SEPARATOR_H: f32 = 24.0;
pub(crate) const SEPARATOR_MX: f32 = 6.0;
/// A group (DR `Cr`): `gap-px px-1.5`, a `border-r` in `border/50`
/// (`first:pl-0`, `last:border-r-0`).
pub(crate) const GROUP_GAP: f32 = 1.0;
pub(crate) const GROUP_PX: f32 = 6.0;
/// The icon dropdown's choices (DR `vc`): 32 square, radius 4, gap 2 in
/// a row; with labels a column (gap 1, min width 200, `0 10px`, 13px).
pub(crate) const CHOICE: f32 = 32.0;
/// The tooltip (DR `vr`): 8 below the control, after 400 ms; `px-2 py-1
/// text-xs font-medium rounded-md shadow-lg`.
pub(crate) const TIP_OFFSET: f32 = 8.0;
pub(crate) const TIP_DELAY_MS: f64 = 400.0;

/// The web's tooltip bubble around `text`: white on the foreground ink in
/// light mode, the foreground on `muted` in dark (`dark:bg-muted
/// dark:text-foreground`).
pub(crate) fn tooltip_of(text_: impl Into<SharedString>) -> nsh_base::TooltipElement {
    nsh_base::tooltip()
        .placement(Placement::Bottom)
        .offset(px(TIP_OFFSET))
        .delay(TIP_DELAY_MS)
        .child(
            view()
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .bg(AdaptiveColor { light: p::TW_FOREGROUND.light, dark: p::TW_MUTED.dark })
                .shadow(shadow_lg())
                .child(
                    text(text_.into())
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(AdaptiveColor { light: white(), dark: p::TW_FOREGROUND.dark }),
                ),
        )
}

/// A choice's tooltip: the web gives choices a `title`, which the
/// browser shows as the system's plain tooltip (a small light box, the
/// core's default hint placement above the control).
pub(crate) fn plain_tip(text_: impl Into<SharedString>) -> AnyElement {
    view()
        .px(px(4.0))
        .py(px(2.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(AdaptiveColor { light: hsla(0.0, 0.0, 0.0, 0.15), dark: hsla(0.0, 0.0, 1.0, 0.15) })
        .bg(AdaptiveColor { light: hsla(0.0, 0.0, 0.97, 1.0), dark: hsla(0.0, 0.0, 0.2, 1.0) })
        .child(text(text_.into()).text_size(px(11.0)).text_color(AdaptiveColor { light: hsla(0.0, 0.0, 0.0, 0.85), dark: hsla(0.0, 0.0, 1.0, 0.85) }))
        .into_element()
}

/// Tailwind's `shadow-lg`: `0 10px 15px -3px` and `0 4px 6px -4px`, both
/// black at 10%.
pub(crate) fn shadow_lg() -> Vec<BoxShadow> {
    let c = hsla(0.0, 0.0, 0.0, 0.1);
    vec![
        BoxShadow { offset_x: px(0.0), offset_y: px(10.0), blur: px(15.0), spread: px(-3.0), color: c.into(), inset: false },
        BoxShadow { offset_x: px(0.0), offset_y: px(4.0), blur: px(6.0), spread: px(-4.0), color: c.into(), inset: false },
    ]
}

type Flag = Rc<dyn Fn() -> bool>;

/// A toolbar button (DR `op`): an icon in the 28 box, its label for
/// screen readers and as its tooltip with the shortcut ("Bold (⌘B)").
/// With [`IconButton::active`] it is a toggle (`aria-pressed`), filled
/// while on (`--doc-toggle-active-bg`).
pub(crate) struct IconButton {
    icon: &'static str,
    label: SharedString,
    shortcut: Option<&'static str>,
    active: Option<Flag>,
    disabled: Flag,
    on_press: Rc<dyn Fn()>,
}

pub(crate) fn icon_button(icon: &'static str, label: impl Into<SharedString>) -> IconButton {
    IconButton { icon, label: label.into(), shortcut: None, active: None, disabled: Rc::new(|| false), on_press: Rc::new(|| {}) }
}

impl IconButton {
    /// Its chord, in the web's form ("Mod+B"), shown in the tooltip.
    pub(crate) fn shortcut(mut self, chord: &'static str) -> Self {
        self.shortcut = Some(chord);
        self
    }

    /// A toggle, on while `on` holds.
    pub(crate) fn active(mut self, on: impl Fn() -> bool + 'static) -> Self {
        self.active = Some(Rc::new(on));
        self
    }

    pub(crate) fn disabled(mut self, off: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Rc::new(off);
        self
    }

    pub(crate) fn on_press(mut self, run: impl Fn() + 'static) -> Self {
        self.on_press = Rc::new(run);
        self
    }
}

impl IntoElement for IconButton {
    fn into_element(self) -> AnyElement {
        let hovered = signal(false);
        let active = self.active.clone();
        let on = move || active.as_ref().is_some_and(|a| a());
        let (on2, off, off2, off3) = (on.clone(), self.disabled.clone(), self.disabled.clone(), self.disabled.clone());
        // `[data-active=true]` wins over the hover; the ghost variant's
        // `hover:bg-accent hover:text-accent-foreground` come after the
        // `.oox-toolbar-toggle:hover` rules in the stylesheet, so they win.
        let bg = move || {
            if on() {
                p::TOGGLE_ACTIVE_BG
            } else if hovered.get() && !off() {
                p::TW_ACCENT
            } else {
                AdaptiveColor::TRANSPARENT
            }
        };
        let ink = move || {
            if on2() {
                p::TOGGLE_ACTIVE_FG
            } else if hovered.get() && !off2() {
                p::TW_ACCENT_FOREGROUND
            } else {
                p::TW_MUTED_FOREGROUND
            }
        };
        let tip = tip_text(&self.label, self.shortcut, apple_keys());
        let run = self.on_press.clone();
        view()
            .w(px(BUTTON))
            .h(px(BUTTON))
            .flex_shrink_0()
            .rounded(px(BUTTON_RADIUS))
            .items_center()
            .justify_center()
            .label(self.label.clone())
            .bg(bg)
            .opacity(move || if off3() { DISABLED_OPACITY } else { 1.0 })
            .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
            .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
            .on_press(move || {
                if !(self.disabled)() {
                    run()
                }
            })
            .tooltip(tooltip_of(tip))
            .child(icon(self.icon, TOOLBAR_ICON, ink))
            .into_element()
    }
}

/// The separator between toolbar parts (DR `xc`): 1×24 in `bg-border`,
/// 6 either side.
pub(crate) fn separator() -> AnyElement {
    view()
        .w(px(1.0))
        .h(px(SEPARATOR_H))
        .mx(px(SEPARATOR_MX))
        .flex_shrink_0()
        .bg(p::TW_BORDER)
        .label("Separator")
        .into_element()
}

/// Toolbar groups in a row (DR `Cr`, `role="group"`): each `(label,
/// controls)` with 1 between its controls, 6 either side and a hairline
/// on its right, but the first starts flush and the last has no line.
pub(crate) fn groups(groups: Vec<(&'static str, Vec<AnyElement>)>) -> AnyElement {
    let n = groups.len();
    view()
        .flex_row()
        .items_center()
        .label("Toolbar groups")
        .children(groups.into_iter().enumerate().map(|(i, (label, controls))| {
            view()
                .flex_row()
                .flex_shrink_0()
                .items_center()
                .gap(px(GROUP_GAP))
                .pl(px(if i == 0 { 0.0 } else { GROUP_PX }))
                .pr(px(GROUP_PX))
                .border_r(px(if i + 1 == n { 0.0 } else { 1.0 }))
                .border_color(p::TW_BORDER_50)
                .label(label)
                .children(controls)
                .into_element()
        }))
        .into_element()
}

/// One choice of an [`IconDropdown`].
#[derive(Clone)]
pub(crate) struct Choice {
    pub value: &'static str,
    pub icon: &'static str,
    pub label: SharedString,
    /// Shown after the label in its tooltip ("Align Left (Ctrl+L)": the
    /// alignment strings are the web's i18n texts, `Ctrl` on a Mac too).
    pub shortcut: Option<&'static str>,
}

/// A trigger showing the current choice's icon and a ▾, which opens the
/// choices (DR `vc`, the alignment dropdown `$h`'s sibling): a row of
/// 32-point icon buttons, or a column with their labels.
pub(crate) struct IconDropdown {
    label: SharedString,
    choices: Vec<Choice>,
    current: Rc<dyn Fn() -> Option<&'static str>>,
    labels: bool,
    disabled: Flag,
    on_select: Rc<dyn Fn(&'static str)>,
}

pub(crate) fn icon_dropdown(label: impl Into<SharedString>, choices: Vec<Choice>) -> IconDropdown {
    IconDropdown {
        label: label.into(),
        choices,
        current: Rc::new(|| None),
        labels: false,
        disabled: Rc::new(|| false),
        on_select: Rc::new(|_| {}),
    }
}

impl IconDropdown {
    /// The choice that is on (its icon is the trigger's; else the first's).
    pub(crate) fn current(mut self, value: impl Fn() -> Option<&'static str> + 'static) -> Self {
        self.current = Rc::new(value);
        self
    }

    /// The choices in a column with their labels (`showLabels`).
    pub(crate) fn labels(mut self) -> Self {
        self.labels = true;
        self
    }

    pub(crate) fn disabled(mut self, off: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Rc::new(off);
        self
    }

    pub(crate) fn on_select(mut self, run: impl Fn(&'static str) + 'static) -> Self {
        self.on_select = Rc::new(run);
        self
    }
}

impl IntoElement for IconDropdown {
    fn into_element(self) -> AnyElement {
        let open = signal(false);
        let hovered = signal(false);
        let anchor = element_ref();
        let IconDropdown { label, choices, current, labels, disabled, on_select } = self;
        let first = choices.first().map_or("", |c| c.icon);
        let shown = {
            let (choices, current) = (choices.clone(), current.clone());
            move || current().and_then(|v| choices.iter().find(|c| c.value == v)).map_or(first, |c| c.icon)
        };
        let off = disabled.clone();
        let ink = move || if hovered.get() { p::TW_FOREGROUND } else { p::TW_MUTED_FOREGROUND };
        let trigger = view()
            .w(px(BUTTON))
            .h(px(BUTTON))
            .flex_shrink_0()
            .flex_row()
            .rounded(px(BUTTON_RADIUS))
            .items_center()
            .justify_center()
            .element_ref(&anchor)
            .label(label.clone())
            // open: `bg-muted`; hover: `hover:bg-muted/80` (after
            // `hover:bg-accent` in the stylesheet)
            .bg(move || {
                if open.get() {
                    p::TW_MUTED
                } else if hovered.get() {
                    p::TW_MUTED_80
                } else {
                    AdaptiveColor::TRANSPARENT
                }
            })
            .opacity(move || if off() { DISABLED_OPACITY } else { 1.0 })
            .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
            .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
            .on_press(move || {
                if !disabled() {
                    open.update(|o| *o = !*o)
                }
            })
            .tooltip(tooltip_of(label.clone()))
            .child(dynamic(move || icon(shown(), TRIGGER_ICON, ink)))
            .child(view().ml(px(-4.0)).child(icon("arrow_drop_down", CARET, ink)));
        let menu = dropdown(anchor, open, format!("{label} choices"), Panel::ICONS, move || {
            let current = current.clone();
            let on_select = on_select.clone();
            let mut col = view();
            col = if labels { col.gap(px(1.0)).min_w(px(200.0)) } else { col.flex_row().gap(px(2.0)) };
            col.children(choices.iter().cloned().map(|c| {
                let (current, on_select) = (current.clone(), on_select.clone());
                choice(c, labels, move |v| current() == Some(v), move |v| {
                    open.set(false);
                    on_select(v)
                })
            }))
            .into_element()
        });
        view().child(trigger).child(menu).into_element()
    }
}

/// One choice button of an icon dropdown: `primary-light` while chosen,
/// `--doc-bg-hover` under the pointer.
fn choice(c: Choice, labels: bool, on: impl Fn(&str) -> bool + 'static, pick: impl Fn(&'static str) + 'static) -> AnyElement {
    let hovered = signal(false);
    let on = Rc::new(on);
    let (on1, on2) = (on.clone(), on);
    let value = c.value;
    let tip = match c.shortcut {
        Some(s) => format!("{} ({s})", c.label),
        None => c.label.to_string(),
    };
    let ink = move || if on1(value) { p::PRIMARY } else { p::TEXT };
    let mut v = view()
        .flex_row()
        .items_center()
        .h(px(CHOICE))
        .border_1()
        .border_color(AdaptiveColor::TRANSPARENT)
        .rounded(px(4.0))
        .label(c.label.clone())
        .bg(move || {
            if on2(value) {
                p::PRIMARY_LIGHT
            } else if hovered.get() {
                p::BG_HOVER
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_press(move || pick(value))
        .tooltip(plain_tip(tip));
    v = if labels { v.w_full().gap(px(10.0)).px(px(10.0)) } else { v.w(px(CHOICE)).justify_center() };
    let ink2 = ink.clone();
    v.child(icon(c.icon, TOOLBAR_ICON, ink))
        .when(labels, |v| v.child(text(c.label.clone()).flex_1().text_size(px(13.0)).text_color(ink2)))
        .into_element()
}
