//! The office views' controls, drawn and behaving as BetterOffice's web
//! editor does them (`@betteroffice/docx-react` 0.4.3, the editor Rox's
//! web view mounts): its colors ([`palette`]), its icons ([`icons`]), the
//! formatting bar's buttons, dropdowns and colour pickers, its menus and
//! menu bar, the table grid and the dialogs' shell and fields. Word's view
//! uses them first, Excel's later.
//!
//! Sizes are the web's: its Tailwind classes (`h-7 w-7`, `px-1.5`, …, as
//! `dist/styles.css` defines them) and inline styles, each constant citing
//! the component it comes from (the minified names of
//! `docx-react/dist/index.mjs`, "DR").

pub(crate) mod button;
pub(crate) mod color;
pub(crate) mod icons;
pub(crate) mod palette;
pub(crate) mod select;

use nsh_core::prelude::*;

use palette as p;

/// A chord as the web labels it (DR `$u`): on Apple keyboards ⌘, ⌥ and ⇧
/// then the key, run together ("⌘⇧="); elsewhere "Ctrl+Alt+Shift+K". The
/// chord is the web's registry form ("Mod+Shift+=", "Mod+B").
pub(crate) fn shortcut_label(chord: &str, apple: bool) -> String {
    // `Uu`: modifiers before the last "+", whose key may itself be "+"
    let (mods, key) = match chord.strip_suffix("++") {
        Some(rest) => (rest, "+"),
        None => chord.rsplit_once('+').unwrap_or(("", chord)),
    };
    let has = |m: &str| mods.split('+').any(|x| x == m);
    let key = if key.chars().count() == 1 { key.to_uppercase() } else { key.to_owned() };
    if apple {
        let mut s = String::new();
        for (on, sign) in [(has("Mod"), "⌘"), (has("Alt"), "⌥"), (has("Shift"), "⇧")] {
            if on {
                s.push_str(sign);
            }
        }
        s + &key
    } else {
        let parts = [(has("Mod"), "Ctrl"), (has("Alt"), "Alt"), (has("Shift"), "Shift")];
        let mut v: Vec<&str> = parts.iter().filter(|(on, _)| *on).map(|(_, n)| *n).collect();
        v.push(&key);
        v.join("+")
    }
}

/// A control's tooltip (DR `op`): "Bold (⌘B)", or the label alone.
pub(crate) fn tip_text(label: &str, chord: Option<&str>, apple: bool) -> String {
    match chord {
        Some(c) => format!("{label} ({})", shortcut_label(c, apple)),
        None => label.to_owned(),
    }
}

/// The look of a dropdown panel (each web dropdown sets its own inline
/// style): background, its 1px border, radius, padding and shadow.
#[derive(Clone, Copy)]
pub(crate) struct Panel {
    pub bg: AdaptiveColor,
    pub border: AdaptiveColor,
    pub radius: f32,
    pub pad_x: f32,
    pub pad_y: f32,
    /// The shadow's y offset and blur (`0 <y>px <blur>px var(--doc-shadow)`).
    pub shadow: (f32, f32),
}

impl Panel {
    /// The menu bar's menus (DR `Ba`): `padding: 4px 0`, radius 6,
    /// `0 4px 12px`.
    pub(crate) const MENU: Panel = Panel { bg: p::SURFACE, border: p::BORDER, radius: 6.0, pad_x: 0.0, pad_y: 4.0, shadow: (4.0, 12.0) };
    /// A menu's submenu (DR `Wk`): padding 8.
    pub(crate) const SUBMENU: Panel = Panel { pad_x: 8.0, pad_y: 8.0, ..Panel::MENU };
    /// The toolbar's icon dropdowns (alignment, wrap, transform; DR `vc`):
    /// radius 8, padding 6, `0 4px 16px`.
    pub(crate) const ICONS: Panel = Panel { bg: p::SURFACE, border: p::BORDER, radius: 8.0, pad_x: 6.0, pad_y: 6.0, shadow: (4.0, 16.0) };
    /// The Insert Table grid's (DR `EE`): radius 8, padding 8, `0 4px 12px`.
    pub(crate) const GRID: Panel = Panel { bg: p::SURFACE, border: p::BORDER, radius: 8.0, pad_x: 8.0, pad_y: 8.0, shadow: (4.0, 12.0) };
    /// The font size list (DR `yy`): radius 6, no padding.
    pub(crate) const LIST: Panel = Panel { bg: p::SURFACE, border: p::BORDER, radius: 6.0, pad_x: 0.0, pad_y: 0.0, shadow: (4.0, 12.0) };
    /// The colour picker's (DR `f1`): `--doc-border-dark`, padding 10,
    /// `0 4px 16px`.
    pub(crate) const COLORS: Panel = Panel { bg: p::SURFACE, border: p::BORDER_DARK, radius: 6.0, pad_x: 10.0, pad_y: 10.0, shadow: (4.0, 16.0) };

    /// `view` with this look.
    pub(crate) fn dress(self, view: View) -> View {
        view.bg(self.bg)
            .border_1()
            .border_color(self.border)
            .rounded(px(self.radius))
            .px(px(self.pad_x))
            .py(px(self.pad_y))
            .shadow([BoxShadow {
                offset_x: px(0.0),
                offset_y: px(self.shadow.0),
                blur: px(self.shadow.1),
                spread: px(0.0),
                // resolved as the popup opens: popups are short-lived
                color: color(p::SHADOW).into(),
                inset: false,
            }])
    }
}

/// A dropdown under `anchor` while `open` (DR `xr`: 4 below the trigger,
/// its left edge on the trigger's), labelled `label` for screen readers
/// and tests: a press outside it and its trigger, or Escape, closes it.
pub(crate) fn dropdown(
    anchor: ElementRef,
    open: Signal<bool>,
    label: impl Into<SharedString>,
    panel: Panel,
    content: impl Fn() -> AnyElement + 'static,
) -> AnyElement {
    let label: SharedString = label.into();
    when(
        move || open.get(),
        move || {
            overlay()
                .anchor(&anchor)
                .placement(Placement::Bottom)
                .align(Align::Start)
                .offset(px(4.0))
                .label(label.clone())
                .on_press_outside(move |_: &Point<Points>| open.set(false))
                .on_cancel(move || open.set(false))
                .child(panel.dress(view()).child(content()))
        },
    )
    .into_element()
}
