//! Settings › Logins: the saved logins on the left, the picked one's
//! details on the right — site, username, kind, when it was added and last
//! used — with Delete (pressed twice). The password is dots until Show,
//! which asks for Touch ID first; it hides again after half a minute, or
//! when another row is picked.
//!
//! Mounted by `settings.rs` as a page: [`settings_page`] is the whole page
//! body, and loads the list itself when it mounts.

use std::time::Duration;

use nsh_core::prelude::*;

use super::{KIND_CODE, KIND_PASSKEY, Secret, SiteLogin, Unlock, off_main, vault};
use crate::theme;

/// How long a shown password stays on screen.
const SHOWN_FOR: Duration = Duration::from_secs(30);

/// The page's state, anchored (like the store) so a hot patch or a
/// remount of Settings keeps the pick.
#[derive(Clone, Copy)]
struct Ui {
    rows: Signal<Vec<SiteLogin>>,
    sel: Signal<String>,
    /// The row id and what Show revealed (a password, or a code).
    shown: Signal<Option<(String, Secret)>>,
    /// The row Delete was pressed once on.
    confirm: Signal<String>,
    /// A sentence under the details (a Touch ID refusal, a failed delete).
    note: Signal<String>,
    busy: Signal<bool>,
}

fn ui() -> Ui {
    let at = ra_jit_live::anchor("rox.vault.page", || {
        let ui = Owner::detached().run(|| Ui {
            rows: signal(Vec::new()),
            sel: signal(String::new()),
            shown: signal(None),
            confirm: signal(String::new()),
            note: signal(String::new()),
            busy: signal(false),
        });
        Box::leak(Box::new(ui)) as *const Ui as usize
    });
    // SAFETY: the anchor holds a leaked `Ui` for the life of the process.
    unsafe { *(at as *const Ui) }
}

/// Reads the table again.
fn reload() {
    let u = ui();
    let rows = vault().list();
    if !rows.iter().any(|r| r.id == u.sel.get_untracked()) {
        u.sel.set(rows.first().map(|r| r.id.clone()).unwrap_or_default());
        u.shown.set(None);
    }
    u.rows.set(rows);
}

fn pick(id: String) {
    let u = ui();
    if u.sel.get_untracked() != id {
        u.shown.set(None);
        u.confirm.set(String::new());
        u.note.set(String::new());
    }
    u.sel.set(id);
}

/// Touch ID, then the password (or the current code) on screen for
/// [`SHOWN_FOR`].
fn show(row: SiteLogin) {
    let u = ui();
    if u.busy.get_untracked() {
        return;
    }
    u.busy.set(true);
    u.note.set(String::new());
    let what = if row.kind == KIND_CODE { "code" } else { "password" };
    let reason = format!("show the saved {what} for {} as {}", row.origin, row.username);
    let id = row.id.clone();
    Owner::detached().run(|| {
        spawn(async move {
            let got = off_main(move || vault().unlock(&row, &reason)).await;
            u.busy.set(false);
            let shown = match got {
                Some(Unlock::Secret(s)) if what == "code" => match super::totp::Totp::parse(s.expose()) {
                    Ok(t) => Secret::new(t.current().0),
                    Err(e) => return u.note.set(e),
                },
                Some(Unlock::Secret(s)) => s,
                Some(Unlock::Missing) => return u.note.set(format!("This Mac's keychain has no {what} for this login.")),
                Some(Unlock::Cancelled) | None => return,
                Some(Unlock::Refused(why)) => return u.note.set(format!("Touch ID didn't confirm: {why}")),
            };
            u.shown.set(Some((id.clone(), shown)));
            timer(SHOWN_FOR).await;
            if u.shown.with_untracked(|s| s.as_ref().is_some_and(|(i, _)| *i == id)) {
                u.shown.set(None);
            }
        });
    });
}

fn delete(id: String) {
    let u = ui();
    if u.confirm.get_untracked() != id {
        u.confirm.set(id);
        return;
    }
    u.confirm.set(String::new());
    u.shown.set(None);
    Owner::detached().run(|| {
        spawn(async move {
            match off_main(move || vault().delete(&id)).await {
                Some(Ok(())) => {}
                Some(Err(e)) => u.note.set(format!("Couldn't delete the login: {e}")),
                None => u.note.set("Couldn't delete the login".into()),
            }
            reload();
        });
    });
}

/// The Logins page body, for Settings to mount beside its other pages.
pub(crate) fn settings_page() -> impl IntoElement {
    reload();
    let u = ui();
    view()
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_4()
                .pt_4()
                .pb_2()
                .child(
                    text("Logins")
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::TEXT)
                        .flex_1(),
                )
                .child(small_btn("Refresh", false, reload)),
        )
        .child(
            text("Saved from sign-in forms an agent asked you to fill. Passwords stay in the macOS Keychain; showing one asks for Touch ID.")
                .text_size(px(12.0))
                .text_color(theme::MUTED)
                .px_4()
                .pb_2(),
        )
        .child(dynamic(move || {
            if u.rows.with(|r| r.is_empty()) {
                return text("No saved logins yet")
                    .text_size(px(13.0))
                    .text_color(theme::GHOST)
                    .px_4()
                    .py_3()
                    .into_element();
            }
            view()
                .flex_1()
                .flex_row()
                .min_h(px(0.0))
                .border_t(px(1.0))
                .border_color(theme::BORDER_F)
                .child(
                    scroll().w(px(250.0)).h_full().child(
                        view()
                            .p_2()
                            .gap(px(2.0))
                            .children(u.rows.get().into_iter().map(list_row)),
                    ),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .border_l(px(1.0))
                        .border_color(theme::BORDER_F)
                        .child(dynamic(move || {
                            let sel = u.sel.get();
                            match u.rows.with(|r| r.iter().find(|r| r.id == sel).cloned()) {
                                Some(row) => detail(row).into_element(),
                                None => text("No login selected")
                                    .text_size(px(13.0))
                                    .text_color(theme::GHOST)
                                    .p_4()
                                    .into_element(),
                            }
                        })),
                )
                .into_element()
        }))
}

fn list_row(r: SiteLogin) -> AnyElement {
    let u = ui();
    let id = r.id.clone();
    let id2 = r.id.clone();
    view()
        .px_2()
        .py(px(6.0))
        .rounded(px(7.0))
        .gap(px(1.0))
        .bg(move || if u.sel.get() == id { theme::ACTIVE } else { AdaptiveColor::TRANSPARENT })
        .on_press(move || pick(id2.clone()))
        .label(r.label.clone())
        .child(
            text(r.username.clone())
                .number_of_lines(1)
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::TEXT),
        )
        .child(
            text(format!("{} · {}", r.origin, kind_word(&r.kind)))
                .number_of_lines(1)
                .text_size(px(12.0))
                .text_color(theme::MUTED),
        )
        .into_element()
}

fn kind_word(kind: &str) -> &'static str {
    match kind {
        KIND_CODE => "code",
        KIND_PASSKEY => "passkey",
        _ => "password",
    }
}

fn detail(row: SiteLogin) -> impl IntoElement {
    let u = ui();
    let id = row.id.clone();
    let (id_shown, id_conf, id_del) = (id.clone(), id.clone(), id.clone());
    let has = vault().has_secret(&row);
    let secretless = row.kind == KIND_PASSKEY;
    let what = if row.kind == KIND_CODE { "Code" } else { "Password" };
    let row2 = row.clone();
    view()
        .p_4()
        .gap(px(10.0))
        .child(
            text(row.label.clone())
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::TEXT),
        )
        .child(kv("Website", row.origin.clone()))
        .child(kv("User name", row.username.clone()))
        .when(!secretless, move |v| {
            v.child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        text(what)
                            .w(px(90.0))
                            .text_size(px(12.0))
                            .text_color(theme::MUTED),
                    )
                    .child(
                        text(move || {
                            u.shown.with(|s| match s {
                                Some((i, s)) if *i == id_shown => s.expose().to_owned(),
                                _ if has => "••••••••".to_owned(),
                                _ => "Not on this Mac".to_owned(),
                            })
                        })
                        .flex_1()
                        .min_w(px(0.0))
                        .number_of_lines(1)
                        .text_size(px(13.0))
                        .text_color(theme::TEXT),
                    )
                    .when(has, move |v| {
                        let id = id.clone();
                        let showing = move || u.shown.with(|s| s.as_ref().is_some_and(|(i, _)| *i == id));
                        let row = row2.clone();
                        v.child(dynamic(move || {
                            if showing() {
                                small_btn("Hide", false, move || u.shown.set(None)).into_element()
                            } else {
                                let row = row.clone();
                                small_btn(if u.busy.get() { "Checking…" } else { "Show" }, false, move || show(row.clone()))
                                    .into_element()
                            }
                        }))
                    }),
            )
        })
        .child(kv("Kind", kind_word(&row.kind).to_owned()))
        .child(kv("Added", format!("{} ago", crate::store::Store::ago(row.created_at as f64))))
        .child(kv(
            "Last used",
            row.last_used_at_ms.map(|ms| format!("{} ago", crate::store::Store::ago(ms as f64))).unwrap_or_else(|| "Never".into()),
        ))
        .when(!row.notes.is_empty(), |v| v.child(kv("Notes", row.notes.clone())))
        .child(dynamic(move || {
            let note = u.note.get();
            if note.is_empty() {
                return view().into_element();
            }
            text(note).number_of_lines(3).text_size(px(12.0)).text_color(theme::DOWN).into_element()
        }))
        .child(
            view().flex_row().justify_end().child(dynamic(move || {
                let id = id_del.clone();
                let armed = u.confirm.get() == id_conf;
                small_btn(if armed { "Delete — press again" } else { "Delete" }, true, move || delete(id.clone()))
                    .into_element()
            })),
        )
}

fn kv(k: &'static str, v: String) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .child(text(k).w(px(90.0)).text_size(px(12.0)).text_color(theme::MUTED))
        .child(
            text(v)
                .flex_1()
                .min_w(px(0.0))
                .number_of_lines(2)
                .text_size(px(13.0))
                .text_color(theme::TEXT),
        )
}

fn small_btn(label: &'static str, danger: bool, on: impl Fn() + 'static) -> impl IntoElement {
    let hov = signal(false);
    text(label)
        .text_size(px(12.0))
        .text_color(move || match (hov.get(), danger) {
            (true, true) => theme::DOWN,
            (true, false) => theme::TEXT,
            _ => theme::MUTED,
        })
        .px_2()
        .py_1()
        .rounded(px(6.0))
        .bg(theme::HOVER)
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(move || on())
        .label(label)
}
