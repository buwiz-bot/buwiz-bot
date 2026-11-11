//! Settings › Logins: the saved logins on the left, the picked one's
//! details on the right — site, username, kind, when it was added and last
//! used, which Bots it's shared with — with Edit, Delete (pressed twice)
//! and a switch per Bot. The password is dots until Show, which asks for
//! Touch ID first; it hides again after half a minute, or when another row
//! is picked.
//!
//! - Add and Edit share one form: site, username, password, and an
//!   optional authenticator secret. Its secret fields are write-only: Edit
//!   opens them empty ("leave empty to keep"), what's typed is masked, and
//!   it leaves memory when the form saves or closes.
//! - Share all, at the top, offers every login to every Bot; off, a login
//!   goes only to the Bots switched on for it. Either way only on a Bot's
//!   own computer (the broker's `shared-computer` rule).
//!
//! Mounted by `settings.rs` as a page: [`settings_page`] is the whole page
//! body, and loads the list itself when it mounts. `/vault` opens it
//! ([`open_page`], [`open_add`]).

use std::time::Duration;

use nsh_base::input;
use nsh_core::prelude::*;

use super::{KIND_CODE, KIND_PASSKEY, Secret, SiteLogin, Unlock, off_main, vault};
use crate::theme;

/// How long a shown password stays on screen.
const SHOWN_FOR: Duration = Duration::from_secs(30);

/// The Add / Edit form, while it's open: `None` adds, `Some(id)` edits; and
/// the site the login had when the form opened.
#[derive(Clone, PartialEq, Debug)]
struct Form(Option<String>, String);

/// What the form says when an edit moves a login to another site.
pub(crate) const SITE_CHANGE_UNSHARES: &str = "Changing the site stops sharing this login; share it again after.";

/// The page's state, anchored (like the store) so a hot patch or a
/// remount of Settings keeps the pick.
#[derive(Clone, Copy)]
struct Ui {
    rows: Signal<Vec<SiteLogin>>,
    sel: Signal<String>,
    share_all: Signal<bool>,
    form: Signal<Option<Form>>,
    f_site: Signal<String>,
    f_user: Signal<String>,
    /// What's typed into the form's secret fields: masked, never prefilled.
    f_pass: Signal<Secret>,
    f_seed: Signal<Secret>,
    f_note: Signal<String>,
    /// The row id and what Show revealed (a password, or a code).
    shown: Signal<Option<(String, Secret)>>,
    /// The row Delete was pressed once on.
    confirm: Signal<String>,
    /// A sentence under the details (a Touch ID refusal, a failed delete).
    note: Signal<String>,
    busy: Signal<bool>,
}

fn ui() -> Ui {
    let at = ra_jit_live::anchor("rox.vault.page.shares", || {
        let ui = Owner::detached().run(|| Ui {
            rows: signal(Vec::new()),
            sel: signal(String::new()),
            share_all: signal(false),
            form: signal(None),
            f_site: signal(String::new()),
            f_user: signal(String::new()),
            f_pass: signal(Secret::new("")),
            f_seed: signal(Secret::new("")),
            f_note: signal(String::new()),
            shown: signal(None),
            confirm: signal(String::new()),
            note: signal(String::new()),
            busy: signal(false),
        });
        Box::leak(Box::new(ui)) as *const Ui as usize
    });
    // SAFETY: the anchor holds a leaked `Ui` for the life of the process.
    let u = unsafe { *(at as *const Ui) };
    watch_settings(u);
    u
}

/// However Settings closes, the form closes with it, and what was typed into
/// it goes. Set up once, outside the anchor (the store is an anchor too).
fn watch_settings(u: Ui) {
    thread_local! {
        static WATCHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if WATCHING.with(|w| w.replace(true)) {
        return;
    }
    Owner::detached().run(|| {
        effect(move || {
            if !crate::live::store().settings_open.get() {
                untracked(|| close_form_of(u));
            }
        })
    });
}

/// Reads the table again.
/// Reads the table again. An open form keeps what was typed into it: a reload comes from an
/// action on this page (Share all, Refresh, a save); the form forgets only when it closes, when
/// Settings closes, or when `/vault` opens the page again.
fn reload() {
    let u = ui();
    let rows = vault().list();
    if !rows.iter().any(|r| r.id == u.sel.get_untracked()) {
        u.sel.set(rows.first().map(|r| r.id.clone()).unwrap_or_default());
        u.shown.set(None);
    }
    u.rows.set(rows);
    u.share_all.set(vault().share_all());
    super::changed();
}

/// What was typed into the form's secret fields, gone. The fields show the
/// same signals, so what they show and what a Save sends never differ.
fn forget_typed() {
    forget_typed_of(ui());
}

fn forget_typed_of(u: Ui) {
    batch(|| {
        u.f_pass.set(Secret::new(""));
        u.f_seed.set(Secret::new(""));
    });
}

/// Opens Settings on Logins; a form left open keeps nothing typed into its secret fields.
pub(crate) fn open_page() {
    forget_typed();
    let st = crate::live::store();
    st.settings_page.set("logins".to_owned());
    st.settings_open.set(true);
}

/// Opens Settings on Logins with the Add form, its site filled in.
pub(crate) fn open_add(site: &str) {
    open_page();
    open_form(None, site.to_owned(), String::new());
}

/// The form, for a new login (`id` none) or row `id`; its secret fields empty.
fn open_form(id: Option<String>, site: String, user: String) {
    let u = ui();
    forget_typed();
    batch(|| {
        u.f_site.set(site);
        u.f_user.set(user);
        u.f_note.set(String::new());
        u.form.set(Some(Form(id, u.f_site.get_untracked())));
    });
}

/// Closes the form; what was typed into its secret fields goes.
fn close_form() {
    close_form_of(ui());
}

fn close_form_of(u: Ui) {
    forget_typed_of(u);
    batch(|| {
        u.form.set(None);
        u.f_note.set(String::new());
    });
}

/// Saves the form: a new login, or the edit. What was typed into its secret
/// fields leaves the form's state once it saves; a refusal keeps the form
/// open, masked, to fix.
fn save_form() {
    let u = ui();
    let Some(Form(id, _)) = u.form.get_untracked() else { return };
    if u.busy.get_untracked() {
        return;
    }
    let (site, user) = (u.f_site.get_untracked(), u.f_user.get_untracked());
    let (pass, seed) = (u.f_pass.get_untracked(), u.f_seed.get_untracked());
    u.busy.set(true);
    Owner::detached().run(|| {
        spawn(async move {
            let done = off_main(move || match &id {
                None => vault().add(&site, &user, &pass, &seed),
                Some(id) => vault().edit(id, &site, &user, &pass, &seed),
            })
            .await;
            u.busy.set(false);
            match done {
                Some(Ok(row)) => {
                    close_form();
                    u.sel.set(row.id.clone());
                    u.note.set(String::new());
                    reload();
                }
                Some(Err(e)) => u.f_note.set(format!("Couldn't save the login: {e}.")),
                None => u.f_note.set("Couldn't save the login.".into()),
            }
        });
    });
}

/// Shares the row with Bot `bot`, or stops.
fn set_share(id: &str, bot: &str, on: bool) {
    if let Err(e) = vault().share(id, bot, on) {
        ui().note.set(format!("Couldn't change who it's shared with: {e}"));
    }
    reload();
}

fn set_share_all(on: bool) {
    if let Err(e) = vault().set_share_all(on) {
        ui().note.set(format!("Couldn't change Share all: {e}"));
    }
    reload();
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
                .child(small_btn("Add a login", false, || open_form(None, String::new(), String::new())))
                .child(small_btn("Refresh", false, reload)),
        )
        .child(match super::keychain::warning(&super::keychain::kind()) {
            Some(w) => view().test_id("logins-store-warning").px_4().pb_2().child(text(w).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::WARN)).into_element(),
            None => view().into_element(),
        })
        .child(
            text("Saved from sign-in forms an agent asked you to fill, or added here. Passwords stay in the macOS Keychain; using or showing one asks for Touch ID every time.")
                .text_size(px(12.0))
                .text_color(theme::MUTED)
                .px_4()
                .pb_2(),
        )
        .child(share_all_row())
        .child(dynamic(move || {
            // the form tracks only whether it's open: a reload under it doesn't redraw it
            if u.form.get().is_some() {
                form_view().into_element()
            } else {
                logins_view().into_element()
            }
        }))
}

/// The list and the picked login's details.
fn logins_view() -> impl IntoElement {
    let u = ui();
    dynamic(move || {
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
    })
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

/// A switch: its words and its accessible name say where it stands ("Share all, on").
fn switch(name: String, on: bool, press: impl Fn() + 'static) -> impl IntoElement {
    let word = if on { "on" } else { "off" };
    text(if on { "On" } else { "Off" })
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if on { theme::ON_FILL } else { theme::MUTED })
        .px_2()
        .py_1()
        .rounded(px(999.0))
        .bg(if on { theme::ACCENT } else { theme::HOVER })
        .on_press(press)
        .label(crate::tools::switch_name(&name, word))
}

/// What Share all does, in a sentence, beside its switch.
pub(crate) const SHARE_ALL_MEANS: &str = "On: every saved login is offered to every Bot. Off: a login is offered only to the Bots it's shared with. Either way, only on a Bot's own computer, never one several Bots share, and Touch ID is asked every time.";

fn share_all_row() -> impl IntoElement {
    let u = ui();
    dynamic(move || {
        let on = u.share_all.get();
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .pb_2()
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(text("Share all").text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT))
                    .child(text(SHARE_ALL_MEANS).number_of_lines(3).text_size(px(12.0)).text_color(theme::MUTED)),
            )
            .child(switch("Share all".to_owned(), on, move || set_share_all(!on)))
            .into_element()
    })
}

/// The Add / Edit form. Its secret fields are masked and start empty; on an
/// edit, empty keeps what's saved.
fn form_view() -> impl IntoElement {
    let u = ui();
    let editing = u.form.get_untracked().is_some_and(|f| f.0.is_some());
    let (keep_pass, keep_seed) = if editing {
        ("Leave empty to keep the saved password", "Leave empty to keep what's saved")
    } else {
        ("", "Optional: otpauth://… or its base32 key")
    };
    view()
        .test_id("login-form")
        .flex_1()
        .p_4()
        .gap(px(10.0))
        .border_t(px(1.0))
        .border_color(theme::BORDER_F)
        .child(
            text(if editing { "Edit login" } else { "Add a login" })
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::TEXT),
        )
        .child(form_field("Website", "Login site", "shop.example", false, move || u.f_site.get(), move |t| u.f_site.set(t)))
        .child(form_field("User name", "Login user name", "", false, move || u.f_user.get(), move |t| u.f_user.set(t)))
        .child(form_field("Password", "Login password", keep_pass, true, move || u.f_pass.with(|s| s.expose().to_owned()), move |t| u.f_pass.set(Secret::new(t))))
        .child(form_field("Authenticator", "Login authenticator secret", keep_seed, true, move || u.f_seed.with(|s| s.expose().to_owned()), move |t| u.f_seed.set(Secret::new(t))))
        .child(dynamic(move || {
            // an edit to another site takes the login's shares with it: said before Save
            let moved = match u.form.get() {
                Some(Form(Some(_), was)) => super::site_origin(&u.f_site.get()).is_some_and(|now| now != was),
                _ => false,
            };
            if !moved {
                return view().into_element();
            }
            text(SITE_CHANGE_UNSHARES).number_of_lines(2).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::WARN).into_element()
        }))
        .child(
            text("Passwords and authenticator secrets are kept in the macOS Keychain and never shown here again.")
                .text_size(px(11.0))
                .text_color(theme::MUTED),
        )
        .child(dynamic(move || {
            let note = u.f_note.get();
            if note.is_empty() {
                return view().into_element();
            }
            text(note).number_of_lines(3).text_size(px(12.0)).text_color(theme::DOWN).into_element()
        }))
        .child(
            view()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(small_btn("Cancel", false, close_form))
                .child(small_btn("Save login", false, save_form)),
        )
}

/// One row of the form: its words, and an input under `label` showing the
/// form's own state (`get`) and writing it (`set`): one owner, so the field
/// and what Save sends can't differ. A secret one is masked; its state is
/// only what was typed into it since the form opened.
fn form_field(words: &'static str, label: &'static str, placeholder: &'static str, secret: bool, get: impl Fn() -> String + 'static, set: impl Fn(String) + 'static) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .child(text(words).w(px(90.0)).text_size(px(12.0)).text_color(theme::MUTED))
        .child(
            view().flex_1().h(px(30.0)).rounded(px(7.0)).bg(theme::HOVER).px(px(8.0)).justify_center().child(
                input()
                    .label(label)
                    .value(get)
                    .placeholder(placeholder)
                    .placeholder_color(|| color(theme::GHOST))
                    .masked(secret)
                    .auto_correct(false)
                    .text_size(px(13.0))
                    .text_color(theme::TEXT)
                    .h(px(30.0))
                    .on_change(move |t: &str| set(t.to_owned())),
            ),
        )
}

/// Who the row is offered to: a switch per Bot, and what Share all does to it.
fn shares(row: &SiteLogin) -> impl IntoElement {
    let u = ui();
    let bots = crate::live::store().bots.get();
    let mut v = view().gap(px(6.0)).child(text("Shared with").text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT));
    if u.share_all.get() {
        v = v.child(text("Share all is on, so every Bot is offered this login.").number_of_lines(2).text_size(px(12.0)).text_color(theme::MUTED));
    }
    if bots.is_empty() {
        return v.child(text("No Bots yet.").text_size(px(12.0)).text_color(theme::GHOST));
    }
    for b in bots {
        let on = row.shared_with.contains(&b.id);
        let (id, bot) = (row.id.clone(), b.id.clone());
        v = v.child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(text(b.name.clone()).flex_1().min_w(px(0.0)).number_of_lines(1).text_size(px(13.0)).text_color(theme::TEXT))
                .child(switch(format!("Share with {}", b.name), on, move || set_share(&id, &bot, !on))),
        );
    }
    v
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
    let (row_shares, row_edit) = (row.clone(), row.clone());
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
        .when(row.kind != KIND_PASSKEY, |v| v.child(kv("Authenticator", if vault().has_code(&row) { "Saved".to_owned() } else { "None".to_owned() })))
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
        .child(dynamic(move || shares(&row_shares).into_element()))
        .child(
            view()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(small_btn("Edit", false, move || open_form(Some(row_edit.id.clone()), row_edit.origin.clone(), row_edit.username.clone())))
                .child(dynamic(move || {
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
        .on_press(on)
        .label(label)
}
