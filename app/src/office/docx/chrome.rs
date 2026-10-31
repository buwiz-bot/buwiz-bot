//! The Word view's title bar as the web editor draws it (DR `Lte`, the
//! title bar of `Uh`): the document icon (`Mte`), the document's name
//! (`Ate`) and the menu bar (`ez`): File, Format, Insert and Help, item for
//! item with their icons and shortcuts. Under Rox's own viewer header.
//!
//! The name is the file's, as Rox's host passes it (`documentName`, the
//! path's last part), without `.docx`. The host passes no
//! `onDocumentNameChange`, so the web's field takes the focus but every
//! change springs back to the name; so does this one.

use std::path::Path;
use std::rc::Rc;

use nsh_core::canvas::{FillRule, svg_path};
use nsh_core::file_dialog::{FileFilter, OpenDialog};
use nsh_core::prelude::*;

use super::WordUi;
use super::dialogs;
use super::doc::{Cmd, Image, Insert};
use super::view::{focus, send};
use crate::live::store;
use crate::office::ui::color::hex;
use crate::office::ui::menu::{BarMenu, Entry, bar_menu, item, menu_bar_in};
use crate::office::ui::palette as p;
use crate::office::ui::shortcut_label;
use crate::office::ui::table_grid::{COLS, ROWS, table_grid};

fn ui() -> &'static WordUi {
    &store().office.word
}

/// Insert › Table of contents, refused (`commands.reasons.tableOfContentsUnsupported`).
pub(crate) const TOC_REFUSAL: &str = "Inserting a table of contents is not supported yet.";
/// Pictures wider than this come in at this width (`nd`, pixels).
const MAX_IMAGE_W: u32 = 612;
/// The kinds the picker offers (`accept="image/*"`, those Rox can read).
pub(crate) const IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "bmp", "webp"];

/// The name the title bar shows (`Pte`): without `.docx`.
pub(crate) fn doc_name(file: &str) -> String {
    let n = file.len();
    if n >= 5 && file.is_char_boundary(n - 5) && file[n - 5..].eq_ignore_ascii_case(".docx") { file[..n - 5].to_owned() } else { file.to_owned() }
}

/// The title bar (`flex items-stretch bg-doc-surface pt-2 pb-1`).
pub(crate) fn title_bar() -> impl IntoElement {
    let u = ui();
    view()
        .flex_row()
        .bg(p::SURFACE)
        .pt(px(8.0))
        .pb(px(4.0))
        .label("Title bar")
        // `flex items-center flex-shrink-0 pl-3 pr-1`
        .child(view().flex_row().items_center().flex_shrink_0().pl(px(12.0)).pr(px(4.0)).child(logo()))
        // `flex flex-col justify-center flex-1 min-w-0 py-1`; rows `px-1`
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .justify_center()
                .py(px(4.0))
                .child(view().flex_row().items_center().gap(px(8.0)).px(px(4.0)).child(name_field()))
                .child(view().flex_row().items_center().px(px(4.0)).child(menu_bar_in(menus(), u.menu_open))),
        )
}

const DOC_D: &str = "M2 0C0.9 0 0 0.9 0 2V38C0 39.1 0.9 40 2 40H30C31.1 40 32 39.1 32 38V10L22 0H2Z";
const FOLD_D: &str = "M22 0L32 10H24C22.9 10 22 9.1 22 8V0Z";
/// The three lines (`rect`s 18, 18 and 12 wide, 2 high, `rx` 1).
const LINES_D: &str = "M8 18h16a1 1 0 0 1 0 2H8a1 1 0 0 1 0-2zM8 23h16a1 1 0 0 1 0 2H8a1 1 0 0 1 0-2zM8 28h10a1 1 0 0 1 0 2H8a1 1 0 0 1 0-2z";

/// The document icon (`Mte`): 32×40, `#cbd5e1` with a `#94a3b8` fold.
fn logo() -> AnyElement {
    let (doc, fold, lines) = (svg_path(DOC_D), svg_path(FOLD_D), svg_path(LINES_D));
    canvas(move || {
        Painter::new()
            .fill_color(hex("cbd5e1"))
            .fill_path(&doc, FillRule::NonZero)
            .fill_color(hex("94a3b8"))
            .fill_path(&fold, FillRule::NonZero)
            .fill_color(white())
            .fill_path(&lines, FillRule::NonZero)
    })
    .w(px(32.0))
    .h(px(40.0))
    .label("Document icon")
    .into_element()
}

/// The name field (`Ate`: `text-base px-2 rounded min-w-[100px]
/// max-w-[300px] leading-tight`, `hover:bg-muted`, `focus:bg-doc-bg-input
/// focus:ring-1 focus:ring-ring`).
fn name_field() -> AnyElement {
    let u = ui();
    let name = move || store().viewing.get().map(|f| doc_name(&f.name)).unwrap_or_default();
    let buf = signal(String::new());
    effect(move || buf.set(name()));
    let (hovered, focused) = (signal(false), signal(false));
    nsh_base::input()
        .label("Document name")
        .placeholder("Untitled")
        .placeholder_color(|| color(p::TEXT_PLACEHOLDER))
        .value(move || buf.get())
        .on_change(move |v: &str| {
            // nothing takes the new name: it goes back, as the web's
            // controlled field does
            buf.set(v.to_owned());
            let back = untracked(name);
            crate::store::spawn_detached(async move { buf.set(back) });
        })
        .on_submit(|_: &str| focus())
        .on_focus(move || {
            focused.set(true);
            u.naming.set(true)
        })
        .on_blur(move || {
            focused.set(false);
            u.naming.set(false)
        })
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .w(px(200.0))
        .min_w(px(100.0))
        .max_w(px(300.0))
        .px(px(8.0))
        .rounded(px(4.0))
        .border(move || px(if focused.get() { 1.0 } else { 0.0 }))
        .border_color(p::TW_RING)
        .bg(move || if focused.get() { p::BG_INPUT } else if hovered.get() { p::TW_MUTED } else { AdaptiveColor::TRANSPARENT })
        .text_size(px(16.0))
        .line_height(px(20.0))
        .text_color(p::TW_FOREGROUND)
        .into_element()
}

/// A menu row's command, the keyboard back on the page after it (the
/// web's `focusActiveEditor`).
fn act(run: impl Fn() + 'static) -> impl Fn() + 'static {
    move || {
        run();
        focus();
    }
}

fn insert(what: Insert) {
    send(Cmd::Insert(what));
    focus();
}

/// The menus, as `ez` builds them for Rox's host: File without Open (its
/// `showFileOpen: false`), Help with Report issue (`showHelpMenu`'s
/// default).
fn menus() -> Vec<BarMenu> {
    vec![bar_menu("File", file_menu), bar_menu("Format", format_menu), bar_menu("Insert", insert_menu), bar_menu("Help", help_menu)]
}

pub(crate) fn file_menu() -> Vec<Entry> {
    vec![
        Entry::Item(item(Some("file_download"), "Save", act(crate::files::viewer_save)).shortcut(shortcut_label("Mod+S", apple_keys()))),
        Entry::Separator,
        Entry::Item(item(Some("print"), "Print", crate::files::viewer_print)),
        Entry::Item(item(Some("settings"), "Page setup", dialogs::open_page_setup)),
    ]
}

pub(crate) fn format_menu() -> Vec<Entry> {
    vec![
        Entry::Item(item(Some("format_textdirection_l_to_r"), "Left-to-right text", act(|| send(Cmd::Bidi(false))))),
        Entry::Item(item(Some("format_textdirection_r_to_l"), "Right-to-left text", act(|| send(Cmd::Bidi(true))))),
    ]
}

pub(crate) fn insert_menu() -> Vec<Entry> {
    let grid: crate::office::ui::menu::Content = Rc::new(|close: Rc<dyn Fn()>| {
        table_grid(ROWS, COLS, move |rows, cols| {
            close();
            insert(Insert::Table { rows, cols })
        })
    });
    vec![
        Entry::Item(item(Some("image"), "Image", pick_image)),
        Entry::Content { icon: Some("grid_on"), label: "Table".into(), disabled: None, content: grid },
        Entry::Separator,
        Entry::Submenu {
            icon: Some("page_break"),
            label: "Break".into(),
            entries: vec![
                Entry::Item(item(Some("page_break"), "Page break", || insert(Insert::PageBreak))),
                Entry::Item(item(Some("horizontal_rule"), "Section break (next page)", || insert(Insert::SectionBreak("nextPage")))),
                Entry::Item(item(Some("border_horizontal"), "Section break (continuous)", || insert(Insert::SectionBreak("continuous")))),
            ],
        },
        Entry::Item(item(Some("format_list_numbered"), "Table of contents", || {}).disabled(Some(TOC_REFUSAL))),
        Entry::Item(item(Some("branding_watermark"), "Watermark", dialogs::open_watermark)),
    ]
}

pub(crate) fn help_menu() -> Vec<Entry> {
    vec![Entry::Item(item(None, "Report issue", report_issue))]
}

/// Help › Report issue (`openReportIssue`): a new issue on the editor's
/// repository, its template filled in.
fn report_issue() {
    drop(open_url(report_issue_url(ui().area.get_untracked())));
}

pub(crate) fn report_issue_url((w, h): (f32, f32)) -> String {
    let body = [
        "### What happened",
        "",
        "### Steps to reproduce",
        "1. ",
        "2. ",
        "3. ",
        "",
        "### Expected",
        "",
        "### Actual",
        "",
        "### Attach the DOCX",
        "Please drag the `.docx` file that reproduces this onto the issue (below this text box). Bugs are much faster to fix with a real file.",
        "",
        "### Environment",
        "- URL: ",
        &format!("- Viewport: {} x {}", w.round(), h.round()),
        "- User agent: Rox",
        "",
    ]
    .join("\n");
    format!("https://github.com/openooxml/docx/issues/new?title={}&body={}", form_encode("[Bug] "), form_encode(&body))
}

/// `URLSearchParams`' encoding: spaces as `+`, the rest of the unsafe
/// bytes as `%XX`.
fn form_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Asks for a picture (`accept="image/*"`): `got` gets its bytes and name.
pub(crate) fn pick_picture(got: impl FnOnce(Vec<u8>, String) + 'static) {
    let answer = pick_paths(OpenDialog::files().filter(FileFilter::new("Images", IMAGE_EXTS)));
    crate::store::spawn_detached(async move {
        match answer.await {
            Ok(Some(paths)) => {
                let Some(path) = paths.into_iter().next() else { return };
                match std::fs::read(&path) {
                    Ok(bytes) => got(bytes, path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                    Err(_) => super::note("Failed to read image file"),
                }
            }
            Ok(None) => {}
            Err(e) => super::note(format!("Couldn't choose a picture: {e}")),
        }
    });
}

/// Insert › Image: the picture picked goes in at the caret, at its own
/// size up to 612 wide (`handleImageFileChange`).
fn pick_image() {
    pick_picture(|bytes, name| match picture(&bytes, &name) {
        Some(img) => insert(Insert::Image(img)),
        None => super::note("Failed to decode image"),
    });
}

/// A picture as Insert takes it, from its bytes.
pub(crate) fn picture(bytes: &[u8], name: &str) -> Option<Image> {
    let (mut w, mut h) = image_size(bytes)?;
    if w > MAX_IMAGE_W {
        h = (f64::from(h) * (f64::from(MAX_IMAGE_W) / f64::from(w))).round() as u32;
        w = MAX_IMAGE_W;
    }
    Some(Image { src: data_url(bytes, Path::new(name)), alt: name.to_owned(), width: w, height: h })
}

/// The bytes as a `data:` URL (`readAsDataURL`), typed by their content.
pub(crate) fn data_url(bytes: &[u8], name: &Path) -> String {
    use base64::Engine as _;
    let mime = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF") {
        "image/gif"
    } else if bytes.starts_with(b"BM") {
        "image/bmp"
    } else if bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        match name.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
            Some("svg") => "image/svg+xml",
            _ => "application/octet-stream",
        }
    };
    format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// A picture's pixel size, read from its header (PNG, JPEG, GIF, BMP,
/// WebP).
pub(crate) fn image_size(b: &[u8]) -> Option<(u32, u32)> {
    let be32 = |i: usize| Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?));
    let le16 = |i: usize| Some(u32::from(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?)));
    let be16 = |i: usize| Some(u32::from(u16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?)));
    let le24 = |i: usize| Some(u32::from(*b.get(i)?) | u32::from(*b.get(i + 1)?) << 8 | u32::from(*b.get(i + 2)?) << 16);
    let size = if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        (be32(16)?, be32(20)?)
    } else if b.starts_with(b"GIF8") {
        (le16(6)?, le16(8)?)
    } else if b.starts_with(b"BM") {
        let h = i32::from_le_bytes(b.get(22..26)?.try_into().ok()?);
        (u32::from_le_bytes(b.get(18..22)?.try_into().ok()?), h.unsigned_abs())
    } else if b.starts_with(b"\xff\xd8") {
        let mut i = 2;
        loop {
            while *b.get(i)? != 0xff {
                i += 1;
            }
            while *b.get(i)? == 0xff {
                i += 1;
            }
            let m = *b.get(i)?;
            i += 1;
            if matches!(m, 0xd0..=0xd9 | 0x01) {
                continue;
            }
            let len = be16(i)? as usize;
            // the frame's header: SOF0–SOF15, not DHT, JPG or DAC
            if (0xc0..=0xcf).contains(&m) && !matches!(m, 0xc4 | 0xc8 | 0xcc) {
                break (be16(i + 5)?, be16(i + 3)?);
            }
            i += len;
        }
    } else if b.starts_with(b"RIFF") && b.get(8..12)? == b"WEBP" {
        match b.get(12..16)? {
            b"VP8 " => (le16(26)? & 0x3fff, le16(28)? & 0x3fff),
            b"VP8L" => {
                let bits = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
                ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
            }
            b"VP8X" => (le24(24)? + 1, le24(27)? + 1),
            _ => return None,
        }
    } else {
        return None;
    };
    (size.0 > 0 && size.1 > 0).then_some(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_drops_docx_only() {
        assert_eq!(doc_name("Report.DOCX"), "Report");
        assert_eq!(doc_name("old.doc"), "old.doc");
        assert_eq!(doc_name(".docx"), "");
    }

    #[test]
    fn pictures_read_their_size_and_come_in_at_most_612_wide() {
        let png = super::super::fixture::png();
        let (w, h) = image_size(&png).unwrap();
        assert!(w > 0 && h > 0);
        let mut wide = png.clone();
        wide[16..20].copy_from_slice(&1224u32.to_be_bytes());
        wide[20..24].copy_from_slice(&300u32.to_be_bytes());
        let img = picture(&wide, "w.png").unwrap();
        assert_eq!((img.width, img.height, img.alt.as_str()), (612, 150, "w.png"));
        assert!(img.src.starts_with("data:image/png;base64,"));
        assert_eq!(image_size(b"GIF89a\x10\x00\x20\x00"), Some((16, 32)));
        assert_eq!(image_size(b"not a picture"), None);
    }

    #[test]
    fn table_of_contents_is_refused_with_the_webs_reason() {
        let toc = insert_menu().into_iter().find_map(|e| match e {
            Entry::Item(i) if i.label == "Table of contents" => Some(i),
            _ => None,
        });
        let toc = toc.expect("the row");
        assert!(toc.disabled);
        assert_eq!(toc.description.as_deref(), Some(TOC_REFUSAL));
        assert_eq!(TOC_REFUSAL, "Inserting a table of contents is not supported yet.");
    }

    #[test]
    fn report_issue_fills_the_webs_template() {
        let url = report_issue_url((800.0, 600.0));
        assert!(url.starts_with("https://github.com/openooxml/docx/issues/new?title=%5BBug%5D+&body=%23%23%23+What+happened%0A"), "{url}");
        assert!(url.contains("Viewport%3A+800+x+600"));
    }
}
