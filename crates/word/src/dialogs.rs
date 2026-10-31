//! The dialogs the menus and the formatting bar open, on the kit's modal
//! shell with the web's fields and texts: Page Setup (`PageSetupDialog`),
//! Watermark (`WatermarkDialog`), Hyperlink (`HyperlinkDialog`, the
//! roomier shell), Table Properties (`TablePropertiesDialog`) and the
//! formatting bar's one-field prompt (`fb`: a custom size or colour).
//!
//! Each holds its fields in signals made with the view; opening one fills
//! them from what the worker last said (the page of the last section, the
//! watermark), as the web's dialogs do on `isOpen`.

use std::cell::Cell;

use nsh_core::prelude::*;
use serde_json::{Value, json};

use super::WordUi;
use super::chrome::{data_url, image_size, pick_picture};
use super::doc::{Cmd, EditMode, Link, PageSetup, TableOp};
use super::view::{focus, send};
use buwiz_office_kit::ui::color::{Mode, Picked, hex};
use buwiz_office_kit::ui::field::{checkbox, form_row, number_field, radio, section_label, select_field, text_field};
use buwiz_office_kit::ui::modal::modal;
use buwiz_office_kit::ui::palette as p;

fn ui() -> &'static WordUi {
    crate::FORMAT.ui()
}

/// Twips in an inch (`TWIPS_PER_INCH`).
const TWIPS: f64 = 1440.0;

/// Page Setup's sizes (`E`): name, width and height in twips.
pub const SIZES: [(&str, f64, f64); 7] = [
    ("Letter (8.5\" \u{d7} 11\")", 12240.0, 15840.0),
    ("A4 (8.27\" \u{d7} 11.69\")", 11906.0, 16838.0),
    ("Legal (8.5\" \u{d7} 14\")", 12240.0, 20160.0),
    ("A3 (11.69\" \u{d7} 16.54\")", 16838.0, 23811.0),
    ("A5 (5.83\" \u{d7} 8.27\")", 8391.0, 11906.0),
    ("B5 (6.93\" \u{d7} 9.84\")", 9979.0, 14175.0),
    ("Executive (7.25\" \u{d7} 10.5\")", 10440.0, 15120.0),
];

/// The size a page of `w` × `h` twips is (`J`: either way round, within 20).
pub fn size_of(w: f64, h: f64) -> Option<usize> {
    let (a, b) = (w.min(h), w.max(h));
    SIZES.iter().position(|(_, sw, sh)| (sw - a).abs() < 20.0 && (sh - b).abs() < 20.0)
}

/// Twips as the margin fields show them (`f`): inches to two places.
pub fn inches(twips: f64) -> f64 {
    (twips / TWIPS * 100.0).round() / 100.0
}

/// The watermark's text presets (`DEFAULT_WATERMARK_PRESETS`).
pub const PRESETS: [&str; 6] = ["CONFIDENTIAL", "DRAFT", "DO NOT COPY", "SAMPLE", "URGENT", "ASAP"];
/// Its fonts (`te`).
pub const FONTS: [&str; 6] = ["Calibri", "Arial", "Times New Roman", "Georgia", "Verdana", "Courier New"];

/// A picture watermark's size (`pictureWatermarkDisplayEmu`): its longer
/// side 3954780 EMU.
pub fn picture_emu(w: f64, h: f64) -> Option<(f64, f64)> {
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    let r = w / h;
    Some(if r >= 1.0 { (3954780.0, (3954780.0 / r).round()) } else { ((3954780.0 * r).round(), 3954780.0) })
}

#[derive(Clone, Copy)]
struct Setup {
    open: Signal<bool>,
    w: Signal<f64>,
    h: Signal<f64>,
    landscape: Signal<bool>,
    /// The margins in inches, as their fields hold them.
    top: Signal<f64>,
    bottom: Signal<f64>,
    left: Signal<f64>,
    right: Signal<f64>,
}

#[derive(Clone, Copy)]
struct Mark {
    open: Signal<bool>,
    /// "none", "picture" or "text".
    mode: Signal<&'static str>,
    text: Signal<String>,
    font: Signal<String>,
    auto: Signal<bool>,
    size: Signal<f64>,
    color: Signal<String>,
    diagonal: Signal<bool>,
    semi: Signal<bool>,
    picture: Signal<Option<String>>,
    emu: Signal<Option<(f64, f64)>>,
    /// Percent.
    scale: Signal<f64>,
    washout: Signal<bool>,
}

thread_local! {
    static SETUP: Cell<Option<Setup>> = const { Cell::new(None) };
    static MARK: Cell<Option<Mark>> = const { Cell::new(None) };
}

/// File › Page setup.
pub fn open_page_setup() {
    let Some(s) = SETUP.with(Cell::get) else { return };
    let cur = ui().meta.with_untracked(|m| m.setup.clone());
    s.w.set(cur.width);
    s.h.set(cur.height);
    s.landscape.set(cur.orientation == "landscape");
    for (f, v) in [(s.top, cur.top), (s.bottom, cur.bottom), (s.left, cur.left), (s.right, cur.right)] {
        f.set(inches(v));
    }
    s.open.set(true);
}

/// Insert › Watermark.
pub fn open_watermark() {
    let Some(m) = MARK.with(Cell::get) else { return };
    let cur = ui().meta.with_untracked(|m| m.watermark.clone());
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    match cur.as_ref().and_then(|c| s(c, "kind")).as_deref() {
        Some("text") => {
            let c = cur.as_ref().expect("a watermark");
            m.mode.set("text");
            m.text.set(s(c, "text").unwrap_or_default());
            m.font.set(s(c, "font").filter(|f| !f.is_empty()).unwrap_or_else(|| "Calibri".into()));
            let size = c.get("fontSize").and_then(Value::as_f64);
            m.auto.set(size.is_none());
            if let Some(z) = size {
                m.size.set(z);
            }
            m.color.set(s(c, "color").filter(|f| !f.is_empty()).unwrap_or_else(|| "#C0C0C0".into()));
            m.diagonal.set(s(c, "layout").as_deref() != Some("horizontal"));
            m.semi.set(c.get("semitransparent").and_then(Value::as_bool).unwrap_or(false));
        }
        Some("picture") => {
            let c = cur.as_ref().expect("a watermark");
            m.mode.set("picture");
            m.picture.set(s(c, "dataUrl"));
            m.emu.set(c.get("widthEmu").and_then(Value::as_f64).zip(c.get("heightEmu").and_then(Value::as_f64)));
            m.scale.set((c.get("scale").and_then(Value::as_f64).filter(|v| *v != 0.0).unwrap_or(1.0) * 100.0).round());
            m.washout.set(c.get("washout").and_then(Value::as_bool).unwrap_or(false));
        }
        _ => m.mode.set("none"),
    }
    m.open.set(true);
}

/// The dialogs, shown while open.
pub fn dialogs() -> impl IntoElement {
    view().child(page_setup()).child(watermark()).child(hyperlink()).child(table_properties()).child(prompt())
}

/// What the formatting bar's prompt asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Prompt {
    /// Custom size… (`commands.customFontSize`).
    Size,
    /// Custom color… for a colour picker.
    Color(Mode),
}

#[derive(Clone, Copy)]
struct LinkForm {
    url: Signal<String>,
    text: Signal<String>,
    tooltip: Signal<String>,
    editing: Signal<bool>,
    error: Signal<String>,
    /// The URL field was left once: its error shows.
    touched: Signal<bool>,
}

#[derive(Clone, Copy)]
struct TableForm {
    open: Signal<bool>,
    kind: Signal<String>,
    width: Signal<f64>,
    align: Signal<String>,
}

thread_local! {
    static LINK: Cell<Option<LinkForm>> = const { Cell::new(None) };
    static TABLE: Cell<Option<TableForm>> = const { Cell::new(None) };
}

/// Whether a link address reads as one (`G`): `mailto:`/`tel:` with
/// something after, `ftp://`, or a web address with a host (`https://`
/// put before one without a scheme).
pub fn valid_url(t: &str) -> bool {
    let e = t.trim();
    if e.is_empty() {
        return false;
    }
    if e.starts_with("mailto:") || e.starts_with("tel:") {
        return e.len() > 7;
    }
    if e.starts_with("ftp://") {
        return e.len() > 6;
    }
    let rest = e.strip_prefix("https://").or_else(|| e.strip_prefix("http://")).unwrap_or(e);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let (host, port) = match host.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => (h, Some(p)),
        _ => (host, None),
    };
    !host.is_empty() && !host.chars().any(|c| c.is_whitespace() || "<>\"^`{|}\\%".contains(c)) && port.is_none_or(|p| p.chars().all(|c| c.is_ascii_digit()))
}

/// A link address as it is set (`M`): a scheme put before one without.
pub fn normalize_url(t: &str) -> String {
    let e = t.trim();
    if ["mailto:", "tel:", "ftp://", "http://", "https://"].iter().any(|s| e.starts_with(s)) { e.to_owned() } else { format!("https://{e}") }
}

/// ⌘K and the bar's link button (`insertLink`): Edit Hyperlink for the
/// link at the caret, else Insert Hyperlink with the selection's text.
pub fn open_link() {
    let Some(f) = LINK.with(Cell::get) else { return };
    let m = ui().meta.get_untracked();
    if m.mode == EditMode::Viewing {
        return;
    }
    match &m.fmt.link {
        Some((href, tip)) => {
            f.url.set(href.clone());
            f.text.set(if m.fmt.text.is_empty() { m.fmt.link_text.clone() } else { m.fmt.text.clone() });
            f.tooltip.set(tip.clone().unwrap_or_default());
            f.editing.set(true);
        }
        None => {
            f.url.set(String::new());
            f.text.set(m.fmt.text.clone());
            f.tooltip.set(String::new());
            f.editing.set(false);
        }
    }
    f.error.set(String::new());
    f.touched.set(false);
    ui().link_open.set(true);
}

/// A field of the Hyperlink dialog (`E`, `b`): its 14px label, a
/// `padding 10px 12px` input (red while its error shows), a hint or the
/// error under it.
fn link_field(label: &'static str, placeholder: &'static str, value: Signal<String>, below: impl Fn() -> (String, bool) + 'static, on_blur: impl Fn() + 'static, on_edit: impl Fn() + 'static) -> AnyElement {
    let r = element_ref();
    buwiz_office_kit::ui::modal::join_trap(&r);
    let below = std::rc::Rc::new(below);
    let (b1, b2, b3) = (below.clone(), below.clone(), below);
    view()
        .mb(px(16.0))
        .child(text(label).mb(px(6.0)).text_size(px(14.0)).font_weight(FontWeight::MEDIUM).text_color(p::TEXT))
        .child(
            nsh_base::input()
                .element_ref(&r)
                .label(label)
                .placeholder(placeholder)
                .placeholder_color(|| color(p::TEXT_PLACEHOLDER))
                .value(move || value.get())
                .on_change(move |v: &str| {
                    value.set(v.to_owned());
                    on_edit()
                })
                .on_blur(on_blur)
                .w_full()
                .px(px(12.0))
                .py(px(10.0))
                .border_1()
                .border_color(move || if b1().1 { p::ERROR } else { p::BORDER_INPUT })
                .rounded(px(4.0))
                .text_size(px(14.0))
                .text_color(p::TEXT),
        )
        .child(when(move || !b2().0.is_empty(), move || {
            let b3 = b3.clone();
            let b4 = b3.clone();
            text(move || b3().0).mt(px(4.0)).text_size(px(12.0)).text_color(move || if b4().1 { p::ERROR } else { p::TEXT_MUTED })
        }))
        .into_element()
}

fn hyperlink() -> AnyElement {
    let u = ui();
    let f = LinkForm { url: signal(String::new()), text: signal(String::new()), tooltip: signal(String::new()), editing: signal(false), error: signal(String::new()), touched: signal(false) };
    LINK.with(|l| l.set(Some(f)));
    let validate = move || {
        let url = f.url.get_untracked();
        f.error.set(if !url.trim().is_empty() && !valid_url(&url) { "Please enter a valid URL".to_owned() } else { String::new() });
    };
    let submit = move || {
        let url = f.url.get_untracked();
        if url.trim().is_empty() {
            f.error.set("URL is required".to_owned());
            f.touched.set(true);
            return;
        }
        if !valid_url(&url) {
            f.error.set("Please enter a valid URL".to_owned());
            f.touched.set(true);
            return;
        }
        let opt = |s: String| Some(s.trim().to_owned()).filter(|t| !t.is_empty());
        send(Cmd::Link(Link { href: normalize_url(&url), text: opt(f.text.get_untracked()), tooltip: opt(f.tooltip.get_untracked()), edit: f.editing.get_untracked() }));
        u.link_open.set(false);
        focus();
    };
    let ready = move || !f.url.get().trim().is_empty() && f.error.get().is_empty();
    let title = move || if f.editing.get() { "Edit Hyperlink" } else { "Insert Hyperlink" };
    dynamic(move || {
        modal(u.link_open, title())
            .roomy()
            .width(400.0, 500.0)
            .gap(0.0)
            .body(move || {
                view()
                    .child(link_field(
                        "URL",
                        "https://example.com",
                        f.url,
                        move || {
                            let e = f.error.get();
                            if !e.is_empty() && f.touched.get() {
                                (e, true)
                            } else if e.is_empty() {
                                ("Enter a web address, email (mailto:), or phone (tel:)".to_owned(), false)
                            } else {
                                (String::new(), false)
                            }
                        },
                        move || {
                            f.touched.set(true);
                            validate()
                        },
                        move || {
                            if f.touched.get_untracked() {
                                f.error.set(String::new())
                            }
                        },
                    ))
                    .child(link_field("Display Text", "Text to display (optional)", f.text, || ("Leave empty to use the selected text".to_owned(), false), || {}, || {}))
                    .child(link_field("Tooltip (optional)", "Text shown on hover", f.tooltip, || (String::new(), false), || {}, || {}))
                    .into_element()
            })
            .danger_when("Remove Link", move || f.editing.get(), move || {
                send(Cmd::Unlink);
                u.link_open.set(false);
                focus();
            })
            .action("Cancel", false, move || {
                u.link_open.set(false);
                focus();
            })
            .action_when(if f.editing.get_untracked() { "Update" } else { "Insert" }, true, ready, submit)
            .on_confirm(submit)
            .on_cancel(focus)
            .into_element()
    })
}

/// The table menu's "Table properties..." (`openTableProperties`).
pub fn open_table_properties() {
    let Some(t) = TABLE.with(Cell::get) else { return };
    let cell = ui().meta.with_untracked(|m| m.cell.clone()).unwrap_or_default();
    t.kind.set(cell.width_type.clone().unwrap_or_else(|| "auto".to_owned()));
    t.width.set(cell.width.unwrap_or(0.0));
    t.align.set(cell.justification.clone().unwrap_or_else(|| "left".to_owned()));
    t.open.set(true);
}

fn table_properties() -> AnyElement {
    let u = ui();
    let t = TableForm { open: u.table_open, kind: signal("auto".to_owned()), width: signal(0.0), align: signal("left".to_owned()) };
    TABLE.with(|c| c.set(Some(t)));
    let apply = move || {
        let kind = t.kind.get_untracked();
        let width = (kind != "auto").then(|| t.width.get_untracked().round());
        send(Cmd::Table(TableOp::Properties { width, width_type: kind, justification: t.align.get_untracked() }));
        t.open.set(false);
        focus();
    };
    let pair = |v: &[(&str, &str)]| v.iter().map(|(a, b)| ((*a).to_owned(), (*b).to_owned())).collect::<Vec<_>>();
    modal(t.open, "Table Properties")
        .width(360.0, 440.0)
        .gap(12.0)
        .body(move || {
            view()
                .gap(px(12.0))
                .child(form_row("Width type", select_field("Width type", pair(&[("auto", "Auto"), ("dxa", "Fixed (twips)"), ("pct", "Percentage")]), move || t.kind.get(), move |v| t.kind.set(v.to_owned()))))
                .child(when(move || t.kind.get() != "auto", move || {
                    form_row(
                        "Width",
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(view().flex_1().child(number_field("Width", t.width, 0.0, 1e7)))
                            .child(text(move || if t.kind.get() == "pct" { "(50ths of %)" } else { "tw" }).text_size(px(11.0)).text_color(p::TEXT_MUTED)),
                    )
                }))
                .child(form_row("Alignment", select_field("Alignment", pair(&[("left", "Left"), ("center", "Center"), ("right", "Right")]), move || t.align.get(), move |v| t.align.set(v.to_owned()))))
                .into_element()
        })
        .action("Cancel", false, move || {
            t.open.set(false);
            focus();
        })
        .action("Apply", true, apply)
        .on_confirm(apply)
        .on_cancel(focus)
        .into_element()
}

/// The Image group's properties button: its dialog is to come.
pub fn open_image_properties() {
    super::note("Image Properties isn't available here yet.");
}

/// A prompt's check (`valid`): a size in points within 1–400, or six hex digits.
pub fn prompt_valid(kind: Prompt, v: &str) -> bool {
    let v = v.trim();
    match kind {
        Prompt::Size => {
            let (a, b) = v.split_once('.').unwrap_or((v, "0"));
            !a.is_empty() && a.chars().all(|c| c.is_ascii_digit()) && !b.is_empty() && b.chars().all(|c| c.is_ascii_digit()) && v.parse::<f32>().is_ok_and(|n| (1.0..=400.0).contains(&n))
        }
        Prompt::Color(_) => {
            let h = v.strip_prefix('#').unwrap_or(v);
            h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit())
        }
    }
}

/// The formatting bar's prompt (`fb`): a 320-wide card, its title, a
/// labelled field, Cancel and Apply.
fn prompt() -> AnyElement {
    let u = ui();
    let value = signal(String::new());
    effect(move || {
        let size = u.meta.with_untracked(|m| m.fmt.size);
        match u.prompt.get() {
            Some(Prompt::Size) => value.set(buwiz_office_kit::ui::select::size_text(size)),
            Some(Prompt::Color(_)) => value.set(String::new()),
            None => {}
        }
    });
    let close = move || {
        u.prompt.set(None);
        focus();
    };
    let apply = move || {
        let Some(kind) = u.prompt.get_untracked() else { return };
        let v = value.get_untracked();
        if !prompt_valid(kind, &v) {
            return;
        }
        u.prompt.set(None);
        match kind {
            Prompt::Size => send(Cmd::FontSize(v.trim().parse().unwrap_or(11.0))),
            Prompt::Color(mode) => super::toolbar::pick_color(mode, Picked::Rgb(v.trim().trim_start_matches('#').to_uppercase())),
        }
        focus();
    };
    when(
        move || u.prompt.with(Option::is_some),
        move || {
            let kind = u.prompt.get_untracked().unwrap_or(Prompt::Size);
            let (title, label, placeholder) = match kind {
                Prompt::Size => ("Font size", "Size in points", ""),
                Prompt::Color(m) => (m.label(), "Hex color, such as FF0000", "FF0000"),
            };
            let ok = move || u.prompt.get().is_some_and(|k| prompt_valid(k, &value.get()));
            let button = |label: &'static str, enabled: std::rc::Rc<dyn Fn() -> bool>, act: std::rc::Rc<dyn Fn()>| {
                let e2 = enabled.clone();
                text(label)
                    .px(px(12.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(p::BORDER_INPUT)
                    .bg(p::BG_HOVER)
                    .text_size(px(13.0))
                    .text_color(p::TEXT)
                    .label(label)
                    .opacity(move || if enabled() { 1.0f32 } else { 0.5 })
                    .on_press(move || {
                        if e2() {
                            act()
                        }
                    })
            };
            overlay()
                .inset_0()
                .items_center()
                .justify_center()
                .label(title)
                .on_cancel(close)
                .child(view().absolute().inset_0().bg(p::OVERLAY).on_press(close))
                .child(
                    view()
                        .w(px(320.0))
                        .p(px(16.0))
                        .rounded(px(8.0))
                        .bg(p::SURFACE)
                        .shadow([BoxShadow { offset_x: px(0.0), offset_y: px(8.0), blur: px(24.0), spread: px(0.0), color: color(p::SHADOW).into(), inset: false }])
                        .label(format!("{title} prompt"))
                        .child(text(title).mb(px(12.0)).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(p::TEXT))
                        .child(text(label).mb(px(4.0)).text_size(px(12.0)).text_color(p::TEXT))
                        .child(
                            nsh_base::input()
                                .label(label)
                                .auto_focus(true)
                                .placeholder(placeholder)
                                .value(move || value.get())
                                .on_change(move |v: &str| value.set(v.to_owned()))
                                .on_submit(move |_: &str| apply())
                                .w_full()
                                .px(px(8.0))
                                .py(px(6.0))
                                .border_1()
                                .border_color(p::BORDER)
                                .rounded(px(4.0))
                                .bg(p::SURFACE)
                                .text_size(px(13.0))
                                .text_color(p::TEXT),
                        )
                        .child(
                            view()
                                .flex_row()
                                .justify_end()
                                .gap(px(8.0))
                                .mt(px(16.0))
                                .child(button("Cancel", std::rc::Rc::new(|| true), std::rc::Rc::new(close)))
                                .child(button("Apply", std::rc::Rc::new(ok), std::rc::Rc::new(apply))),
                        ),
                )
        },
    )
    .into_element()
}

fn page_setup() -> AnyElement {
    let u = ui();
    let s = Setup {
        open: u.setup_open,
        w: signal(12240.0),
        h: signal(15840.0),
        landscape: signal(false),
        top: signal(1.0),
        bottom: signal(1.0),
        left: signal(1.0),
        right: signal(1.0),
    };
    SETUP.with(|c| c.set(Some(s)));
    let close = move || {
        s.open.set(false);
        focus();
    };
    let apply = move || {
        let twips = |f: Signal<f64>| (f.get_untracked() * TWIPS).round();
        send(Cmd::PageSetup(PageSetup {
            width: s.w.get_untracked(),
            height: s.h.get_untracked(),
            orientation: if s.landscape.get_untracked() { "landscape" } else { "portrait" }.to_owned(),
            top: twips(s.top),
            bottom: twips(s.bottom),
            left: twips(s.left),
            right: twips(s.right),
        }));
        close();
    };
    modal(s.open, "Page Setup")
        .gap(14.0)
        .body(move || {
            let margin = |label: &'static str, f: Signal<f64>| {
                form_row(
                    label,
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(12.0))
                        .child(view().flex_1().child(number_field(label, f, 0.0, 10.0)))
                        .child(text("in").w(px(16.0)).text_size(px(11.0)).text_color(p::TEXT_MUTED)),
                )
            };
            view()
                .gap(px(14.0))
                .child(section_label("Page size"))
                .child(dynamic(move || {
                    // "Custom" is listed only while the size is none of them
                    let at = size_of(s.w.get(), s.h.get());
                    let mut options: Vec<(String, String)> = SIZES.iter().enumerate().map(|(i, (n, _, _))| (i.to_string(), (*n).to_owned())).collect();
                    if at.is_none() {
                        options.push(("-1".into(), "Custom".into()));
                    }
                    let value = at.map_or("-1".to_owned(), |i| i.to_string());
                    form_row(
                        "Size",
                        select_field("Size", options, move || value.clone(), move |v| {
                            let Some((_, w, h)) = v.parse::<usize>().ok().and_then(|i| SIZES.get(i)) else { return };
                            let (w, h) = if s.landscape.get_untracked() { (*h, *w) } else { (*w, *h) };
                            s.w.set(w);
                            s.h.set(h);
                        }),
                    )
                }))
                .child(form_row(
                    "Orientation",
                    select_field(
                        "Orientation",
                        vec![("portrait".into(), "Portrait".into()), ("landscape".into(), "Landscape".into())],
                        move || if s.landscape.get() { "landscape" } else { "portrait" }.to_owned(),
                        move |v| {
                            let landscape = v == "landscape";
                            if landscape != s.landscape.get_untracked() {
                                s.landscape.set(landscape);
                                let (w, h) = (s.w.get_untracked(), s.h.get_untracked());
                                s.w.set(h);
                                s.h.set(w);
                            }
                        },
                    ),
                ))
                .child(view().mt(px(4.0)).child(section_label("Margins")))
                .child(margin("Top", s.top))
                .child(margin("Bottom", s.bottom))
                .child(margin("Left", s.left))
                .child(margin("Right", s.right))
                .into_element()
        })
        .action("Cancel", false, close)
        .action("Apply", true, apply)
        .on_cancel(focus)
        .on_confirm(apply)
        .into_element()
}

/// What Apply sends (`Q`): none, the text one, or the picture.
fn watermark_value(m: &Mark) -> Option<Option<Value>> {
    match m.mode.get_untracked() {
        "none" => Some(None),
        "text" => {
            let mut v = json!({
                "kind": "text",
                "text": m.text.get_untracked(),
                "font": m.font.get_untracked(),
                "color": m.color.get_untracked(),
                "semitransparent": m.semi.get_untracked(),
                "layout": if m.diagonal.get_untracked() { "diagonal" } else { "horizontal" },
            });
            if !m.auto.get_untracked() {
                v["fontSize"] = json!(m.size.get_untracked());
            }
            Some(Some(v))
        }
        _ => {
            let url = m.picture.get_untracked()?;
            let mut v = json!({"kind": "picture", "dataUrl": url, "scale": m.scale.get_untracked() / 100.0, "washout": m.washout.get_untracked()});
            if let Some((w, h)) = m.emu.get_untracked() {
                v["widthEmu"] = json!(w);
                v["heightEmu"] = json!(h);
            }
            Some(Some(v))
        }
    }
}

fn watermark() -> AnyElement {
    let u = ui();
    let m = Mark {
        open: u.watermark_open,
        mode: signal("none"),
        text: signal(PRESETS[0].to_owned()),
        font: signal("Calibri".to_owned()),
        auto: signal(true),
        size: signal(54.0),
        color: signal("#C0C0C0".to_owned()),
        diagonal: signal(true),
        semi: signal(true),
        picture: signal(None),
        emu: signal(None),
        scale: signal(100.0),
        washout: signal(true),
    };
    MARK.with(|c| c.set(Some(m)));
    let close = move || {
        m.open.set(false);
        focus();
    };
    let apply = move || {
        let Some(v) = watermark_value(&m) else { return };
        send(Cmd::Watermark(v));
        close();
    };
    // `K`: a picture watermark without its picture can't be applied
    let ready = move || !(m.mode.get() == "picture" && m.picture.with(Option::is_none));
    let row = |label: &'static str, field: AnyElement| view().flex_row().items_center().gap(px(12.0)).child(text(label).w(px(80.0)).text_size(px(13.0)).text_color(p::TEXT_MUTED)).child(field).into_element();
    modal(m.open, "Watermark")
        .gap(14.0)
        .body(move || {
            let picture = move || {
                view()
                    .gap(px(10.0))
                    .child(
                        text("Select Picture...")
                            .self_start()
                            .px(px(12.0))
                            .py(px(4.0))
                            .border_1()
                            .border_color(p::BORDER)
                            .rounded(px(4.0))
                            .text_size(px(13.0))
                            .text_color(p::TEXT)
                            .label("Select Picture...")
                            .on_press(move || {
                                pick_picture(move |bytes, name| {
                                    m.picture.set(Some(data_url(&bytes, std::path::Path::new(&name))));
                                    m.emu.set(image_size(&bytes).and_then(|(w, h)| picture_emu(f64::from(w), f64::from(h))));
                                })
                            }),
                    )
                    .child(dynamic(move || match m.picture.get() {
                        Some(url) => image(url).object_fit(ResizeMode::Contain).w(px(120.0)).h(px(60.0)).label("Watermark picture").into_element(),
                        None => view().into_element(),
                    }))
                    .child(row(
                        "Scale",
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(12.0))
                            .child(view().w(px(80.0)).child(number_field("Scale", m.scale, 10.0, 500.0)))
                            .child(text("%").text_size(px(11.0)).text_color(p::TEXT_MUTED))
                            .into_element(),
                    ))
                    .child(checkbox("Washout", m.washout))
                    .into_element()
            };
            let text_part = move || {
                let presets: Vec<(String, String)> = std::iter::once((String::new(), "\u{2014}".to_owned())).chain(PRESETS.iter().map(|p| ((*p).to_owned(), (*p).to_owned()))).collect();
                view()
                    .gap(px(10.0))
                    .child(row(
                        "Preset",
                        view()
                            .flex_1()
                            .child(select_field("Preset", presets, move || m.text.with(|t| if PRESETS.contains(&t.as_str()) { t.clone() } else { String::new() }), move |v| {
                                if !v.is_empty() {
                                    m.text.set(v.to_owned())
                                }
                            }))
                            .into_element(),
                    ))
                    .child(row("Text", view().flex_1().child(text_field("Text", m.text)).into_element()))
                    .child(row(
                        "Font",
                        view()
                            .flex_1()
                            .child(select_field("Font", FONTS.iter().map(|f| ((*f).to_owned(), (*f).to_owned())).collect(), move || m.font.get(), move |v| m.font.set(v.to_owned())))
                            .into_element(),
                    ))
                    .child(row(
                        "Size",
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(12.0))
                            .child(checkbox("Auto", m.auto))
                            .child(when(move || !m.auto.get(), move || view().w(px(70.0)).child(number_field("Font size", m.size, 8.0, 200.0))))
                            .into_element(),
                    ))
                    .child(row(
                        "Color",
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(view().w(px(28.0)).h(px(20.0)).rounded(px(3.0)).border_1().border_color(p::BORDER).bg(move || {
                                let c = hex(&m.color.get());
                                AdaptiveColor { light: c, dark: c }
                            }))
                            .child(view().w(px(100.0)).child(text_field("Color", m.color)))
                            .into_element(),
                    ))
                    .child(row(
                        "Layout",
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(12.0))
                            .child(radio("Diagonal", move || m.diagonal.get(), move || m.diagonal.set(true)))
                            .child(radio("Horizontal", move || !m.diagonal.get(), move || m.diagonal.set(false)))
                            .into_element(),
                    ))
                    .child(checkbox("Semitransparent", m.semi))
                    .into_element()
            };
            view()
                .gap(px(14.0))
                .child(radio("No watermark", move || m.mode.get() == "none", move || m.mode.set("none")))
                .child(radio("Picture watermark", move || m.mode.get() == "picture", move || m.mode.set("picture")))
                .child(when(move || m.mode.get() == "picture", picture))
                .child(radio("Text watermark", move || m.mode.get() == "text", move || m.mode.set("text")))
                .child(when(move || m.mode.get() == "text", text_part))
                .into_element()
        })
        .action("Cancel", false, close)
        .action_when("Apply", true, ready, apply)
        .on_cancel(focus)
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_match_either_way_round_and_margins_read_in_inches() {
        assert_eq!(size_of(12240.0, 15840.0), Some(0));
        assert_eq!(size_of(16838.0, 11906.0), Some(1));
        assert_eq!(size_of(12250.0, 15835.0), Some(0));
        assert_eq!(size_of(10000.0, 10000.0), None);
        assert_eq!(inches(1440.0), 1.0);
        assert_eq!(inches(1080.0), 0.75);
        assert_eq!(inches(1000.0), 0.69);
    }

    #[test]
    fn link_addresses_check_and_normalize_as_the_webs() {
        assert!(valid_url("example.com") && valid_url("https://example.com/a?b") && valid_url("mailto:a@b.c") && valid_url("tel:+15551234"));
        assert!(!valid_url("") && !valid_url("mailto:") && !valid_url("exa mple.com") && !valid_url("https://"));
        assert_eq!(normalize_url(" example.com "), "https://example.com");
        assert_eq!(normalize_url("mailto:a@b.c"), "mailto:a@b.c");
        assert!(prompt_valid(Prompt::Size, "13") && prompt_valid(Prompt::Size, "10.5") && !prompt_valid(Prompt::Size, "10.") && !prompt_valid(Prompt::Size, "500"));
        assert!(prompt_valid(Prompt::Color(Mode::Text), "#ff0000") && !prompt_valid(Prompt::Color(Mode::Text), "red"));
    }

    #[test]
    fn a_picture_watermarks_longer_side_is_fixed() {
        assert_eq!(picture_emu(200.0, 100.0), Some((3954780.0, 1977390.0)));
        assert_eq!(picture_emu(100.0, 200.0), Some((1977390.0, 3954780.0)));
        assert_eq!(picture_emu(0.0, 1.0), None);
    }
}
