//! The file viewer (`fileviewer.tsx`): an overlay that shows a file's
//! contents (text in a scroll view; images as `image()`), opened by
//! `viewing`. Lightbox behavior folds into the same surface.

use nsh_core::ui::prelude::*;

use crate::icons::icon;
use crate::live::store;
use crate::theme;

/// The overlay layer drawn while `viewing` is set.
pub(crate) fn layer() -> impl IntoElement {
    dynamic(|| {
        let Some(fv) = store().viewing.get() else {
            return view().into_element();
        };
        viewer(fv).into_element()
    })
}

fn viewer(fv: crate::store::FileView) -> impl IntoElement {
    let is_image = fv.kind.starts_with("image") || fv.mime_is_image();
    view()
        .size_full()
        .absolute()
        .bg(theme::MAIN)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_4()
                .pt(px(46.0))
                .pb_2()
                .border_b_1()
                .border_b_color(theme::BORDER_F)
                .child(
                    text(if fv.title.is_empty() { fv.path.clone() } else { fv.title.clone() })
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::TEXT)
                        .flex_1()
                        .number_of_lines(1),
                )
                .child(
                    view()
                        .label("Close")
                        .child(icon("close"))
                        .on_press(|| store().viewing.set(None)),
                ),
        )
        .child(if is_image {
            view()
                .flex_1()
                .items_center()
                .justify_center()
                .child(image(fv.path.clone()).max_w(px(720.0)).max_h(px(560.0)))
                .into_element()
        } else {
            scroll()
                .flex_1()
                .child(
                    text(fv.body.clone())
                        .text_size(px(12.0))
                        .font_family("monospace")
                        .text_color(theme::TEXT)
                        .px_4()
                        .py_3(),
                )
                .into_element()
        })
}

impl crate::store::FileView {
    fn mime_is_image(&self) -> bool {
        self.kind == "image"
    }
}

// ---------------------------------------------------------------------------
// File tiles (`app/filetile.tsx`)
// ---------------------------------------------------------------------------

/// `mediaUrl` — server-hosted copies of generated attachments/avatars.
pub(crate) fn media_url(uri: &str) -> String {
    if let Some(rest) = uri.strip_prefix("file://") {
        for bucket in ["attachments", "avatars"] {
            let marker = format!("/generated/{bucket}/");
            if let Some(at) = rest.find(&marker) {
                let name = &rest[at + marker.len()..];
                if !name.contains('/') {
                    return format!(
                        "{}/files/{bucket}/{}",
                        crate::client::GEN,
                        name.replace(' ', "%20")
                    );
                }
            }
        }
    }
    uri.to_owned()
}

/// The extension glyph + accent of `filetile`'s `ICONS` table.
pub(crate) fn ext_icon(ext: &str) -> (&'static str, Hsla) {
    let (glyph, hex): (&'static str, u32) = match ext {
        "md" | "markdown" | "mdx" => ("M↓", 0),
        "pdf" => ("PDF", 0xf2555a),
        "pptx" | "ppt" | "docx" | "doc" | "pages" | "rtf" => ("≡", 0x4d9bff),
        "key" => ("⚿", 0xf5b400),
        "xlsx" | "xls" | "csv" | "numbers" | "tsv" => ("▦", 0x3ecf8e),
        "txt" | "log" => ("¶", 0),
        "json" => ("{ }", 0xf5b400),
        "yaml" | "yml" => ("Y", 0xf2555a),
        "toml" => ("⚙︎", 0),
        "html" => ("</>", 0x4d9bff),
        "css" | "scss" => ("#", 0xf062a8),
        "js" => ("JS", 0xf5b400),
        "jsx" => ("⚛︎", 0x22c3b4),
        "ts" => ("TS", 0x4d9bff),
        "tsx" => ("⚛︎", 0x22c3b4),
        "py" => ("Py", 0x4d9bff),
        "rs" => ("Rs", 0xff8a3d),
        "go" => ("Go", 0x22c3b4),
        "swift" => ("Sw", 0xff8a3d),
        "sql" => ("⛁", 0x22c3b4),
        "zip" | "gz" | "tar" | "rar" | "7z" => ("⊞", 0xf5b400),
        "mov" | "mp4" | "m4v" | "webm" => ("▶", 0xf2555a),
        "mp3" | "wav" | "m4a" | "aac" => ("∿", 0xa78bfa),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" => ("▣", 0x22c3b4),
        "svg" => ("◇", 0xff8a3d),
        _ => ("▤", 0),
    };
    let color = if hex == 0 {
        theme::hsla(0.0, 0.0, 0.66, 1.0)
    } else {
        rgb(hex).to_hsla()
    };
    (glyph, color)
}

/// A file's extension (the name's suffix, lowercased).
pub(crate) fn ext_of(name: &str, ext: &str) -> String {
    if !ext.is_empty() {
        return ext.to_lowercase();
    }
    name.rsplit('.').next().unwrap_or("").to_lowercase()
}

/// `fmtSize` — the tile's size readout.
pub(crate) fn fmt_size(n: f64) -> String {
    if n < 1000.0 {
        format!("{}B", n as i64)
    } else if n < 1e6 {
        let k = n / 1e3;
        if k < 10.0 { format!("{k:.1}kB") } else { format!("{}kB", k as i64) }
    } else {
        let m = n / 1e6;
        if m < 10.0 { format!("{m:.1}MB") } else { format!("{}MB", m as i64) }
    }
}

/// `FileTile` — a square tile: the image when it has a picture, the
/// extension glyph + name otherwise. Press opens the file viewer.
pub(crate) fn file_tile(a: crate::store::Attach) -> impl IntoElement {
    let pic = if !a.thumb.is_empty() {
        a.thumb.clone()
    } else if !a.feed.is_empty() {
        a.feed.clone()
    } else if a.kind == "image" {
        a.url.clone()
    } else {
        String::new()
    };
    let ext = ext_of(&a.name, &a.ext);
    let (glyph, color) = ext_icon(&ext);
    let name = if a.name.is_empty() { a.path.clone() } else { a.name.clone() };
    view()
        .size(px(52.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(theme::BORDER_S)
        .bg(theme::CARD)
        .items_center()
        .justify_center()
        .child(if pic.is_empty() {
            view()
                .items_center()
                .gap(px(2.0))
                .child(
                    text(glyph)
                        .text_size(px(if glyph.len() > 2 { 10.0 } else { 16.0 }))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color),
                )
                .child(
                    text(name)
                        .text_size(px(8.0))
                        .text_color(theme::MUTED)
                        .number_of_lines(2)
                        .text_center(),
                )
                .into_element()
        } else {
            image(media_url(&pic))
                .size(px(52.0))
                .into_element()
        })
        .on_press(move || {
            store().viewing.set(Some(crate::store::FileView {
                title: a.name.clone(),
                path: media_url(&a.url),
                body: String::new(),
                kind: a.kind.clone(),
            }));
        })
}
