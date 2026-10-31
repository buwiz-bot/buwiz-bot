//! The dialogs the menus open, on the kit's modal shell with the web's
//! fields and texts: Page Setup (`PageSetupDialog`) and Watermark
//! (`WatermarkDialog`).
//!
//! Each holds its fields in signals made with the view; opening one fills
//! them from what the worker last said (the page of the last section, the
//! watermark), as the web's dialogs do on `isOpen`.

use std::cell::Cell;

use nsh_core::prelude::*;
use serde_json::{Value, json};

use super::WordUi;
use super::chrome::{data_url, image_size, pick_picture};
use super::doc::{Cmd, PageSetup};
use super::view::{focus, send};
use crate::live::store;
use crate::office::ui::color::hex;
use crate::office::ui::field::{checkbox, form_row, number_field, radio, section_label, select_field, text_field};
use crate::office::ui::modal::modal;
use crate::office::ui::palette as p;

fn ui() -> &'static WordUi {
    &store().office.word
}

/// Twips in an inch (`TWIPS_PER_INCH`).
const TWIPS: f64 = 1440.0;

/// Page Setup's sizes (`E`): name, width and height in twips.
pub(crate) const SIZES: [(&str, f64, f64); 7] = [
    ("Letter (8.5\" \u{d7} 11\")", 12240.0, 15840.0),
    ("A4 (8.27\" \u{d7} 11.69\")", 11906.0, 16838.0),
    ("Legal (8.5\" \u{d7} 14\")", 12240.0, 20160.0),
    ("A3 (11.69\" \u{d7} 16.54\")", 16838.0, 23811.0),
    ("A5 (5.83\" \u{d7} 8.27\")", 8391.0, 11906.0),
    ("B5 (6.93\" \u{d7} 9.84\")", 9979.0, 14175.0),
    ("Executive (7.25\" \u{d7} 10.5\")", 10440.0, 15120.0),
];

/// The size a page of `w` × `h` twips is (`J`: either way round, within 20).
pub(crate) fn size_of(w: f64, h: f64) -> Option<usize> {
    let (a, b) = (w.min(h), w.max(h));
    SIZES.iter().position(|(_, sw, sh)| (sw - a).abs() < 20.0 && (sh - b).abs() < 20.0)
}

/// Twips as the margin fields show them (`f`): inches to two places.
pub(crate) fn inches(twips: f64) -> f64 {
    (twips / TWIPS * 100.0).round() / 100.0
}

/// The watermark's text presets (`DEFAULT_WATERMARK_PRESETS`).
pub(crate) const PRESETS: [&str; 6] = ["CONFIDENTIAL", "DRAFT", "DO NOT COPY", "SAMPLE", "URGENT", "ASAP"];
/// Its fonts (`te`).
pub(crate) const FONTS: [&str; 6] = ["Calibri", "Arial", "Times New Roman", "Georgia", "Verdana", "Courier New"];

/// A picture watermark's size (`pictureWatermarkDisplayEmu`): its longer
/// side 3954780 EMU.
pub(crate) fn picture_emu(w: f64, h: f64) -> Option<(f64, f64)> {
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
pub(crate) fn open_page_setup() {
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
pub(crate) fn open_watermark() {
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
pub(crate) fn dialogs() -> impl IntoElement {
    view().child(page_setup()).child(watermark())
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
    fn a_picture_watermarks_longer_side_is_fixed() {
        assert_eq!(picture_emu(200.0, 100.0), Some((3954780.0, 1977390.0)));
        assert_eq!(picture_emu(100.0, 200.0), Some((1977390.0, 3954780.0)));
        assert_eq!(picture_emu(0.0, 1.0), None);
    }
}
