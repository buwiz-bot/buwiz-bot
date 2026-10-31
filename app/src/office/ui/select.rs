//! The formatting bar's dropdowns: the select (paragraph style, font
//! family in its groups, zoom; the web's Radix select, DR `li`/`ui`/`di`/
//! `Qr`/`Zi`/`ji`) on `nsh_base::Select`, and the font size box (DR `yy`):
//! −, the size, +, the size opening the preset list and taking a typed
//! size.

use std::rc::Rc;

use nsh_base::{SelectGroup, SelectItem, SelectItemState};
use nsh_core::canvas::{FillRule, svg_path};
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use super::button::{BUTTON, BUTTON_RADIUS, DISABLED_OPACITY, TOOLBAR_ICON, tooltip_of};
use super::icons::icon;
use super::palette as p;
use super::{Panel, dropdown};

/// The select's trigger (DR `ui`): `h-8 gap-1 rounded px-2 py-1 text-sm`.
pub(crate) const TRIGGER_H: f32 = 32.0;
pub(crate) const TRIGGER_PX: f32 = 8.0;
pub(crate) const TRIGGER_RADIUS: f32 = 4.0;
/// Its chevron (DR `VR`, `h-4 w-4`).
pub(crate) const CHEVRON: f32 = 16.0;
/// `text-sm`: 14 on a 20 line.
pub(crate) const TEXT_SM: f32 = 14.0;
pub(crate) const LINE_SM: f32 = 20.0;
/// The list (DR `di`): `rounded-lg border shadow-lg max-h-72`, `p-1`
/// inside, 4 below the trigger (`translate-y-1`).
pub(crate) const LIST_RADIUS: f32 = 8.0;
pub(crate) const LIST_PAD: f32 = 4.0;
pub(crate) const LIST_MAX_H: f32 = 288.0;
/// An item (DR `Qr`): `rounded px-2 py-1.5 text-sm`, its check `right-2`.
pub(crate) const ITEM_PX: f32 = 8.0;
pub(crate) const ITEM_PY: f32 = 6.0;
/// A group's label (DR `Zi`): `px-2 py-1.5 text-xs font-medium`.
pub(crate) const LABEL_SIZE: f32 = 12.0;
/// The font size box (DR `yy`): the steppers `h-7 w-7`, the size `h-7
/// w-10`; its list `maxHeight 240, minWidth 60`, rows `px-3 py-1.5`.
pub(crate) const SIZE_W: f32 = 40.0;
pub(crate) const SIZE_LIST_MAX_H: f32 = 240.0;
pub(crate) const SIZE_LIST_MIN_W: f32 = 60.0;

/// The web's font size presets (DR `w1`) and limits (`T1`, `I1`).
pub(crate) const FONT_SIZES: [f32; 14] = [8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 36.0, 48.0, 72.0];
pub(crate) const MIN_SIZE: f32 = 1.0;
pub(crate) const MAX_SIZE: f32 = 400.0;
/// The zoom presets (DR `xR`), labelled `Math.round(z*100)+"%"`.
pub(crate) const ZOOMS: [f32; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];

/// The select's chevron and the chosen item's check (heroicons, 20 box).
const CHEVRON_D: &str = "M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z";
const CHECK_D: &str = "M16.704 4.153a.75.75 0 01.143 1.052l-8 10.5a.75.75 0 01-1.127.075l-4.5-4.5a.75.75 0 011.06-1.06l3.894 3.893 7.48-9.817a.75.75 0 011.05-.143z";

fn mini_icon(d: &'static str, size: f32, ink: AdaptiveColor) -> AnyElement {
    let path = svg_path(d);
    canvas(move || Painter::new().scale(size / 20.0, size / 20.0).fill_color(color(ink)).fill_path(&path, FillRule::EvenOdd))
        .size(px(size))
        .flex_shrink_0()
        .into_element()
}

/// How an option's row previews it: a font family (the font list), or a
/// paragraph style's size, weight and colour (the style list).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Preview {
    pub family: Option<SharedString>,
    pub size: Option<f32>,
    pub bold: bool,
    pub italic: bool,
    pub color: Option<Hsla>,
}

/// An option of a [`Dropdown`].
#[derive(Clone, Debug)]
pub(crate) struct Opt {
    pub value: SharedString,
    pub label: SharedString,
    /// Its section's title; `Some("")` a section with no title (the font
    /// list's "other" fonts).
    pub group: Option<SharedString>,
    pub preview: Preview,
}

impl Opt {
    pub(crate) fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Opt { value: value.into(), label: label.into(), group: None, preview: Preview::default() }
    }
}

/// How wide the trigger is: at least (the font list's `minWidth`) or
/// exactly (the style list's `width`).
#[derive(Clone, Copy)]
pub(crate) enum Width {
    Min(f32),
    Fixed(f32),
}

/// A select styled as the web's: a borderless 32-point trigger with the
/// chosen label and a chevron, the list in a rounded card below.
pub(crate) struct Dropdown {
    label: SharedString,
    options: Vec<Opt>,
    value: Rc<dyn Fn() -> Option<SharedString>>,
    width: Width,
    /// The trigger's text: `text-sm` (14), or the zoom's `h-7 text-xs`.
    compact: bool,
    list_min_w: Option<f32>,
    list_max_h: f32,
    item_pad: (f32, f32),
    radius: f32,
    disabled: Rc<dyn Fn() -> bool>,
    on_change: Rc<dyn Fn(&str)>,
}

pub(crate) fn dropdown_select(label: impl Into<SharedString>, options: Vec<Opt>) -> Dropdown {
    Dropdown {
        label: label.into(),
        options,
        value: Rc::new(|| None),
        width: Width::Min(0.0),
        compact: false,
        list_min_w: None,
        list_max_h: LIST_MAX_H,
        item_pad: (ITEM_PX, ITEM_PY),
        radius: TRIGGER_RADIUS,
        disabled: Rc::new(|| false),
        on_change: Rc::new(|_| {}),
    }
}

impl Dropdown {
    pub(crate) fn value(mut self, v: impl Fn() -> Option<SharedString> + 'static) -> Self {
        self.value = Rc::new(v);
        self
    }
    pub(crate) fn width(mut self, w: Width) -> Self {
        self.width = w;
        self
    }
    pub(crate) fn list(mut self, min_w: Option<f32>, max_h: f32) -> Self {
        self.list_min_w = min_w;
        self.list_max_h = max_h;
        self
    }
    pub(crate) fn item_padding(mut self, x: f32, y: f32) -> Self {
        self.item_pad = (x, y);
        self
    }
    pub(crate) fn disabled(mut self, off: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Rc::new(off);
        self
    }
    pub(crate) fn on_change(mut self, run: impl Fn(&str) + 'static) -> Self {
        self.on_change = Rc::new(run);
        self
    }
}

impl IntoElement for Dropdown {
    fn into_element(self) -> AnyElement {
        let Dropdown { label, options, value, width, compact, list_min_w, list_max_h, item_pad, radius, disabled, on_change } = self;
        let items: Vec<SelectItem> = options
            .iter()
            .map(|o| {
                let it = SelectItem::new(o.value.clone(), o.label.clone());
                match &o.group {
                    Some(g) => it.group(g.clone()),
                    None => it,
                }
            })
            .collect();
        let first_group = options.first().and_then(|o| o.group.clone());
        let opts = Rc::new(options);
        let shown = {
            let (opts, value) = (opts.clone(), value.clone());
            move || {
                let v = value();
                v.as_ref().and_then(|v| opts.iter().find(|o| &o.value == v)).map(|o| o.label.clone()).or(v).unwrap_or_default()
            }
        };
        let hovered = signal(false);
        let off = disabled.clone();
        let (size, line, h) = if compact { (12.0, 16.0, BUTTON) } else { (TEXT_SM, LINE_SM, TRIGGER_H) };
        let mut trigger = view()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(4.0))
            .h(px(h))
            .px(px(TRIGGER_PX))
            .py(px(4.0))
            .rounded(px(radius))
            .label(label.clone())
            .bg(move || if hovered.get() { p::TW_MUTED_80 } else { AdaptiveColor::TRANSPARENT })
            .opacity(move || if off() { 0.5f32 } else { 1.0 })
            .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
            .on_mouse_leave(move |_: &MouseEvent| hovered.set(false));
        trigger = match width {
            Width::Min(w) => trigger.min_w(px(w)),
            Width::Fixed(w) => trigger.w(px(w)),
        };
        let trigger = trigger
            .child(
                text(shown)
                    .flex_shrink(1.0)
                    .number_of_lines(1)
                    .text_size(px(size))
                    .line_height(px(line))
                    .text_color(p::TW_FOREGROUND),
            )
            .child(mini_icon(CHEVRON_D, CHEVRON, p::TW_MUTED_FOREGROUND));
        let row_opts = opts.clone();
        let value2 = value.clone();
        nsh_base::select()
            .items(items)
            .value(move || nsh_base::SelectValue::Controlled(value2()))
            .disabled(move || disabled())
            .accessibility_label(label)
            .placement(Placement::Bottom)
            .align(Align::Start)
            .offset(px(4.0))
            .popup_style(move |s: StyleRefinement| {
                let s = s.bg(p::TW_POPOVER).border_1().border_color(p::TW_BORDER).rounded(px(LIST_RADIUS)).p(px(LIST_PAD)).max_h(px(list_max_h));
                match list_min_w {
                    Some(w) => s.min_w(px(w)),
                    None => s.min_w(px(128.0)),
                }
            })
            .trigger(trigger)
            .render_item(move |s: &SelectItemState| {
                let preview = row_opts.iter().find(|o| o.value == s.value).map(|o| o.preview.clone()).unwrap_or_default();
                item_row(s, &preview, item_pad)
            })
            .render_group(move |g: &SelectGroup| group_header(g, first_group.as_ref() == Some(&g.title)))
            .on_change(move |v: &Option<SharedString>| {
                if let Some(v) = v {
                    on_change(v)
                }
            })
            .into_element()
    }
}

/// An item's row: `hover:bg-muted focus:bg-muted`, the check at its right
/// while chosen, its text in its own preview.
fn item_row(s: &SelectItemState, preview: &Preview, (px_, py_): (f32, f32)) -> AnyElement {
    let mut t = text(s.title.clone())
        .text_size(px(preview.size.unwrap_or(TEXT_SM)))
        .line_height(px(preview.size.map_or(LINE_SM, |z| z * 1.3)))
        .text_color(preview.color.map_or(p::TW_FOREGROUND, |c| AdaptiveColor { light: c, dark: c }));
    if let Some(f) = &preview.family {
        t = t.font_family(f.clone());
    }
    if preview.bold {
        t = t.font_weight(FontWeight::BOLD);
    }
    if preview.italic {
        t = t.italic();
    }
    view()
        .flex_row()
        .items_center()
        .w_full()
        .rounded(px(4.0))
        .px(px(px_))
        .py(px(py_))
        .bg(if s.highlighted { p::TW_MUTED } else { AdaptiveColor::TRANSPARENT })
        .opacity(if s.disabled { 0.5 } else { 1.0 })
        .child(t)
        .when(s.selected, |v| {
            v.child(
                view()
                    .absolute()
                    .right(px(8.0))
                    .size(px(14.0))
                    .items_center()
                    .justify_center()
                    .child(mini_icon(CHECK_D, 16.0, p::TW_FOREGROUND)),
            )
        })
        .into_element()
}

/// A section's label (DR `Zi`), after a separator (DR `ji`, `-mx-1 my-1
/// h-px bg-muted`) unless it is the first; an untitled section has the
/// separator only.
fn group_header(g: &SelectGroup, first: bool) -> AnyElement {
    view()
        .when(!first, |v| v.child(view().h(px(1.0)).mx(px(-4.0)).my(px(4.0)).bg(p::TW_MUTED)))
        .when(!g.title.is_empty(), |v| {
            v.child(
                text(g.title.clone())
                    .px(px(8.0))
                    .py(px(6.0))
                    .text_size(px(LABEL_SIZE))
                    .line_height(px(16.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p::TW_MUTED_FOREGROUND),
            )
        })
        .into_element()
}

/// A font of the font list (DR `dd`): its name, its CSS family and its
/// section.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Font {
    pub name: &'static str,
    pub family: &'static str,
    pub category: Category,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Category {
    SansSerif,
    Serif,
    Monospace,
    Other,
}

/// The web's twelve default fonts (DR `dd`).
pub(crate) const FONTS: [Font; 12] = [
    Font { name: "Arial", family: "Arial", category: Category::SansSerif },
    Font { name: "Calibri", family: "Calibri", category: Category::SansSerif },
    Font { name: "Helvetica", family: "Helvetica", category: Category::SansSerif },
    Font { name: "Verdana", family: "Verdana", category: Category::SansSerif },
    Font { name: "Open Sans", family: "Open Sans", category: Category::SansSerif },
    Font { name: "Roboto", family: "Roboto", category: Category::SansSerif },
    Font { name: "Times New Roman", family: "Times New Roman", category: Category::Serif },
    Font { name: "Georgia", family: "Georgia", category: Category::Serif },
    Font { name: "Cambria", family: "Cambria", category: Category::Serif },
    Font { name: "Garamond", family: "Garamond", category: Category::Serif },
    Font { name: "Courier New", family: "Courier New", category: Category::Monospace },
    Font { name: "Consolas", family: "Consolas", category: Category::Monospace },
];

/// The font list's options (DR `rg`): the document's own fonts first
/// ("Document fonts", those not in `fonts`), then Sans Serif, Serif,
/// Monospace and the untitled rest, each previewed in its font.
pub(crate) fn font_options(fonts: &[Font], document: &[&str]) -> Vec<Opt> {
    let mut out = Vec::new();
    let known = |n: &str| fonts.iter().any(|f| f.name.eq_ignore_ascii_case(n));
    for d in document.iter().filter(|d| !known(d)) {
        out.push(Opt {
            value: SharedString::from(d.to_string()),
            label: SharedString::from(d.to_string()),
            group: Some("Document fonts".into()),
            preview: Preview { family: Some(SharedString::from(d.to_string())), ..Preview::default() },
        });
    }
    for (cat, title) in [(Category::SansSerif, "Sans Serif"), (Category::Serif, "Serif"), (Category::Monospace, "Monospace"), (Category::Other, "")] {
        for f in fonts.iter().filter(|f| f.category == cat) {
            out.push(Opt {
                value: f.name.into(),
                label: f.name.into(),
                group: Some(title.into()),
                preview: Preview { family: Some(f.family.into()), ..Preview::default() },
            });
        }
    }
    out
}

/// The font list (DR `rg`): at least 60 wide in the toolbar (`width:
/// 60`), its list up to 300 high.
pub(crate) fn font_family_select(document: &[&str]) -> Dropdown {
    dropdown_select("Select font family", font_options(&FONTS, document)).width(Width::Min(60.0)).list(None, 300.0)
}

/// A paragraph style of the style list.
#[derive(Clone, Debug)]
pub(crate) struct StyleOpt {
    pub id: &'static str,
    pub name: &'static str,
    /// In half-points (`w:sz`), as the web's style table gives it.
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub italic: bool,
    pub color: Option<&'static str>,
}

/// The web's default styles (DR `Gl`, names from `styles.*`).
pub(crate) const STYLES: [StyleOpt; 6] = [
    StyleOpt { id: "Normal", name: "Normal text", size: Some(22.0), bold: None, italic: false, color: None },
    StyleOpt { id: "Title", name: "Title", size: Some(52.0), bold: Some(true), italic: false, color: None },
    StyleOpt { id: "Subtitle", name: "Subtitle", size: Some(30.0), bold: None, italic: false, color: Some("666666") },
    StyleOpt { id: "Heading1", name: "Heading 1", size: Some(40.0), bold: Some(true), italic: false, color: None },
    StyleOpt { id: "Heading2", name: "Heading 2", size: Some(32.0), bold: Some(true), italic: false, color: None },
    StyleOpt { id: "Heading3", name: "Heading 3", size: Some(28.0), bold: Some(true), italic: false, color: None },
];

/// How the style list previews a style (`getStylePreviewProps`,
/// `@betteroffice/docx/utils/stylePreview`): fixed sizes for the built-in
/// styles, else half the style's size within 11–20; bold for Title and
/// Headings 1–3; Subtitle grey, the other headings `#4a6c8c`.
pub(crate) fn style_preview(s: &StyleOpt) -> Preview {
    let fixed = match s.id {
        "Title" => Some(26.0),
        "Subtitle" | "Heading2" => Some(18.0),
        "Heading1" => Some(24.0),
        "Heading3" => Some(16.0),
        "Heading4" => Some(14.0),
        "Heading5" | "Heading6" => Some(13.0),
        "Normal" => Some(14.0),
        _ => None,
    };
    let size = fixed.unwrap_or_else(|| s.size.map_or(11.0, |z| z / 2.0).clamp(11.0, 20.0));
    let bold = s.bold.unwrap_or(matches!(s.id, "Title" | "Heading1" | "Heading2" | "Heading3"));
    let hex = s.color.or(if s.id == "Subtitle" { Some("666666") } else { None });
    let color = match hex {
        Some(h) => Some(super::color::hex(h)),
        None if s.id.starts_with("Heading") => Some(super::color::hex("4a6c8c")),
        None => None,
    };
    Preview { family: None, size: Some(size), bold, italic: s.italic, color }
}

/// The style list (DR `sg`): 120 wide, its list at least 260 wide and up
/// to 400 high, rows `py-2.5 px-3`.
pub(crate) fn style_select(styles: &[StyleOpt]) -> Dropdown {
    let opts = styles.iter().map(|s| Opt { preview: style_preview(s), ..Opt::new(s.id, s.name) }).collect();
    dropdown_select("Select paragraph style", opts).width(Width::Fixed(120.0)).list(Some(260.0), 400.0).item_padding(12.0, 10.0)
}

/// A zoom's label (DR `Oy`): "125%".
pub(crate) fn zoom_label(z: f32) -> String {
    format!("{}%", (z * 100.0).round())
}

/// The zoom control (DR `Oy`, `compact`): −, the level select (`h-7
/// min-w-[55px] text-xs rounded-none`), +; − and + step to the next
/// preset, dimmed past the ends.
pub(crate) fn zoom_control(value: impl Fn() -> f32 + 'static, on_change: impl Fn(f32) + 'static) -> AnyElement {
    let value = Rc::new(value);
    let on_change = Rc::new(on_change);
    let below = {
        let v = value.clone();
        move || ZOOMS.iter().rev().copied().find(|z| *z < v() - 0.001)
    };
    let above = {
        let v = value.clone();
        move || ZOOMS.iter().copied().find(|z| *z > v() + 0.001)
    };
    let opts = ZOOMS.iter().map(|z| Opt::new(z.to_string(), zoom_label(*z))).collect();
    let (v, set) = (value.clone(), on_change.clone());
    let mut select = dropdown_select("Zoom level", opts)
        .width(Width::Min(55.0))
        .value(move || {
            let z = v();
            Some(ZOOMS.iter().find(|p| (*p - z).abs() < 0.001).map_or_else(|| zoom_label(z), |p| p.to_string()).into())
        })
        .on_change(move |s| {
            if let Ok(z) = s.parse::<f32>() {
                set(z)
            }
        });
    select.compact = true;
    select.radius = 0.0;
    let (b2, a2) = (below.clone(), above.clone());
    let (s1, s2) = (on_change.clone(), on_change);
    view()
        .flex_row()
        .items_center()
        .child(stepper("Zoom out", "remove", Side::Left, move || b2().is_none(), move || {
            if let Some(z) = below() {
                s1(z)
            }
        }))
        .child(select)
        .child(stepper("Zoom in", "add", Side::Right, move || a2().is_none(), move || {
            if let Some(z) = above() {
                s2(z)
            }
        }))
        .into_element()
}

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
}

/// A − or + beside a box (DR `yy`, `Oy`): `h-7 w-7`, rounded on its outer
/// side only, `hover:text-foreground hover:bg-muted/80`.
fn stepper(label: &'static str, name: &'static str, side: Side, off: impl Fn() -> bool + 'static, run: impl Fn() + 'static) -> AnyElement {
    let hovered = signal(false);
    let off = Rc::new(off);
    let (o1, o2, o3) = (off.clone(), off.clone(), off);
    let r = px(BUTTON_RADIUS);
    let zero = px(0.0);
    let v = view()
        .w(px(BUTTON))
        .h(px(BUTTON))
        .items_center()
        .justify_center()
        .label(label)
        .bg(move || if hovered.get() && !o1() { p::TW_MUTED_80 } else { AdaptiveColor::TRANSPARENT })
        .opacity(move || if o2() { DISABLED_OPACITY } else { 1.0 })
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_press(move || {
            if !o3() {
                run()
            }
        });
    let v = match side {
        Side::Left => v.rounded_tl(r).rounded_bl(r).rounded_tr(zero).rounded_br(zero),
        Side::Right => v.rounded_tr(r).rounded_br(r).rounded_tl(zero).rounded_bl(zero),
    };
    v.child(icon(name, TOOLBAR_ICON, move || if hovered.get() { p::TW_FOREGROUND } else { p::TW_MUTED_FOREGROUND }))
        .into_element()
}

/// The next preset above `size`, else one more, at most `MAX_SIZE` (DR
/// `fy`).
pub(crate) fn size_up(size: f32) -> f32 {
    FONT_SIZES.iter().copied().find(|s| *s > size).unwrap_or((size + 1.0).min(MAX_SIZE))
}

/// The preset below `size`, else one less, at least `MIN_SIZE` (DR `gy`).
pub(crate) fn size_down(size: f32) -> f32 {
    FONT_SIZES.iter().rev().copied().find(|s| *s < size).unwrap_or((size - 1.0).max(MIN_SIZE))
}

/// A typed size: a number within 1–400, to the nearest half point (DR
/// `yy`'s commit); anything else is no size.
pub(crate) fn typed_size(s: &str) -> Option<f32> {
    let n: f32 = s.trim().parse().ok()?;
    (n.is_finite() && (MIN_SIZE..=MAX_SIZE).contains(&n)).then(|| (n * 2.0).round() / 2.0)
}

/// A size as the box shows it: "11", "10.5".
pub(crate) fn size_text(s: f32) -> String {
    if s.fract() == 0.0 { format!("{}", s as i64) } else { format!("{s}") }
}

/// The font size box (DR `yy`). `value` is the size in points, `None` for
/// a mixed selection (the box shows the placeholder, "11"). `custom`
/// adds "Custom size…" under the presets (the web's overflow menu has it,
/// `commands.customFontSize`; the toolbar box takes a typed size instead).
pub(crate) struct FontSize {
    value: Rc<dyn Fn() -> Option<f32>>,
    disabled: Rc<dyn Fn() -> bool>,
    on_change: Rc<dyn Fn(f32)>,
    custom: Option<Rc<dyn Fn()>>,
}

pub(crate) fn font_size(value: impl Fn() -> Option<f32> + 'static) -> FontSize {
    FontSize { value: Rc::new(value), disabled: Rc::new(|| false), on_change: Rc::new(|_| {}), custom: None }
}

impl FontSize {
    pub(crate) fn disabled(mut self, off: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Rc::new(off);
        self
    }
    pub(crate) fn on_change(mut self, run: impl Fn(f32) + 'static) -> Self {
        self.on_change = Rc::new(run);
        self
    }
    pub(crate) fn custom(mut self, ask: impl Fn() + 'static) -> Self {
        self.custom = Some(Rc::new(ask));
        self
    }
}

impl IntoElement for FontSize {
    fn into_element(self) -> AnyElement {
        let FontSize { value, disabled, on_change, custom } = self;
        let current = {
            let v = value.clone();
            move || v().unwrap_or(11.0)
        };
        let current = Rc::new(current);
        // editing: the box is a field; open: the list shows
        let (editing, open) = (signal(false), signal(false));
        let draft = signal(String::new());
        let anchor = element_ref();
        let hovered = signal(false);
        let commit = {
            let on_change = on_change.clone();
            move || {
                editing.set(false);
                if let Some(s) = typed_size(&draft.get_untracked()) {
                    on_change(s)
                }
            }
        };
        let commit2 = commit.clone();
        let shown = {
            let v = value.clone();
            move || v().map_or_else(|| "11".to_owned(), size_text)
        };
        let shown2 = shown.clone();
        let off = disabled.clone();
        let display = dynamic(move || {
            if editing.get() {
                let commit = commit.clone();
                let commit2 = commit2.clone();
                return nsh_base::input()
                    .auto_focus(true)
                    .value(move || draft.get())
                    .w(px(SIZE_W))
                    .h(px(BUTTON))
                    .border_1()
                    .border_color(p::TW_RING)
                    .bg(p::BG_INPUT)
                    .text_size(px(TEXT_SM))
                    .text_color(p::TEXT)
                    .text_center()
                    .label("Select font size")
                    .on_change(move |s: &str| draft.set(s.to_owned()))
                    .on_submit(move |_: &str| {
                        commit();
                        open.set(false)
                    })
                    .on_blur(move || {
                        if editing.get_untracked() {
                            commit2()
                        }
                    })
                    .into_element();
            }
            let shown = shown2.clone();
            let off = off.clone();
            let (shown_t, off_t) = (shown.clone(), off.clone());
            text(shown_t)
                .w(px(SIZE_W))
                .h(px(BUTTON))
                .text_size(px(TEXT_SM))
                .line_height(px(BUTTON - 2.0))
                .text_center()
                .text_color(p::TEXT)
                .border_1()
                .border_color(move || if hovered.get() { p::TW_INPUT } else { p::TW_BORDER })
                .bg(move || if hovered.get() { p::BG_HOVER } else { p::BG_INPUT })
                .opacity(move || if off_t() { 0.5f32 } else { 1.0 })
                .label("Select font size")
                .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
                .on_press({
                    let shown = shown.clone();
                    let off = off.clone();
                    move || {
                        if !off() {
                            draft.set(shown());
                            editing.set(true);
                            open.set(true);
                        }
                    }
                })
                .into_element()
        });
        // Up and Down step while the field is open (DR `yy`'s keys)
        let (c_up, c_down, set_up, set_down) = (current.clone(), current.clone(), on_change.clone(), on_change.clone());
        effect(move || {
            if !editing.get() {
                return;
            }
            let (c, s) = (c_up.clone(), set_up.clone());
            on_key_with("up", KeyOptions { when: nsh_core::keymap::When::Always, repeat: true }, move |_| {
                let n = size_up(c());
                draft.set(size_text(n));
                s(n)
            });
            let (c, s) = (c_down.clone(), set_down.clone());
            on_key_with("down", KeyOptions { when: nsh_core::keymap::When::Always, repeat: true }, move |_| {
                let n = size_down(c());
                draft.set(size_text(n));
                s(n)
            });
        });
        let list = {
            let current = current.clone();
            let on_change = on_change.clone();
            let custom = custom.clone();
            dropdown(anchor, open, "Font sizes", Panel::LIST, move || {
                let (current, on_change) = (current.clone(), on_change.clone());
                scroll()
                    .max_h(px(SIZE_LIST_MAX_H))
                    .min_w(px(SIZE_LIST_MIN_W))
                    .children(FONT_SIZES.iter().map(|s| {
                        let (current, on_change) = (current.clone(), on_change.clone());
                        let s = *s;
                        size_row(size_text(s), move || current() == s, move || {
                            editing.set(false);
                            open.set(false);
                            on_change(s)
                        })
                    }))
                    .when_some(custom.clone(), |v, ask| {
                        v.child(size_row("Custom size…".to_owned(), || false, move || {
                            editing.set(false);
                            open.set(false);
                            ask()
                        }))
                    })
                    .into_element()
            })
        };
        let (c1, c2) = (current.clone(), current);
        let (s1, s2) = (on_change.clone(), on_change);
        let (d1, d2) = (disabled.clone(), disabled);
        let (c1b, c2b) = (c1.clone(), c2.clone());
        view()
            .flex_row()
            .items_center()
            .element_ref(&anchor)
            .tooltip(tooltip_of("Font size"))
            .child(stepper("Decrease font size", "remove", Side::Left, move || d1() || c1b() <= MIN_SIZE, move || s1(size_down(c1()))))
            .child(display)
            .child(stepper("Increase font size", "add", Side::Right, move || d2() || c2b() >= MAX_SIZE, move || s2(size_up(c2()))))
            .child(list)
            .into_element()
    }
}

/// A row of the size list: `px-3 py-1.5 text-sm`, `bg-muted` (and
/// medium) while it is the size, `hover:bg-muted`.
fn size_row(label: String, on: impl Fn() -> bool + 'static, pick: impl Fn() + 'static) -> AnyElement {
    let hovered = signal(false);
    let on = Rc::new(on);
    let on2 = on.clone();
    text(label.clone())
        .w_full()
        .px(px(12.0))
        .py(px(6.0))
        .text_size(px(TEXT_SM))
        .line_height(px(LINE_SM))
        .text_color(p::TW_FOREGROUND)
        // the list is built as it opens: the size it shows is the one now
        .font_weight(if on2() { FontWeight::MEDIUM } else { FontWeight::NORMAL })
        .bg(move || if on() || hovered.get() { p::TW_MUTED } else { AdaptiveColor::TRANSPARENT })
        .label(label)
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_press(pick)
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_steppers_walk_the_presets_then_by_one() {
        assert_eq!(size_up(11.0), 12.0);
        assert_eq!(size_up(10.5), 11.0);
        assert_eq!(size_up(72.0), 73.0);
        assert_eq!(size_up(400.0), 400.0);
        assert_eq!(size_down(12.0), 11.0);
        assert_eq!(size_down(8.0), 7.0);
        assert_eq!(size_down(1.0), 1.0);
    }

    #[test]
    fn a_typed_size_rounds_to_half_points_within_the_limits() {
        assert_eq!(typed_size("13"), Some(13.0));
        assert_eq!(typed_size(" 10.3 "), Some(10.5));
        assert_eq!(typed_size("0.5"), None);
        assert_eq!(typed_size("401"), None);
        assert_eq!(typed_size("big"), None);
        assert_eq!(size_text(10.5), "10.5");
        assert_eq!(size_text(11.0), "11");
    }

    #[test]
    fn fonts_fall_into_the_webs_sections_document_fonts_first() {
        let o = font_options(&FONTS, &["Carlito", "arial"]);
        assert_eq!(o[0].label.as_ref(), "Carlito");
        assert_eq!(o[0].group.as_deref(), Some("Document fonts"));
        assert_eq!(o.len(), 13, "Arial is one of the list's own");
        let groups: Vec<_> = o.iter().filter_map(|x| x.group.as_deref()).collect();
        assert_eq!(groups.iter().filter(|g| **g == "Sans Serif").count(), 6);
        assert_eq!(groups.iter().filter(|g| **g == "Serif").count(), 4);
        assert_eq!(groups.iter().filter(|g| **g == "Monospace").count(), 2);
        assert_eq!(o[1].preview.family.as_deref(), Some("Arial"));
    }

    #[test]
    fn styles_preview_as_the_web_does() {
        let t = style_preview(&STYLES[1]);
        assert_eq!((t.size, t.bold), (Some(26.0), true));
        let s = style_preview(&STYLES[2]);
        assert_eq!(s.size, Some(18.0));
        assert_eq!(s.color, Some(super::super::color::hex("666666")));
        let h = style_preview(&StyleOpt { id: "Heading4", name: "Heading 4", size: None, bold: None, italic: false, color: None });
        assert_eq!((h.size, h.bold, h.color), (Some(14.0), false, Some(super::super::color::hex("4a6c8c"))));
        let other = style_preview(&StyleOpt { id: "Quote", name: "Quote", size: Some(48.0), bold: None, italic: true, color: None });
        assert_eq!((other.size, other.italic), (Some(20.0), true));
        assert_eq!(zoom_label(1.25), "125%");
    }
}
