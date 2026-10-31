//! `filetile.tsx` + `fileviewer.tsx` + `lightbox.tsx`: attachment tiles,
//! Grok's elevated file/audio cards, the justified media gallery, the
//! fullscreen per-kind viewer (`viewing` + `/file/render`) and the media
//! lightbox (`light_items`).

use std::str::FromStr;

use nsh_core::prelude::*;

use crate::live::store;
use crate::store::{Attach, Sheet, Store};
use crate::theme;

// ——————————————————— colors (`tc()` over the file UIs' literals) ———————————————————

use crate::theme::tc;

const WHITE85: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 1.0,
    a: 0.85,
};
const LIGHT_BG: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.0,
    a: 0.6,
};

// ——————————————————— small formatters ———————————————————

/// `fmtSize` — "506B", "1.4kB", "11kB", "2.3MB".
pub(crate) fn fmt_size(n: f64) -> String {
    let f = |v: f64, u: &str| {
        if v < 10.0 {
            format!("{:.1}{u}", (v * 10.0).round() / 10.0)
        } else {
            format!("{}{u}", v.round() as i64)
        }
    };
    if n < 1000.0 {
        format!("{}B", n as i64)
    } else if n < 1e6 {
        f(n / 1e3, "kB")
    } else {
        f(n / 1e6, "MB")
    }
}

/// `fmtTime` — m:ss.
pub(crate) fn fmt_time(t: f64) -> String {
    format!("{}:{:02}", (t / 60.0) as i64, (t % 60.0) as i64)
}

/// `stemExt` — the name split so the stem truncates but the extension
/// always shows.
fn stem_ext(name: &str) -> (String, String) {
    match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_owned(), name[i..].to_owned()),
        _ => (name.to_owned(), String::new()),
    }
}

/// `mediaUrl` — `file://…/generated/{attachments,avatars}/x` → the local
/// server's `/files/…` route.
pub(crate) fn media_url(uri: &str) -> String {
    if let Some(rest) = uri.strip_prefix("file://") {
        for bucket in ["attachments", "avatars"] {
            let marker = format!("/generated/{bucket}/");
            if let Some(at) = rest.find(&marker) {
                let name = &rest[at + marker.len()..];
                if !name.contains('/') {
                    return format!(
                        "{}/files/{bucket}/{}",
                        crate::client::backend(),
                        name.replace(' ', "%20")
                    );
                }
            }
        }
    }
    uri.to_owned()
}

/// `isSvg` — SVGs draw through WebKit, the native decoder does raster.
fn is_svg(uri: &str) -> bool {
    let u = uri.to_lowercase();
    u.split('?').next().unwrap_or("").ends_with(".svg")
}

/// `MermaidBlock` (md.tsx:119): a ```mermaid fence drawn natively — Pele
/// ported to Rust lays it out, nsh's canvas draws it (`mermaid::diagram`)
/// at its natural size, scaled down to the column. A diagram Pele does not
/// draw shows its message instead. A press opens the lightbox; a node's
/// `click` link opens through `links.rs`.
pub(crate) fn mermaid_block(code: &str) -> impl IntoElement {
    let code = code.to_owned();
    view().max_w(percentage(1.0)).child(dynamic(move || {
        let d = crate::mermaid::diagram(&code, dark());
        match d.get() {
            // Laying out (well under a millisecond): hold the line's height.
            None => view().h(px(24.0)).into_element(),
            Some(Ok(g)) => {
                let code = code.clone();
                diagram_canvas(g, None).label("Open diagram").on_press(move || open_diagram(&code)).into_element()
            }
            Some(Err(e)) => diagram_note(&e).into_element(),
        }
    }))
}

/// Why Pele did not draw a diagram, in place of it.
fn diagram_note(e: &crate::mermaid::Unported) -> impl IntoElement {
    text(e.note()).text_size(px(13.0)).text_color(theme::MUTED).pt(px(4.0)).pb(px(4.0))
}

/// The diagram `code` in the lightbox.
fn open_diagram(code: &str) {
    store().open_lightbox(
        vec![crate::store::LightItem {
            kind: "diagram".to_owned(),
            uri: String::new(),
            name: "Diagram".to_owned(),
            path: String::new(),
            poster: String::new(),
            w: 0.0,
            h: 0.0,
            code: code.to_owned(),
        }],
        0,
    );
}

/// A native diagram: its drawing at its natural size, scaled to fit the
/// column as an image would (`fixed`: drawn at that scale instead, for the
/// lightbox). Its `click` links are pressable areas over it.
fn diagram_canvas(g: std::sync::Arc<crate::mermaid::Drawn>, fixed: Option<f32>) -> View {
    let width = signal(g.w);
    let gw = g.w;
    let k = move || fixed.unwrap_or_else(|| (width.get() / gw).min(1.0).max(0.01));
    let gd = g.clone();
    let mut out = view()
        .w(px(g.w * fixed.unwrap_or(1.0)))
        .max_w(percentage(1.0))
        .aspect_ratio(g.w / g.h)
        .child(
            canvas(move || {
                let k = k();
                let mut ops = Vec::with_capacity(gd.drawing.ops.len() + 1);
                ops.push(nsh_core::canvas::CanvasOp::Scale(nsh_core::canvas::Num::Const(k), nsh_core::canvas::Num::Const(k)));
                ops.extend(gd.drawing.ops.iter().cloned());
                let nodes = gd
                    .drawing
                    .nodes
                    .iter()
                    .map(|n| {
                        let mut n = n.clone();
                        n.bounds = n.bounds.map(|v| px(v.0 * k));
                        n
                    })
                    .collect();
                Painter::from(nsh_core::canvas::Drawing { ops, nodes, ..Default::default() })
            })
            .absolute()
            .inset(px(0.0)),
        )
        .on_layout(move |b: &Bounds<Points>| {
            if fixed.is_none() && (b.size.width.0 - width.get_untracked()).abs() > 0.5 {
                width.set(b.size.width.0);
            }
        });
    for (b, href) in g.links.iter().cloned() {
        let href_p = href.clone();
        out = out.child(
            view()
                .absolute()
                .left(move || px(b.origin.x.0 * k()))
                .top(move || px(b.origin.y.0 * k()))
                .w(move || px(b.size.width.0 * k()))
                .h(move || px(b.size.height.0 * k()))
                .label(href)
                .on_press(move || store().open_link(&href_p)),
        );
    }
    out
}

// ——————————————————— FileTile (`filetile.tsx`) ———————————————————

/// The extension glyph + accent of `filetile`'s `ICONS` table.
pub(crate) fn ext_icon(ext: &str) -> (&'static str, AdaptiveColor) {
    let (glyph, hex): (&'static str, &str) = match ext {
        "md" | "markdown" | "mdx" => ("M↓", ""),
        "pdf" => ("PDF", "#f2555a"),
        "pptx" | "ppt" | "docx" | "doc" | "pages" | "rtf" => ("≡", "#4d9bff"),
        "key" => ("⚿", "#f5b400"),
        "xlsx" | "xls" | "csv" | "numbers" | "tsv" => ("▦", "#3ecf8e"),
        "txt" | "log" => ("¶", ""),
        "json" => ("{ }", "#f5b400"),
        "yaml" | "yml" => ("Y", "#f2555a"),
        "toml" => ("⚙︎", ""),
        "html" => ("</>", "#4d9bff"),
        "css" | "scss" => ("#", "#f062a8"),
        "js" => ("JS", "#f5b400"),
        "jsx" => ("⚛︎", "#22c3b4"),
        "ts" => ("TS", "#4d9bff"),
        "tsx" => ("⚛︎", "#22c3b4"),
        "py" => ("Py", "#4d9bff"),
        "rs" => ("Rs", "#ff8a3d"),
        "go" => ("Go", "#22c3b4"),
        "swift" => ("Sw", "#ff8a3d"),
        "sql" => ("⛁", "#22c3b4"),
        "zip" | "gz" | "tar" | "rar" | "7z" => ("⊞", "#f5b400"),
        "mov" | "mp4" | "m4v" | "webm" => ("▶", "#f2555a"),
        "mp3" | "wav" | "m4a" | "aac" => ("∿", "#a78bfa"),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" => ("▣", "#22c3b4"),
        "svg" => ("◇", "#ff8a3d"),
        _ => ("▤", ""),
    };
    let color = if hex.is_empty() {
        tc("#f5f5f5a8")
    } else {
        AdaptiveColor::from(Hsla::from_str(hex).unwrap_or_default())
    };
    (glyph, color)
}

/// A file's extension (the record's, else the name's suffix).
pub(crate) fn ext_of(name: &str, ext: &str) -> String {
    if !ext.is_empty() {
        return ext.to_lowercase();
    }
    name.rsplit('.').next().unwrap_or("").to_lowercase()
}

/// `FileTile` — a square tile (`size` pt): the thumbnail (a video gets a
/// play badge), else the extension glyph + name over `#161616`.
pub(crate) fn file_tile(a: &Attach, size: f32) -> impl IntoElement {
    let pic = if !a.thumb.is_empty() {
        media_url(&a.thumb)
    } else if !a.feed.is_empty() {
        Store::file_raw_url(&a.feed)
    } else if a.kind == "image" {
        media_url(&a.url)
    } else {
        String::new()
    };
    let is_video = a.kind == "video" && !pic.is_empty();
    let (glyph, color) = ext_icon(&ext_of(&a.name, &a.ext));
    let name = if a.name.is_empty() {
        a.path.clone()
    } else {
        a.name.clone()
    };
    view()
        .w(px(size))
        .h(px(size))
        .rounded(px(12.0))
        .border(px(0.5))
        .border_color(tc("#f5f5f51f"))
        .overflow_hidden()
        .bg(tc("#161616"))
        .items_center()
        .justify_center()
        .child(if pic.is_empty() {
            view()
                .items_center()
                .gap(px(3.0))
                .px(px(5.0))
                .child(
                    text(glyph)
                        .text_size(px(if glyph.len() > 2 { 11.0 } else { 17.0 }))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color),
                )
                .child(
                    text(name)
                        .text_size(px(9.0))
                        .line_height(px(11.0))
                        .text_color(tc("#f5f5f58a"))
                        .number_of_lines(2)
                        .text_center(),
                )
                .into_element()
        } else {
            image(pic)
                .object_fit(ResizeMode::Cover)
                .fade_duration(0.0)
                .absolute()
                .w(px(size))
                .h(px(size))
                .into_element()
        })
        .when(is_video, |v| {
            v.child(
                view()
                    .size(px(22.0))
                    .rounded(px(11.0))
                    .bg(LIGHT_BG)
                    .items_center()
                    .justify_center()
                    .child(
                        text("▶")
                            .text_size(px(9.0))
                            .text_color(theme::hsla(0.0, 0.0, 1.0, 1.0))
                            .pl(px(1.0)),
                    ),
            )
        })
}

/// A small square icon for a file in a list (the Library): an image's or
/// video's own picture, else the extension glyph on its accent.
pub(crate) fn file_icon(a: &Attach, size: f32) -> impl IntoElement {
    let pic = if !a.thumb.is_empty() {
        media_url(&a.thumb)
    } else if a.kind == "image" || (a.kind == "video" && !a.poster.is_empty()) {
        let src = if a.kind == "video" { &a.poster } else if a.feed.is_empty() { &a.path } else { &a.feed };
        Store::file_raw_url(src)
    } else {
        String::new()
    };
    let (glyph, color) = ext_icon(&ext_of(&a.name, &a.ext));
    let tile = view()
        .size(px(size))
        .rounded(px(size * 0.25))
        .overflow_hidden()
        .items_center()
        .justify_center();
    if !pic.is_empty() {
        return tile
            .bg(theme::CARD2)
            .child(image(pic).object_fit(ResizeMode::Cover).fade_duration(0.0).size(px(size)))
            .into_element();
    }
    tile.bg(color.opacity(0.16))
        .child(
            text(glyph)
                .text_size(px(if glyph.chars().count() > 2 { size * 0.3 } else { size * 0.5 }))
                .font_weight(FontWeight::BOLD)
                .text_color(color),
        )
        .into_element()
}

// ——————————————————— DocGlyph (`Xg` + `XF`) ———————————————————

/// Grok's 44pt document illustration: a 32×40 page (40×34 for slides),
/// radius 6, 4pt bars, a 12pt colored stripe per kind. One canvas on
/// the page's box: drawn as views it was 7–20 of them per card (a
/// spreadsheet's grid alone 16), and a reply's two dozen cards mounted
/// hundreds of views in one frame. The bars sit where the views' layout
/// put them: the page's 0.5pt border and 4pt padding, a 3pt gap.
fn doc_glyph(variant: &str) -> impl IntoElement {
    let landscape = variant == "ppt" || variant == "keynote";
    let stripe = match variant {
        "docx" | "keynote" => Some(tc("#116dd2")),
        "pdf" => Some(tc("#cf2b35")),
        "ppt" => Some(tc("#cd5309")),
        "spreadsheet" => Some(tc("#0e9f5a")),
        _ => None,
    };
    let (w, h) = if landscape { (40.0f32, 34.0f32) } else { (32.0, 40.0) };
    let variant = variant.to_owned();
    canvas(move || {
        let bar = color(tc("#f5f5f51f"));
        // the body: inside the border, padded 4; the stripe under it
        let (x0, y0) = (4.5f32, 4.5f32);
        let cw = w - 9.0;
        let ch = h - 9.0 - if stripe.is_some() { 12.0 } else { 0.0 };
        let rr = |p: Painter, x: f32, y: f32, bw: f32, bh: f32, r: f32| {
            p.begin_path()
                .round_rect(Bounds { origin: point(px(x0 + x), px(y0 + y)), size: size(px(bw), px(bh)) }, px(r))
                .fill()
        };
        // a column of 4pt bars 3pt apart, each `(width, left)`
        let bars = |p: Painter, rows: &[(f32, f32)]| {
            rows.iter().enumerate().fold(p, |p, (k, &(bw, at))| rr(p, at, k as f32 * 7.0, bw, 4.0, 2.0))
        };
        let mut p = Painter::new().fill_color(bar);
        p = match variant.as_str() {
            "docx" => bars(p, &[(cw, 0.0), (cw, 0.0), (cw * 0.48, 0.0)]),
            "ppt" | "keynote" => bars(p, &[(cw, 0.0), (cw * 0.48, 0.0)]),
            "json" => {
                // dot, an indented dot and bar twice, dot
                let p = rr(p, 0.0, 0.0, 4.0, 4.0, 2.0);
                let p = rr(p, 4.0, 7.0, 4.0, 4.0, 2.0);
                let p = rr(p, 10.0, 7.0, cw - 10.0, 4.0, 2.0);
                let p = rr(p, 4.0, 14.0, 4.0, 4.0, 2.0);
                let p = rr(p, 10.0, 14.0, cw - 10.0, 4.0, 2.0);
                rr(p, 0.0, 21.0, 4.0, 4.0, 2.0)
            }
            "pdf" => {
                // a bar, then two bars beside a block filling the rest
                let half = (cw - 2.0) / 2.0;
                let p = rr(p, 0.0, 0.0, cw, 4.0, 2.0);
                let p = rr(p, 0.0, 7.0, half, 4.0, 2.0);
                let p = rr(p, 0.0, 14.0, half, 4.0, 2.0);
                rr(p, half + 2.0, 7.0, half, ch - 7.0, 2.0)
            }
            "spreadsheet" => {
                // 3×5 cells: a hairline right of the first two columns,
                // above every row but the first
                let (cell_w, cell_h) = (cw / 3.0, ch / 5.0);
                let mut p = p.fill_color(color(tc("#f5f5f51a")));
                for i in 0..15 {
                    let (cx, cy) = ((i % 3) as f32 * cell_w, (i / 3) as f32 * cell_h);
                    let right = i % 3 < 2;
                    if right {
                        p = p.fill_rect(x0 + cx + cell_w - 0.5, y0 + cy, 0.5, cell_h);
                    }
                    if i >= 3 {
                        let top_w = cell_w - if right { 0.5 } else { 0.0 };
                        p = p.fill_rect(x0 + cx, y0 + cy, top_w, 0.5);
                    }
                }
                p
            }
            "zip" => {
                let rows: Vec<(f32, f32)> =
                    [0.5, 0.25, 0.5, 0.25, 0.5, 0.25].iter().map(|at| (cw * 0.25, cw * at)).collect();
                bars(p, &rows)
            }
            "audio" => {
                // three bars, 16 / 24 / 16 tall, centered
                let left = (cw - 18.0) / 2.0;
                [16.0f32, 24.0, 16.0].iter().enumerate().fold(p, |p, (k, bh)| {
                    rr(p, left + k as f32 * 7.0, (ch - bh) / 2.0, 4.0, *bh, 2.0)
                })
            }
            "font" | "unknown" => p
                .font("600 15px -apple-system, system-ui, sans-serif")
                .text_align(nsh_core::canvas::CanvasTextAlign::Center)
                .text_baseline(nsh_core::canvas::CanvasTextBaseline::Middle)
                .fill_color(color(tc("#f5f5f58a")))
                .fill_text(if variant == "font" { "Aa" } else { "?" }, x0 + cw / 2.0, y0 + ch / 2.0),
            _ => bars(p, &[(cw * 0.48, 0.0), (cw, 0.0), (cw, 0.0), (cw * 0.48, 0.0)]),
        };
        if let Some(s) = stripe {
            p = p.fill_color(color(s)).fill_rect(0.5, h - 12.5, w - 1.0, 12.0);
        }
        p
    })
    .w(px(w))
    .h(px(h))
    // the 44pt square the page sits centered in
    .mx(px((44.0 - w) / 2.0))
    .my(px((44.0 - h) / 2.0))
    .rounded(px(6.0))
    .overflow_hidden()
    .bg(tc("#1f1f1f"))
    .border(px(0.5))
    .border_color(tc("#f5f5f51a"))
}

/// `FileName` — the stem truncates, the extension always shows.
fn file_name(name: &str) -> View {
    let (stem, ext) = stem_ext(name);
    // the stem shrinks (ellipsis) so a long name stops short of the
    // card's trailing button instead of running under it
    view()
        .flex_row()
        .min_w(px(0.0))
        .child(
            text(stem)
                .number_of_lines(1)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .text_size(px(14.0))
                .line_height(px(20.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(tc("#f5f5f5")),
        )
        .child(
            text(ext)
                .text_size(px(14.0))
                .line_height(px(20.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(tc("#f5f5f5")),
        )
}

/// The card's ⤓: one text, its 28pt line centering the glyph.
/// The width of a 230pt card's name and size lines: what the glyph, the
/// ⤓ and the paddings and gaps leave.
const LINES_W: f32 = 119.0;

/// A card's name at `(left, top)` of its row, its size line set 20pt
/// under it by the caller: placed, not stacked in a column view, and one
/// text when the name fits (stem and extension share a style, so it
/// reads the same); `file_name`'s two only when the stem must truncate.
fn card_name(name: &str, left: f32, top: f32) -> AnyElement {
    let fits = measure_text("500 14px -apple-system, system-ui, sans-serif", name).0 <= LINES_W - 2.0;
    if !fits {
        return file_name(name).absolute().left(px(left)).top(px(top)).w(px(LINES_W)).into_element();
    }
    text(name.to_owned())
        .number_of_lines(1)
        .text_size(px(14.0))
        .line_height(px(20.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(tc("#f5f5f5"))
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(LINES_W))
        .into_element()
}

fn save_btn(f: &Attach) -> Text {
    let path = f.path.clone();
    let name = f.name.clone();
    text("⤓")
        .label(format!("Save {name}"))
        .size(px(28.0))
        .rounded(px(14.0))
        .text_center()
        .line_height(px(28.0))
        .number_of_lines(1)
        .text_size(px(14.0))
        .text_color(tc("#f5f5f5a8"))
        .on_press(move || store().save_file(&path))
}

fn card_box() -> View {
    view()
        .w(px(230.0))
        .rounded(px(12.0))
        .border(px(0.5))
        .border_color(tc("#f5f5f51f"))
        .overflow_hidden()
        .bg(tc("#1f1f1f"))
}

/// `FileCard` — 230×64: glyph, name, size, save.
pub(crate) fn file_card(f: &Attach) -> impl IntoElement {
    let f2 = f.clone();
    let hov = signal(false);
    // the box is the button: no wrapper view around it
    card_box()
        .label(format!("Open {}", f.name))
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(64.0))
        .pl(px(10.0))
        .pr(px(12.0))
        .bg(move || if hov.get() { tc("#262626") } else { tc("#1f1f1f") })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .child(doc_glyph(&f.variant))
        // name and size where the column centered them: past the border,
        // the padding, the glyph and a gap; 38pt of lines in 63
        .child(card_name(&f.name, 62.0, 12.5))
        .child(
            text(fmt_size(f.size))
                .number_of_lines(1)
                .text_size(px(13.0))
                .line_height(px(18.0))
                .text_color(tc("#f5f5f5ad"))
                .absolute()
                .left(px(62.0))
                .top(px(32.5))
                .w(px(LINES_W)),
        )
        .child(save_btn(f).ml(auto()))
        .on_press(move || store().open_file(&f2))
}

/// A Drive file the reply made or changed — `FileCard`'s 230×64 box: its
/// type tile with the Google badge, name, "Google Doc · Google Drive";
/// a press opens it in the browser. A server's mini app (`mcp-app`) has
/// its name's tile and opens full screen.
pub(crate) fn link_card(l: &crate::store::DriveLink) -> impl IntoElement {
    let url = l.link.clone();
    let hov = signal(false);
    // a server's mini app (`rox/open`): opens full screen, not in Drive
    let app = (l.provider == "mcp-app").then(|| (l.id.clone(), l.name.clone()));
    let name = if l.name.is_empty() { format!("Untitled {}", l.file_type) } else { l.name.clone() };
    let kind = if l.file_type.is_empty() { "File" } else { l.file_type.as_str() };
    let place = match &app {
        Some((key, _)) => key.trim_start_matches("mcp:").split('/').next().unwrap_or("").to_owned(),
        None => "Google Drive".to_owned(),
    };
    view()
        .label(format!("Open {name} in {place}"))
        .child(
            card_box()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(64.0))
                .pl(px(12.0))
                .pr(px(12.0))
                .bg(move || if hov.get() { tc("#262626") } else { tc("#1f1f1f") })
                .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                // a server's app draws its name's tile, not a Drive file's
                .child(if app.is_some() {
                    crate::appdetails::named_tile("", &name, 36.0).into_element()
                } else {
                    crate::appdetails::connector_tile(&l.kind, &l.name, 36.0).into_element()
                })
                .child(
                    view().flex_1().min_w(px(0.0)).child(file_name(&name)).child(
                        text(format!("{kind} · {place}"))
                            .number_of_lines(1)
                            .text_size(px(13.0))
                            .line_height(px(18.0))
                            .text_color(tc("#f5f5f5ad")),
                    ),
                )
                .child(crate::icons::icon_ink("arrowUpRight", 14.0, || tc("#f5f5f5a8"))),
        )
        .on_press(move || match &app {
            Some((key, title)) => store().open_mini_token(key, title, &crate::store::link_token(&url)),
            None => store().open_link(&url),
        })
}

// ——————————————————— audio (`useAudio` + AudioCard) ———————————————————

const SPEEDS: [f32; 4] = [1.0, 1.5, 2.0, 0.5];

/// One card's player: the hidden `video()` node and its controls. The
/// store's `audio_cur` keeps one sounding at a time (TSX's `stopCurrent`).
#[derive(Clone)]
struct Audio {
    id: u64,
    paused: Signal<bool>,
    time: Signal<f64>,
    dur: Signal<f64>,
    rate: Signal<f32>,
    muted: Signal<bool>,
    scrub: Signal<bool>,
    last_x: Signal<f32>,
    player: ElementRef,
    path: String,
}

impl Audio {
    fn new(f: &Attach) -> Audio {
        // FNV-1a of the path — stable per file, no two cards share it.
        let mut id = 0xcbf29ce484222325u64;
        for b in f.path.bytes() {
            id = (id ^ b as u64).wrapping_mul(0x100000001b3);
        }
        Audio {
            id: id.max(1),
            paused: signal(true),
            time: signal(0.0),
            dur: signal(f.duration),
            rate: signal(1.0),
            muted: signal(false),
            scrub: signal(false),
            last_x: signal(0.0),
            player: element_ref(),
            path: f.path.clone(),
        }
    }

    /// The press lands: seek to the tracked pointer x, then scrub.
    fn scrub_on(&self, w: f32) {
        self.scrub.set(true);
        let d = self.dur.get_untracked();
        if d > 0.0 {
            self.seek((self.last_x.get_untracked() / w).clamp(0.0, 1.0) as f64 * d);
        }
    }

    fn scrub_off(&self) {
        self.scrub.set(false);
    }

    /// A pointer moved over a `w`-wide track: seek when scrubbing.
    fn seek_at(&self, x: f32, w: f32) {
        self.last_x.set(x);
        let d = self.dur.get_untracked();
        if d > 0.0 && self.scrub.get_untracked() {
            self.seek((x / w).clamp(0.0, 1.0) as f64 * d);
        }
    }

    fn playing(&self) -> bool {
        !self.paused.get() && store().audio_cur.get() == self.id
    }

    fn toggle(&self) {
        if self.playing() {
            self.paused.set(true);
            store().audio_cur.set(0);
        } else {
            self.paused.set(false);
            store().audio_cur.set(self.id);
        }
    }

    fn seek(&self, t: f64) {
        self.time.set(t);
        self.player.seek(t);
    }

    fn next_rate(&self) {
        let i = SPEEDS
            .iter()
            .position(|r| *r == self.rate.get_untracked())
            .unwrap_or(0);
        self.rate.set(SPEEDS[(i + 1) % SPEEDS.len()]);
    }

    /// The hidden 1×1 `video()` node that plays the audio.
    fn node(&self) -> impl IntoElement {
        let (paused, time, dur, rate, muted, id) =
            (self.paused, self.time, self.dur, self.rate, self.muted, self.id);
        let player = self.player;
        video(Store::file_raw_url(&self.path))
            .element_ref(&player)
            .paused(move || paused.get() || store().audio_cur.get() != id)
            .rate(move || rate.get())
            .muted(move || muted.get())
            .progress_interval(100.0)
            .on_load(move |e: &VideoInfo| {
                if e.duration > 0.0 {
                    dur.set(e.duration as f64);
                }
                // a card's player comes on first use: where a scrub
                // before it put the time
                let at = time.get_untracked();
                if at > 0.0 {
                    player.seek(at);
                }
            })
            .on_progress(move |e: &VideoProgress| time.set(e.current_time as f64))
            .on_end(move || {
                paused.set(true);
                time.set(0.0);
                player.seek(0.0);
            })
            .absolute()
            .size(px(1.0))
            .opacity(0.0)
    }
}

/// `AudioCard` — 230×112: glyph · name · "0:00 · size" · save, then
/// ▶ · 1× · waveform · time (played bars take the bot's tint).
pub(crate) fn audio_card(f: &Attach, tint: AdaptiveColor) -> impl IntoElement {
    let f2 = f.clone();
    let a = Audio::new(f);
    let a2 = a.clone();
    let a2b = a.clone();
    let a3 = a.clone();
    let a3b = a.clone();
    let a4 = a.clone();
    let a6 = a.clone();
    let a5 = a.clone();
    let a5b = a.clone();
    let a5c = a.clone();
    let peaks: Vec<f64> = if f.peaks.is_empty() {
        vec![0.3; 48]
    } else {
        f.peaks.clone()
    };
    // TSX tints unplayed bars at ~55% alpha of the bubble fill.
    let fade = AdaptiveColor {
        light: Hsla { a: tint.light.a * 0.55, ..tint.light },
        dark: Hsla { a: tint.dark.a * 0.55, ..tint.dark },
    };
    const W: f32 = 93.0;
    let bars = peaks.len().div_ceil(2).max(1) as f64;
    let an = a.clone();
    // The player (a native video view) is made the first time the card
    // plays or seeks: a reply's audio cards made three of them on mount.
    let used = std::cell::Cell::new(false);
    let used = computed(move || {
        if !used.get() && (!an.paused.get() || an.scrub.get()) {
            used.set(true);
        }
        used.get()
    });
    let an = a.clone();
    card_box()
        .child(when(used, move || an.node().into_element()))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(64.0))
                .pl(px(10.0))
                .pr(px(12.0))
                .child(doc_glyph("audio"))
                // as a file card's lines (this row has no border)
                .child(card_name(&f.name, 62.0, 13.0))
                .child(
                    text({
                        let size = f.size;
                        move || format!("{} · {}", fmt_time(a.dur.get()), fmt_size(size))
                    })
                        .number_of_lines(1)
                        .text_size(px(13.0))
                        .line_height(px(18.0))
                        .text_color(tc("#f5f5f5ad"))
                        .absolute()
                        .left(px(62.0))
                        .top(px(33.0))
                        .w(px(LINES_W)),
                )
                .child(save_btn(f).ml(auto()))
                .on_press(move || store().open_file(&f2)),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .h(px(48.0))
                .pl(px(14.0))
                .pr(px(12.0))
                // each control is one text, its line centering the glyph
                .child(
                    text(move || if a2.playing() { "❚❚" } else { "▶" })
                        .label("Play/Pause")
                        .size(px(24.0))
                        .rounded(px(12.0))
                        .text_center()
                        .line_height(px(24.0))
                        .number_of_lines(1)
                        .text_size(px(13.0))
                        .text_color(tc("#f5f5f5"))
                        .on_press(move || a2b.toggle()),
                )
                .child(
                    text(move || format!("{}×", a3.rate.get()))
                        .label("Playback speed")
                        .w(px(34.0))
                        .h(px(32.0))
                        .text_center()
                        .line_height(px(32.0))
                        .number_of_lines(1)
                        .border_r(px(0.5))
                        .border_r_color(tc("#f5f5f50f"))
                        .text_size(px(14.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(tc("#f5f5f5ad"))
                        .on_press(move || a3b.next_rate()),
                )
                // The waveform: one canvas, 1.5pt bars spread edge to
                // edge, played ones in the tint (24 views before).
                .child(
                    canvas(move || {
                        let (d, t) = (a4.dur.get(), a4.time.get());
                        let (played, rest) = (color(tint), color(fade));
                        let n = peaks.len().div_ceil(2).max(1);
                        let step = if n > 1 { (W - 1.5) / (n - 1) as f32 } else { 0.0 };
                        peaks.iter().step_by(2).enumerate().fold(Painter::new(), |p, (i, peak)| {
                            let h = (*peak as f32 * 24.0).max(2.0);
                            let on = d > 0.0 && (i as f64 / bars) < t / d;
                            p.fill_color(if on { played } else { rest })
                                .begin_path()
                                .round_rect(
                                    Bounds { origin: point(px(i as f32 * step), px((24.0 - h) / 2.0)), size: size(px(1.5), px(h)) },
                                    px(0.75),
                                )
                                .fill()
                        })
                    })
                    .label("Seek")
                    .w(px(W))
                    .h(px(24.0))
                    .ml(px(10.0))
                    .on_mouse_move(move |e: &nsh_core::MouseEvent| {
                        a5.seek_at(e.position.x.0, W);
                    })
                    .on_press_down(move || a5b.scrub_on(W))
                    .on_press_up(move || a5c.scrub_off()),
                )
                .child(dynamic(move || {
                    let show = if a6.paused.get() && a6.time.get() == 0.0 {
                        a6.dur.get()
                    } else {
                        a6.time.get()
                    };
                    text(fmt_time(show))
                        .ml(px(10.0))
                        .text_size(px(13.0))
                        .line_height(px(18.0))
                        .text_color(tc("#f5f5f5ad"))
                        .into_element()
                })),
        )
}

// ——————————————————— MediaGallery ———————————————————

const GAL_W: f64 = 480.0;
const GAP: f64 = 4.0;

fn image_unavailable(w: f32, h: f32) -> impl IntoElement {
    view()
        .w(px(w))
        .h(px(h))
        .rounded(px(10.0))
        .border(px(0.5))
        .border_color(tc("#f5f5f51f"))
        .bg(tc("#f5f5f50a"))
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .p_2()
        .child(
            text("🖼︎").text_size(px(20.0)).text_color(tc("#f5f5f58a")),
        )
        .child(
            text("Image unavailable")
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_center()
                .text_color(tc("#f5f5f58a")),
        )
}

fn media_tile(f: Attach, w: f32, h: f32, more: usize, on_open: impl Fn() + 'static) -> impl IntoElement {
    let failed = signal(f.size == 0.0 && f.kind == "image");
    let pic = if f.kind == "video" {
        if f.poster.is_empty() {
            String::new()
        } else {
            Store::file_raw_url(&f.poster)
        }
    } else if !f.feed.is_empty() {
        Store::file_raw_url(&f.feed)
    } else {
        Store::file_raw_url(&f.path)
    };
    let svg = is_svg(&pic);
    let dur = f.duration;
    let animated = f.animated;
    let ext = f.ext.clone();
    view()
        .label(format!("Open {}", f.name))
        .w(px(w))
        .h(px(h))
        .rounded(px(10.0))
        .overflow_hidden()
        .bg(tc("#f5f5f50a"))
        .child(when(move || failed.get(), move || {
            image_unavailable(w, h).into_element()
        }))
        .when(!pic.is_empty() && svg, |v| {
            v.child(
                svg_view(&pic, "cover")
                    .absolute()
                    .w(px(w))
                    .h(px(h))
                    .into_element(),
            )
        })
        .when(!pic.is_empty() && !svg, |v| {
            v.child(
                image(pic.clone())
                    .object_fit(ResizeMode::Cover)
                    .fade_duration(0.0)
                    .on_error(move |_: &str| failed.set(true))
                    .absolute()
                    .w(px(w))
                    .h(px(h)),
            )
        })
        // The badges are one text each, padded and plated: a view around
        // a text per badge, and the play disc in a centering layer, cost a
        // gallery a dozen views.
        // a still for animated media carries Grok's GIF badge
        .when(animated, move |v| {
            v.child(
                text(if ext == "gif" { "GIF".to_owned() } else { ext.to_uppercase() })
                    .absolute()
                    .left(px(6.0))
                    .bottom(px(6.0))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(6.0))
                    .bg(LIGHT_BG)
                    .text_size(px(11.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::hsla(0.0, 0.0, 1.0, 1.0)),
            )
        })
        .when(f.kind == "video", move |v| {
            v.child(
                // the 44pt disc centered on the tile; the glyph 3pt right
                // of center (its margin, halved by the centering)
                text("▶")
                    .absolute()
                    .left(px((w - 44.0) / 2.0))
                    .top(px((h - 44.0) / 2.0))
                    .size(px(44.0))
                    .rounded(px(22.0))
                    .bg(Hsla { a: 0.65, ..LIGHT_BG })
                    .pl(px(3.0))
                    .text_center()
                    .line_height(px(44.0))
                    .number_of_lines(1)
                    .text_size(px(16.0))
                    .text_color(theme::hsla(0.0, 0.0, 1.0, 1.0)),
            )
            .when(dur > 0.0, move |v| {
                v.child(
                    text(fmt_time(dur))
                        .absolute()
                        .right(px(6.0))
                        .bottom(px(6.0))
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(6.0))
                        .bg(LIGHT_BG)
                        .text_size(px(11.0))
                        .text_color(theme::hsla(0.0, 0.0, 1.0, 1.0)),
                )
            })
        })
        .when(more > 0, move |v| {
            v.child(
                text(format!("+{more}"))
                    .absolute()
                    .left(px(0.0))
                    .top(px(0.0))
                    .w(px(w))
                    .h(px(h))
                    .bg(Hsla { a: 0.5, ..LIGHT_BG })
                    .text_center()
                    .line_height(px(h))
                    .number_of_lines(1)
                    .text_size(px(20.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::hsla(0.0, 0.0, 1.0, 1.0)),
            )
        })
        .on_press(move || {
            if !failed.get_untracked() {
                on_open();
            }
        })
}

/// `MediaGallery` — justified rows: images up to 3, videos up to 2; the
/// last tile of a kind carries "+N".
pub(crate) fn media_gallery(files: Vec<Attach>) -> impl IntoElement {
    let all = files;
    let aspect = |f: &Attach| {
        if f.width > 0.0 && f.height > 0.0 {
            f.width / f.height
        } else if f.kind == "video" {
            16.0 / 9.0
        } else {
            4.0 / 3.0
        }
    };
    let mut rows: Vec<(Vec<Attach>, usize)> = Vec::new();
    for (kind, per) in [("image", 3usize), ("video", 2usize)] {
        let list: Vec<Attach> = all.iter().filter(|f| f.kind == kind).cloned().collect();
        if !list.is_empty() {
            let more = list.len().saturating_sub(per);
            rows.push((list.into_iter().take(per).collect(), more));
        }
    }
    let ok: Vec<Attach> = all.iter().filter(|x| x.size > 0.0).cloned().collect();
    view()
        .gap(px(GAP as f32))
        .w(px(GAL_W as f32))
        .max_w(percentage(1.0))
        .children(
            rows.into_iter()
                .map(|(files, more)| {
                    if files.len() == 1 && more == 0 {
                        let f = files[0].clone();
                        let a = aspect(&f);
                        let w = (360.0f64).min(if a >= 1.0 { 360.0 } else { 320.0 * a });
                        let h = (w / a).min(320.0);
                        let ok2 = ok.clone();
                        return media_tile(f.clone(), w.round() as f32, h.round() as f32, 0, move || {
                            let at = ok2.iter().position(|x| x.path == f.path).unwrap_or(0);
                            store().open_lightbox(
                                ok2.iter().map(Store::to_light).collect(),
                                at,
                            );
                        })
                        .into_element();
                    }
                    let sum: f64 = files.iter().map(|f| aspect(f)).sum();
                    let h = 240.0f64.min((GAL_W - GAP * (files.len() - 1) as f64) / sum);
                    let last = files.len() - 1;
                    view()
                        .flex_row()
                        .gap(px(GAP as f32))
                        .children(
                            files
                                .into_iter()
                                .enumerate()
                                .map(|(i, f)| {
                                    let f2 = f.clone();
                                    let ok2 = ok.clone();
                                    media_tile(
                                        f,
                                        (aspect(&f2) * h).floor() as f32,
                                        h.round() as f32,
                                        if i == last { more } else { 0 },
                                        move || {
                                            let at = ok2
                                                .iter()
                                                .position(|x| x.path == f2.path)
                                                .unwrap_or(0);
                                            store().open_lightbox(
                                                ok2.iter().map(Store::to_light).collect(),
                                                at,
                                            );
                                        },
                                    )
                                    .into_element()
                                })
                                .collect::<Vec<_>>(),
                        )
                        .into_element()
                })
                .collect::<Vec<_>>(),
        )
}

// ——————————————————— SvgView (lightbox.tsx) ———————————————————

/// `SvgView` — an `<img>` in a transparent WebView (SMIL/CSS animate).
/// Returns `View` so callers can size/position it.
pub(crate) fn svg_view(uri: &str, fit: &str) -> View {
    match svg_page(uri, fit) {
        Some(html) => view().child(
            web_view_html(html, None)
                .scrollable(false)
                .size_full()
                .bg(AdaptiveColor::TRANSPARENT),
        ),
        None => view(),
    }
}

/// The page of `svg_view`: only the server's own files (`Store::file_raw_url`),
/// so a link in a reply cannot make the viewer fetch from another host.
fn svg_page(uri: &str, fit: &str) -> Option<String> {
    if !uri
        .strip_prefix(crate::client::backend())
        .is_some_and(|rest| rest.starts_with("/file/raw/"))
    {
        return None;
    }

    let fit = if fit == "cover" { "cover" } else { "contain" };
    let uri = html_attr_escape(uri);
    Some(format!(
        "<html><body style=\"margin:0;background:transparent;overflow:hidden\"><img src=\"{uri}\" style=\"width:100vw;height:100vh;object-fit:{fit};display:block\"></body></html>"
    ))
}

fn html_attr_escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ => escaped.push(c),
        }
    }
    escaped
}

// ——————————————————— Lightbox (`sand-media-viewer`) ———————————————————

fn round_btn(glyph: &'static str, label: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    view()
        .label(label)
        .size(px(32.0))
        .rounded(px(16.0))
        .items_center()
        .justify_center()
        .bg(AdaptiveColor::from(Hsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 0.10,
        }))
        .child(
            text(glyph)
                .text_size(px(14.0))
                .text_color(Hsla {
                    h: 0.0,
                    s: 0.0,
                    l: 1.0,
                    a: 0.85,
                }),
        )
        .on_press(move || on())
}

/// The lightbox layer — mount in the root stack (lightbox.tsx `Lightbox`).
pub(crate) fn lightbox_layer() -> impl IntoElement {
    dynamic(move || {
        let st = store();
        let items = st.light_items.get();
        if items.is_empty() {
            return view().into_element();
        }
        let idx = st.light_idx.get().min(items.len() - 1);
        let cur = items[idx].clone();
        let n = items.len();
        let zoom = st.light_zoom.get();
        let (sw, sh) = st.light_stage.get();
        // fit the media's own aspect so a player never letterboxes
        let (mut mw, mut mh) = (cur.w, cur.h);
        if mw <= 0.0 || mh <= 0.0 {
            if cur.kind == "video" {
                mw = 16.0;
                mh = 9.0;
            }
        }
        let (fw, fh) = if sw <= 0.0 || sh <= 0.0 {
            (0.0, 0.0)
        } else if mw <= 0.0 || mh <= 0.0 {
            (sw, sh)
        } else {
            let k = (sw / mw).min(sh / mh);
            ((mw * k).round(), (mh * k).round())
        };
        let media_card_border = Hsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 0.10,
        };
        // the media node per kind
        let media: AnyElement = match cur.kind.as_str() {
            "image" if is_svg(&cur.uri) => svg_view(&cur.uri, "contain")
                .w(px(fw as f32))
                .h(px(fh as f32))
                .scale(zoom as f32)
                .rounded(px(12.0))
                .overflow_hidden()
                .border(px(0.5))
                .border_color(media_card_border)
                .into_element(),
            "image" => view()
                .w(px(fw as f32))
                .h(px(fh as f32))
                .scale(zoom as f32)
                .rounded(px(12.0))
                .overflow_hidden()
                .border(px(0.5))
                .border_color(media_card_border)
                .child(
                    image(cur.uri.clone())
                        .object_fit(ResizeMode::Contain)
                        .fade_duration(0.0)
                        .size_full(),
                )
                .into_element(),
            "video" => view()
                .w(px(fw as f32))
                .h(px(fh as f32))
                .rounded(px(12.0))
                .overflow_hidden()
                .border(px(0.5))
                .border_color(media_card_border)
                .child(
                    video(cur.uri.clone())
                        .controls(true)
                        .paused(false)
                        .object_fit(ResizeMode::Contain)
                        .size_full(),
                )
                .into_element(),
            // The same drawing as inline, fitted to the window on the
            // bubble's surface.
            "diagram" => {
                match crate::mermaid::diagram(&cur.code, dark()).get() {
                    Some(Ok(g)) => {
                        let pad = 24.0f32;
                        let k = ((sw as f32 - 2.0 * pad) / g.w).min((sh as f32 - 2.0 * pad) / g.h).clamp(0.1, 3.0);
                        view()
                            .scale(zoom as f32)
                            .p(px(pad))
                            .rounded(px(12.0))
                            .bg(theme::AGENT_BUBBLE)
                            .border(px(0.5))
                            .border_color(media_card_border)
                            .child(diagram_canvas(g, Some(k)))
                            .into_element()
                    }
                    Some(Err(e)) => diagram_note(&e).into_element(),
                    _ => view().into_element(),
                }
            }
            _ => view().into_element(),
        };
        let path = cur.path.clone();
        let name = cur.name.clone();
        view()
            .absolute()
            .size_full()
            .bg(AdaptiveColor::from(Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 0.92,
            }))
            // backdrop closes
            .child(
                view()
                    .absolute()
                    .size_full()
                    .on_press(|| store().light_close()),
            )
            // the media, centered in the stage
            .child(
                view()
                    .pointer_events(PointerEvents::BoxNone)
                    .absolute()
                    .left(px(60.0))
                    .right(px(60.0))
                    .top(px(64.0))
                    .bottom(px(if n > 1 { 120.0 } else { 76.0 }))
                    .items_center()
                    .justify_center()
                    .on_layout(|b: &Bounds<Points>| {
                        store()
                            .light_stage
                            .set((b.size.width.0 as f64, b.size.height.0 as f64))
                    })
                    .child(media),
            )
            // top bar: download (not for diagrams) + close
            .child(
                view()
                    .absolute()
                    .top(px(12.0))
                    .right(px(12.0))
                    .flex_row()
                    .gap_2()
                    .when(cur.kind != "diagram", |v| {
                        v.child(round_btn("⤓", "Download", move || {
                            if !path.is_empty() {
                                store().save_file(&path);
                            }
                        }))
                    })
                    .child(round_btn("✕", "Close", || store().light_close())),
            )
            // caption + filmstrip
            .child(
                view()
                    .absolute()
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(16.0))
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        text(if n > 1 {
                            format!("{} · {} / {n}", name, idx + 1)
                        } else {
                            name.clone()
                        })
                        .number_of_lines(1)
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(Hsla {
                            h: 0.0,
                            s: 0.0,
                            l: 1.0,
                            a: 0.8,
                        }),
                    )
                    .when(n > 1, |v| {
                        v.child(view().flex_row().gap(px(6.0)).children(
                            items
                                .iter()
                                .enumerate()
                                .map(|(i, it)| {
                                    let sel = i == idx;
                                    let it = it.clone();
                                    let thumb: AnyElement = if it.kind == "image"
                                        && is_svg(&it.uri)
                                    {
                                        svg_view(&it.uri, "cover")
                                            .w(px(40.0))
                                            .h(px(40.0))
                                            .into_element()
                                    } else if it.kind == "image"
                                        || (it.kind == "video" && !it.poster.is_empty())
                                    {
                                        image(if it.kind == "video" {
                                            it.poster.clone()
                                        } else {
                                            it.uri.clone()
                                        })
                                        .object_fit(ResizeMode::Cover)
                                        .fade_duration(0.0)
                                        .w(px(40.0))
                                        .h(px(40.0))
                                        .into_element()
                                    } else {
                                        text(if it.kind == "video" { "" } else { "◇" })
                                            .text_size(px(12.0))
                                            .text_color(Hsla {
                                                h: 0.0,
                                                s: 0.0,
                                                l: 1.0,
                                                a: 0.7,
                                            })
                                            .into_element()
                                    };
                                    view()
                                        .size(px(44.0))
                                        .rounded(px(6.0))
                                        .overflow_hidden()
                                        .border(px(2.0))
                                        .border_color(if sel {
                                            Hsla {
                                                h: 0.0,
                                                s: 0.0,
                                                l: 1.0,
                                                a: 1.0,
                                            }
                                        } else {
                                            Hsla::default()
                                        })
                                        .opacity(if sel { 1.0 } else { 0.6 })
                                        .bg(AdaptiveColor::from(Hsla {
                                            h: 0.0,
                                            s: 0.0,
                                            l: 1.0,
                                            a: 0.10,
                                        }))
                                        .items_center()
                                        .justify_center()
                                        .child(thumb)
                                        .when(it.kind == "video", |v| {
                                            v.child(
                                                view()
                                                    .absolute()
                                                    .size(px(20.0))
                                                    .rounded(px(10.0))
                                                    .bg(Hsla { a: 0.65, ..LIGHT_BG })
                                                    .items_center()
                                                    .justify_center()
                                                    .child(
                                                        text("▶")
                                                            .text_size(px(9.0))
                                                            .ml(px(1.0))
                                                            .text_color(theme::hsla(
                                                                0.0, 0.0, 1.0, 1.0,
                                                            )),
                                                    ),
                                            )
                                        })
                                        .on_press(move || {
                                            let st = store();
                                            st.light_idx.set(i);
                                            st.light_zoom.set(1.0);
                                        })
                                        .into_element()
                                })
                                .collect::<Vec<_>>(),
                        ))
                    }),
            )
            // zoom pill (images and diagrams)
            .when(cur.kind == "diagram" || cur.kind == "image", move |v| {
                let zoom_by: std::rc::Rc<dyn Fn(f64)> = std::rc::Rc::new(move |k: f64| {
                    let st = store();
                    st.light_zoom.set((st.light_zoom.get_untracked() * k).clamp(0.25, 4.0));
                });
                let (z1, z2) = (zoom_by.clone(), zoom_by);
                v.child(
                    view()
                        .absolute()
                        .self_center()
                        .bottom(px(if n > 1 { 96.0 } else { 44.0 }))
                        .flex_row()
                        .gap(px(2.0))
                        .p(px(3.0))
                        .rounded(px(10.0))
                        .bg(AdaptiveColor::from(Hsla {
                            h: 0.0,
                            s: 0.0,
                            l: 0.122,
                            a: 0.9,
                        }))
                        .border(px(0.5))
                        .border_color(media_card_border)
                        .child(round_pill("−", move || z1(1.0 / 1.25)))
                        .child(round_pill("+", move || z2(1.25)))
                        .child(round_pill("⤢", move || store().light_zoom.set(1.0))),
                )
            })
            .into_element()
    })
}

fn round_pill(glyph: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    view()
        .size(px(28.0))
        .rounded(px(7.0))
        .items_center()
        .justify_center()
        .child(
            text(glyph)
                .text_size(px(14.0))
                .text_color(Hsla {
                    h: 0.0,
                    s: 0.0,
                    l: 1.0,
                    a: 0.85,
                }),
        )
        .on_press(move || on())
}

// ——————————————————— the fullscreen viewer (`FileViewer`) ———————————————————

/// A page the viewer shows in a web view: a URL (an Office editor's
/// when `.1`) or a local HTML document.
#[derive(Clone, PartialEq)]
enum WebPage {
    Url(String, bool),
    Html(String),
}

/// What the open file shows as a web page, if it does.
fn web_page() -> Option<WebPage> {
    let st = store();
    let f = st.viewing.get()?;
    let r = st.rendered.get();
    if r.as_ref().is_some_and(|r| !r.error.is_empty()) || crate::office::showing() {
        return None;
    }
    let office = r.as_ref().map(|r| r.office.clone()).unwrap_or_default();
    if !office.is_empty() {
        return Some(WebPage::Url(format!("{}{office}", crate::client::backend()), true));
    }
    match f.kind.as_str() {
        "pdf" => Some(WebPage::Url(Store::file_raw_url(&f.path), false)),
        "html" if st.html_mode.get() != "source" => Some(if f.path.starts_with('/') {
            // a file on this Mac (`rox://view`): its text, read here;
            // WebKit shows nothing for a string under a `file://` base, so
            // it gets the server's (a print preview inlines its images)
            WebPage::Html(r.and_then(|r| r.text).unwrap_or_default())
        } else {
            WebPage::Url(Store::file_raw_url(&f.path), false)
        }),
        _ => None,
    }
}

/// The viewer's web views, made once and kept over its body (a new
/// WKWebView per open cost a 30–50 ms frame): the page binds to the open
/// file, and an unused one takes no room (Print takes the largest visible
/// web view) and drops its page. Over the body, not under it: under, the
/// header's presses went to a local page.
fn kept_pages(page: Memo<Option<WebPage>>) -> impl IntoElement {
    let url = move || page.with(|p| matches!(p, Some(WebPage::Url(..))));
    let office = move || page.with(|p| matches!(p, Some(WebPage::Url(_, true))));
    let html = move || page.with(|p| matches!(p, Some(WebPage::Html(_))));
    let sized = |shown: fn(Memo<Option<WebPage>>) -> bool| {
        move || -> Length { if shown(page) { Length::Auto } else { px(0.0).into() } }
    };
    let white = || {
        AdaptiveColor::from(Hsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 1.0,
        })
    };
    let office_page = element_ref();
    OFFICE_PAGE.with(|p| p.set(Some(office_page.clone())));
    let place = |v: WebView| v.absolute().top(px(52.0)).left(px(0.0)).right(px(0.0)).bottom(px(0.0));
    view()
        .absolute()
        .size_full()
        .pointer_events(PointerEvents::BoxNone)
        .child(
            place(web_view(move || {
                page.with(|p| match p {
                    Some(WebPage::Url(u, _)) => u.clone(),
                    _ => "about:blank".to_owned(),
                })
            }))
            .element_ref(&office_page)
            .w(sized(|p| p.with(|p| matches!(p, Some(WebPage::Url(..))))))
            .h(sized(|p| p.with(|p| matches!(p, Some(WebPage::Url(..))))))
            .opacity(move || if url() { 1.0f32 } else { 0.0 })
            // shows through the (transparent) WKWebView until the page
            // paints: light is the Office editor page's own `#f5f5f5` body.
            .bg(move || {
                if office() {
                    theme::tone(
                        Hsla::from_str("#f5f5f5").unwrap_or_default(),
                        Hsla::from_str("#161616").unwrap_or_default(),
                    )
                } else {
                    white()
                }
            }),
        )
        .child(
            place(web_view_html(
                move || {
                    page.with(|p| match p {
                        Some(WebPage::Html(h)) => h.clone(),
                        _ => String::new(),
                    })
                },
                Some(crate::client::backend()),
            ))
            .w(sized(|p| p.with(|p| matches!(p, Some(WebPage::Html(_))))))
            .h(sized(|p| p.with(|p| matches!(p, Some(WebPage::Html(_))))))
            .opacity(move || if html() { 1.0f32 } else { 0.0 })
            .bg(white()),
        )
}

/// The viewer overlay — mount in the root stack (`viewing` set).
pub(crate) fn layer() -> impl IntoElement {
    let page = computed(web_page);
    view()
        .absolute()
        .size_full()
        .pointer_events(PointerEvents::BoxNone)
        .child(dynamic(move || {
        let st = store();
        let Some(f) = st.viewing.get() else {
            return view().into_element();
        };
        // a web page shows in `kept_pages`, over the body: the body is
        // left empty
        let on_web = page.with(Option::is_some);
        let r = st.rendered.get();
        let f2 = f.clone();
        let name = f.name.clone();
        let rows_note = if f.kind == "table" {
            st.cur_sheet()
                .map(|sh| format!("{} rows", sh.rows.len().saturating_sub(1)))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let sheets: Vec<Sheet> = r.as_ref().map(|r| r.sheets.clone()).unwrap_or_default();
        let office = r.as_ref().map(|r| r.office.clone()).unwrap_or_default();
        let sheet_i = st.sheet_idx.get();
        let html_mode = st.html_mode.get();
        // body per kind
        let body: AnyElement = if crate::office::showing() && st.office.is_word.get() {
            crate::office::docx::view::word_view().into_element()
        } else if crate::office::showing() {
            crate::office::xlsx::edit::sheet_view().into_element()
        } else if on_web {
            view().flex_1().pointer_events(PointerEvents::None).into_element()
        } else if r.as_ref().map(|r| !r.error.is_empty()).unwrap_or(false) {
            unavailable(&f, "Couldn't load preview", &r.unwrap().error).into_element()
        } else {
            match f.kind.as_str() {
                "image" => image(Store::file_raw_url(&f.path))
                    .object_fit(ResizeMode::Contain)
                    .fade_duration(0.0)
                    .flex_1()
                    .m(px(24.0))
                    .into_element(),
                "video" => video(Store::file_raw_url(&f.path))
                    .controls(true)
                    .paused(false)
                    .object_fit(ResizeMode::Contain)
                    .flex_1()
                    .m(px(24.0))
                    .into_element(),
                "audio" => audio_player_view(&f).into_element(),
                // (the preview is a web page: `kept_pages`)
                "html" => match r.as_ref().map(|r| r.lines.clone()) {
                    Some(lines) if !lines.is_empty() => code_view(lines).into_element(),
                    _ => view().into_element(),
                },
                "markdown" | "docx" => match r.as_ref().and_then(|r| r.text.clone()) {
                    Some(t) => scroll()
                        .flex_1()
                        .child(
                            view()
                                .self_center()
                                .w(percentage(1.0))
                                .max_w(px(760.0))
                                .p(px(24.0))
                                .child(crate::md::markdown(move || t.clone(), || false, || false)),
                        )
                        .into_element(),
                    None => view().into_element(),
                },
                "json" => {
                    if r.as_ref().map(|r| r.has_json).unwrap_or(false) {
                        let v = r.as_ref().map(|r| r.json.clone()).unwrap_or_default();
                        scroll()
                            .flex_1()
                            .child(json_node(None, &v, 0).p(px(16.0)))
                            .into_element()
                    } else if let Some(lines) = r.as_ref().map(|r| r.lines.clone()) {
                        code_view(lines).into_element()
                    } else {
                        view().into_element()
                    }
                }
                "text" => match r.as_ref().map(|r| r.lines.clone()) {
                    Some(lines) => code_view(lines).into_element(),
                    None => view().into_element(),
                },
                "table" => match r.as_ref().map(|r| r.sheets.clone()) {
                    Some(sheets) if !sheets.is_empty() => table_view(sheets).into_element(),
                    _ => view().into_element(),
                },
                _ => unavailable(
                    &f,
                    "Preview unavailable",
                    "Previews aren't available for this file type. Download the file to open it on your computer.",
                )
                .into_element(),
            }
        };
        let code_bg = f.kind == "text"
            || f.kind == "json"
            || (f.kind == "html" && html_mode == "source");
        view()
            .absolute()
            .size_full()
            .child(
                // header: drag strip, centered title, tabs/toggles, buttons
                view()
                    .bg(tc("#161616"))
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(52.0))
                    .pl(px(84.0))
                    .pr(px(12.0))
                    .border_b(px(0.5))
                    .border_b_color(tc("#f5f5f51a"))
                    .child(
                        // title centered against the window (an overlay so
                        // the traffic lights and buttons don't skew it)
                        view()
                            .pointer_events(PointerEvents::None)
                            .absolute()
                            .left(px(84.0))
                            .right(px(84.0))
                            .top(px(0.0))
                            .bottom(px(0.0))
                            .items_center()
                            .justify_center()
                            .child(
                                text(name)
                                    .number_of_lines(1)
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(tc("#f5f5f5")),
                            )
                            .when(!rows_note.is_empty(), |v| {
                                v.child(
                                    text(rows_note)
                                        .text_size(px(11.0))
                                        .line_height(px(14.0))
                                        .text_color(tc("#f5f5f58f")),
                                )
                            }),
                    )
                    .child(view().flex_1())
                    // spreadsheet: sheet tabs in the header (the office view
                    // has its own)
                    .when(f.kind == "table" && office.is_empty() && sheets.len() > 1, |v| {
                        v.child(view().flex_row().gap_1().mr_1().children(
                            sheets
                                .iter()
                                .enumerate()
                                .map(|(i, sh)| {
                                    let sel = i == sheet_i;
                                    view()
                                        .h(px(24.0))
                                        .px(px(10.0))
                                        .rounded(px(7.0))
                                        .justify_center()
                                        .bg(if sel {
                                            tc("#f5f5f514")
                                        } else {
                                            AdaptiveColor::TRANSPARENT
                                        })
                                        .child(
                                            text(sh.name.clone())
                                                .text_size(px(13.0))
                                                .line_height(px(18.0))
                                                .text_color(if sel {
                                                    tc("#f5f5f5")
                                                } else {
                                                    tc("#f5f5f58f")
                                                }),
                                        )
                                        .on_press(move || {
                                            let st = store();
                                            st.sheet_idx.set(i);
                                            st.sel_a.set(None);
                                            st.sel_b.set(None);
                                        })
                                        .into_element()
                                })
                                .collect::<Vec<_>>(),
                        ))
                    })
                    // HTML: Preview / Source toggle
                    .when(f.kind == "html", |v| {
                        v.child(view().flex_row().gap_1().mr_1().children(
                            ["preview", "source"]
                                .iter()
                                .map(|m| {
                                    let m = *m;
                                    let sel = html_mode == m;
                                    view()
                                        .label(m)
                                        .h(px(24.0))
                                        .px(px(10.0))
                                        .rounded(px(7.0))
                                        .justify_center()
                                        .bg(if sel {
                                            tc("#f5f5f514")
                                        } else {
                                            AdaptiveColor::TRANSPARENT
                                        })
                                        .child(
                                            text(if m == "preview" { "Preview" } else { "Source" })
                                                .text_size(px(13.0))
                                                .line_height(px(18.0))
                                                .text_color(if sel {
                                                    tc("#f5f5f5")
                                                } else {
                                                    tc("#f5f5f58f")
                                                }),
                                        )
                                        .on_press(move || {
                                            store().html_mode.set(m.to_owned())
                                        })
                                        .into_element()
                                })
                                .collect::<Vec<_>>(),
                        ))
                    })
                    // a mini app's code: the version shown, a menu of the others
                    .child(version_chip(f.clone()))
                    // ⌘K: the file's actions
                    .child(
                        view()
                            .label("File actions")
                            .size(px(32.0))
                            .rounded(px(16.0))
                            .items_center()
                            .justify_center()
                            .bg(tc("#f5f5f50f"))
                            .child(text("⋯").text_size(px(15.0)).text_color(tc("#f5f5f5a8")))
                            .on_press(viewer_actions),
                    )
                    .child(
                        view()
                            .label("Download")
                            .size(px(32.0))
                            .rounded(px(16.0))
                            .items_center()
                            .justify_center()
                            .bg(tc("#f5f5f50f"))
                            .child(
                                text("⤓")
                                    .text_size(px(14.0))
                                    .text_color(tc("#f5f5f5a8")),
                            )
                            .on_press(move || store().save_file(&f2.path)),
                    )
                    .child(
                        view()
                            .label("Close")
                            .size(px(32.0))
                            .rounded(px(16.0))
                            .items_center()
                            .justify_center()
                            .bg(tc("#f5f5f50f"))
                            .child(
                                text("✕")
                                    .text_size(px(13.0))
                                    .text_color(tc("#f5f5f5a8")),
                            )
                            .on_press(|| {
                                if crate::office::may_close() {
                                    store().viewing.set(None);
                                }
                            }),
                    ),
            )
            .child(
                view()
                    .flex_1()
                    .when(on_web, |v| v.pointer_events(PointerEvents::BoxNone))
                    .bg(if on_web {
                        AdaptiveColor::TRANSPARENT
                    } else if code_bg {
                        // `#212121` is outside `tc`'s light table (it would
                        // stay dark in light mode): white on light.
                        theme::tone(
                            theme::hsla(0.0, 0.0, 1.0, 1.0),
                            Hsla::from_str("#212121").unwrap_or_default(),
                        )
                    } else {
                        tc("#161616")
                    })
                    .child(body),
            )
            .into_element()
    }))
        .child(kept_pages(page))
}

/// `Unavailable`.
fn unavailable(f: &Attach, title: &str, detail: &str) -> impl IntoElement {
    let path = f.path.clone();
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .p(px(24.0))
        .child(doc_glyph(&f.variant))
        .child(
            text(title.to_owned())
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tc("#f5f5f5")),
        )
        .child(
            text(detail.to_owned())
                .text_size(px(13.0))
                .line_height(px(18.0))
                .text_center()
                .max_w(px(360.0))
                .text_color(tc("#f5f5f5a8")),
        )
        .child(
            view()
                .mt_1()
                .h(px(30.0))
                .px(px(14.0))
                .rounded(px(15.0))
                .justify_center()
                .bg(tc("#f5f5f514"))
                .child(
                    text("Download")
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(tc("#f5f5f5")),
                )
                .on_press(move || store().save_file(&path)),
        )
}

/// `CodeView` — gutter + Shiki lines (`[content, dark, light]` spans).
fn code_view(lines: Vec<Vec<(String, String, String)>>) -> impl IntoElement {
    let gutter = lines.len().to_string().len() as f32 * 8.0 + 16.0;
    scroll().flex_1().child(
        scroll().horizontal().child(
            view()
                .py(px(12.0))
                .pr(px(24.0))
                .children(
                    lines
                        .into_iter()
                        .enumerate()
                        .map(|(i, line)| {
                            view()
                                .flex_row()
                                .min_h(px(20.0))
                                .child(
                                    text(format!("{}", i + 1))
                                        .w(px(gutter))
                                        .text_right()
                                        .pr(px(12.0))
                                        .font_family("Menlo")
                                        .text_size(px(12.0))
                                        .line_height(px(20.0))
                                        .text_color(tc("#f5f5f561")),
                                )
                                .child(
                                    text("")
                                        .font_family("Menlo")
                                        .text_size(px(13.0))
                                        .line_height(px(20.0))
                                        .text_color(tc("#f5f5f5"))
                                        .children(
                                            line.into_iter()
                                                .map(|(t, d, l)| {
                                                    text(t)
                                                        .font_family("Menlo")
                                                        .text_size(px(13.0))
                                                        .line_height(px(20.0))
                                                        .text_color(if d.is_empty() && l.is_empty() {
                                                            tc("#f5f5f5")
                                                        } else {
                                                            AdaptiveColor {
                                                                light: Hsla::from_str(&l)
                                                                    .unwrap_or_default(),
                                                                dark: Hsla::from_str(&d)
                                                                    .unwrap_or_default(),
                                                            }
                                                        })
                                                        .into_element()
                                                })
                                                .collect::<Vec<_>>(),
                                        ),
                                )
                                .into_element()
                        })
                        .collect::<Vec<_>>(),
                ),
        ),
    )
}

// ——————————————————— JsonNode ———————————————————

fn j_key() -> AdaptiveColor {
    AdaptiveColor {
        light: Hsla::from_str("rgb(170,99,0)").unwrap_or_default(),
        dark: Hsla::from_str("#e0a54a").unwrap_or_default(),
    }
}
fn j_str() -> AdaptiveColor {
    AdaptiveColor {
        light: Hsla::from_str("rgb(0,133,74)").unwrap_or_default(),
        dark: Hsla::from_str("#3ecf8e").unwrap_or_default(),
    }
}
fn j_lit() -> AdaptiveColor {
    AdaptiveColor {
        light: Hsla::from_str("rgb(117,71,208)").unwrap_or_default(),
        dark: Hsla::from_str("#b69cff").unwrap_or_default(),
    }
}

/// Grok's collapsible JSON tree — monospace 12/20.4, keys amber, strings
/// green, literals violet, `{n keys}`/`[n items]` summaries.
fn json_node(k: Option<String>, v: &serde_json::Value, depth: usize) -> View {
    let open = signal(depth < 3);
    let entries: Vec<(String, serde_json::Value)> = match v {
        serde_json::Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, x)| (i.to_string(), x.clone()))
            .collect(),
        serde_json::Value::Object(o) => {
            o.iter().map(|(k, x)| (k.clone(), x.clone())).collect()
        }
        _ => Vec::new(),
    };
    let is_obj = !entries.is_empty() || v.is_object() || v.is_array();
    let summary = if v.is_array() {
        format!("[{} {}]", entries.len(), if entries.len() == 1 { "item" } else { "items" })
    } else {
        format!("{{{} {}}}", entries.len(), if entries.len() == 1 { "key" } else { "keys" })
    };
    let lit = match v {
        serde_json::Value::String(s) => serde_json::to_string(s).unwrap_or_default(),
        serde_json::Value::Null => "null".to_owned(),
        x => x.to_string(),
    };
    let mono = |t: String, c: AdaptiveColor| {
        text(t)
            .font_family("Menlo")
            .text_size(px(12.0))
            .line_height(px(20.4))
            .text_color(c)
    };
    let caret = move || if is_obj {
        if open.get() { "⌄" } else { "›" }
    } else {
        ""
    };
    view()
        .pl(px(if depth > 0 { 17.0 } else { 0.0 }))
        .child(
            view()
                .flex_row()
                .items_center()
                .child(
                    text(move || caret().to_owned())
                        .font_family("Menlo")
                        .text_size(px(12.0))
                        .line_height(px(20.4))
                        .text_color(tc("#f5f5f58f"))
                        .w(px(12.0)),
                )
                .when_some(k, |v, k| {
                    v.child(mono(k, j_key()))
                        .child(mono(": ".to_owned(), tc("#f5f5f566")))
                })
                .child(mono(
                    if is_obj { summary } else { lit },
                    if is_obj {
                        tc("#f5f5f58f")
                    } else if v.is_string() {
                        j_str()
                    } else {
                        j_lit()
                    },
                ))
                .on_press(move || {
                    if is_obj {
                        open.update(|o| *o = !*o);
                    }
                }),
        )
        .child(when(move || is_obj && open.get(), move || {
            view()
                .border_l(px(0.5))
                .border_l_color(tc("#f5f5f51a"))
                .ml(px(5.0))
                .children(
                    entries
                        .iter()
                        .map(|(k, x)| json_node(Some(k.clone()), x, depth + 1).into_element())
                        .collect::<Vec<_>>(),
                )
                .into_element()
        }))
}

// ——————————————————— TableView ———————————————————

/// Grok's spreadsheet viewer: hint bar, row numbers, cells 24pt/13px on a
/// 0.5pt grid, bold kept; a selection the arrows move, Shift extends and
/// ⌘C copies as TSV.
fn table_view(sheets: Vec<Sheet>) -> impl IntoElement {
    let grid = tc("#f5f5f51a");
    let sh = sheets
        .get(store().sheet_idx.get().min(sheets.len().saturating_sub(1)))
        .cloned()
        .unwrap_or_default();
    let cols = sh.rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
    let width = |c: usize| -> f32 {
        if let Some(w) = sh.widths.get(c) {
            if *w > 0.0 {
                return (w * 7.0).round().clamp(48.0, 320.0) as f32;
            }
        }
        let longest = sh
            .rows
            .iter()
            .take(200)
            .map(|r| r.get(c).map(|s| s.len()).unwrap_or(0))
            .max()
            .unwrap_or(1)
            .max(1);
        ((longest as f64 * 8.0 + 22.0).clamp(56.0, 260.0)) as f32
    };
    view()
        .flex_1()
        // hint bar
        .child(when(move || store().hint_open.get(), move || {
            view()
                .flex_row()
                .items_center()
                .py_2()
                .px_3()
                .border_b(px(0.5))
                .border_b_color(grid)
                .child(
                    text("Click a cell to select it. Move with the arrow keys, extend with Shift, copy with ⌘C.")
                        .flex_1()
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(tc("#f5f5f58f")),
                )
                .child(
                    view()
                        .label("Hide hint")
                        .p_1()
                        .child(
                            text("⌄")
                                .text_size(px(12.0))
                                .text_color(tc("#f5f5f58f")),
                        )
                        .on_press(|| store().hint_open.set(false)),
                )
                .into_element()
        }))
        .child(
            scroll().flex_1().child(
                scroll().horizontal().child(
                    view()
                        .p(px(12.0))
                        .child(
                            view()
                                .border_t(px(0.5))
                                .border_l(px(0.5))
                                .border_color(grid)
                                .children(
                                    sh.rows
                                        .iter()
                                        .enumerate()
                                        .map(|(r, row)| {
                                            let row = row.clone();
                                            view().flex_row().child(
                                                view()
                                                    .w(px(34.0))
                                                    .h(px(24.0))
                                                    .justify_center()
                                                    .pr_2()
                                                    .border_r(px(0.5))
                                                    .border_b(px(0.5))
                                                    .border_color(grid)
                                                    .bg(tc("#f5f5f505"))
                                                    .child(
                                                        text(format!("{}", r + 1))
                                                            .text_right()
                                                            .text_size(px(12.0))
                                                            .text_color(tc("#f5f5f58f")),
                                                    ),
                                            ).children(
                                                (0..cols)
                                                    .map(|c| {
                                                        let cell = row.get(c).cloned().unwrap_or_default();
                                                        let bold = sh
                                                            .bold
                                                            .get(r)
                                                            .and_then(|b| b.get(c))
                                                            .copied()
                                                            .unwrap_or(false);
                                                        view()
                                                            .w(px(width(c)))
                                                            .h(px(24.0))
                                                            .justify_center()
                                                            .px_2()
                                                            .border_r(px(0.5))
                                                            .border_b(px(0.5))
                                                            .border_color(grid)
                                                            .bg(move || {
                                                                if store().in_sel(r, c) {
                                                                    tc("#116dd21f")
                                                                } else {
                                                                    AdaptiveColor::TRANSPARENT
                                                                }
                                                            })
                                                            .child(
                                                                text(cell)
                                                                    .number_of_lines(1)
                                                                    .text_size(px(13.0))
                                                                    .font_weight(if bold {
                                                                        FontWeight::SEMIBOLD
                                                                    } else {
                                                                        FontWeight::NORMAL
                                                                    })
                                                                    .text_color(tc("#f5f5f5")),
                                                            )
                                                            .child(when(
                                                                move || {
                                                                    store().sel_b.get()
                                                                        == Some((r, c))
                                                                },
                                                                || {
                                                                    view()
                                                                        .absolute()
                                                                        .inset(px(-1.0))
                                                                        .border(px(2.0))
                                                                        .border_color(
                                                                            tc("#116dd2"),
                                                                        )
                                                                        .into_element()
                                                                },
                                                            ))
                                                            .on_press(move || {
                                                                let st = store();
                                                                st.sel_a.set(Some((r, c)));
                                                                st.sel_b.set(Some((r, c)));
                                                            })
                                                            .into_element()
                                                    })
                                                    .collect::<Vec<_>>(),
                                            ).into_element()
                                        })
                                        .collect::<Vec<_>>(),
                                ),
                        ),
                ),
            ),
        )
}

// ——————————————————— AudioPlayer (the viewer's 53pt bar) ———————————————————

fn audio_player_view(f: &Attach) -> impl IntoElement {
    let a = Audio::new(f);
    let a2 = a.clone();
    let a2b = a.clone();
    let a3 = a.clone();
    let a3b = a.clone();
    let a3c = a.clone();
    let a4 = a.clone();
    let a5 = a.clone();
    let a5b = a.clone();
    let a6 = a.clone();
    let track_w = signal(351.0f32);
    let pct = move || {
        let d = a4.dur.get();
        if d > 0.0 {
            (a4.time.get() / d).min(1.0) as f32
        } else {
            0.0
        }
    };
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .p(px(24.0))
        .child(a.node())
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .w(percentage(1.0))
                .max_w(px(561.0))
                .h(px(53.0))
                .px(px(12.0))
                .rounded(px(12.0))
                .border(px(0.5))
                .border_color(tc("#f5f5f51f"))
                .bg(tc("#1f1f1f"))
                .child(
                    view()
                        .label("Play/Pause")
                        .size(px(24.0))
                        .rounded(px(12.0))
                        .items_center()
                        .justify_center()
                        .child(dynamic(move || {
                            text(if a2.playing() { "❚❚" } else { "▶" })
                                .text_size(px(14.0))
                                .text_color(tc("#f5f5f5"))
                                .into_element()
                        }))
                        .on_press(move || a2b.toggle()),
                )
                .child(
                    view()
                        .label("Seek")
                        .flex_1()
                        .h(px(16.0))
                        .justify_center()
                        .on_layout(move |b: &Bounds<Points>| track_w.set(b.size.width.0))
                        .on_mouse_move(move |e: &nsh_core::MouseEvent| {
                            a3.seek_at(e.position.x.0, track_w.get_untracked());
                        })
                        .on_press_down(move || a3b.scrub_on(track_w.get_untracked()))
                        .on_press_up(move || a3c.scrub_off())
                        .child(
                            view()
                                .h(px(4.0))
                                .rounded(px(2.0))
                                .bg(tc("#f5f5f51a")),
                        )
                        .child(
                            view()
                                .absolute()
                                .left(px(0.0))
                                .w(move || percentage(pct().clamp(0.0, 1.0)))
                                .h(px(4.0))
                                .rounded(px(2.0))
                                .bg(tc("#f5f5f5")),
                        )
                        .child(
                            view()
                                .absolute()
                                .left(move || px(track_w.get() * pct() - 6.0))
                                .size(px(12.0))
                                .rounded(px(6.0))
                                .bg(tc("#f5f5f5")),
                        ),
                )
                .child(dynamic(move || {
                    text(format!(
                        "{} / {}",
                        fmt_time(a4.time.get()),
                        fmt_time(a4.dur.get())
                    ))
                    .text_size(px(11.0))
                    .text_color(tc("#f5f5f58f"))
                    .into_element()
                }))
                .child(
                    view()
                        .label("Playback speed")
                        .w(px(38.0))
                        .h(px(24.0))
                        .rounded(px(6.0))
                        .items_center()
                        .justify_center()
                        .child(dynamic(move || {
                            text(format!("{}x", a5.rate.get()))
                                .text_size(px(11.0))
                                .text_color(tc("#f5f5f58f"))
                                .into_element()
                        }))
                        .on_press(move || a5b.next_rate()),
                )
                .child(
                    view()
                        .label("Mute")
                        .size(px(24.0))
                        .rounded(px(6.0))
                        .items_center()
                        .justify_center()
                        .child(dynamic(move || {
                            text(if a6.muted.get() { "🔇︎" } else { "🔊︎" })
                                .text_size(px(13.0))
                                .text_color(tc("#f5f5f5a8"))
                                .into_element()
                        }))
                        .on_press(move || a.muted.update(|m| *m = !*m)),
                ),
        )
}

// ——————————————— the viewer's versions and actions (⌘K) ———————————————

thread_local! {
    /// The viewer's Office editor page, while one shows (its Save).
    static OFFICE_PAGE: std::cell::Cell<Option<ElementRef>> = const { std::cell::Cell::new(None) };
}

/// The open file's versions: (its key — a mini app's name, a file's own
/// path —, versions oldest first, the current one). Filled by
/// `Store::view_code` and, for a file, `load_file_versions`.
pub(crate) fn viewer_versions() -> Signal<(String, Vec<i64>, i64)> {
    store().viewer_versions
}

/// A saved file's version path (`.versions/<path>/v<n>.<ext>`): the file's
/// own path and the number.
fn file_version(path: &str) -> Option<(String, i64)> {
    let rest = path.strip_prefix(".versions/")?;
    let (rel, last) = rest.rsplit_once('/')?;
    let n = last.strip_prefix('v')?.split('.').next()?.parse().ok()?;
    Some((rel.to_owned(), n))
}

/// What the viewer shows, versioned: its key and the version shown (the
/// current one when the path names none).
fn shown_version(f: &Attach) -> Option<(String, i64)> {
    let (key, v) = if let Some(key) = f.path.strip_prefix("app:") {
        match key.split_once("@v") {
            Some((n, v)) => (n.to_owned(), v.parse().ok()),
            None => (key.to_owned(), None),
        }
    } else if let Some((rel, n)) = file_version(&f.path) {
        (rel, Some(n))
    } else {
        (f.path.clone(), None)
    };
    let (shown, _, head) = viewer_versions().get_untracked();
    let v = v.unwrap_or(if shown == key { head } else { 0 });
    Some((key, v))
}

/// The file (or mini app code) at version `v` of `key`, for the viewer.
fn open_version(f: &Attach, key: &str, v: i64, head: i64) {
    let st = store();
    if f.path.starts_with("app:") {
        let k = if v == head { key.to_owned() } else { format!("{key}@v{v}") };
        st.view_code(&k, &f.name);
        return;
    }
    let path = if v == head {
        key.to_owned()
    } else {
        let ext = key.rsplit_once('.').map(|(_, e)| e).unwrap_or("bin");
        format!(".versions/{key}/v{v}.{ext}")
    };
    st.open_file(&Attach { path, ..f.clone() });
}

/// Fetches the versions of the reply file at `path` (`GET /file/versions`).
pub(crate) fn load_file_versions(path: &str) {
    let key = file_version(path).map(|(rel, _)| rel).unwrap_or_else(|| path.to_owned());
    spawn(async move {
        let url = format!("/file/versions?path={}", crate::client::urlenc(&key));
        let Some(v) = crate::client::get(&url).await else { return };
        let vs: Vec<i64> = v
            .get("versions")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|x| x.get("v")?.as_i64()).collect())
            .unwrap_or_default();
        let head = v.get("v").and_then(|x| x.as_i64()).unwrap_or(0);
        viewer_versions().set((key, vs, head));
    });
}

/// The version menu's rows for the viewer's file.
fn version_items(f: &Attach) -> Vec<crate::store::CtxItem> {
    let Some((key, shown)) = shown_version(f) else { return Vec::new() };
    let (k, vs, head) = viewer_versions().get_untracked();
    if k != key {
        return Vec::new();
    }
    vs.iter()
        .rev()
        .map(|v| {
            let label = if *v == head { format!("v{v} · current") } else { format!("v{v}") };
            crate::store::CtxItem::row(&label, "history", "viewer-version", v.to_string()).on(*v == shown)
        })
        .collect()
}

/// A version picked from the viewer's menu.
pub(crate) fn viewer_open_version(v: i64) {
    let st = store();
    let Some(f) = st.viewing.get_untracked() else { return };
    let Some((key, _)) = shown_version(&f) else { return };
    let (k, _, head) = viewer_versions().get_untracked();
    if k == key {
        open_version(&f, &key, v, head);
    }
}

/// ⌘⇧[ / ⌘⇧]: the viewer's file one version older (-1) or newer.
pub(crate) fn viewer_version_step(d: i64) {
    let st = store();
    let Some(f) = st.viewing.get_untracked() else { return };
    let Some((key, shown)) = shown_version(&f) else { return };
    let (k, vs, head) = viewer_versions().get_untracked();
    if k != key || vs.is_empty() {
        return;
    }
    let at = vs.iter().position(|v| *v == shown).unwrap_or(vs.len() - 1) as i64;
    let next = vs[(at + d).clamp(0, vs.len() as i64 - 1) as usize];
    if next != shown {
        open_version(&f, &key, next, head);
    }
}

/// Whether the viewer shows an Office document in its editor.
fn office_open() -> bool {
    store().rendered.get_untracked().is_some_and(|r| !r.office.is_empty())
}

/// ⌘S in the viewer: an Office document saves through its editor — the
/// file gets the edits, and the save becomes its next version — then the
/// version list catches up.
pub(crate) fn viewer_save() {
    let st = store();
    let Some(f) = st.viewing.get_untracked() else { return };
    if crate::office::showing() {
        crate::office::xlsx::edit::commit_editing();
        return crate::office::save_asking();
    }
    if !office_open() {
        return;
    }
    if let Some(page) = OFFICE_PAGE.with(std::cell::Cell::get) {
        page.inject_java_script("window.roxSave && window.roxSave(); true;");
    }
    let path = f.path.clone();
    spawn(async move {
        timer(std::time::Duration::from_millis(1500)).await;
        load_file_versions(&path);
        store().refresh_thread_apps();
    });
}

/// What Print can do with the open file: text (code, markdown, JSON,
/// HTML source) or an image.
fn printable(f: &Attach) -> Option<&'static str> {
    match f.kind.as_str() {
        "image" => Some("image"),
        "text" | "markdown" | "json" | "html" | "docx" => Some("text"),
        _ => None,
    }
}

/// ⌘P in the viewer: the system print panel for the file's text or
/// image; a file Rox can't lay out (PDF, slides, sheets) opens in its own
/// app, which prints it.
pub(crate) fn viewer_print() {
    let st = store();
    let Some(f) = st.viewing.get_untracked() else { return };
    // a native Word document prints as it is now, edits included
    if crate::office::showing() && st.office.is_word.get_untracked() {
        return crate::office::docx::view::print();
    }
    match printable(&f) {
        Some("image") => crate::keys::print("image", Store::file_raw_url(&f.path), &f.name),
        // an html file prints laid out, as the preview shows it
        Some(_) if f.kind == "html" && st.html_mode.get_untracked() != "source" => {
            let r = st.rendered.get_untracked();
            let html = r.as_ref().and_then(|r| r.text.clone()).unwrap_or_else(|| {
                r.as_ref()
                    .map(|r| r.lines.iter().map(|l| l.iter().map(|(t, _, _)| t.as_str()).collect::<String>()).collect::<Vec<_>>().join("\n"))
                    .unwrap_or_default()
            });
            if !html.is_empty() {
                crate::keys::print("html", html, &f.name);
            }
        }
        Some(_) => {
            let r = st.rendered.get_untracked();
            let text = r.as_ref().and_then(|r| r.text.clone()).unwrap_or_else(|| {
                r.as_ref()
                    .map(|r| {
                        r.lines
                            .iter()
                            .map(|l| l.iter().map(|(t, _, _)| t.as_str()).collect::<String>())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default()
            });
            if !text.is_empty() {
                crate::keys::print("text", text, &f.name);
            }
        }
        _ => st.open_externally(&f.path),
    }
}

/// ⌘K in the viewer: the file's actions — its versions (a mini app's
/// code), Print, Download, open in its own app, copy its name, close.
pub(crate) fn viewer_actions() {
    use crate::store::CtxItem;
    let st = store();
    let Some(f) = st.viewing.get_untracked() else { return };
    let mut items = Vec::new();
    let versions = version_items(&f);
    if versions.len() > 1 {
        items.push(CtxItem::parent("Versions", "history", versions));
        items.push(CtxItem::separator());
    }
    if office_open() {
        items.push(CtxItem::row("Save as New Version", "download", "viewer-save", String::new()));
    }
    if untracked(crate::office::showing) {
        items.push(CtxItem::row("Save", "download", "viewer-save", String::new()));
    }
    // this file in the other view (native or the web editor), to compare them
    if crate::office::native_ext(&f.path)
        && let Some(switch) = crate::office::switch_label(&f.path)
    {
        let icon = if crate::office::is_word(&f.path) { "document" } else { "spreadsheet" };
        items.push(CtxItem::row(switch, icon, "office-view", String::new()));
    }
    if printable(&f).is_some() {
        items.push(CtxItem::row("Print…", "document", "viewer-print", String::new()));
    }
    items.push(CtxItem::row("Download", "download", "viewer-download", String::new()));
    if !f.path.starts_with("app:") {
        items.push(CtxItem::row("Open in Its App", "popout", "viewer-open", String::new()));
    }
    items.push(CtxItem::row("Copy File Name", "copy", "copy", f.name.clone()));
    items.push(CtxItem::separator());
    items.push(CtxItem::row("Close", "close", "viewer-close", String::new()));
    let win = window();
    crate::ctx::open_at(win.width.0 - 240.0, 56.0, items);
}

/// The viewer header's version chip for a mini app's code ("v2 ▾"; its
/// menu lists every version) — nothing for other files or a single
/// version.
fn version_chip(f: Attach) -> impl IntoElement {
    dynamic(move || {
        let (app, vs, _) = viewer_versions().get();
        let Some((name, shown)) = shown_version(&f) else { return view().into_element() };
        if app != name || vs.len() < 2 {
            return view().into_element();
        }
        let f2 = f.clone();
        view()
            .label("Versions")
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .h(px(28.0))
            .px_3()
            .rounded(px(14.0))
            .bg(tc("#f5f5f50f"))
            .child(text(format!("v{shown}")).text_size(px(13.0)).text_color(tc("#f5f5f5")))
            .child(text("▾").text_size(px(10.0)).text_color(tc("#f5f5f58f")))
            .on_press(move || {
                let win = window();
                crate::ctx::open_at(win.width.0 - 300.0, 52.0, version_items(&f2));
            })
            .into_element()
    })
}
#[cfg(test)]
mod tests {
    use super::{Store, svg_page};

    fn raw(path: &str) -> String {
        format!("{}/file/raw/{path}", crate::client::backend())
    }

    #[test]
    fn escapes_uri_in_html_attributes() {
        let page = svg_page(&raw("a\"><script>alert(1)</script><img b=\".svg"), "contain")
            .expect("a server file is accepted");
        assert!(!page.contains("<script"));
        assert!(!page.contains("\"><script"));
        assert!(page.contains("&quot;&gt;&lt;script&gt;"));
        let expected = svg_page(&raw("a.svg"), "contain").expect("a server file is accepted");
        assert_eq!(page.matches('"').count(), expected.matches('"').count());
    }

    #[test]
    fn escapes_single_quotes_and_ampersands() {
        let page = svg_page(&raw("a'b&c.svg"), "contain").expect("a server file is accepted");
        assert!(page.contains("a&#39;b&amp;c.svg"));
    }

    #[test]
    fn rejects_everything_but_the_servers_files() {
        for uri in [
            "javascript:alert(1)//.svg",
            "data:image/svg+xml,<svg/onload=alert(1)>.svg",
            "file:///etc/x.svg",
            "https://x/a.svg",
            "http://127.0.0.1:8787/other/a.svg",
            "http://127.0.0.1:87870/file/raw/a.svg",
            "http://127.0.0.1:8787@evil.example/file/raw/a.svg",
            " http://127.0.0.1:8787/file/raw/a.svg",
            "",
        ] {
            assert!(svg_page(uri, "contain").is_none(), "accepted {uri:?}");
        }
    }

    #[test]
    fn accepts_the_servers_raw_files() {
        assert!(svg_page(&Store::file_raw_url("a b/c.svg"), "contain").is_some());
        assert!(svg_page(&raw("dir/x.svg"), "cover").is_some());
    }

    #[test]
    fn whitelists_object_fit() {
        let page = svg_page(&raw("a.svg"), "cover;}</style><script>").expect("a server file");
        assert!(page.contains("object-fit:contain"));
        assert!(!page.contains("<script"));

        let page = svg_page(&raw("a.svg"), "cover").expect("a server file");
        assert!(page.contains("object-fit:cover"));
    }
}
