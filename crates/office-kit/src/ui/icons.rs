//! The web editor's icons: Material Symbols Outlined (Google, Apache-2.0;
//! the static instance at weight 400, grade 0, optical size 24, unfilled),
//! the glyphs `docx-react` inlines as SVG (`ze`'s `cD` map, each a
//! `viewBox="0 -960 960 960"` path). The font is registered with the root
//! (`Root::font`, `crate::fonts`) and each icon is drawn by codepoint, not
//! ligature, on a canvas: the glyph's em box is the icon's box, its
//! baseline the box's bottom, as the SVGs place it.
//!
//! The font ships in the app bundle, not in git: `app/bundle-dev.sh`
//! fetches the pinned file and copies it, with its licence, into
//! `Contents/Resources/fonts/`.
//!
//! The font's current glyph differs from the web's SVG for a few names
//! (the web's set is older); those draw the web's own path data instead,
//! and `select_all` is the web's `border_all` glyph (`cD` maps both to
//! one component).

use std::path::PathBuf;

use nsh_core::canvas::{CanvasTextBaseline, Path, svg_path};
use nsh_core::prelude::*;

/// The font's family name, as styles and canvas fonts name it.
pub const FAMILY: &str = "Material Symbols Outlined";
/// Its file in the bundle's `Resources/fonts/`.
pub const FILE: &str = "MaterialSymbolsOutlined.ttf";

/// The codepoint of icon `name` in the font, U+0000 for a name it lacks
/// (or one drawn from the web's path data, [`web_paths`]).
pub fn sym(name: &str) -> char {
    match name {
        "add" => '\u{e145}',
        "add_comment" => '\u{e266}',
        "arrow_back" => '\u{e5c4}',
        "arrow_drop_down" => '\u{e5c5}',
        "border_all" => '\u{e228}',
        "border_bottom" => '\u{e229}',
        "border_clear" => '\u{e22a}',
        "border_color" => '\u{e22b}',
        "border_inner" => '\u{e22d}',
        "border_left" => '\u{e22e}',
        "border_outer" => '\u{e22f}',
        "border_top" => '\u{e232}',
        "call_merge" => '\u{e0b3}',
        "call_split" => '\u{e0b6}',
        "chat_bubble_outline" => '\u{e0ca}',
        "check" => '\u{e5ca}',
        "close" => '\u{e14c}',
        "comment" => '\u{e0b9}',
        "delete" => '\u{e872}',
        "delete_sweep" => '\u{e16c}',
        "done_all" => '\u{e877}',
        "drag_indicator" => '\u{e945}',
        "expand_less" => '\u{e5ce}',
        "expand_more" => '\u{e5cf}',
        "file_download" => '\u{e171}',
        "file_upload" => '\u{e2c6}',
        "fit_width" => '\u{f779}',
        "flip_to_back" => '\u{e882}',
        "flip_to_front" => '\u{e883}',
        "format_align_center" => '\u{e234}',
        "format_align_justify" => '\u{e235}',
        "format_align_left" => '\u{e236}',
        "format_align_right" => '\u{e237}',
        "format_bold" => '\u{e238}',
        "format_clear" => '\u{e239}',
        "format_color_fill" => '\u{e23a}',
        "format_color_reset" => '\u{e23b}',
        "format_color_text" => '\u{e23c}',
        "format_image_left" => '\u{f863}',
        "format_image_right" => '\u{f864}',
        "format_indent_decrease" => '\u{e23d}',
        "format_indent_increase" => '\u{e23e}',
        "format_italic" => '\u{e23f}',
        "format_line_spacing" => '\u{e240}',
        "format_list_bulleted" => '\u{e241}',
        "format_list_numbered" => '\u{e242}',
        "format_paint" => '\u{e243}',
        "format_textdirection_l_to_r" => '\u{e247}',
        "format_textdirection_r_to_l" => '\u{e248}',
        "format_underlined" => '\u{e249}',
        "grid_on" => '\u{e3ec}',
        "height" => '\u{ea16}',
        "horizontal_rule" => '\u{f108}',
        "image" => '\u{e251}',
        "ink_highlighter" => '\u{e6d1}',
        "keyboard_arrow_down" => '\u{e313}',
        "keyboard_arrow_left" => '\u{e314}',
        "keyboard_arrow_right" => '\u{e315}',
        "keyboard_arrow_up" => '\u{e316}',
        "line_weight" => '\u{e91a}',
        "link" => '\u{e157}',
        "more_vert" => '\u{e5d4}',
        "open_with" => '\u{e89f}',
        "padding" => '\u{e9c8}',
        "print" => '\u{e555}',
        "rate_review" => '\u{e560}',
        "redo" => '\u{e15a}',
        "remove" => '\u{e15b}',
        "rotate_left" => '\u{e419}',
        "rotate_right" => '\u{e41a}',
        "select_all" => '\u{e228}',
        "settings" => '\u{e8b8}',
        "shapes" => '\u{e602}',
        "strikethrough_s" => '\u{e257}',
        "subscript" => '\u{f111}',
        "superscript" => '\u{f112}',
        "swap_horiz" => '\u{e8d4}',
        "swap_vert" => '\u{e0c3}',
        "table" => '\u{f191}',
        "table_chart" => '\u{e265}',
        "table_rows" => '\u{f101}',
        "text_rotation_none" => '\u{e93f}',
        "tune" => '\u{e429}',
        "undo" => '\u{e166}',
        "vertical_align_bottom" => '\u{e258}',
        "vertical_align_center" => '\u{e259}',
        "vertical_align_top" => '\u{e25a}',
        "view_column" => '\u{e8ec}',
        "visibility" => '\u{e417}',
        "wrap_text" => '\u{e25b}',
        _ => '\0',
    }
}

/// The web's path data for the icons whose glyph the font draws
/// differently, each path with whether it takes the icon's ink (else the
/// web's `--doc-success`, `chat_bubble_check`'s tick).
fn web_paths(name: &str) -> Option<&'static [(&'static str, bool)]> {
    Some(match name {
        "page_break" => &[("M120-440v-80h160v80H120Zm200 0v-80h160v80H320Zm200 0v-80h160v80H520Zm200 0v-80h120v80H720ZM240-120q-33 0-56.5-23.5T160-200v-120h80v120h480v-120h80v120q0 33-23.5 56.5T720-120H240Zm-80-520v-120q0-33 23.5-56.5T240-840h480q33 0 56.5 23.5T800-760v120h-80v-120H240v120h-80Z", true)],
        "border_right" => &[("M120-120v-80h80v80h-80Zm0-160v-80h80v80h-80Zm0-160v-80h80v80h-80Zm0-160v-80h80v80h-80Zm160 640v-80h80v80h-80Zm0-320v-80h80v80h-80Zm0-320v-80h80v80h-80Zm160 640v-80h80v80h-80Zm0-160v-80h80v80h-80Zm0-160v-80h80v80h-80Zm0-160v-80h80v80h-80Zm0-160v-80h80v80h-80Zm160 640v-80h80v80h-80Zm0-320v-80h80v80h-80Zm0-320v-80h80v80h-80Zm160 640v-720h80v720h-80Z", true)],
        "border_horizontal" => &[("M120-120v-60h60v60h-60Zm0-165v-60h60v60h-60Zm0-165v-60h720v60H120Zm0-165v-60h60v60h-60Zm0-165v-60h60v60h-60Zm165 660v-60h60v60h-60Zm0-660v-60h60v60h-60Zm165 660v-60h60v60h-60Zm0-165v-60h60v60h-60Zm0-330v-60h60v60h-60Zm0-165v-60h60v60h-60Zm165 660v-60h60v60h-60Zm0-660v-60h60v60h-60Zm165 660v-60h60v60h-60Zm0-165v-60h60v60h-60Zm0-330v-60h60v60h-60Zm0-165v-60h60v60h-60Z", true)],
        "border_vertical" => &[("M120-120v-60h60v60h-60Zm0-165v-60h60v60h-60Zm0-165v-60h60v60h-60Zm0-165v-60h60v60h-60Zm0-165v-60h60v60h-60Zm165 660v-60h60v60h-60Zm0-330v-60h60v60h-60Zm0-330v-60h60v60h-60Zm165 660v-720h60v720h-60Zm165 0v-60h60v60h-60Zm0-330v-60h60v60h-60Zm0-330v-60h60v60h-60Zm165 660v-60h60v60h-60Zm0-165v-60h60v60h-60Zm0-165v-60h60v60h-60Zm0-165v-60h60v60h-60Zm0-165v-60h60v60h-60Z", true)],
        "branding_watermark" => &[("M160-160q-33 0-56.5-23.5T80-240v-480q0-33 23.5-56.5T160-800h640q33 0 56.5 23.5T880-720v480q0 33-23.5 56.5T800-160H160Zm0-80h640v-480H160v480Zm320-60q66 0 113-37t47-93q0-29-13-58t-34-55q-21-26-50-49t-50-31q-21 8-50 31t-50 49q-21 26-34 55t-13 58q0 56 47 93t140 37Z", true)],
        "check_circle" => &[("m424-296 282-282-56-56-226 226-114-114-56 56 170 170Zm56 216q-83 0-156-31.5T197-197q-54-54-85.5-127T80-480q0-83 31.5-156T197-763q54-54 127-85.5T480-880q83 0 156 31.5T763-763q54 54 85.5 127T880-480q0 83-31.5 156T763-197q-54 54-127 85.5T480-80Z", true)],
        "edit_note" => &[("M160-400h280v-80H160v80Zm0-160h440v-80H160v80Zm0-160h440v-80H160v80Zm360 360v-123l221-220q9-9 20-13t22-4q12 0 23 4.5t20 13.5l37 37q8 9 12.5 20t4.5 22q0 11-4 22.5T863-380L643-160H520Zm300-263-37-37 37 37ZM580-220h38l121-122-18-19-19-18-122 121v38Zm141-141-19-18 37 37-18-19Z", true)],
        "chat_bubble_check" => &[("M80-80v-720q0-33 23.5-56.5T160-880h640q33 0 56.5 23.5T880-800v480q0 33-23.5 56.5T800-240H240L80-80Zm126-240h594v-480H160v525l46-45Zm-46 0v-480 480Z", true), ("m421-380 227-227-45-45-182 182-92-91-45 45 137 136Z", false)],
        _ => return None,
    })
}

/// Whether there is an icon called `name`.
pub fn has(name: &str) -> bool {
    sym(name) != '\0' || web_paths(name).is_some()
}

/// The font file's bytes: in the app bundle beside the executable
/// (`Contents/Resources/fonts/`), else in the dev bundle
/// (`~/Applications/Rox.app`, for a bare `target/debug/rox`). `None`
/// without either: the icons draw nothing.
pub fn font_data() -> Option<Vec<u8>> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(contents) = exe.parent().and_then(|m| m.parent())
    {
        dirs.push(contents.join("Resources/fonts"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Applications/Rox.app/Contents/Resources/fonts"));
    }
    let found = dirs.iter().find_map(|d| std::fs::read(d.join(FILE)).ok());
    if found.is_none() {
        log::warn!("no {FILE} in {dirs:?}: the office icons draw nothing (app/bundle-dev.sh copies it)");
    }
    found
}

/// What an icon draws.
enum Shape {
    Glyph(char),
    Paths(Vec<(Path, bool)>),
    Nothing,
}

fn shape(name: &str) -> Shape {
    if let Some(paths) = web_paths(name) {
        return Shape::Paths(paths.iter().map(|(d, ink)| (svg_path(d), *ink)).collect());
    }
    match sym(name) {
        '\0' => Shape::Nothing,
        c => Shape::Glyph(c),
    }
}

/// The drawing of an icon `size` points square in `ink`.
fn paint(shape: &Shape, size: f32, ink: Hsla) -> Painter {
    match shape {
        Shape::Glyph(c) => Painter::new()
            .font(&format!("{size}px \"{FAMILY}\""))
            .text_baseline(CanvasTextBaseline::Alphabetic)
            .fill_color(ink)
            .fill_text(&c.to_string(), px(0.0), px(size)),
        Shape::Paths(paths) => {
            let mut p = Painter::new().scale(size / 960.0, size / 960.0).translate(px(0.0), px(960.0));
            for (path, inked) in paths {
                let fill = if *inked { ink } else { color(super::palette::SUCCESS) };
                p = p.fill_color(fill).fill_path(path, nsh_core::canvas::FillRule::NonZero);
            }
            p
        }
        Shape::Nothing => Painter::new(),
    }
}

/// Icon `name`, `size` points square (the web's `<ze size>`), in the ink
/// `ink` gives; `ink` runs inside the painter, so a hover or theme change
/// repaints it.
pub fn icon(name: &str, size: f32, ink: impl Fn() -> AdaptiveColor + 'static) -> AnyElement {
    let shape = shape(name);
    canvas(move || paint(&shape, size, color(ink())))
        .size(px(size))
        .flex_shrink_0()
        .into_element()
}

#[cfg(test)]
mod tests {
    use nsh_core::canvas::CanvasOp;

    use super::*;

    /// Every icon the web's toolbar, menus and dialogs name (`zy`, `sE`,
    /// the menu bar's `_r` entries, the table menu, the review controls).
    const NAMES: &[&str] = &[
        "undo", "redo", "format_bold", "format_italic", "format_underlined", "strikethrough_s",
        "format_color_text", "ink_highlighter", "link", "superscript", "subscript",
        "format_align_left", "format_align_center", "format_align_right", "format_align_justify",
        "format_list_bulleted", "format_list_numbered", "format_indent_increase",
        "format_indent_decrease", "format_line_spacing", "format_clear", "comment", "edit_note",
        "table", "border_all", "border_outer", "border_inner", "border_top", "border_bottom",
        "border_left", "border_right", "border_clear", "border_color", "line_weight",
        "format_color_fill", "wrap_text", "format_image_left", "format_image_right",
        "flip_to_back", "flip_to_front", "rotate_right", "rotate_left", "swap_horiz",
        "swap_vert", "tune", "image", "grid_on", "page_break", "horizontal_rule",
        "border_horizontal", "branding_watermark", "print", "settings", "file_download",
        "file_upload", "format_textdirection_l_to_r", "format_textdirection_r_to_l", "add",
        "remove", "call_merge", "call_split", "select_all", "delete", "table_rows",
        "view_column", "fit_width", "check", "close", "keyboard_arrow_up",
        "keyboard_arrow_down", "keyboard_arrow_right", "expand_more", "arrow_drop_down",
    ];

    #[test]
    fn every_web_icon_has_a_glyph_or_the_webs_path() {
        let missing: Vec<_> = NAMES.iter().filter(|n| !has(n)).collect();
        assert!(missing.is_empty(), "no icon for {missing:?}");
    }

    #[test]
    fn icons_draw_by_codepoint_at_the_boxs_baseline() {
        // `format_bold` is U+E238 in Material Symbols (its codepoints file)
        assert_eq!(sym("format_bold"), '\u{e238}');
        assert_eq!(sym("select_all"), sym("border_all"), "the web draws select_all as border_all");
        let d = paint(&shape("format_bold"), 18.0, Hsla::default()).finish();
        let text = d.ops.iter().find_map(|op| match op {
            CanvasOp::FillText { text, y, .. } => Some((text.clone(), *y)),
            _ => None,
        });
        let (t, y) = text.expect("a glyph");
        assert_eq!(&*t, "\u{e238}");
        assert_eq!(format!("{y:?}"), format!("{:?}", nsh_core::canvas::Num::Const(18.0)));
    }

    #[test]
    fn icons_the_font_draws_differently_take_the_webs_path() {
        assert_eq!(sym("page_break"), '\0');
        assert!(matches!(shape("page_break"), Shape::Paths(_)));
        assert!(matches!(shape("nothing_of_that_name"), Shape::Nothing));
    }
}
