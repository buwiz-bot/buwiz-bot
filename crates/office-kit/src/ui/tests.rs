//! The kit's controls on one page (a gallery, the harness each control is
//! tried in) driven headless as a person drives them (`ui_tests`'
//! driver): their sizes against the web's, and what pressing, hovering,
//! typing, Escape and Return do. The probe text says what each control
//! last reported.

use std::time::Duration;

use nsh_core::element::Root;
use nsh_core::prelude::*;

use super::button::{self, Choice, groups, icon_button, icon_dropdown, separator};
use super::color::{Mode, Picked, color_button};
use super::field::{checkbox, form_row, number_field, section_label, tabs, text_field};
use super::menu::{Entry, bar_menu, item, menu_bar};
use super::modal::modal;
use super::palette as p;
use super::select::{STYLES, font_family_select, font_size, style_select, zoom_control};
use super::table_grid::{insert_table_button, table_grid};
use super::{shortcut_label, tip_text};
use crate::testing::{Headless, num, read, start_root, until};

#[derive(Clone, Copy)]
struct Probe {
    bold: Signal<bool>,
    align: Signal<&'static str>,
    style: Signal<String>,
    font: Signal<String>,
    size: Signal<f32>,
    custom: Signal<u32>,
    zoom: Signal<f32>,
    color: Signal<String>,
    grid: Signal<String>,
    menu: Signal<String>,
    dialog: Signal<bool>,
    confirmed: Signal<u32>,
    cancelled: Signal<u32>,
    tab: Signal<usize>,
    check: Signal<bool>,
}

fn picked(c: &Picked) -> String {
    match c {
        Picked::Clear => "clear".into(),
        Picked::Theme(s) => format!("theme:{}:{}:{}", s.slot, s.tint.or(s.shade).unwrap_or("-"), s.hex),
        Picked::Rgb(h) => format!("rgb:{h}"),
    }
}

#[component]
fn Gallery() -> impl IntoElement {
    let pr = Probe {
        bold: signal(false),
        align: signal("left"),
        style: signal("Normal".into()),
        font: signal("Arial".into()),
        size: signal(11.0),
        custom: signal(0),
        zoom: signal(1.0),
        color: signal("-".into()),
        grid: signal("-".into()),
        menu: signal("-".into()),
        dialog: signal(false),
        confirmed: signal(0),
        cancelled: signal(0),
        tab: signal(0),
        check: signal(false),
    };
    let name = signal(String::new());
    let margin = signal(1.0f64);
    let tab = signal(0usize);
    let align = |v: &'static str, icon: &'static str, label: &'static str, s: &'static str| Choice { value: v, icon, label: label.into(), shortcut: Some(s) };
    let toolbar = view()
        .flex_row()
        .items_center()
        .label("Toolbar")
        .child(groups(vec![
            (
                "Text formatting",
                vec![
                    icon_button("format_bold", "Bold").shortcut("Mod+B").active(move || pr.bold.get()).on_press(move || pr.bold.update(|b| *b = !*b)).into_element(),
                    icon_button("format_italic", "Italic").shortcut("Mod+I").disabled(|| true).into_element(),
                ],
            ),
            ("Script", vec![icon_button("superscript", "Superscript").shortcut("Mod+Shift+=").into_element()]),
        ]))
        .child(separator())
        .child(
            icon_dropdown(
                "Align",
                vec![
                    align("left", "format_align_left", "Align Left", "Ctrl+L"),
                    align("center", "format_align_center", "Center", "Ctrl+E"),
                    align("right", "format_align_right", "Align Right", "Ctrl+R"),
                    align("both", "format_align_justify", "Justify", "Ctrl+J"),
                ],
            )
            .current(move || Some(pr.align.get()))
            .on_select(move |v| pr.align.set(v)),
        );
    let selects = view()
        .flex_row()
        .items_center()
        .child(style_select(&STYLES).value(move || Some(pr.style.get().into())).on_change(move |v| pr.style.set(v.to_owned())))
        .child(font_family_select(&["Carlito"]).value(move || Some(pr.font.get().into())).on_change(move |v| pr.font.set(v.to_owned())))
        .child(font_size(move || Some(pr.size.get())).on_change(move |s| pr.size.set(s)).custom(move || pr.custom.update(|n| *n += 1)))
        .child(zoom_control(move || pr.zoom.get(), move |z| pr.zoom.set(z)));
    let colors = view()
        .flex_row()
        .items_center()
        .child(color_button(Mode::Text).on_pick(move |c| pr.color.set(picked(&c))))
        .child(color_button(Mode::Highlight).single().on_pick(move |c| pr.color.set(picked(&c))))
        .child(insert_table_button("Insert table", move |r, c| pr.grid.set(format!("{r}x{c}"))));
    let chose = move |what: &'static str| move || pr.menu.set(what.to_owned());
    let menus = menu_bar(vec![
        bar_menu("File", move || {
            vec![
                Entry::Item(item(Some("file_download"), "Save", chose("save")).shortcut(shortcut_label("Mod+S", true))),
                Entry::Separator,
                Entry::Item(item(Some("settings"), "Page setup", chose("setup")).disabled(Some("Not here"))),
            ]
        }),
        bar_menu("Insert", move || {
            vec![
                Entry::Item(item(Some("image"), "Image", chose("image"))),
                Entry::Content {
                    icon: Some("grid_on"),
                    label: "Table".into(),
                    disabled: None,
                    content: std::rc::Rc::new(move |close| {
                        table_grid(6, 6, move |r, c| {
                            close();
                            pr.grid.set(format!("{r}x{c}"))
                        })
                    }),
                },
                Entry::Separator,
                Entry::Submenu {
                    icon: Some("page_break"),
                    label: "Break".into(),
                    entries: vec![
                        Entry::Item(item(Some("page_break"), "Page break", chose("page-break"))),
                        Entry::Item(item(Some("horizontal_rule"), "Section break (next page)", chose("next-page"))),
                    ],
                },
                Entry::Item(item(Some("delete"), "Delete table", chose("delete")).danger()),
            ]
        }),
    ]);
    let dialog = modal(pr.dialog, "Page Setup")
        .tab_index(pr.tab)
        .body(move || {
            view()
                .gap(px(14.0))
                .child(tabs(vec!["Web Address", "Bookmark"], tab))
                .child(section_label("Margins"))
                .child(form_row("Name", text_field("Name", name)))
                .child(form_row("Top", number_field("Top", margin, 0.0, 10.0)))
                .child(checkbox("Match case", pr.check))
                .into_element()
        })
        .action("Cancel", false, move || {
            pr.dialog.set(false);
            pr.cancelled.update(|n| *n += 1)
        })
        .action("Apply", true, move || {
            pr.dialog.set(false);
            pr.confirmed.update(|n| *n += 1)
        })
        .on_cancel(move || pr.cancelled.update(|n| *n += 1))
        .on_confirm(move || {
            pr.dialog.set(false);
            pr.confirmed.update(|n| *n += 1)
        });
    let shown = signal(String::new());
    drop(set_interval(Duration::from_millis(20), move || {
        shown.set(format!(
            "probe bold={} align={} style={} font={} size={} custom={} zoom={} color={} grid={} menu={} dialog={} confirmed={} cancelled={} tab={} check={}",
            pr.bold.get_untracked(),
            pr.align.get_untracked(),
            pr.style.get_untracked(),
            pr.font.get_untracked().replace(' ', "_"),
            pr.size.get_untracked(),
            pr.custom.get_untracked(),
            pr.zoom.get_untracked(),
            pr.color.get_untracked(),
            pr.grid.get_untracked(),
            pr.menu.get_untracked(),
            pr.dialog.get_untracked(),
            pr.confirmed.get_untracked(),
            pr.cancelled.get_untracked(),
            pr.tab.get_untracked(),
            pr.check.get_untracked(),
        ))
    }));
    view()
        .size_full()
        .gap(px(8.0))
        .child(menus)
        .child(toolbar)
        .child(selects)
        .child(colors)
        .child(view().w(px(80.0)).h(px(24.0)).label("Open dialog").on_press(move || pr.dialog.set(true)))
        .child(dialog)
        .child(text(move || shown.get()).text_size(px(8.0)))
}

fn start_gallery() -> Headless {
    start_root(Root::new::<Gallery>(|| ()))
}

fn is(p: &std::collections::HashMap<String, String>, k: &str, v: &str) -> bool {
    p.get(k).map(String::as_str) == Some(v)
}

fn bg(app: &Headless, label: &str) -> nsh_core::Color {
    app.style(label).background_color
}

fn light(c: AdaptiveColor) -> nsh_core::Color {
    nsh_core::Color::from(c.light)
}

// ---- the web's numbers ----

#[test]
fn toolbar_buttons_are_the_webs_size() {
    let app = start_gallery();
    // `size: "icon-sm"` → `h-7 w-7`, `rounded-md` (DR `Jk`)
    let b = app.frame("Bold");
    assert_eq!((b.size.width.0, b.size.height.0), (28.0, 28.0));
    assert_eq!(app.style("Bold").border_radius.0, 6.0);
    // `opacity-30` while disabled (DR `op`)
    assert!((app.style("Italic").opacity - 0.3).abs() < 1e-6);
    // the first group starts flush (`first:pl-0`), the next 6 in (`px-1.5`)
    assert_eq!(b.origin.x.0, 0.0);
    assert_eq!(app.frame("Superscript").origin.x.0, 6.0);
    let first = app.frame("Text formatting");
    // two buttons 1 apart (`gap-px`), 6 on the right, a 1px hairline
    assert_eq!(first.size.width.0, 28.0 + 1.0 + 28.0 + 6.0 + 1.0);
    // the separator: `w-px h-6 mx-1.5` (DR `xc`)
    let s = app.frame("Separator");
    assert_eq!((s.size.width.0, s.size.height.0), (1.0, 24.0));
    let groups = app.frame("Toolbar groups");
    assert_eq!(s.origin.x.0, groups.origin.x.0 + groups.size.width.0 + 6.0);
    // the icon dropdown's trigger is a toolbar button too (DR `vc`)
    let a = app.frame("Align");
    assert_eq!((a.size.width.0, a.size.height.0), (28.0, 28.0));
}

#[test]
fn selects_and_the_size_box_are_the_webs_size() {
    let app = start_gallery();
    // the trigger: `h-8`, the style list's `width: 120` (DR `ui`, `sg`)
    let s = app.frame("Select paragraph style");
    assert_eq!((s.size.width.0, s.size.height.0), (120.0, 32.0));
    assert_eq!(app.style("Select paragraph style").border_radius.0, 4.0);
    let f = app.frame("Select font family");
    assert_eq!(f.size.height.0, 32.0);
    assert!(f.size.width.0 >= 60.0, "`width: 60` is the font list's minimum");
    // the size box: steppers `h-7 w-7`, the size `h-7 w-10` (DR `yy`)
    let d = app.frame("Select font size");
    assert_eq!((d.size.width.0, d.size.height.0), (40.0, 28.0));
    let m = app.frame("Decrease font size");
    assert_eq!((m.size.width.0, m.size.height.0), (28.0, 28.0));
    assert_eq!(d.origin.x.0, m.origin.x.0 + 28.0);
    // the zoom: `h-7` steppers around the level (DR `Oy`)
    assert_eq!(app.frame("Zoom out").size.height.0, 28.0);
}

#[test]
fn the_colour_buttons_and_grid_are_the_webs_size() {
    let mut app = start_gallery();
    // split: 32 high, the colour part 28 wide, the ▾ 18 (DR `v1`/`x1`/`C1`)
    assert_eq!(app.frame("Font Color").size.width.0, 28.0);
    assert_eq!(app.frame("Font Color choices").size.width.0, 18.0);
    assert_eq!(app.parent_frame("Font Color").size.height.0, 32.0);
    // single: 40×32 (DR `m1`)
    let h = app.frame("Text Highlight Color");
    assert_eq!((h.size.width.0, h.size.height.0), (40.0, 32.0));
    app.press("Font Color choices");
    let sw = app.frame("Red");
    assert_eq!((sw.size.width.0, sw.size.height.0), (18.0, 18.0));
    assert_eq!(app.frame("Orange").origin.x.0 - sw.origin.x.0, 20.0, "18 cells 2 apart (`yc`, `d1`)");
    app.cancel("Font Color picker");
    app.press("Insert table");
    let c = app.frame("row 1 column 1");
    assert_eq!((c.size.width.0, c.size.height.0), (18.0, 18.0));
    assert_eq!(app.frame("row 1 column 2").origin.x.0 - c.origin.x.0, 20.0, "`Bd = 18`, `qk = 2` (DR `gc`)");
    assert_eq!(app.style("row 1 column 1").border_radius.0, 2.0);
}

#[test]
fn menus_and_dialogs_are_the_webs_size() {
    let mut app = start_gallery();
    // the menu bar's trigger: 28 high, radius 4 (DR `zh`)
    assert_eq!(app.frame("File").size.height.0, 28.0);
    assert_eq!(app.style("File").border_radius.0, 4.0);
    app.press("File");
    // the menu: at least 200 wide, rows full width from its left (DR `Ba`)
    let save = app.frame("Save");
    assert!(save.size.width.0 >= 200.0, "{save:?}");
    assert_eq!(save.origin.x.0, 0.0);
    // a disabled row is dimmed to .4 (DR `Nk`)
    assert!((app.style("Page setup").opacity - 0.4).abs() < 1e-6);
    app.cancel("File menu");
    app.press("Open dialog");
    // Page Setup's card: 400–480 wide, radius 8 (DR `PageSetupDialog` `Q`)
    let card = app.frame("Page Setup dialog");
    assert_eq!(card.size.width.0, 480.0);
    assert_eq!(app.style("Page Setup dialog").border_radius.0, 8.0);
    // a form row's label is 80 wide (`c`)
    assert_eq!(app.parent_frame("Name").origin.x.0, 80.0 + 12.0);
}

// ---- what they do ----

#[test]
fn a_toggle_shows_its_state_in_the_webs_colours() {
    let mut app = start_gallery();
    assert_eq!(bg(&app, "Bold"), nsh_core::Color::from(AdaptiveColor::TRANSPARENT.light));
    app.press("Bold");
    until(&mut app, "bold on", |p| is(p, "bold", "true"));
    app.pump(Duration::from_millis(50));
    assert_eq!(bg(&app, "Bold"), light(p::TOGGLE_ACTIVE_BG));
    // a disabled button takes no press
    app.press("Italic");
    app.press("Bold");
    until(&mut app, "bold off", |p| is(p, "bold", "false"));
}

#[test]
fn menus_open_choose_cascade_and_close() {
    let mut app = start_gallery();
    app.press("File");
    assert!(app.has("File menu") && app.has("Save"));
    assert!(app.texts().contains(&"⌘S"), "the shortcut column: {:?}", app.texts());
    // a disabled row does nothing
    app.press("Page setup");
    assert!(app.has("File menu"));
    app.press("Save");
    until(&mut app, "Save chosen", |p| is(p, "menu", "save"));
    assert!(!app.has("File menu"), "choosing closes the menu");
    // Escape closes it
    app.press("File");
    app.cancel("File menu");
    assert!(!app.has("File menu"));
    // a submenu opens while the pointer is on its row, closes on another row
    app.press("Insert");
    app.hover("Break", true);
    assert!(app.has("Break submenu") && app.has("Page break"));
    app.hover("Image", true);
    assert!(!app.has("Break submenu"));
    app.hover("Break", true);
    app.press("Section break (next page)");
    until(&mut app, "the submenu's row chosen", |p| is(p, "menu", "next-page"));
    assert!(!app.has("Insert menu"));
    // the menu's table grid
    app.press("Insert");
    app.hover("Table", true);
    app.hover("row 2 column 4", true);
    assert!(app.texts().contains(&"4 \u{d7} 2"), "{:?}", app.texts());
    app.press("row 2 column 4");
    until(&mut app, "the grid's size", |p| is(p, "grid", "2x4"));
    assert!(!app.has("Insert menu"));
}

#[test]
fn a_select_chooses_from_its_list() {
    let mut app = start_gallery();
    app.press("Select font family");
    assert!(app.texts().contains(&"Serif") && app.texts().contains(&"Document fonts"), "{:?}", app.texts());
    app.press("Georgia");
    until(&mut app, "Georgia chosen", |p| is(p, "font", "Georgia"));
    app.press("Select paragraph style");
    app.press("Heading 1");
    until(&mut app, "Heading 1 chosen", |p| is(p, "style", "Heading1"));
    app.press("Zoom in");
    until(&mut app, "zoomed in a step", |p| num(p, "zoom") == 1.25);
    app.press("Align");
    app.press("Center");
    until(&mut app, "centred", |p| is(p, "align", "center"));
    assert!(!app.has("Align choices"));
}

#[test]
fn the_colour_dropdown_picks_theme_standard_custom_and_clear() {
    let mut app = start_gallery();
    app.press("Font Color choices");
    assert!(app.has("Font Color picker"));
    app.press("Accent 1, Lighter 80%");
    until(&mut app, "a theme tint", |p| is(p, "color", "theme:accent1:CC:DAE3F3"));
    assert!(!app.has("Font Color picker"));
    // the colour part applies the last colour again
    app.press("Font Color choices");
    app.press("Blue");
    until(&mut app, "a standard colour", |p| is(p, "color", "rgb:0070C0"));
    app.press("Font Color choices");
    app.change("Custom hex color", "12ab34");
    app.press("Apply custom color");
    until(&mut app, "a typed colour", |p| is(p, "color", "rgb:12AB34"));
    app.press("Font Color choices");
    app.press("Automatic");
    until(&mut app, "automatic", |p| is(p, "color", "clear"));
    app.press("Text Highlight Color");
    assert!(app.has("No Color"), "a highlight clears to No Color");
    app.press("Yellow");
    until(&mut app, "a highlight", |p| is(p, "color", "rgb:FFFF00"));
}

#[test]
fn the_table_grid_gives_rows_by_columns() {
    let mut app = start_gallery();
    app.press("Insert table");
    assert!(app.texts().contains(&"Select size"));
    app.hover("row 3 column 2", true);
    assert!(app.texts().contains(&"2 \u{d7} 3"));
    assert_eq!(bg(&app, "row 3 column 2"), light(p::PRIMARY));
    assert_eq!(bg(&app, "row 4 column 2"), light(p::SURFACE));
    // leaving the grid clears it
    app.hover("Table size selector", false);
    assert!(app.texts().contains(&"Select size"));
    app.hover("row 3 column 2", true);
    app.press("row 3 column 2");
    until(&mut app, "3 rows of 2", |p| is(p, "grid", "3x2"));
    assert!(!app.has("Insert table grid"));
}

#[test]
fn a_dialog_confirms_on_return_cancels_on_escape_and_keeps_tab_inside() {
    let mut app = start_gallery();
    app.press("Open dialog");
    assert!(app.has("Page Setup dialog"));
    // Tab walks its three fields and comes round again
    for want in ["1", "2", "0"] {
        app.key("tab");
        until(&mut app, "the next field", |p| is(p, "tab", want));
    }
    app.key("shift+tab");
    until(&mut app, "back round", |p| is(p, "tab", "2"));
    app.press("Match case");
    until(&mut app, "checked", |p| is(p, "check", "true"));
    app.press("Bookmark");
    app.key("enter");
    until(&mut app, "Return confirmed", |p| is(p, "confirmed", "1") && is(p, "dialog", "false"));
    app.press("Open dialog");
    app.cancel("Page Setup");
    until(&mut app, "Escape cancelled", |p| is(p, "cancelled", "1") && is(p, "dialog", "false"));
    app.press("Open dialog");
    app.press("Dialog backdrop");
    until(&mut app, "the backdrop cancelled", |p| is(p, "cancelled", "2"));
    app.press("Open dialog");
    app.press("Apply");
    until(&mut app, "Apply", |p| is(p, "confirmed", "2"));
}

#[test]
fn the_size_box_steps_lists_and_takes_a_typed_size() {
    let mut app = start_gallery();
    app.press("Increase font size");
    until(&mut app, "the next preset", |p| num(p, "size") == 12.0);
    app.press("Decrease font size");
    until(&mut app, "back", |p| num(p, "size") == 11.0);
    // a press makes the size a field and opens the presets
    app.press("Select font size");
    assert!(app.has("Font sizes"));
    app.change("Select font size", "13");
    app.submit("Select font size");
    until(&mut app, "the typed size", |p| num(p, "size") == 13.0);
    assert!(!app.has("Font sizes"));
    app.press("Select font size");
    app.press("36");
    until(&mut app, "a preset", |p| num(p, "size") == 36.0);
    app.press("Select font size");
    app.press("Custom size…");
    until(&mut app, "the custom row", |p| is(p, "custom", "1"));
    let p = read(&app);
    assert_eq!(num(&p, "size"), 36.0);
}

// ---- labels and colours ----

#[test]
fn shortcuts_read_as_the_web_writes_them() {
    assert_eq!(shortcut_label("Mod+B", true), "⌘B");
    assert_eq!(shortcut_label("Mod+Shift+=", true), "⌘⇧=");
    assert_eq!(shortcut_label("Mod+Alt+Shift+k", true), "⌘⌥⇧K");
    assert_eq!(shortcut_label("Mod+Shift+=", false), "Ctrl+Shift+=");
    assert_eq!(shortcut_label("Mod++", false), "Ctrl++");
    assert_eq!(tip_text("Bold", Some("Mod+B"), true), "Bold (⌘B)");
    assert_eq!(tip_text("Strikethrough", None, true), "Strikethrough");
    assert_eq!(button::TIP_DELAY_MS, 400.0);
}

/// Each token against the stylesheet's hex (`.oox-root` / `.oox-root.dark`).
#[test]
fn the_palette_is_the_stylesheets() {
    use std::str::FromStr;
    let table: &[(AdaptiveColor, &str, &str)] = &[
        (p::SURFACE, "#ffffff", "#1f1f1f"),
        (p::CARD, "#f8fbff", "#2d2d2d"),
        (p::BG, "#f8f9fa", "#262626"),
        (p::BG_SUBTLE, "#f5f5f5", "#2d2d2d"),
        (p::BG_HOVER, "#f1f3f4", "#3a3a3a"),
        (p::BG_INPUT, "#f8f9fa", "#333333"),
        (p::PRIMARY, "#1a73e8", "#4e9dff"),
        (p::PRIMARY_HOVER, "#1557b0", "#6cb0ff"),
        (p::PRIMARY_LIGHT, "#e8f0fe", "#4e9dff2e"),
        (p::ACCENT, "#6366f1", "#4e9dff"),
        (p::ACCENT_BG, "#eef2ff", "#1f3047"),
        (p::ON_PRIMARY, "#ffffff", "#1f1f1f"),
        (p::TEXT, "#202124", "#e8e8e8"),
        (p::TEXT_MUTED, "#5f6368", "#a0a0a0"),
        (p::TEXT_SUBTLE, "#9aa0a6", "#6e6e6e"),
        (p::TEXT_PLACEHOLDER, "#999999", "#6e6e6e"),
        (p::BORDER, "#e0e0e0", "#3c3c3c"),
        (p::BORDER_LIGHT, "#dadce0", "#353535"),
        (p::BORDER_DARK, "#d0d0d0", "#555555"),
        (p::BORDER_INPUT, "#cccccc", "#4a4a4a"),
        (p::ERROR, "#c5221f", "#f28b82"),
        (p::ERROR_BG, "#fce8e6", "#3c1f1d"),
        (p::SUCCESS, "#34a853", "#81c995"),
        (p::SUCCESS_BG, "#e8f5e9", "#1f2e22"),
        (p::WARNING, "#f9a825", "#fdd663"),
        (p::WARNING_BG, "#fff8e1", "#332a1a"),
        (p::LINK, "#0563c1", "#6eb9ff"),
        (p::PAGE_BG, "#ffffff", "#cccccc"),
        (p::CARET, "#000000", "#e8e8e8"),
    ];
    let rgba = |c: Hsla| {
        let r = Rgba::from(c);
        [r.r, r.g, r.b, r.a]
    };
    for (i, (c, l, d)) in table.iter().enumerate() {
        for (got, want) in [(c.light, l), (c.dark, d)] {
            let (g, w) = (rgba(got), rgba(Hsla::from_str(want).unwrap()));
            assert!(g.iter().zip(w).all(|(a, b)| (a - b).abs() < 0.003), "token {i}: {g:?} is not {want}");
        }
    }
}
