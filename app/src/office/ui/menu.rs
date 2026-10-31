//! Menus as the web draws them: the menu bar's (DR `qd` of `Ba`s: File,
//! Format, Insert, Help) and the toolbar's table menu (DR `tableAction`):
//! rows with an icon, a label and a shortcut, separators, section labels,
//! disabled rows (their reason as a tooltip), danger rows, submenus that
//! open on hover beside their row, and rows of custom content (the table
//! grid).
//!
//! Each menu opens and closes on its own, as the web's: a press on its
//! trigger opens it; a press outside, Escape or choosing a row closes it.
//! Pressing another menu's trigger closes this one and opens that.

use std::rc::Rc;

use nsh_core::prelude::*;

use super::button::plain_tip;
use super::icons::icon;
use super::palette as p;
use super::Panel;

/// How a menu's rows look.
#[derive(Clone, Copy)]
pub(crate) struct Look {
    pub panel: Panel,
    pub min_w: f32,
    pub gap: f32,
    pub pad: (f32, f32),
    pub icon: f32,
    /// A disabled row is dimmed (the menu bar, `opacity: .4`) or greyed
    /// (the table menu, `--doc-text-muted`).
    pub dim: bool,
}

impl Look {
    /// The menu bar's menus (DR `Ba`, rows `qh`/`Nk`): min width 200; rows
    /// `gap 8, padding 6px 12px`, 13px, icons 18.
    pub(crate) const BAR: Look = Look { panel: Panel::MENU, min_w: 200.0, gap: 8.0, pad: (12.0, 6.0), icon: 18.0, dim: true };
    /// The table menu (rows `Z1`): radius 8, `0 4px 16px`, min width 200;
    /// rows `gap 10, padding 7px 14px`, icons 16.
    pub(crate) const TABLE: Look = Look {
        panel: Panel { radius: 8.0, shadow: (4.0, 16.0), ..Panel::MENU },
        min_w: 200.0,
        gap: 10.0,
        pad: (14.0, 7.0),
        icon: 16.0,
        dim: false,
    };
}

/// The menu bar's trigger (DR `zh`): `padding 2px 8px`, radius 4, 13px on
/// a 28 line; `--doc-bg-hover` while its menu is open (`Yk`).
pub(crate) const TRIGGER_H: f32 = 28.0;
pub(crate) const TRIGGER_PX: f32 = 8.0;
/// A submenu's list (DR `Gk`): min width 220.
pub(crate) const SUBMENU_MIN_W: f32 = 220.0;

/// A menu row.
#[derive(Clone)]
pub(crate) struct Item {
    pub icon: Option<&'static str>,
    pub label: SharedString,
    pub shortcut: Option<SharedString>,
    pub disabled: bool,
    /// Why it is disabled: its tooltip.
    pub description: Option<SharedString>,
    pub danger: bool,
    pub on_select: Rc<dyn Fn()>,
}

pub(crate) fn item(icon: Option<&'static str>, label: impl Into<SharedString>, on_select: impl Fn() + 'static) -> Item {
    Item { icon, label: label.into(), shortcut: None, disabled: false, description: None, danger: false, on_select: Rc::new(on_select) }
}

impl Item {
    pub(crate) fn shortcut(mut self, s: impl Into<SharedString>) -> Self {
        self.shortcut = Some(s.into());
        self
    }
    /// Disabled, with the reason shown over it.
    pub(crate) fn disabled(mut self, why: Option<&str>) -> Self {
        self.disabled = true;
        self.description = why.map(|w| SharedString::from(w.to_owned()));
        self
    }
    pub(crate) fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

/// Content built with the menu's close (a grid closes it as it inserts).
pub(crate) type Content = Rc<dyn Fn(Rc<dyn Fn()>) -> AnyElement>;

/// What a menu shows. A content row's builder gets the menu's close.
#[derive(Clone)]
pub(crate) enum Entry {
    Item(Item),
    Separator,
    /// A section label (the table menu's `Ly`: `6px 14px 2px`, 11px).
    Heading(SharedString),
    /// A row whose submenu lists `entries` (DR `Gk`).
    Submenu { icon: Option<&'static str>, label: SharedString, entries: Vec<Entry> },
    /// A row whose submenu is `content` (Insert › Table's grid); disabled
    /// with its reason when `disabled` is `Some`.
    Content { icon: Option<&'static str>, label: SharedString, disabled: Option<SharedString>, content: Content },
    /// Content in place of a row (`customContent`).
    Custom(Content),
}

/// The menu's separator (DR `_k`, `os`): 1px `--doc-border`, `margin 4px
/// 0`.
fn rule() -> AnyElement {
    view().h(px(1.0)).my(px(4.0)).bg(p::BORDER).into_element()
}

/// A row: the icon, the label, the shortcut at the right (DR `Bk`:
/// `margin-left: auto`, 12px muted) or a submenu's chevron; under the
/// pointer `--doc-bg-hover`.
fn row(look: Look, it: &Item, hovered: Signal<bool>, chevron: bool, enter: impl Fn() + 'static) -> View {
    let (disabled, danger) = (it.disabled, it.danger);
    let ink = move || {
        if disabled && !look.dim {
            p::TEXT_MUTED
        } else if danger {
            p::ERROR
        } else {
            p::TEXT
        }
    };
    let icon_ink = move || if danger && !disabled { p::TW_DESTRUCTIVE } else { ink() };
    view()
        .flex_row()
        .items_center()
        .w_full()
        .gap(px(look.gap))
        .px(px(look.pad.0))
        .py(px(look.pad.1))
        .label(it.label.clone())
        .bg(move || if hovered.get() && !disabled { p::BG_HOVER } else { AdaptiveColor::TRANSPARENT })
        .opacity(if disabled && look.dim { 0.4f32 } else { 1.0 })
        .on_mouse_enter(move |_: &MouseEvent| {
            hovered.set(true);
            enter()
        })
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .when_some(it.icon, |v, name| v.child(icon(name, look.icon, icon_ink)))
        .child(text(it.label.clone()).text_size(px(13.0)).text_color(ink).number_of_lines(1))
        .when(it.shortcut.is_some() || chevron, |v| v.child(view().flex_1()))
        .when_some(it.shortcut.clone(), |v, s| v.child(text(s).text_size(px(12.0)).text_color(p::TEXT_MUTED)))
        .when(chevron, |v| v.child(icon("keyboard_arrow_right", 16.0, || p::TEXT)))
        .when_some(it.description.clone(), |v, d| v.tooltip(plain_tip(d)))
}

/// A menu's rows, in `look`; `close` closes the whole menu.
pub(crate) fn menu_list(look: Look, entries: Vec<Entry>, close: Rc<dyn Fn()>) -> AnyElement {
    // the submenu open in this list (one at a time): the row's index
    let sub = signal(None::<usize>);
    view()
        .min_w(px(look.min_w))
        .children(entries.into_iter().enumerate().map(|(i, e)| {
            let close = close.clone();
            match e {
                Entry::Separator => rule(),
                Entry::Heading(t) => text(t)
                    .pt(px(6.0))
                    .px(px(14.0))
                    .pb(px(2.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p::TEXT_MUTED)
                    .into_element(),
                Entry::Custom(content) => content(close),
                Entry::Item(it) => {
                    let hovered = signal(false);
                    let run = it.on_select.clone();
                    let disabled = it.disabled;
                    row(look, &it, hovered, false, move || sub.set(None))
                        .on_press(move || {
                            if !disabled {
                                close();
                                run()
                            }
                        })
                        .into_element()
                }
                Entry::Submenu { icon, label, entries } => {
                    let it = item(icon, label, || {});
                    submenu_row(look, it, i, sub, move |close| menu_list(Look { min_w: SUBMENU_MIN_W, ..look }, entries.clone(), close), close)
                }
                Entry::Content { icon, label, disabled, content } => {
                    let mut it = item(icon, label, || {});
                    if let Some(why) = &disabled {
                        it = it.disabled(Some(why));
                    }
                    submenu_row(look, it, i, sub, move |close| content(close), close)
                }
            }
        }))
        .into_element()
}

/// A row opening a submenu while the pointer is on it (DR `Ba`'s
/// `onMouseEnter`), in a card beside it (`Wk`: 2 to its right, its top 4
/// above the row's, padding 8).
fn submenu_row(look: Look, it: Item, i: usize, sub: Signal<Option<usize>>, content: impl Fn(Rc<dyn Fn()>) -> AnyElement + 'static, close: Rc<dyn Fn()>) -> AnyElement {
    let hovered = signal(false);
    let marker = element_ref();
    let disabled = it.disabled;
    let label = it.label.clone();
    let open = move || sub.get() == Some(i);
    view()
        .child(
            row(look, &it, hovered, !disabled, move || sub.set(if disabled { None } else { Some(i) }))
                // the submenu's top edge: 4 above the row, at its right
                .child(view().absolute().top(px(-4.0)).right(px(0.0)).size(px(0.0)).element_ref(&marker)),
        )
        .child(when(open, move || {
            overlay()
                .anchor(&marker)
                .placement(Placement::Right)
                .align(Align::Start)
                .offset(px(2.0))
                .label(format!("{label} submenu"))
                .on_cancel(move || sub.set(None))
                .child(Panel::SUBMENU.dress(view()).child(content(close.clone())))
        }))
        .into_element()
}

/// A menu at `anchor` while `open` (`offset` below it, its left edge on
/// the anchor's), its rows built as it opens.
pub(crate) fn menu_popup(
    anchor: ElementRef,
    open: Signal<bool>,
    label: impl Into<SharedString>,
    look: Look,
    offset: f32,
    entries: impl Fn() -> Vec<Entry> + 'static,
) -> AnyElement {
    let label: SharedString = label.into();
    when(
        move || open.get(),
        move || {
            let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
            overlay()
                .anchor(&anchor)
                .placement(Placement::Bottom)
                .align(Align::Start)
                .offset(px(offset))
                .label(label.clone())
                .on_press_outside(move |_: &Point<Points>| open.set(false))
                .on_cancel(move || open.set(false))
                .child(look.panel.dress(view()).child(menu_list(look, entries(), close)))
        },
    )
    .into_element()
}

/// A menu of the menu bar.
pub(crate) struct BarMenu {
    pub label: SharedString,
    pub entries: Rc<dyn Fn() -> Vec<Entry>>,
}

pub(crate) fn bar_menu(label: impl Into<SharedString>, entries: impl Fn() -> Vec<Entry> + 'static) -> BarMenu {
    BarMenu { label: label.into(), entries: Rc::new(entries) }
}

/// The menu bar (DR `qd`, `role="menubar"`): each menu's trigger, its
/// menu 2 below it (DR `Ba`: `top: rect.bottom + 2`).
pub(crate) fn menu_bar(menus: Vec<BarMenu>) -> AnyElement {
    menu_bar_in(menus, signal(false))
}

/// [`menu_bar`], `any` holding while one of its menus is open (a view's
/// keys stand back for it).
pub(crate) fn menu_bar_in(menus: Vec<BarMenu>, any: Signal<bool>) -> AnyElement {
    let opens: Vec<Signal<bool>> = menus.iter().map(|_| signal(false)).collect();
    let all = opens.clone();
    effect(move || any.set(all.iter().any(|o| o.get())));
    view()
        .flex_row()
        .items_center()
        .label("Menu bar")
        .children(menus.into_iter().zip(opens).map(|(m, open)| {
            let anchor = element_ref();
            let entries = m.entries.clone();
            view()
                .child(
                    text(m.label.clone())
                        .h(px(TRIGGER_H))
                        .px(px(TRIGGER_PX))
                        .py(px(2.0))
                        .line_height(px(TRIGGER_H - 4.0))
                        .rounded(px(4.0))
                        .text_size(px(13.0))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(p::TEXT)
                        .number_of_lines(1)
                        .bg(move || if open.get() { p::BG_HOVER } else { AdaptiveColor::TRANSPARENT })
                        .element_ref(&anchor)
                        .label(m.label.clone())
                        .on_press(move || open.update(|o| *o = !*o)),
                )
                .child(menu_popup(anchor, open, format!("{} menu", m.label), Look::BAR, 2.0, move || entries()))
                .into_element()
        }))
        .into_element()
}
