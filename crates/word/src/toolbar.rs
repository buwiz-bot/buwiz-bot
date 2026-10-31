//! The formatting bar as the web editor draws it (DR `lL` in `mp`'s
//! `bb`): the groups in its order, each control from the kit, its active
//! and disabled states from the selection the worker last reported
//! ([`Formats`], the cell, the mode), the contextual Image and Table
//! groups, and the overflow: when the bar is narrower than its controls,
//! everything from the first part that doesn't fit on goes into the "More
//! actions" menu, each part as the menu entries the web gives it (`RE`).

use std::rc::Rc;

use nsh_core::prelude::*;

use super::WordUi;
use super::dialogs::{self, Prompt};
use super::doc::{Cmd, EditMode, Fmt, Meta, Picture, TableOp, TextColor};
use super::view::{focus, key, send, set_zoom};
use crate::live::store;
use crate::office::ui::button::{BUTTON, BUTTON_RADIUS, Choice, DISABLED_OPACITY, GROUP_GAP, GROUP_PX, icon_button, icon_dropdown, separator, tooltip_of};
use crate::office::ui::color::{Mode, Picked, STANDARD, color_button, theme_matrix};
use crate::office::ui::icons::icon;
use crate::office::ui::menu::{Entry, Item, Look, item, menu_popup};
use crate::office::ui::palette as p;
use crate::office::ui::select::{FONT_SIZES, Opt, STYLES, StyleOpt, Width, ZOOMS, dropdown_select, font_family_select, font_size, style_preview, zoom_control, zoom_label};
use crate::office::ui::{Panel, dropdown, shortcut_label};

fn ui() -> &'static WordUi {
    &store().office.word
}

fn meta() -> std::sync::Arc<Meta> {
    ui().meta.get()
}

fn viewing() -> bool {
    ui().meta.with(|m| m.mode == EditMode::Viewing)
}

fn suggesting() -> bool {
    ui().meta.with(|m| m.mode == EditMode::Suggesting)
}

/// A command, the keyboard back on the page after it.
fn run(cmd: Cmd) {
    send(cmd);
    focus();
}

fn fmt(f: Fmt) {
    run(Cmd::Format(f));
}

/// The chord as a menu row shows it.
fn chord(c: &str) -> SharedString {
    shortcut_label(c, apple_keys()).into()
}

/// The overflow's row for a button.
fn row(icon: &'static str, label: &str, shortcut: Option<&str>, checked: bool, off: Option<&str>, on: impl Fn() + 'static) -> Entry {
    let mut it = item(Some(icon), label.to_owned(), on).checked(checked);
    if let Some(s) = shortcut {
        it = it.shortcut(chord(s));
    }
    if let Some(why) = off {
        it = it.disabled(Some(why));
    }
    Entry::Item(it)
}

/// A choice of a submenu (a radio row).
fn pick(label: impl Into<SharedString>, checked: bool, off: Option<&str>, on: impl Fn() + 'static) -> Entry {
    let mut it = item(None, label, on).checked(checked);
    if let Some(why) = off {
        it = it.disabled(Some(why));
    }
    Entry::Item(it)
}

fn submenu(label: &str, entries: Vec<Entry>) -> Entry {
    Entry::Submenu { icon: None, label: label.to_owned().into(), entries }
}

/// Why the selection's commands are off (`cr`): Viewing.
fn gate() -> Option<&'static str> {
    viewing().then_some(super::doc::format::VIEWING)
}

/// Gated also for what Suggesting can't track (`id`).
fn gate_tracked() -> Option<&'static str> {
    gate().or(suggesting().then_some(super::doc::format::NOT_SUGGESTED))
}

/// A part of the bar: a group (`Cr`, labelled) or a lone control or
/// separator; shown while `present` (the Image and Table groups), its
/// controls built by `build`, its overflow rows by `entries`.
struct Part {
    label: Option<&'static str>,
    present: Rc<dyn Fn() -> bool>,
    build: Rc<dyn Fn() -> Vec<AnyElement>>,
    entries: Rc<dyn Fn() -> Vec<Entry>>,
}

fn part(label: Option<&'static str>, build: impl Fn() -> Vec<AnyElement> + 'static, entries: impl Fn() -> Vec<Entry> + 'static) -> Part {
    Part { label, present: Rc::new(|| true), build: Rc::new(build), entries: Rc::new(entries) }
}

fn sep() -> Part {
    part(None, || vec![separator()], Vec::new)
}

const ALIGNS: [(&str, &str, &str, &str); 4] = [
    ("left", "format_align_left", "Align Left", "Ctrl+L"),
    ("center", "format_align_center", "Center", "Ctrl+E"),
    ("right", "format_align_right", "Align Right", "Ctrl+R"),
    ("both", "format_align_justify", "Justify", "Ctrl+J"),
];

/// The line spacing presets (`F1`): 240ths of a line.
pub(crate) const SPACINGS: [(u32, &str); 4] = [(240, "Single"), (276, "1.15"), (360, "1.5"), (480, "Double")];

/// The wrap menu (`M1`) and the overflow's (`TR`).
const WRAPS: [(&str, &str, &str); 5] = [
    ("inline", "wrap_text", "In Line with Text"),
    ("wrapRight", "format_image_left", "Square Left"),
    ("wrapLeft", "format_image_right", "Square Right"),
    ("behind", "flip_to_back", "Behind Text"),
    ("inFront", "flip_to_front", "In Front of Text"),
];
const TRANSFORMS: [(&str, &str, &str); 4] = [
    ("rotateCW", "rotate_right", "Rotate clockwise"),
    ("rotateCCW", "rotate_left", "Rotate counter-clockwise"),
    ("flipH", "swap_horiz", "Flip horizontal"),
    ("flipV", "swap_vert", "Flip vertical"),
];
const BORDERS: [(&str, &str, &str); 8] = [
    ("all", "border_all", "All borders"),
    ("outside", "border_outer", "Outside borders"),
    ("inside", "border_inner", "Inside borders"),
    ("top", "border_top", "Top border"),
    ("bottom", "border_bottom", "Bottom border"),
    ("left", "border_left", "Left border"),
    ("right", "border_right", "Right border"),
    ("none", "border_clear", "No borders"),
];
/// The border widths (`W1`): eighths of a point, label, line thickness.
pub(crate) const WIDTHS: [(u32, &str, f32); 5] = [(4, "0.5 pt", 0.5), (8, "1 pt", 1.0), (12, "1.5 pt", 1.5), (16, "2 pt", 2.0), (24, "3 pt", 3.0)];
/// The editing modes (`ks`): value, icon, label, description.
pub(crate) const MODES: [(EditMode, &str, &str, &str); 3] = [
    (EditMode::Editing, "edit_note", "Editing", "Edit document directly"),
    (EditMode::Suggesting, "rate_review", "Suggesting", "Edits become suggestions"),
    (EditMode::Viewing, "visibility", "Viewing", "Read-only, no edits"),
];

/// The style list's styles (`gw`): the document's, else the web's six;
/// the six named as the web names them.
fn styles(m: &Meta) -> Vec<StyleOpt> {
    if m.styles.is_empty() {
        return STYLES.to_vec();
    }
    m.styles
        .iter()
        .map(|s| {
            let name = STYLES.iter().find(|d| d.id == s.id).map_or_else(|| s.name.clone(), |d| d.name.to_string());
            StyleOpt { id: s.id.clone().into(), name: name.into(), size: s.size, bold: s.bold, italic: s.italic, color: s.color.clone().map(Into::into) }
        })
        .collect()
}

pub(crate) fn pick_color(mode: Mode, picked: Picked) {
    match mode {
        Mode::Text => run(Cmd::Color(match picked {
            Picked::Clear => None,
            Picked::Theme(s) => Some(TextColor::Theme(s.slot.to_owned())),
            Picked::Rgb(h) => Some(TextColor::Rgb(h)),
        })),
        Mode::Highlight => run(Cmd::Highlight(match picked {
            Picked::Clear => None,
            Picked::Theme(s) => Some(s.hex),
            Picked::Rgb(h) => Some(h),
        })),
        Mode::Border => run(Cmd::Table(TableOp::BorderColor(match picked {
            Picked::Clear => "000000".to_owned(),
            Picked::Theme(s) => s.hex,
            Picked::Rgb(h) => h,
        }))),
        Mode::Fill => run(Cmd::Table(TableOp::Fill(match picked {
            Picked::Clear => None,
            Picked::Theme(s) => Some(s.hex),
            Picked::Rgb(h) => Some(h),
        }))),
    }
}

/// A colour picker's overflow rows (`ip`): the clear row, the theme
/// colours by column, the standard colours, a typed colour.
fn color_entries(mode: Mode, label: &str, clear: &str, current: Option<String>, off: Option<&'static str>) -> Entry {
    let cur = current.map(|c| c.to_uppercase());
    let theme = theme_matrix(&|_| None);
    let columns: Vec<Entry> = (0..theme[0].len())
        .map(|c| {
            let cells: Vec<Entry> = theme
                .iter()
                .map(|r| {
                    let s = r[c].clone();
                    let on = cur.as_deref() == Some(s.hex.as_str());
                    pick(s.label.clone(), on, off, move || pick_color(mode, Picked::Theme(s.clone())))
                })
                .collect();
            submenu(&theme[0][c].label, cells)
        })
        .collect();
    let mut entries = vec![pick(clear.to_owned(), cur.is_none(), off, move || pick_color(mode, Picked::Clear)), submenu("Theme Colors", columns), Entry::Heading("Standard Colors".into())];
    for (name, hex) in STANDARD {
        entries.push(pick(name, cur.as_deref() == Some(hex), off, move || pick_color(mode, Picked::Rgb(hex.to_owned()))));
    }
    entries.push(pick("Custom color…", false, off, move || ui().prompt.set(Some(Prompt::Color(mode)))));
    submenu(label, entries)
}

/// The table menu's rows (`gE`, then the formatting submenus and the
/// advanced rows the web refuses), as the overflow lists them (`vE`).
fn table_entries(m: &Meta) -> Vec<Entry> {
    let cell = m.cell.clone().unwrap_or_default();
    let off = gate();
    let tracked = gate_tracked();
    let op = |label: &str, o: TableOp, why: Option<&str>| pick(label.to_owned(), false, why, move || run(Cmd::Table(o.clone())));
    let mut v = vec![
        op("Insert row above", TableOp::RowAbove, off),
        op("Insert row below", TableOp::RowBelow, off),
        op("Insert column left", TableOp::ColumnLeft, tracked),
        op("Insert column right", TableOp::ColumnRight, tracked),
        Entry::Separator,
        op("Merge cells", TableOp::Merge, tracked.or((!cell.multi).then_some("Select more than one cell."))),
        op("Split cell", TableOp::Split, tracked.or((!cell.can_split).then_some("This cell can't be split."))),
        Entry::Separator,
        op("Select entire table", TableOp::SelectTable, None),
        Entry::Separator,
        op("Delete row", TableOp::DeleteRow, off.or((cell.rows <= 1).then_some("A table needs at least one row."))),
        op("Delete column", TableOp::DeleteColumn, tracked.or((cell.cols <= 1).then_some("A table needs at least one column."))),
        op("Delete table", TableOp::DeleteTable, tracked),
        Entry::Separator,
        submenu("Borders", BORDERS.iter().map(|(v, _, l)| op(l, TableOp::Borders(v), tracked)).collect()),
        color_entries(Mode::Border, "Border Color", "Automatic", cell.border.clone(), tracked),
        submenu("Border width", WIDTHS.iter().map(|(s, l, _)| op(l, TableOp::BorderWidth(*s), tracked)).collect()),
        color_entries(Mode::Fill, "Cell Fill Color", "No Color", cell.fill.clone(), tracked),
        submenu("Vertical alignment", ["Top", "Middle", "Bottom"].iter().map(|l| pick(*l, false, Some(super::doc::format::UNSUPPORTED), || {})).collect()),
        submenu(
            "Table alignment",
            [("left", "Align table left"), ("center", "Align table center"), ("right", "Align table right")]
                .iter()
                .map(|(j, l)| {
                    let on = cell.justification.as_deref().unwrap_or("left") == *j;
                    pick(*l, on, tracked, move || run(Cmd::Table(TableOp::Justify(j))))
                })
                .collect(),
        ),
        Entry::Separator,
    ];
    for l in ["Toggle header row", "Distribute columns evenly", "Auto-fit to contents", "Toggle no-wrap"] {
        v.push(pick(l, false, Some(super::doc::format::UNSUPPORTED), || {}));
    }
    v.push(pick("Table properties...", false, tracked, dialogs::open_table_properties));
    v
}

/// The parts of the bar, in the web's order (`lL`).
fn parts() -> Vec<Part> {
    let u = ui();
    let m = meta;
    let toggle = move |icon: &'static str, label: &'static str, chord: Option<&'static str>, f: Fmt, on: fn(&Meta) -> bool| {
        let mut b = icon_button(icon, label).active(move || on(&m())).disabled(viewing).on_press(move || fmt(f));
        if let Some(c) = chord {
            b = b.shortcut(c);
        }
        b.into_element()
    };
    let toggle_row = move |icon: &'static str, label: &'static str, chord: Option<&'static str>, f: Fmt, on: fn(&Meta) -> bool| row(icon, label, chord, on(&m()), gate(), move || fmt(f));
    vec![
        part(
            Some("History"),
            || {
                vec![
                    icon_button("undo", "Undo").shortcut("Mod+Z").disabled(|| viewing() || !meta().can_undo).on_press(|| {
                        key("undo");
                        focus()
                    })
                    .into_element(),
                    icon_button("redo", "Redo").shortcut("Mod+Y").disabled(|| viewing() || !meta().can_redo).on_press(|| {
                        key("redo");
                        focus()
                    })
                    .into_element(),
                ]
            },
            || {
                let m = meta();
                let undo = (m.mode == EditMode::Viewing).then_some(super::doc::format::VIEWING);
                vec![
                    row("undo", "Undo", Some("Mod+Z"), false, undo.or((!m.can_undo).then_some("There is nothing to undo.")), || key("undo")),
                    row("redo", "Redo", Some("Mod+Y"), false, undo.or((!m.can_redo).then_some("There is nothing to redo.")), || key("redo")),
                ]
            },
        ),
        part(
            Some("Zoom"),
            move || vec![zoom_control(move || u.zoom.get(), set_zoom)],
            move || {
                let z = u.zoom.get_untracked();
                vec![submenu("Zoom", ZOOMS.iter().map(|v| pick(zoom_label(*v), (z - v).abs() < 0.001, None, move || set_zoom(*v))).collect())]
            },
        ),
        part(
            Some("Styles"),
            || {
                let list = computed(move || styles(&meta()));
                vec![dynamic(move || {
                    let styles = list.get();
                    let opts = styles.iter().map(|s| Opt { preview: style_preview(s), ..Opt::new(s.id.to_string(), s.name.to_string()) }).collect();
                    dropdown_select("Select paragraph style", opts)
                        .width(Width::Fixed(120.0))
                        .list(Some(260.0), 400.0)
                        .item_padding(12.0, 10.0)
                        .value(|| Some(meta().fmt.style.clone().into()))
                        .disabled(viewing)
                        .on_change(|id| run(Cmd::Style(id.to_owned())))
                        .into_element()
                })]
            },
            || {
                let m = meta();
                vec![submenu("Paragraph style", styles(&m).into_iter().map(|s| {
                    let id = s.id.to_string();
                    pick(s.name.to_string(), m.fmt.style == id, gate(), move || run(Cmd::Style(id.clone())))
                }).collect())]
            },
        ),
        part(
            Some("Font"),
            || {
                let fonts = computed(move || meta().fonts.clone());
                vec![
                    dynamic(move || {
                        let fonts = fonts.get();
                        let names: Vec<&str> = fonts.iter().map(String::as_str).collect();
                        font_family_select(&names)
                            .value(|| Some(meta().fmt.family.clone().into()).filter(|f: &SharedString| !f.is_empty()).or(Some("Arial".into())))
                            .disabled(viewing)
                            .on_change(|f| run(Cmd::FontFamily(f.to_owned())))
                            .into_element()
                    }),
                    font_size(|| Some(meta().fmt.size)).disabled(viewing).on_change(|s| run(Cmd::FontSize(s))).custom(|| ui().prompt.set(Some(Prompt::Size))).into_element(),
                ]
            },
            || {
                let m = meta();
                let fam = m.fmt.family.to_lowercase();
                let mut sizes: Vec<Entry> = FONT_SIZES.iter().map(|s| pick(s.to_string(), m.fmt.size == *s, gate(), move || run(Cmd::FontSize(*s)))).collect();
                sizes.push(pick("Custom size…", false, gate(), || ui().prompt.set(Some(Prompt::Size))));
                vec![
                    submenu(
                        "Font",
                        crate::office::ui::select::FONTS
                            .iter()
                            .map(|f| pick(f.name, f.name.to_lowercase() == fam, gate(), move || run(Cmd::FontFamily(f.name.to_owned()))))
                            .collect(),
                    ),
                    submenu("Font size", sizes),
                ]
            },
        ),
        part(
            Some("Text formatting"),
            move || {
                vec![
                    toggle("format_bold", "Bold", Some("Mod+B"), Fmt::Bold, |m| m.fmt.bold),
                    toggle("format_italic", "Italic", Some("Mod+I"), Fmt::Italic, |m| m.fmt.italic),
                    toggle("format_underlined", "Underline", Some("Mod+U"), Fmt::Underline, |m| m.fmt.underline),
                    toggle("strikethrough_s", "Strikethrough", None, Fmt::Strike, |m| m.fmt.strike),
                    color_button(Mode::Text).current(|| meta().fmt.color.clone()).disabled(viewing).on_pick(|c| pick_color(Mode::Text, c)).into_element(),
                    color_button(Mode::Highlight).current(|| meta().fmt.highlight.clone()).disabled(viewing).on_pick(|c| pick_color(Mode::Highlight, c)).into_element(),
                    icon_button("link", "Insert link").shortcut("Mod+K").disabled(viewing).on_press(dialogs::open_link).into_element(),
                ]
            },
            move || {
                let m = meta();
                vec![
                    toggle_row("format_bold", "Bold", Some("Mod+B"), Fmt::Bold, |m| m.fmt.bold),
                    toggle_row("format_italic", "Italic", Some("Mod+I"), Fmt::Italic, |m| m.fmt.italic),
                    toggle_row("format_underlined", "Underline", Some("Mod+U"), Fmt::Underline, |m| m.fmt.underline),
                    toggle_row("strikethrough_s", "Strikethrough", None, Fmt::Strike, |m| m.fmt.strike),
                    color_entries(Mode::Text, "Font Color", "Automatic", m.fmt.color.clone(), gate()),
                    color_entries(Mode::Highlight, "Text Highlight Color", "No Color", m.fmt.highlight.clone(), gate()),
                    row("link", "Insert link", Some("Mod+K"), false, gate(), dialogs::open_link),
                ]
            },
        ),
        part(
            Some("Script"),
            move || {
                vec![
                    toggle("superscript", "Superscript", Some("Mod+Shift+="), Fmt::Superscript, |m| m.fmt.superscript),
                    toggle("subscript", "Subscript", Some("Mod+="), Fmt::Subscript, |m| m.fmt.subscript),
                ]
            },
            move || {
                vec![
                    toggle_row("superscript", "Superscript", Some("Mod+Shift+="), Fmt::Superscript, |m| m.fmt.superscript),
                    toggle_row("subscript", "Subscript", Some("Mod+="), Fmt::Subscript, |m| m.fmt.subscript),
                ]
            },
        ),
        part(
            Some("Alignment"),
            || {
                let choices = ALIGNS.iter().map(|(v, i, l, s)| Choice { value: v, icon: i, label: (*l).into(), shortcut: Some(s) }).collect();
                vec![
                    icon_dropdown("Alignment", choices)
                        .titled()
                        .current(|| {
                            let a = meta().fmt.align.clone();
                            ALIGNS.iter().find(|x| x.0 == a).map(|x| x.0)
                        })
                        .disabled(viewing)
                        .on_select(|v| fmt(Fmt::Align(v)))
                        .into_element(),
                ]
            },
            || {
                let a = meta().fmt.align.clone();
                vec![submenu("Alignment", ALIGNS.iter().map(|(v, _, l, _)| pick(*l, a == *v, gate(), move || fmt(Fmt::Align(v)))).collect())]
            },
        ),
        part(
            Some("List formatting"),
            || {
                let no_outdent = || {
                    let m = meta();
                    viewing() || (m.fmt.list.is_none() && m.fmt.indent_left <= 0.0)
                };
                vec![
                    icon_button("format_list_bulleted", "Bullet List").active(|| meta().fmt.list.is_some_and(|l| l.0 == 1)).disabled(viewing).on_press(|| fmt(Fmt::Bullets)).into_element(),
                    icon_button("format_list_numbered", "Numbered List").active(|| meta().fmt.list.is_some_and(|l| l.0 != 1)).disabled(viewing).on_press(|| fmt(Fmt::Numbers)).into_element(),
                    icon_button("format_indent_decrease", "Decrease Indent").disabled(no_outdent).on_press(|| fmt(Fmt::Indent(false))).into_element(),
                    icon_button("format_indent_increase", "Increase Indent").disabled(viewing).on_press(|| fmt(Fmt::Indent(true))).into_element(),
                    line_spacing(),
                ]
            },
            || {
                let m = meta();
                let outdent = gate().or((m.fmt.list.is_none() && m.fmt.indent_left <= 0.0).then_some("This paragraph has no indentation to decrease."));
                let ls = m.fmt.line_spacing;
                vec![
                    row("format_list_bulleted", "Bullet List", None, m.fmt.list.is_some_and(|l| l.0 == 1), gate(), || fmt(Fmt::Bullets)),
                    row("format_list_numbered", "Numbered List", None, m.fmt.list.is_some_and(|l| l.0 != 1), gate(), || fmt(Fmt::Numbers)),
                    row("format_indent_decrease", "Decrease Indent", None, false, outdent, || fmt(Fmt::Indent(false))),
                    row("format_indent_increase", "Increase Indent", None, false, gate(), || fmt(Fmt::Indent(true))),
                    submenu("Line spacing", SPACINGS.iter().map(|(t, l)| pick(*l, ls == Some(f64::from(*t)), gate(), move || fmt(Fmt::LineSpacing(*t)))).collect()),
                ]
            },
        ),
        Part {
            label: Some("Image"),
            present: Rc::new(|| meta().fmt.image.is_some()),
            build: Rc::new(|| {
                let off = || gate_tracked().is_some();
                let wraps = WRAPS.iter().map(|(v, i, l)| Choice { value: v, icon: i, label: (*l).into(), shortcut: None }).collect();
                let turns = TRANSFORMS.iter().map(|(v, i, l)| Choice { value: v, icon: i, label: (*l).into(), shortcut: None }).collect();
                vec![
                    icon_dropdown("Text wrapping", wraps)
                        .labels()
                        .current(|| {
                            let w = meta().fmt.image.clone().unwrap_or_default();
                            WRAPS.iter().find(|x| x.0 == w).map(|x| x.0)
                        })
                        .disabled(off)
                        .on_select(|v| run(Cmd::Picture(Picture::Wrap(v))))
                        .into_element(),
                    icon_dropdown("Transform", turns).disabled(off).on_select(|v| run(Cmd::Picture(Picture::Transform(v)))).into_element(),
                    icon_button("tune", "Image properties").disabled(off).on_press(dialogs::open_image_properties).into_element(),
                ]
            }),
            entries: Rc::new(|| {
                let off = gate_tracked();
                let w = meta().fmt.image.clone().unwrap_or_default();
                vec![
                    submenu(
                        "Text wrapping",
                        [("inline", "In Line with Text"), ("wrapRight", "Square Left"), ("wrapLeft", "Square Right"), ("topAndBottom", "Top and Bottom"), ("behind", "Behind Text"), ("inFront", "In Front of Text")]
                            .iter()
                            .map(|(v, l)| pick(*l, w == *v, off, move || run(Cmd::Picture(Picture::Wrap(v)))))
                            .collect(),
                    ),
                    submenu("Transform", TRANSFORMS.iter().map(|(v, _, l)| pick(*l, false, off, move || run(Cmd::Picture(Picture::Transform(v))))).collect()),
                    row("tune", "Image properties", None, false, off, dialogs::open_image_properties),
                ]
            }),
        },
        Part {
            label: Some("Table"),
            present: Rc::new(|| meta().cell.is_some()),
            build: Rc::new(table_controls),
            entries: Rc::new(|| vec![submenu("Table", table_entries(&meta()))]),
        },
        part(
            None,
            || vec![icon_button("format_clear", "Clear formatting").disabled(viewing).on_press(|| fmt(Fmt::Clear)).into_element()],
            || vec![row("format_clear", "Clear formatting", None, false, gate(), || fmt(Fmt::Clear))],
        ),
        sep(),
        part(
            None,
            move || vec![icon_button("comment", "Comments & Changes").active(move || u.review.get()).on_press(move || u.review.update(|r| *r = !*r)).into_element()],
            move || vec![row("comment", "Comments & Changes", None, u.review.get_untracked(), None, move || u.review.update(|r| *r = !*r))],
        ),
        sep(),
        part(None, || vec![editing_mode()], || {
            let mode = meta().mode;
            vec![submenu("Editing mode", MODES.iter().map(|(v, _, l, _)| pick(*l, mode == *v, None, move || set_mode(*v))).collect())]
        }),
    ]
}

pub(crate) fn set_mode(m: EditMode) {
    run(Cmd::Mode(m));
}

/// The line spacing select (DR `Cy`): its icon, the presets, then the
/// "Paragraph spacing" label.
fn line_spacing() -> AnyElement {
    let mut opts: Vec<Opt> = SPACINGS.iter().map(|(t, l)| Opt::new(t.to_string(), *l)).collect();
    opts.push(Opt { heading: true, ..Opt::new("paragraph-spacing", "Paragraph spacing") });
    dropdown_select("Line spacing", opts)
        .icon("format_line_spacing")
        .value(|| {
            let ls = meta().fmt.line_spacing;
            let t = SPACINGS.iter().find(|(t, _)| ls == Some(f64::from(*t))).map_or(240, |x| x.0);
            Some(t.to_string().into())
        })
        .disabled(viewing)
        .on_change(|v| {
            if let Ok(t) = v.parse() {
                fmt(Fmt::LineSpacing(t))
            }
        })
        .into_element()
}

/// The Table group (DR `kE`): borders, border colour, border width, cell
/// fill and the table menu.
fn table_controls() -> Vec<AnyElement> {
    let off = || gate_tracked().is_some();
    let borders = BORDERS.iter().map(|(v, i, l)| Choice { value: v, icon: i, label: (*l).into(), shortcut: None }).collect();
    vec![
        icon_dropdown("Borders", borders).columns(4).disabled(off).on_select(|v| run(Cmd::Table(TableOp::Borders(v)))).into_element(),
        color_button(Mode::Border).current(|| meta().cell.clone().and_then(|c| c.border)).disabled(off).on_pick(|c| pick_color(Mode::Border, c)).into_element(),
        border_width(),
        color_button(Mode::Fill).current(|| meta().cell.clone().and_then(|c| c.fill)).disabled(off).on_pick(|c| pick_color(Mode::Fill, c)).into_element(),
        table_menu(),
    ]
}

/// A trigger with an icon and a ▾ (`text-muted-foreground`, `bg-muted`
/// while open), as the table group's dropdowns have.
fn trigger(icon_name: &'static str, label: &'static str, open: Signal<bool>, caret: bool, off: impl Fn() -> bool + 'static) -> View {
    let hovered = signal(false);
    let off = Rc::new(off);
    let (o1, o2) = (off.clone(), off);
    let ink = move || if hovered.get() { p::TW_FOREGROUND } else { p::TW_MUTED_FOREGROUND };
    view()
        .flex_row()
        .w(px(BUTTON))
        .h(px(BUTTON))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(px(BUTTON_RADIUS))
        .label(label)
        .bg(move || {
            if open.get() {
                p::TW_MUTED
            } else if hovered.get() {
                p::TW_MUTED_80
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .opacity(move || if o1() { DISABLED_OPACITY } else { 1.0 })
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_press(move || {
            if !o2() {
                open.update(|o| *o = !*o)
            }
        })
        .tooltip(tooltip_of(label))
        .child(icon(icon_name, 20.0, ink))
        .when(caret, |v| v.child(view().ml(px(-4.0)).child(icon("arrow_drop_down", 14.0, ink))))
}

/// The border width dropdown (DR `Iy`): each width as a 50-wide line of
/// its thickness and its label.
fn border_width() -> AnyElement {
    let open = signal(false);
    let anchor = element_ref();
    let panel = Panel { radius: 8.0, pad_x: 0.0, pad_y: 4.0, shadow: (4.0, 16.0), ..Panel::MENU };
    view()
        .child(trigger("line_weight", "Border width", open, true, || gate_tracked().is_some()).element_ref(&anchor))
        .child(dropdown(anchor, open, "Border width choices", panel, move || {
            view()
                .min_w(px(120.0))
                .children(WIDTHS.iter().map(|(size, label, thick)| {
                    let hovered = signal(false);
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(10.0))
                        .px(px(12.0))
                        .py(px(6.0))
                        .w_full()
                        .label(*label)
                        .bg(move || if hovered.get() { p::BG_HOVER } else { AdaptiveColor::TRANSPARENT })
                        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
                        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
                        .on_press(move || {
                            open.set(false);
                            run(Cmd::Table(TableOp::BorderWidth(*size)))
                        })
                        .child(view().w(px(50.0)).h(px(thick.max(1.0))).rounded(px(if *thick > 2.0 { 1.0 } else { 0.0 })).bg(hsla(0.0, 0.0, 0.0, 1.0)))
                        .child(text(*label).text_size(px(12.0)).text_color(p::TEXT))
                        .into_element()
                }))
                .into_element()
        }))
        .into_element()
}

/// A row of three square buttons in the table menu (vertical alignment,
/// table alignment: `32×28`, 1px border, radius 4, 4 apart, `4px 14px`).
/// A square button of a row: icon, title, on, why it's refused, its command.
type Square = (&'static str, &'static str, bool, Option<&'static str>, Rc<dyn Fn()>);

fn button_row(buttons: Vec<Square>) -> AnyElement {
    view()
        .flex_row()
        .gap(px(4.0))
        .px(px(14.0))
        .py(px(4.0))
        .children(buttons.into_iter().map(|(name, title, on, refused, act)| {
            let hovered = signal(false);
            view()
                .w(px(32.0))
                .h(px(28.0))
                .items_center()
                .justify_center()
                .border_1()
                .rounded(px(4.0))
                .border_color(if on { p::PRIMARY } else { p::BORDER })
                .label(title)
                .bg(move || if on { p::PRIMARY_LIGHT } else if hovered.get() { p::BG_HOVER } else { AdaptiveColor::TRANSPARENT })
                .opacity(if refused.is_some() { 0.5f32 } else { 1.0 })
                .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
                .on_press(move || {
                    if refused.is_none() {
                        act()
                    }
                })
                .tooltip(crate::office::ui::button::plain_tip(refused.unwrap_or(title)))
                .child(icon(name, 16.0, move || if on { p::PRIMARY } else { p::TEXT }))
                .into_element()
        }))
        .into_element()
}

/// The table menu (DR `My`, `table.moreOptions`): rows, separators, the
/// two button rows and the advanced rows the web refuses.
fn table_menu() -> AnyElement {
    let open = signal(false);
    let anchor = element_ref();
    let u = ui();
    effect(move || {
        if open.get() {
            u.bar_open.set(true)
        } else {
            untracked(|| u.bar_open.set(false))
        }
    });
    view()
        .child(trigger("more_vert", "More table options", open, false, || false).element_ref(&anchor))
        .child(menu_popup(anchor, open, "More table options menu", Look::TABLE, 4.0, move || {
            let m = meta();
            let cell = m.cell.clone().unwrap_or_default();
            let (off, tracked) = (gate(), gate_tracked());
            let it = |icon: &'static str, label: &'static str, o: TableOp, why: Option<&str>, danger: bool| {
                let mut i: Item = item(Some(icon), label, move || run(Cmd::Table(o.clone())));
                if let Some(w) = why {
                    i = i.disabled(Some(w));
                }
                if danger {
                    i = i.danger();
                }
                Entry::Item(i)
            };
            let refused = |icon: &'static str, label: &'static str| Entry::Item(item(Some(icon), label, || {}).disabled(Some(super::doc::format::UNSUPPORTED)));
            let just = cell.justification.clone().unwrap_or_else(|| "left".to_owned());
            let vertical: super::super::ui::menu::Content = Rc::new(|_close: Rc<dyn Fn()>| {
                let none: Rc<dyn Fn()> = Rc::new(|| {});
                button_row(vec![
                    ("vertical_align_top", "Top", false, Some(super::doc::format::UNSUPPORTED), none.clone()),
                    ("vertical_align_center", "Middle", false, Some(super::doc::format::UNSUPPORTED), none.clone()),
                    ("vertical_align_bottom", "Bottom", false, Some(super::doc::format::UNSUPPORTED), none),
                ])
            });
            let aligns: super::super::ui::menu::Content = Rc::new(move |close: Rc<dyn Fn()>| {
                let b = |j: &'static str, name: &'static str, title: &'static str| {
                    let close = close.clone();
                    let act: Rc<dyn Fn()> = Rc::new(move || {
                        close();
                        run(Cmd::Table(TableOp::Justify(j)))
                    });
                    (name, title, just == j, tracked, act)
                };
                button_row(vec![b("left", "format_align_left", "Align table left"), b("center", "format_align_center", "Align table center"), b("right", "format_align_right", "Align table right")])
            });
            vec![
                it("add", "Insert row above", TableOp::RowAbove, off, false),
                it("add", "Insert row below", TableOp::RowBelow, off, false),
                it("add", "Insert column left", TableOp::ColumnLeft, tracked, false),
                it("add", "Insert column right", TableOp::ColumnRight, tracked, false),
                Entry::Separator,
                it("call_merge", "Merge cells", TableOp::Merge, tracked.or((!cell.multi).then_some("Select more than one cell.")), false),
                it("call_split", "Split cell", TableOp::Split, tracked.or((!cell.can_split).then_some("This cell can't be split.")), false),
                Entry::Separator,
                it("select_all", "Select entire table", TableOp::SelectTable, None, false),
                Entry::Separator,
                it("delete", "Delete row", TableOp::DeleteRow, off.or((cell.rows <= 1).then_some("A table needs at least one row.")), true),
                it("delete", "Delete column", TableOp::DeleteColumn, tracked.or((cell.cols <= 1).then_some("A table needs at least one column.")), true),
                it("delete", "Delete table", TableOp::DeleteTable, tracked, true),
                Entry::Separator,
                Entry::Heading("Vertical alignment".into()),
                Entry::Custom(vertical),
                Entry::Separator,
                Entry::Heading("Table alignment".into()),
                Entry::Custom(aligns),
                Entry::Separator,
                refused("table_rows", "Toggle header row"),
                refused("view_column", "Distribute columns evenly"),
                refused("fit_width", "Auto-fit to contents"),
                refused("wrap_text", "Toggle no-wrap"),
                Entry::Separator,
                Entry::Item({
                    let mut i = item(Some("settings"), "Table properties...", dialogs::open_table_properties);
                    if let Some(w) = tracked {
                        i = i.disabled(Some(w));
                    }
                    i
                }),
            ]
        }))
        .into_element()
}

/// The editing mode dropdown (DR `$h`): the mode's icon, its label when
/// the window is wider than 1400, a ▾; its menu 220 wide under the
/// trigger's right edge.
fn editing_mode() -> AnyElement {
    let open = signal(false);
    let anchor = element_ref();
    let u = ui();
    let mode = move || meta().mode;
    let current = move || MODES.iter().find(|x| x.0 == mode()).copied().unwrap_or(MODES[0]);
    effect(move || {
        if open.get() {
            u.bar_open.set(true)
        } else {
            untracked(|| u.bar_open.set(false))
        }
    });
    let wide = move || u.area.get().0 > 1400.0;
    let trigger = view()
        .flex_row()
        .items_center()
        .gap(move || px(if wide() { 4.0 } else { 0.0 }))
        .pl(px(4.0))
        .pr(move || px(if wide() { 6.0 } else { 4.0 }))
        .py(px(2.0))
        .h(px(28.0))
        .rounded(px(4.0))
        .element_ref(&anchor)
        .label(move || current().2)
        .bg(move || if open.get() { p::BG_HOVER } else { AdaptiveColor::TRANSPARENT })
        .on_press(move || open.update(|o| *o = !*o))
        .child(dynamic(move || icon(current().1, 18.0, || p::TEXT)))
        .child(when(wide, move || text(move || current().2).text_size(px(13.0)).text_color(p::TEXT)))
        .child(icon("arrow_drop_down", 16.0, || p::TEXT));
    let menu = when(
        move || open.get(),
        move || {
            overlay()
                .anchor(&anchor)
                .placement(Placement::Bottom)
                .align(Align::End)
                .offset(px(2.0))
                .label("Editing mode")
                .on_press_outside(move |_: &Point<Points>| open.set(false))
                .on_cancel(move || open.set(false))
                .child(Panel { pad_y: 4.0, radius: 8.0, ..Panel::MENU }.dress(view()).min_w(px(220.0)).children(MODES.iter().map(|(v, name, label, desc)| {
                    let hovered = signal(false);
                    let v = *v;
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(10.0))
                        .px(px(12.0))
                        .py(px(8.0))
                        .w_full()
                        .label(*label)
                        .bg(move || if hovered.get() { p::BG_HOVER } else { AdaptiveColor::TRANSPARENT })
                        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
                        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
                        .on_press(move || {
                            open.set(false);
                            set_mode(v)
                        })
                        .child(icon(name, 20.0, || p::TEXT))
                        .child(
                            view()
                                .child(text(*label).text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(p::TEXT))
                                .child(text(*desc).text_size(px(11.0)).text_color(p::TEXT_MUTED)),
                        )
                        .child(view().flex_1())
                        .child(when(move || mode() == v, || icon("check", 18.0, || p::PRIMARY)))
                        .into_element()
                })))
        },
    );
    view().child(trigger).child(menu).into_element()
}

/// The More menu's trigger (`more_vert` in `h-7 w-7`).
const MORE_W: f32 = BUTTON;

/// Where the parts overflow (`ub`): all fit, or from the first that
/// doesn't fit beside the More button on, every part goes.
pub(crate) fn first_hidden(widths: &[f32], room: f32) -> usize {
    let total: f32 = widths.iter().sum();
    if total <= room + 0.5 {
        return widths.len();
    }
    let room = room - MORE_W;
    let mut used = 0.0;
    for (i, w) in widths.iter().enumerate() {
        if used + w <= room {
            used += w;
        } else {
            return i;
        }
    }
    widths.len()
}

/// The formatting bar (`bb`: `flex items-center px-2 py-1 bg-muted
/// rounded-full min-h-[36px] mx-2 mb-1`).
pub(crate) fn toolbar() -> impl IntoElement {
    let u = ui();
    let parts = Rc::new(parts());
    let n = parts.len();
    let widths: Vec<Signal<f32>> = (0..n).map(|_| signal(0.0)).collect();
    let room = signal(0.0f32);
    let ws = widths.clone();
    let cut = computed(move || first_hidden(&ws.iter().map(|w| w.get()).collect::<Vec<_>>(), room.get()));
    let more_open = signal(false);
    let more = element_ref();
    effect(move || {
        if more_open.get() {
            u.bar_open.set(true)
        } else {
            untracked(|| u.bar_open.set(false))
        }
    });
    let first_group = parts.iter().position(|p| p.label.is_some()).unwrap_or(0);
    let items = view()
        .flex_row()
        .items_center()
        .flex_1()
        .min_w(px(0.0))
        .overflow_hidden()
        .children(parts.iter().enumerate().map(|(i, part)| {
            let w = widths[i];
            let present = part.present.clone();
            let shown = Rc::new(move || present() && i < cut.get());
            let build = part.build.clone();
            let mut v = view().flex_row().items_center().flex_shrink_0().on_layout(move |b: &Bounds<Points>| {
                if i < cut.get_untracked() {
                    w.set(b.size.width.0)
                }
            });
            if let Some(label) = part.label {
                // `Cr`: `gap-px px-1.5 border-r border/50`, the first flush;
                // a group not shown takes no room
                let (s1, s2, s3) = (shown.clone(), shown.clone(), shown.clone());
                v = v
                    .gap(px(GROUP_GAP))
                    .pl(move || px(if i == first_group || !s1() { 0.0 } else { GROUP_PX }))
                    .pr(move || px(if s2() { GROUP_PX } else { 0.0 }))
                    .border_r(move || px(if s3() { 1.0 } else { 0.0 }))
                    .border_color(p::TW_BORDER_50)
                    .label(label);
            }
            v.child(when(move || shown(), move || build())).into_element()
        }));
    let more_entries = {
        let parts = parts.clone();
        move || {
            let from = cut.get_untracked();
            let mut out = Vec::new();
            for part in parts.iter().skip(from).filter(|p| (p.present)()) {
                let entries = (part.entries)();
                if entries.is_empty() {
                    continue;
                }
                if !out.is_empty() {
                    out.push(Entry::Separator);
                }
                if let Some(l) = part.label {
                    out.push(Entry::Heading(l.into()));
                }
                out.extend(entries);
            }
            out
        }
    };
    let hidden_any = {
        let parts = parts.clone();
        move || {
            let c = cut.get();
            parts.iter().skip(c).any(|p| (p.present)())
        }
    };
    view()
        .flex_row()
        .items_center()
        .px(px(8.0))
        .py(px(4.0))
        .mx(px(8.0))
        .mb(px(4.0))
        .min_h(px(36.0))
        .rounded(px(18.0))
        .bg(p::TW_MUTED)
        .label("Formatting toolbar")
        .on_layout(move |b: &Bounds<Points>| room.set(b.size.width.0 - 16.0))
        .child(items)
        .child(when(hidden_any, move || {
            let entries = more_entries.clone();
            view()
                .flex_shrink_0()
                .child(trigger("more_vert", "More actions", more_open, false, || false).element_ref(&more))
                .child(menu_popup(more, more_open, "More actions menu", Look::BAR, 4.0, entries))
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_overflow_from_the_first_that_doesnt_fit() {
        assert_eq!(first_hidden(&[50.0, 50.0, 50.0], 150.0), 3);
        // 120 left beside the More button: two fit, the third goes
        assert_eq!(first_hidden(&[50.0, 50.0, 50.0], 148.0), 2);
        // once one goes every part after it goes, a small one too
        assert_eq!(first_hidden(&[50.0, 80.0, 10.0], 120.0), 1);
    }
}
