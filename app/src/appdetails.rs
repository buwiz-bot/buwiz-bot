//! An artifact's identity: the icon tile every app row draws, the
//! Library's tile for any kind, and Edit Details — one modal, adapted by
//! kind, for the icon (generate / upload / paste for a mini app; upload /
//! paste / thumbnail for an image or a file), title, file name,
//! description and an app's window size, over the facts the server keeps
//! (`/app/info`, `/artifact/info`). A file kept by a connected app (Google
//! Drive) shows its title — a rename goes to Drive — and where it lives.
//! A server's app attached to a chat (`mcp-app`, a tax profile say) shows
//! its picture, name, subtitle, facts and versions, and renames in Rox
//! only.

use nsh_base::input;
use nsh_core::prelude::*;

use crate::icons::icon;
use crate::live::store;
use crate::store::{app_name, AppInfo, Attach, RoxApp, Store};
use crate::theme;

/// The app's icon at `size`: its generated (or uploaded) image, else the
/// default — the title's initial on a gradient of the app's hue.
pub(crate) fn app_icon(app: &str, size: f32) -> impl IntoElement {
    let app = app_name(app).to_owned();
    let radius = (size * 0.225).round();
    dynamic(move || {
        let meta = store().meta_of(&app).unwrap_or_default();
        if !meta.icon.is_empty() {
            return view()
                .size(px(size))
                .rounded(px(radius))
                .overflow_hidden()
                .child(image(meta.icon.clone()).size(px(size)))
                .into_element();
        }
        let base = crate::plugins::hue(&app);
        let top = Hsla { l: (base.l + 0.10).min(0.92), ..base };
        let bottom = Hsla { l: (base.l - 0.06).max(0.08), ..base };
        let initial: String = meta
            .title
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_else(|| "</>".to_owned());
        view()
            .size(px(size))
            .rounded(px(radius))
            .items_center()
            .justify_center()
            .bg_linear_to_b()
            .gradient_from(AdaptiveColor::from(top))
            .gradient_to(AdaptiveColor::from(bottom))
            .child(
                text(initial)
                    .text_size(px((size * 0.44).round()))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(hsla(0.0, 0.0, 1.0, 1.0)),
            )
            .into_element()
    })
}

/// A Library item's square tile at `size`: an app's icon; an image's
/// set icon, else its picture; a file's set icon, else its type tile (or
/// its own thumbnail); a connector's file its type tile with the app's
/// badge.
pub(crate) fn artifact_tile(a: &RoxApp, size: f32) -> impl IntoElement {
    if a.kind == "mcp-app" {
        let name = if a.details.name.is_empty() { a.title.clone() } else { a.details.name.clone() };
        return named_tile(&a.image, &name, size).into_element();
    }
    if a.storage.connector() {
        return connector_tile(&a.kind, &a.filename, size).into_element();
    }
    if a.kind == "app" {
        return app_icon(&a.app, size).into_element();
    }
    let pic = if !a.icon.is_empty() {
        a.icon.clone()
    } else if a.attach.is_none() && a.kind == "image" {
        format!("{}/asset/generated/assets/{}", crate::client::backend(), a.file)
    } else {
        String::new()
    };
    if !pic.is_empty() {
        return view()
            .size(px(size))
            .rounded(px((size * 0.225).round()))
            .overflow_hidden()
            .bg(theme::CARD2)
            .child(image(pic).size(px(size)).object_fit(ResizeMode::Cover))
            .into_element();
    }
    match &a.attach {
        Some(f) => crate::files::file_icon(f, size).into_element(),
        None => app_icon(&a.app, size).into_element(),
    }
}

/// An attached server app's tile: its picture, else its name's initial on
/// a gradient of the name's hue (as an app's default icon).
pub(crate) fn named_tile(image_url: &str, name: &str, size: f32) -> impl IntoElement {
    let radius = (size * 0.225).round();
    if !image_url.is_empty() {
        return view()
            .size(px(size))
            .rounded(px(radius))
            .overflow_hidden()
            .child(image(image_url.to_owned()).size(px(size)).object_fit(ResizeMode::Cover))
            .into_element();
    }
    let base = crate::plugins::hue(name);
    let top = Hsla { l: (base.l + 0.10).min(0.92), ..base };
    let bottom = Hsla { l: (base.l - 0.06).max(0.08), ..base };
    let initial: String =
        name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".to_owned());
    view()
        .size(px(size))
        .rounded(px(radius))
        .items_center()
        .justify_center()
        .bg_linear_to_b()
        .gradient_from(AdaptiveColor::from(top))
        .gradient_to(AdaptiveColor::from(bottom))
        .child(
            text(initial)
                .text_size(px((size * 0.44).round()))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(hsla(0.0, 0.0, 1.0, 1.0)),
        )
        .into_element()
}

/// A connected app's file (Google Drive): its kind's type tile — nothing
/// local to draw a thumbnail from — with the Google tile as a badge at
/// the bottom right.
pub(crate) fn connector_tile(kind: &str, name: &str, size: f32) -> impl IntoElement {
    let ext = match kind {
        "document" => "docx".to_owned(),
        "spreadsheet" => "xlsx".to_owned(),
        "presentation" => "pptx".to_owned(),
        "pdf" => "pdf".to_owned(),
        "image" => "png".to_owned(),
        "media" => "mp4".to_owned(),
        _ => crate::files::ext_of(name, ""),
    };
    let f = Attach { name: name.to_owned(), ext, ..Attach::default() };
    let badge = (size * 0.42).round().max(11.0);
    view()
        .size(px(size))
        .child(crate::files::file_icon(&f, size))
        .child(
            view()
                .absolute()
                .right(px(-2.0))
                .bottom(px(-2.0))
                .child(crate::plugins::plug_tile("google-workspace", badge)),
        )
}

/// The kind's name in the modal's header and facts.
fn kind_label(kind: &str) -> &'static str {
    match kind {
        "app" | "mcp-app" => "App",
        "image" => "Image",
        "document" => "Document",
        "presentation" => "Presentation",
        "spreadsheet" => "Spreadsheet",
        "pdf" => "PDF",
        "media" => "Media",
        _ => "File",
    }
}

/// The open modal's item, read once as the sheet builds.
fn current() -> AppInfo {
    store().app_details.get_untracked().unwrap_or_default()
}

/// The modal, drawn while `app_details` is `Some`.
pub(crate) fn layer() -> impl IntoElement {
    when(|| store().app_details.get().is_some(), || sheet().into_element())
}

fn close() {
    store().app_details.set(None);
}

fn sheet() -> impl IntoElement {
    view()
        .size_full()
        .absolute()
        .items_center()
        .justify_center()
        .child(
            view()
                .size_full()
                .absolute()
                .bg(hsla(0.0, 0.0, 0.0, 0.5))
                .on_press(close),
        )
        .child(
            view()
                .w(px(500.0))
                .max_h(px(720.0))
                .rounded(px(14.0))
                .bg(theme::MENU_BG)
                .border_1()
                .border_color(theme::BORDER_S)
                .overflow_hidden()
                .on_press(|| {})
                .child(header())
                .child(
                    // The header and footer are 55pt each.
                    scroll().max_h(px(720.0 - 110.0)).child(
                        view()
                            .px_4()
                            .pb_4()
                            .gap(px(14.0))
                            .when(current().kind == "mcp-app", |v| {
                                v.child(mcp_identity()).child(mcp_fields()).child(mcp_facts())
                            })
                            .when(current().kind != "mcp-app", |v| {
                                v.child(identity()).child(fields()).child(facts())
                            }),
                    ),
                )
                .child(footer()),
        )
}

fn header() -> impl IntoElement {
    let hov = signal(false);
    let title = format!("{} Details", kind_label(&current().kind));
    view()
        .h(px(55.0))
        .flex_row()
        .items_center()
        .pl_4()
        .pr(px(10.0))
        .child(
            text(title)
                .flex_1()
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::TEXT),
        )
        .child(
            view()
                .size(px(32.0))
                .rounded(px(16.0))
                .items_center()
                .justify_center()
                .bg(move || if hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                .child(icon("close"))
                .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                .on_press(close)
                .label("Close dialog"),
        )
}

/// The 84pt icon with its sources — a mini app's Generate / Upload /
/// Paste / Remove and the prompt the generator draws from; an image's or
/// file's Upload / Paste and Use thumbnail (or Remove).
fn identity() -> impl IntoElement {
    let cur = current();
    let name = cur.name.clone();
    let app = cur.kind == "app";
    let remote = cur.storage.connector();
    let thumb = cur.thumb;
    let ext = cur.ext.clone();
    let kind_line = format!("{} · {}", cur.meta.file_type, cur.storage.place_name());
    let tile = {
        let name = name.clone();
        let (kind, filename) = (cur.kind.clone(), cur.filename.clone());
        dynamic(move || {
            if remote {
                return connector_tile(&kind, &filename, 84.0).into_element();
            }
            if app {
                return app_icon(&name, 84.0).into_element();
            }
            // the Library's record carries the tile's sources
            match store().thread_apps.get().into_iter().find(|a| a.app == name) {
                Some(a) => artifact_tile(&a, 84.0).into_element(),
                None => app_icon(&name, 84.0).into_element(),
            }
        })
    };
    view()
        .gap(px(10.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(14.0))
                .child(
                    view()
                        .size(px(84.0))
                        .child(tile)
                        .child(when(
                            || store().ad_busy.get() == "gen" || store().ad_busy.get() == "pick",
                            || {
                                view()
                                    .absolute()
                                    .size(px(84.0))
                                    .rounded(px(19.0))
                                    .items_center()
                                    .justify_center()
                                    .bg(hsla(0.0, 0.0, 0.0, 0.55))
                                    .child(
                                        text(|| {
                                            if store().ad_busy.get() == "gen" { "Drawing…" } else { "Loading…" }
                                        })
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(hsla(0.0, 0.0, 1.0, 1.0)),
                                    )
                                    .into_element()
                            },
                        )),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap(px(2.0))
                        .child(
                            text(move || {
                                let t = store().ad_title.get();
                                if !t.trim().is_empty() {
                                    t
                                } else if app {
                                    "Untitled app".to_owned()
                                } else {
                                    "Untitled".to_owned()
                                }
                            })
                            .text_size(px(16.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT)
                            .number_of_lines(1),
                        )
                        .child(
                            text(move || {
                                let st = store();
                                let d = st.ad_desc.get();
                                if remote {
                                    kind_line.clone()
                                } else if d.trim().is_empty() {
                                    format!("{}.{ext}", st.ad_file.get())
                                } else {
                                    d
                                }
                            })
                            .text_size(px(12.0))
                            .line_height(px(16.0))
                            .text_color(theme::MUTED)
                            .number_of_lines(2),
                        )
                        // a connector's file has no icon of its own here
                        .when(!remote, |v| v.child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .gap(px(6.0))
                                .mt(px(8.0))
                                .when(app, |v| v.child(generate_pill()))
                                .child(pill("Upload", "upload", || store().pick_app_icon("pick")))
                                .child(pill("Paste", "copy", || store().pick_app_icon("paste")))
                                .child(when(
                                    || store().app_details.get().is_some_and(|i| !i.icon.is_empty()),
                                    move || {
                                        // without its own icon an image or a
                                        // file shows its thumbnail again
                                        if thumb {
                                            pill("Use thumbnail", "imageSquare", || store().clear_app_icon()).into_element()
                                        } else {
                                            pill("Remove", "trash", || store().clear_app_icon()).into_element()
                                        }
                                    },
                                )),
                        )),
                ),
        )
        .when(app, |v| v.child(icon_prompt()))
        .child(when(
            || !store().ad_err.get().is_empty(),
            || {
                text(|| store().ad_err.get())
                    .text_size(px(12.0))
                    .text_color(theme::DOWN)
                    .into_element()
            },
        ))
}

/// A mini app's icon prompt, and why Generate is off when it is.
fn icon_prompt() -> impl IntoElement {
    view()
        .gap(px(10.0))
        .child(
            field_box(
                input()
                    .value(|| store().ad_prompt.get())
                    .on_change(|v: &str| store().ad_prompt.set(v.to_owned()))
                    .on_submit(|_: &str| store().gen_app_icon())
                    .on_key_press(escape_closes)
                    .placeholder("Describe the icon, or leave empty to draw from the title")
                    .placeholder_color(|| color(theme::GHOST))
                    .text_size(px(13.0))
                    .text_color(theme::TEXT)
                    .h(px(30.0)),
            ),
        )
        .child(when(
            || !store().icon_model().2,
            || {
                text(|| {
                    format!(
                        "{} can't generate images. Pick an OpenAI model for this chat to draw icons, or upload one.",
                        store().icon_model().1
                    )
                })
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(theme::MUTED)
                .into_element()
            },
        ))
}

fn fields() -> impl IntoElement {
    let cur = current();
    let app = cur.kind == "app";
    let ext = format!(".{}", cur.ext);
    // A connector's file: its title is its name there; Save renames it.
    if cur.storage.connector() {
        return view()
            .gap(px(6.0))
            .child(row(
                "Title",
                field_box(
                    input()
                        .value(|| store().ad_title.get())
                        .on_change(|v: &str| store().ad_title.set(v.to_owned()))
                        .on_submit(|_: &str| store().save_app_details())
                        .on_key_press(escape_closes)
                        .auto_focus(true)
                        .placeholder("File name")
                        .placeholder_color(|| color(theme::GHOST))
                        .text_size(px(13.0))
                        .text_color(theme::TEXT)
                        .h(px(30.0)),
                ),
            ))
            .child(
                text(format!("Saving renames the file in {}.", cur.storage.place_name()))
                    .pl(px(94.0))
                    .text_size(px(12.0))
                    .text_color(theme::MUTED),
            )
            .into_element();
    }
    view()
        .gap(px(10.0))
        .child(row(
            "Title",
            field_box(
                input()
                    .value(|| store().ad_title.get())
                    .on_change(|v: &str| store().ad_title.set(v.to_owned()))
                    .on_submit(|_: &str| store().save_app_details())
                    .on_key_press(escape_closes)
                    .auto_focus(true)
                    .placeholder(if app { "App title" } else { "Title" })
                    .placeholder_color(|| color(theme::GHOST))
                    .text_size(px(13.0))
                    .text_color(theme::TEXT)
                    .h(px(30.0)),
            ),
        ))
        .child(row(
            "File name",
            field_box(
                view()
                    .flex_row()
                    .items_center()
                    .child(
                        input()
                            .value(|| store().ad_file.get())
                            .on_change(|v: &str| store().ad_file.set(v.to_owned()))
                            .on_submit(|_: &str| store().save_app_details())
                            .on_key_press(escape_closes)
                            .placeholder("file-name")
                            .placeholder_color(|| color(theme::GHOST))
                            .text_size(px(13.0))
                            .text_color(theme::TEXT)
                            .flex_1()
                            .h(px(30.0)),
                    )
                    .when(ext.len() > 1, |v| v.child(text(ext).text_size(px(13.0)).text_color(theme::GHOST))),
            ),
        ))
        .child(row(
            "Description",
            field_box(
                input()
                    .value(|| store().ad_desc.get())
                    .on_change(|v: &str| store().ad_desc.set(v.to_owned()))
                    .on_key_press(escape_closes)
                    .multiline(true)
                    .placeholder(if app { "What the app does" } else { "What it is" })
                    .placeholder_color(|| color(theme::GHOST))
                    .text_size(px(13.0))
                    .text_color(theme::TEXT)
                    .h(px(58.0))
                    .py(px(7.0)),
            ),
        ))
        .when(app, |v| {
            v.child(row(
                "Window",
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(size_field(store().ad_w, "Width"))
                    .child(text("×").text_size(px(13.0)).text_color(theme::GHOST))
                    .child(size_field(store().ad_h, "Height"))
                    .child(text("pt").text_size(px(12.0)).text_color(theme::GHOST)),
            ))
        })
        .into_element()
}

/// The server's facts, read-only, by kind: a mini app's version, window,
/// source and prompt; an image's size and what drew it; a file's pages /
/// slides / sheets, length and size — each with where it was made (back to
/// the chat) and where it is stored.
fn facts() -> impl IntoElement {
    dynamic(|| {
        let Some(i) = store().app_details.get() else {
            return view().into_element();
        };
        let ago = |ms: f64| {
            let a = Store::ago(ms);
            if a.is_empty() { "—".to_owned() } else { format!("{a} ago") }
        };
        let made_in = match (i.bot_name.is_empty(), i.thread_title.is_empty()) {
            (false, false) => format!("{} · {}", i.bot_name, i.thread_title),
            (false, true) => i.bot_name.clone(),
            (true, false) => i.thread_title.clone(),
            (true, true) => "—".to_owned(),
        };
        let size = if i.bytes > 0.0 { crate::files::fmt_size(i.bytes) } else { "—".to_owned() };
        let plural = |n: i64, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
        let m = &i.meta;
        let mut rows: Vec<(&'static str, String)> = Vec::new();
        let remote = i.storage.connector();
        match i.kind.as_str() {
            // a connector's file: what it is and what the AI last did to it
            _ if remote => {
                rows.push(("Type", if m.file_type.is_empty() { kind_label(&i.kind).to_owned() } else { m.file_type.clone() }));
                let mut did = m.action.clone();
                if let Some(c) = did.get(..1) {
                    did = format!("{}{}", c.to_uppercase(), &did[1..]);
                }
                if !did.is_empty() {
                    rows.push(("Last change", format!("{did} · {}", ago(i.updated_at))));
                }
                rows.push(("Created", ago(i.created_at)));
            }
            "app" => {
                rows.push(("ID", i.name.clone()));
                rows.push(("Created", ago(i.created_at)));
                rows.push(("Updated", ago(i.updated_at)));
                rows.push(("Published", if i.published { "Yes".to_owned() } else { "No".to_owned() }));
                rows.push(("Source", size));
            }
            "image" => {
                if m.w > 0.0 {
                    rows.push(("Size", format!("{}×{}", m.w, m.h)));
                }
                rows.push(("Made with", if m.made_with.is_empty() { "—".to_owned() } else { m.made_with.clone() }));
                rows.push(("File", size));
                rows.push(("Created", ago(i.created_at)));
            }
            _ => {
                if m.pages > 0 {
                    rows.push(("Pages", plural(m.pages, "page")));
                }
                if m.slides > 0 {
                    rows.push(("Slides", plural(m.slides, "slide")));
                }
                if !m.sheets.is_empty() {
                    rows.push(("Sheets", format!("{} — {}", m.sheets.len(), m.sheets.join(", "))));
                }
                if m.duration > 0.0 {
                    let s = m.duration.round() as i64;
                    rows.push(("Length", format!("{}:{:02}", s / 60, s % 60)));
                }
                rows.push(("Size", size));
                rows.push(("Created", ago(i.created_at)));
            }
        }
        rows.push(("Stored on", i.storage.place_name().to_owned()));
        if !i.path.is_empty() {
            rows.push(("Path", i.path.clone()));
        }
        let mut list = view()
            .rounded(px(10.0))
            .bg(theme::SUBTLE)
            .px(px(12.0))
            .py(px(8.0))
            .gap(px(5.0));
        // Version: the picker chip (switch from here), then the count.
        if i.kind == "app" || i.v > 0 {
            let name = i.name.clone();
            list = list.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .child(text("Version").w(px(72.0)).text_size(px(12.0)).text_color(theme::MUTED))
                    .child(crate::versions::version_chip(name, || {
                        store()
                            .app_details
                            .get()
                            .map(|i| (i.v, i.history))
                            .unwrap_or_default()
                    }))
                    .child(
                        text(if i.versions == 1 { "1 version".to_owned() } else { format!("{} versions", i.versions) })
                            .text_size(px(12.0))
                            .text_color(theme::MUTED),
                    ),
            );
        }
        list = list.child(made_in_fact("Made in", made_in));
        for (k, v) in rows {
            list = list.child(fact(k, v, 1));
        }
        // its versions are the provider's: Drive's history, by its link
        if i.versioning == "provider" && !i.storage.link.is_empty() {
            list = list.child(link_fact("Versions", format!("Version history in {} ↗", i.storage.place_name()), i.storage.link.clone()));
        }
        if !i.prompt.trim().is_empty() {
            list = list.child(fact("Prompt", i.prompt.trim().to_owned(), 4));
        }
        view()
            .gap(px(6.0))
            .child(
                text(if !remote && (i.kind == "app" || i.kind == "image") { "Generated" } else { "Details" })
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::MUTED),
            )
            .child(list)
            .into_element()
    })
}

// ——————————————— an attached server app (`mcp-app`) ———————————————

/// Its picture (or the name's tile), name and subtitle — the TIN and
/// branch of a tax profile.
fn mcp_identity() -> impl IntoElement {
    let cur = current();
    let name = if cur.details.name.is_empty() { cur.title.clone() } else { cur.details.name.clone() };
    let subtitle = if cur.details.subtitle.is_empty() { cur.token.clone() } else { cur.details.subtitle.clone() };
    view()
        .gap(px(10.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(14.0))
                .child(named_tile(&cur.image, &name, 84.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap(px(2.0))
                        .child(
                            text(move || {
                                let t = store().ad_title.get();
                                if t.trim().is_empty() { name.clone() } else { t }
                            })
                            .text_size(px(16.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT)
                            .number_of_lines(1),
                        )
                        .child(
                            text(subtitle)
                                .text_size(px(12.0))
                                .line_height(px(16.0))
                                .text_color(theme::MUTED)
                                .number_of_lines(2),
                        ),
                ),
        )
        .child(when(
            || !store().ad_err.get().is_empty(),
            || {
                text(|| store().ad_err.get())
                    .text_size(px(12.0))
                    .text_color(theme::DOWN)
                    .into_element()
            },
        ))
}

/// Rename: the name Rox shows for it, in every chat and pin; empty goes
/// back to the server's.
fn mcp_fields() -> impl IntoElement {
    view()
        .gap(px(6.0))
        .child(row(
            "Name",
            field_box(
                input()
                    .value(|| store().ad_title.get())
                    .on_change(|v: &str| store().ad_title.set(v.to_owned()))
                    .on_submit(|_: &str| store().save_app_details())
                    .on_key_press(escape_closes)
                    .auto_focus(true)
                    .placeholder(current().details.name)
                    .placeholder_color(|| color(theme::GHOST))
                    .text_size(px(13.0))
                    .text_color(theme::TEXT)
                    .h(px(30.0)),
            ),
        ))
        .child(
            text("Renames it in Rox only; the app keeps its own name.")
                .pl(px(94.0))
                .text_size(px(12.0))
                .text_color(theme::MUTED),
        )
}

/// The server's facts, where it's attached (Show in Chat), and its
/// versions — a tax profile's years — each opening its link.
fn mcp_facts() -> impl IntoElement {
    let cur = current();
    let mut list = view()
        .rounded(px(10.0))
        .bg(theme::SUBTLE)
        .px(px(12.0))
        .py(px(8.0))
        .gap(px(5.0))
        .child(made_in_fact("Attached", "Its chats and bots".to_owned()));
    for (k, v) in cur.details.facts.iter().cloned() {
        list = list.child(fact_owned(k, v));
    }
    if !cur.token.is_empty() {
        list = list.child(fact("Token", cur.token.clone(), 2));
    }
    let mut out = view()
        .gap(px(6.0))
        .child(text("Details").text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::MUTED))
        .child(list);
    if !cur.details.versions.is_empty() {
        let mut versions = view().rounded(px(10.0)).bg(theme::SUBTLE).py(px(4.0));
        for (label, detail, link) in cur.details.versions.iter().cloned() {
            versions = versions.child(version_row(label, detail, link));
        }
        out = out
            .child(
                text("Versions")
                    .mt(px(8.0))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::MUTED),
            )
            .child(versions);
    }
    out
}

/// One of its versions (a tax year): label, detail, and a press opens it.
fn version_row(label: String, detail: String, link: String) -> impl IntoElement {
    let hov = signal(false);
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(12.0))
        .py(px(6.0))
        .bg(move || if hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .label(format!("Open {label}"))
        .child(text(label).w(px(72.0)).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT))
        .child(text(detail).flex_1().min_w(px(0.0)).text_size(px(12.0)).text_color(theme::MUTED).number_of_lines(1))
        .child(crate::icons::icon_ink("arrowUpRight", 12.0, || theme::MUTED))
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(move || {
            let st = store();
            st.app_details.set(None);
            match crate::store::mini_link(&link) {
                Some(key) => st.open_mini_full(&key, ""),
                None if !link.is_empty() => st.open_link(&link),
                None => {}
            }
        })
}

/// "Made in": the bot and chat, with Show in Chat back to the message (an
/// attached app's: the chats it's attached in).
fn made_in_fact(k: &'static str, v: String) -> impl IntoElement {
    let hov = signal(false);
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .child(text(k).w(px(72.0)).text_size(px(12.0)).line_height(px(17.0)).text_color(theme::MUTED))
        .child(
            text(v)
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(12.0))
                .line_height(px(17.0))
                .text_color(theme::TEXT)
                .number_of_lines(1),
        )
        .child(
            text("Show in Chat")
                .text_size(px(12.0))
                .line_height(px(17.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || if hov.get() { theme::TEXT } else { theme::DIM })
                .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                .on_press(|| store().show_details_origin())
                .label("Show in Chat"),
        )
}

/// A fact that is a link out of Rox (a connector's version history).
fn link_fact(k: &'static str, v: String, url: String) -> impl IntoElement {
    let hov = signal(false);
    view()
        .flex_row()
        .gap(px(10.0))
        .child(text(k).w(px(72.0)).text_size(px(12.0)).line_height(px(17.0)).text_color(theme::MUTED))
        .child(
            text(v)
                .text_size(px(12.0))
                .line_height(px(17.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || if hov.get() { theme::TEXT } else { theme::DIM })
                .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                .on_press(move || store().open_link(&url))
                .label("Open version history"),
        )
}

/// A server's fact: its own label.
fn fact_owned(k: String, v: String) -> impl IntoElement {
    view()
        .flex_row()
        .gap(px(10.0))
        .child(text(k).w(px(72.0)).text_size(px(12.0)).line_height(px(17.0)).text_color(theme::MUTED).number_of_lines(1))
        .child(
            text(v)
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(12.0))
                .line_height(px(17.0))
                .text_color(theme::TEXT)
                .number_of_lines(2),
        )
}

fn fact(k: &'static str, v: String, lines: u32) -> impl IntoElement {
    view()
        .flex_row()
        .gap(px(10.0))
        .child(text(k).w(px(72.0)).text_size(px(12.0)).line_height(px(17.0)).text_color(theme::MUTED))
        .child(
            text(v)
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(12.0))
                .line_height(px(17.0))
                .text_color(theme::TEXT)
                .number_of_lines(lines),
        )
}

fn footer() -> impl IntoElement {
    view()
        .flex_row()
        .justify_end()
        .gap_2()
        .px_4()
        .py(px(12.0))
        .border_t(px(0.5))
        .border_t_color(theme::BORDER_F)
        .child(button("Cancel", false, close))
        .child(button("Save", true, || store().save_app_details()))
}

fn row(label: &'static str, field: impl IntoElement) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .child(text(label).w(px(84.0)).text_size(px(13.0)).text_color(theme::DIM))
        .child(view().flex_1().min_w(px(0.0)).child(field))
}

fn field_box(inner: impl IntoElement) -> impl IntoElement {
    view()
        .rounded(px(8.0))
        .border_1()
        .border_color(theme::BORDER_S)
        .justify_center()
        .px(px(10.0))
        .child(inner)
}

fn size_field(sig: Signal<String>, placeholder: &'static str) -> impl IntoElement {
    view().w(px(86.0)).child(field_box(
        input()
            .value(move || sig.get())
            .on_change(move |v: &str| sig.set(v.chars().filter(char::is_ascii_digit).collect()))
            .on_submit(|_: &str| store().save_app_details())
            .on_key_press(escape_closes)
            .placeholder(placeholder)
            .placeholder_color(|| color(theme::GHOST))
            .text_size(px(13.0))
            .text_color(theme::TEXT)
            .h(px(30.0)),
    ))
}

/// A focused field keeps Escape for itself: close from here.
fn escape_closes(key: &str) {
    if key == "Escape" {
        close();
    }
}

fn pill(label: &'static str, glyph: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    let hov = signal(false);
    view()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .h(px(28.0))
        .px(px(11.0))
        .rounded(px(14.0))
        .bg(move || if hov.get() { theme::HOVER } else { theme::SUBTLE })
        .opacity(|| if store().ad_busy.get().is_empty() { 1.0f32 } else { 0.5 })
        .child(crate::icons::icon_ink(glyph, 12.0, || theme::TEXT))
        .child(
            text(label)
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::TEXT),
        )
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(on)
}

/// Generate, live only when the chat's model reaches an image model.
fn generate_pill() -> impl IntoElement {
    let hov = signal(false);
    let can = || store().icon_model().2;
    view()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .h(px(28.0))
        .px(px(11.0))
        .rounded(px(14.0))
        .bg(move || if can() { theme::TEXT } else { theme::SUBTLE })
        .opacity(move || {
            if !can() || !store().ad_busy.get().is_empty() { 0.5f32 } else if hov.get() { 0.9 } else { 1.0 }
        })
        .child(crate::icons::icon_ink("sparkles", 12.0, move || if can() { theme::POPOVER } else { theme::DIM }))
        .child(
            text(|| if store().ad_busy.get() == "gen" { "Drawing…" } else { "Generate" })
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || if can() { theme::POPOVER } else { theme::DIM }),
        )
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(|| store().gen_app_icon())
        .label("Generate icon")
}

fn button(label: &'static str, primary: bool, on: impl Fn() + 'static) -> impl IntoElement {
    view()
        .h(px(30.0))
        .px_3()
        .rounded(px(15.0))
        .items_center()
        .justify_center()
        .bg(if primary { theme::TEXT } else { theme::SUBTLE })
        .child(
            text(move || {
                if primary && store().ad_busy.get() == "save" { "Saving…" } else { label }
            })
            .text_size(px(13.0))
            .text_color(if primary { theme::POPOVER } else { theme::TEXT }),
        )
        .on_press(on)
}
