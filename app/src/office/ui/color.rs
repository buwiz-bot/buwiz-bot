//! The colour dropdown (DR `qs`): text colour, highlight, table border and
//! cell fill. A split button (the colour last chosen, applied again by a
//! press, and a ▾) opens a panel: Automatic or No Color, the theme
//! colours with their tints and shades, the ten standard colours and a
//! hex field with Apply.

use std::rc::Rc;
use std::str::FromStr;

use nsh_core::canvas::LineCap;
use nsh_core::prelude::*;

use super::button::plain_tip;
use super::icons::icon;
use super::palette as p;
use super::{Panel, dropdown};

/// The split button (DR `v1`, `x1`, `C1`, `ay`): 32 high with 2 between
/// its parts; the colour part 28 wide (icon 18 over a 16×4 bar), the ▾
/// part 18; dimmed to .38 while disabled.
pub(crate) const SPLIT_H: f32 = 32.0;
pub(crate) const APPLY_W: f32 = 28.0;
pub(crate) const ARROW_W: f32 = 18.0;
pub(crate) const BAR: (f32, f32) = (16.0, 4.0);
pub(crate) const DISABLED_OPACITY: f32 = 0.38;
/// The single button (DR `m1`, `splitButton: false`): 40×32.
pub(crate) const SINGLE_W: f32 = 40.0;
/// A swatch (DR `yc`, `d1`, `bc`): 18 square, 2 apart, ten to a row.
pub(crate) const SWATCH: f32 = 18.0;
pub(crate) const SWATCH_GAP: f32 = 2.0;

/// A colour as `RRGGBB` (with or without `#`); black when unreadable.
pub(crate) fn hex(h: &str) -> Hsla {
    let h = h.trim_start_matches('#');
    Hsla::from_str(&format!("#{h}")).unwrap_or(Hsla { h: 0.0, s: 0.0, l: 0.0, a: 1.0 })
}

fn rgb(h: &str) -> (u8, u8, u8) {
    let h = format!("{:0<6}", h.trim_start_matches('#'));
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
    (c(0), c(2), c(4))
}

fn to_hex(r: f32, g: f32, b: f32) -> String {
    let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    format!("{:02X}{:02X}{:02X}", c(r), c(g), c(b))
}

/// Toward white by `1 - keep` (docx core `c`): a tint.
fn lighten(h: &str, keep: f32) -> String {
    let (r, g, b) = rgb(h);
    let m = |v: u8| v as f32 * keep + 255.0 * (1.0 - keep);
    to_hex(m(r), m(g), m(b))
}

/// Scaled to `keep` (docx core `d`): a shade.
fn darken(h: &str, keep: f32) -> String {
    let (r, g, b) = rgb(h);
    let m = |v: u8| v as f32 * keep;
    to_hex(m(r), m(g), m(b))
}

/// The default Office theme (docx core `s`), the slots the grid shows.
pub(crate) const THEME: [(&str, &str); 10] = [
    ("lt1", "FFFFFF"),
    ("dk1", "000000"),
    ("lt2", "E7E6E6"),
    ("dk2", "44546A"),
    ("accent1", "4472C4"),
    ("accent2", "ED7D31"),
    ("accent3", "A5A5A5"),
    ("accent4", "FFC000"),
    ("accent5", "5B9BD5"),
    ("accent6", "70AD47"),
];

const SLOT_NAMES: [&str; 10] = ["Background 1", "Text 1", "Background 2", "Text 2", "Accent 1", "Accent 2", "Accent 3", "Accent 4", "Accent 5", "Accent 6"];

/// The standard colours (DR `Qd`, names from `colorPicker.colors`).
pub(crate) const STANDARD: [(&str, &str); 10] = [
    ("Dark Red", "C00000"),
    ("Red", "FF0000"),
    ("Orange", "FFC000"),
    ("Yellow", "FFFF00"),
    ("Light Green", "92D050"),
    ("Green", "00B050"),
    ("Light Blue", "00B0F0"),
    ("Blue", "0070C0"),
    ("Dark Blue", "002060"),
    ("Purple", "7030A0"),
];

/// A theme colour's cell: its hex, its slot and the tint or shade it
/// carries into the document (`w:themeTint` / `w:themeShade`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Swatch {
    pub hex: String,
    pub label: String,
    pub slot: &'static str,
    pub tint: Option<&'static str>,
    pub shade: Option<&'static str>,
}

/// The theme grid (docx core `generateThemeTintShadeMatrix`): six rows of
/// the ten slots, the colour itself, then lighter 80/60/40% and darker
/// 25/50%. `scheme` gives a document's own slot colours.
pub(crate) fn theme_matrix(scheme: &dyn Fn(&str) -> Option<String>) -> Vec<Vec<Swatch>> {
    // (kind, value, hex value, label suffix): docx core `V`
    let rows: [(u8, f32, Option<&'static str>, &str); 6] = [
        (0, 0.0, None, ""),
        (1, 0.8, Some("CC"), ", Lighter 80%"),
        (1, 0.6, Some("99"), ", Lighter 60%"),
        (1, 0.4, Some("66"), ", Lighter 40%"),
        (2, 0.75, Some("BF"), ", Darker 25%"),
        (2, 0.5, Some("80"), ", Darker 50%"),
    ];
    rows.iter()
        .map(|&(kind, v, hv, suffix)| {
            THEME
                .iter()
                .zip(SLOT_NAMES)
                .map(|(&(slot, base), name)| {
                    let base = scheme(slot).unwrap_or_else(|| base.to_owned());
                    let hex = match kind {
                        0 => base.to_uppercase(),
                        1 => lighten(&base, 1.0 - v),
                        _ => darken(&base, v),
                    };
                    Swatch {
                        hex,
                        label: format!("{name}{suffix}"),
                        slot,
                        tint: if kind == 1 { hv } else { None },
                        shade: if kind == 2 { hv } else { None },
                    }
                })
                .collect()
        })
        .collect()
}

/// What the colour dropdown is for: its label, icon, clear row and the
/// colour its bar starts with (DR `qs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Text,
    Highlight,
    Border,
    /// A table cell's fill: the highlight's look with its own label.
    Fill,
}

impl Mode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Mode::Text => "Font Color",
            Mode::Highlight => "Text Highlight Color",
            Mode::Border => "Border Color",
            Mode::Fill => "Cell Fill Color",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Mode::Text => "format_color_text",
            Mode::Highlight => "ink_highlighter",
            Mode::Border => "border_color",
            Mode::Fill => "format_color_fill",
        }
    }
    /// The clear row: "Automatic" (text, border) or "No Color".
    fn clear(self) -> &'static str {
        match self {
            Mode::Text | Mode::Border => "Automatic",
            Mode::Highlight | Mode::Fill => "No Color",
        }
    }
    fn first(self) -> &'static str {
        match self {
            Mode::Highlight | Mode::Fill => "FFFF00",
            Mode::Border => "000000",
            Mode::Text => "FF0000",
        }
    }
    fn none_is_clear(self) -> bool {
        matches!(self, Mode::Highlight | Mode::Fill)
    }
}

/// A choice from the dropdown.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Picked {
    /// Automatic, or No Color.
    Clear,
    /// A theme cell (a highlight takes its hex only).
    Theme(Swatch),
    /// A standard colour or a typed hex, `RRGGBB`.
    Rgb(String),
}

impl Picked {
    /// The colour a bar shows for it: `None` for transparent (no colour).
    fn bar(&self, mode: Mode) -> Option<String> {
        match self {
            Picked::Clear if mode.none_is_clear() => None,
            Picked::Clear => Some("000000".into()),
            Picked::Theme(s) => Some(s.hex.clone()),
            Picked::Rgb(h) => Some(h.clone()),
        }
    }
}

/// `ly`: a colour too light to see on white (gets a `#bbb` ring).
fn near_white(h: &str) -> bool {
    let (r, g, b) = rgb(h);
    (r as f32 * 299.0 + g as f32 * 587.0 + b as f32 * 114.0) / 1000.0 > 230.0
}

/// The colour dropdown. `current` is the selection's colour as `RRGGBB`
/// (its swatch shows chosen); `on_pick` gets each choice.
pub(crate) struct ColorButton {
    mode: Mode,
    split: bool,
    current: Rc<dyn Fn() -> Option<String>>,
    scheme: Scheme,
    disabled: Rc<dyn Fn() -> bool>,
    on_pick: Rc<dyn Fn(Picked)>,
}

pub(crate) fn color_button(mode: Mode) -> ColorButton {
    ColorButton { mode, split: true, current: Rc::new(|| None), scheme: Rc::new(|_| None), disabled: Rc::new(|| false), on_pick: Rc::new(|_| {}) }
}

impl ColorButton {
    /// One button opening the panel (`splitButton: false`).
    pub(crate) fn single(mut self) -> Self {
        self.split = false;
        self
    }
    pub(crate) fn current(mut self, hex: impl Fn() -> Option<String> + 'static) -> Self {
        self.current = Rc::new(hex);
        self
    }
    /// The document theme's slot colours (`colorScheme`).
    pub(crate) fn scheme(mut self, slot: impl Fn(&str) -> Option<String> + 'static) -> Self {
        self.scheme = Rc::new(slot);
        self
    }
    pub(crate) fn disabled(mut self, off: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Rc::new(off);
        self
    }
    pub(crate) fn on_pick(mut self, run: impl Fn(Picked) + 'static) -> Self {
        self.on_pick = Rc::new(run);
        self
    }
}

/// The colour bar under the icon (`ay`).
fn bar(shown: impl Fn() -> Option<String> + 'static) -> AnyElement {
    let shown = Rc::new(shown);
    let s2 = shown.clone();
    view()
        .w(px(BAR.0))
        .h(px(BAR.1))
        .mt(px(-2.0))
        .rounded(px(1.0))
        .bg(move || shown().map_or(AdaptiveColor::from(white()), |h| hex(&h).into()))
        .border_1()
        .border_color(move || match s2() {
            None => p::SWATCH_RING,
            Some(h) if near_white(&h) => p::SWATCH_RING,
            Some(_) => AdaptiveColor::TRANSPARENT,
        })
        .into_element()
}

impl IntoElement for ColorButton {
    fn into_element(self) -> AnyElement {
        let ColorButton { mode, split, current, scheme, disabled, on_pick } = self;
        let open = signal(false);
        let last = signal(Picked::Rgb(mode.first().to_owned()));
        let anchor = element_ref();
        let label = mode.label();
        let (h_apply, h_arrow) = (signal(false), signal(false));
        let off = disabled.clone();
        let ink = || p::TEXT_MUTED;
        let shown_last = move || last.get().bar(mode);
        let pick: Rc<dyn Fn(Picked)> = {
            let on_pick = on_pick.clone();
            Rc::new(move |c: Picked| {
                last.set(c.clone());
                open.set(false);
                on_pick(c)
            })
        };
        let part_bg = move |hovered: Signal<bool>, opens: bool| {
            move || {
                if opens && open.get() {
                    p::PRIMARY_LIGHT
                } else if hovered.get() {
                    p::BG_HOVER
                } else {
                    AdaptiveColor::TRANSPARENT
                }
            }
        };
        let column = move |shown: Rc<dyn Fn() -> Option<String>>| {
            view()
                .items_center()
                .justify_center()
                .child(icon(mode.icon(), 18.0, ink))
                .child(bar(move || shown()))
        };
        let trigger: AnyElement = if split {
            let (d1, d2) = (disabled.clone(), disabled.clone());
            let apply = {
                let on_pick = on_pick.clone();
                view()
                    .w(px(APPLY_W))
                    .py(px(2.0))
                    .px(px(4.0))
                    .rounded(px(4.0))
                    .items_center()
                    .justify_center()
                    .label(label)
                    .bg(part_bg(h_apply, false))
                    .on_mouse_enter(move |_: &MouseEvent| h_apply.set(true))
                    .on_mouse_leave(move |_: &MouseEvent| h_apply.set(false))
                    .on_press(move || {
                        if !d1() {
                            on_pick(last.get_untracked())
                        }
                    })
                    .child(column(Rc::new(shown_last)))
            };
            let arrow = view()
                .w(px(ARROW_W))
                .rounded(px(4.0))
                .items_center()
                .justify_center()
                .label(format!("{label} choices"))
                .bg(part_bg(h_arrow, true))
                .on_mouse_enter(move |_: &MouseEvent| h_arrow.set(true))
                .on_mouse_leave(move |_: &MouseEvent| h_arrow.set(false))
                .on_press(move || {
                    if !d2() {
                        open.update(|o| *o = !*o)
                    }
                })
                .child(icon("arrow_drop_down", 18.0, move || if open.get() { p::PRIMARY } else { p::TEXT_MUTED }));
            view()
                .flex_row()
                .h(px(SPLIT_H))
                .gap(px(2.0))
                .element_ref(&anchor)
                .opacity(move || if off() { DISABLED_OPACITY } else { 1.0 })
                .tooltip(plain_tip(label))
                .child(apply)
                .child(arrow)
                .into_element()
        } else {
            let d1 = disabled.clone();
            let cur = current.clone();
            let shown: Rc<dyn Fn() -> Option<String>> = Rc::new(move || cur().or_else(|| (!mode.none_is_clear()).then(|| "000000".to_owned())));
            view()
                .flex_row()
                .w(px(SINGLE_W))
                .h(px(SPLIT_H))
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(4.0))
                .items_center()
                .justify_center()
                .element_ref(&anchor)
                .label(label)
                .bg(move || {
                    if open.get() {
                        p::PRIMARY_LIGHT
                    } else if h_apply.get() {
                        p::BG_HOVER
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .opacity(move || if off() { DISABLED_OPACITY } else { 1.0 })
                .on_mouse_enter(move |_: &MouseEvent| h_apply.set(true))
                .on_mouse_leave(move |_: &MouseEvent| h_apply.set(false))
                .on_press(move || {
                    if !d1() {
                        open.update(|o| *o = !*o)
                    }
                })
                .tooltip(plain_tip(label))
                .child(column(shown))
                .child(icon("arrow_drop_down", 14.0, ink))
                .into_element()
        };
        let panel = dropdown(anchor, open, format!("{label} picker"), Panel::COLORS, move || {
            panel(mode, current.clone(), scheme.clone(), pick.clone())
        });
        view().child(trigger).child(panel).into_element()
    }
}

/// The panel's separator (`Gd`) and section heading (`Ud`).
fn rule() -> AnyElement {
    view().h(px(1.0)).my(px(8.0)).bg(p::BORDER).into_element()
}

fn heading(t: &'static str) -> AnyElement {
    text(t).text_size(px(11.0)).font_weight(FontWeight::MEDIUM).text_color(p::TEXT_MUTED).mb(px(4.0)).into_element()
}

fn panel(mode: Mode, current: Rc<dyn Fn() -> Option<String>>, scheme: Scheme, pick: Rc<dyn Fn(Picked)>) -> AnyElement {
    let draft = signal(current().filter(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit())).map(|h| h.to_uppercase()).unwrap_or_default());
    let valid = move || {
        let d = draft.get();
        d.len() == 6 && d.chars().all(|c| c.is_ascii_hexdigit())
    };
    let apply = {
        let pick = pick.clone();
        Rc::new(move || {
            let d = draft.get_untracked().to_uppercase();
            if d.len() == 6 && d.chars().all(|c| c.is_ascii_hexdigit()) {
                draft.set(String::new());
                pick(Picked::Rgb(d))
            }
        })
    };
    let chosen = {
        let current = current.clone();
        move |h: &str| current().is_some_and(|c| c.trim_start_matches('#').eq_ignore_ascii_case(h))
    };
    let chosen = Rc::new(chosen);
    let matrix = theme_matrix(&*scheme);
    let theme_pick = pick.clone();
    let std_pick = pick.clone();
    let clear_pick = pick;
    let (apply1, apply2) = (apply.clone(), apply);
    view()
        .child(clear_row(mode, move || clear_pick(Picked::Clear)))
        .child(rule())
        .child(heading("Theme Colors"))
        .child(grid(matrix.into_iter().flatten().map(|s| (s.label.clone(), s.hex.clone(), Some(s))).collect(), chosen.clone(), move |c| {
            theme_pick(match c {
                (_, Some(s)) => Picked::Theme(s),
                (h, None) => Picked::Rgb(h),
            })
        }))
        .child(rule())
        .child(heading("Standard Colors"))
        .child(grid(STANDARD.iter().map(|(n, h)| (n.to_string(), h.to_string(), None)).collect(), chosen, move |(h, _)| std_pick(Picked::Rgb(h))))
        .child(rule())
        .child(heading("Custom Color"))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(text("#").text_size(px(12.0)).text_color(p::TEXT_MUTED))
                .child(
                    nsh_base::input()
                        .value(move || draft.get())
                        .placeholder("FF0000")
                        .placeholder_color(|| color(p::TEXT_PLACEHOLDER))
                        .max_length(6)
                        .w(px(70.0))
                        .h(px(24.0))
                        .px(px(6.0))
                        .py(px(2.0))
                        .border_1()
                        .border_color(p::BORDER_INPUT)
                        .rounded(px(3.0))
                        .text_size(px(12.0))
                        .text_color(p::TEXT)
                        .label("Custom hex color")
                        .on_change(move |s: &str| draft.set(s.chars().filter(|c| c.is_ascii_hexdigit()).take(6).collect()))
                        .on_submit(move |_: &str| apply1()),
                )
                .child(
                    text("Apply")
                        .h(px(24.0))
                        .px(px(10.0))
                        .line_height(px(22.0))
                        .border_1()
                        .border_color(p::BORDER_INPUT)
                        .rounded(px(3.0))
                        .bg(p::BG_SUBTLE)
                        .text_size(px(12.0))
                        .text_color(p::TEXT)
                        .opacity(move || if valid() { 1.0f32 } else { 0.4 })
                        .label("Apply custom color")
                        .on_press(move || apply2()),
                ),
        )
        .into_element()
}

/// The clear row (`g1`): a full-width bordered button with a black
/// square (Automatic) or a white one struck through in red (No Color).
fn clear_row(mode: Mode, run: impl Fn() + 'static) -> AnyElement {
    let mark = if mode.none_is_clear() {
        canvas(|| {
            Painter::new()
                .fill_color(white())
                .fill_rect(px(0.0), px(0.0), px(16.0), px(16.0))
                .stroke_color(hex("CCCCCC"))
                .line_width(1.0)
                .stroke_rect(Bounds { origin: point(px(0.5), px(0.5)), size: size(px(15.0), px(15.0)) })
                .stroke_color(hex("FF0000"))
                .line_width(2.0)
                .line_cap(LineCap::Butt)
                // an 18-wide line through the middle, turned −45°
                .begin_path()
                .move_to(px(8.0 - 6.364), px(8.0 + 6.364))
                .line_to(px(8.0 + 6.364), px(8.0 - 6.364))
                .stroke()
        })
        .size(px(16.0))
        .into_element()
    } else {
        view().size(px(16.0)).rounded(px(2.0)).bg(AdaptiveColor::from(black())).into_element()
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .w_full()
        .px(px(8.0))
        .py(px(5.0))
        .border_1()
        .border_color(p::BORDER_DARK)
        .rounded(px(4.0))
        .bg(p::SURFACE)
        .label(mode.clear())
        .on_press(run)
        .child(mark)
        .child(text(mode.clear()).text_size(px(12.0)).text_color(p::TEXT))
        .into_element()
}

type Cell = (String, String, Option<Swatch>);
/// A document theme's slot colours (`colorScheme`).
type Scheme = Rc<dyn Fn(&str) -> Option<String>>;

/// A grid of swatches, ten to a row (`S1`, `P1`): 1px `--doc-border-input`
/// rims; under the pointer 1.15× with a `--doc-text` rim; the chosen one
/// a 2px `--doc-primary` rim and a 1px ring.
fn grid(cells: Vec<Cell>, chosen: Rc<dyn Fn(&str) -> bool>, pick: impl Fn((String, Option<Swatch>)) + 'static) -> AnyElement {
    let pick = Rc::new(pick);
    let rows: Vec<Vec<Cell>> = cells.chunks(10).map(|c| c.to_vec()).collect();
    view()
        .gap(px(SWATCH_GAP))
        .children(rows.into_iter().map(|row| {
            view()
                .flex_row()
                .gap(px(SWATCH_GAP))
                .children(row.into_iter().map(|(label, h, sw)| {
                    let hovered = signal(false);
                    let (c1, c2, c3) = (chosen.clone(), chosen.clone(), chosen.clone());
                    let (h1, h2, h3) = (h.clone(), h.clone(), h.clone());
                    let pick = pick.clone();
                    view()
                        .size(px(SWATCH))
                        .rounded(px(2.0))
                        .bg(hex(&h))
                        .border(move || px(if c1(&h1) { 2.0 } else { 1.0 }))
                        .border_color(move || {
                            if c2(&h2) {
                                p::PRIMARY
                            } else if hovered.get() {
                                p::TEXT
                            } else {
                                p::BORDER_INPUT
                            }
                        })
                        .scale(move || if hovered.get() && !c3(&h3) { 1.15f32 } else { 1.0 })
                        .z_index(move || if hovered.get() { 1 } else { 0 })
                        .label(label.clone())
                        .tooltip(plain_tip(label.clone()))
                        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
                        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
                        .on_press(move || pick((h.clone(), sw.clone())))
                        .into_element()
                }))
                .into_element()
        }))
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_grid_tints_and_shades_as_the_web() {
        let m = theme_matrix(&|_| None);
        assert_eq!(m.len(), 6);
        assert!(m.iter().all(|r| r.len() == 10));
        // accent1 4472C4: lighter 80% is 20% of it over white
        let a = &m[1][4];
        assert_eq!((a.hex.as_str(), a.tint, a.label.as_str()), ("DAE3F3", Some("CC"), "Accent 1, Lighter 80%"));
        let d = &m[5][4];
        assert_eq!((d.hex.as_str(), d.shade), ("223962", Some("80")));
        assert_eq!(m[0][1].hex, "000000");
        assert_eq!(m[4][0].hex, "BFBFBF", "Background 1, Darker 25%");
        let custom = theme_matrix(&|s| (s == "accent1").then(|| "112233".to_owned()));
        assert_eq!(custom[0][4].hex, "112233");
    }

    #[test]
    fn a_near_white_bar_gets_its_ring() {
        assert!(near_white("FFFFFF"));
        assert!(!near_white("FFFF00"));
        assert_eq!(Picked::Clear.bar(Mode::Highlight), None);
        assert_eq!(Picked::Clear.bar(Mode::Text).as_deref(), Some("000000"));
    }
}
