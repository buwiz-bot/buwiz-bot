//! A mini app's version picker: the `v<n> ⌄` chip and the dropdown it
//! opens — every version newest first, with its label, age and the
//! version it branched from; picking one makes it current.

use nsh_core::ui::prelude::*;

use crate::icons::{icon_ink, icon_sized};
use crate::live::store;
use crate::store::{AppVersion, Store, VersPop};
use crate::theme;

const W: f32 = 280.0;
const CHIP_H: f32 = 20.0;

/// The `v<n> ⌄` chip of `app`; `state` reads its current version and
/// history live.
pub(crate) fn version_chip(
    app: String,
    state: impl Fn() -> (i64, Vec<AppVersion>) + Clone + 'static,
) -> impl IntoElement {
    let hovered = signal(false);
    let origin = signal((0.0f32, 0.0f32));
    let open_app = app.clone();
    let is_open = move || store().vers_pop.get().is_some_and(|p| p.app == open_app);
    let label = state.clone();
    view()
        .flex_row()
        .items_center()
        .gap(px(3.0))
        .h(px(CHIP_H))
        .rounded(px(6.0))
        .px(px(6.0))
        .bg(move || if hovered.get() || is_open() { theme::ACTIVE } else { theme::HOVER })
        .child(
            text(move || format!("v{}", label().0))
                .text_size(px(11.0))
                .font_family("monospace")
                .text_color(theme::TEXT),
        )
        .child(icon_sized("chevronDown", 10.0))
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_hover_move(move |e: &HoverEvent| {
            origin.set((
                e.page_position.x.0 - e.position.x.0,
                e.page_position.y.0 - e.position.y.0,
            ));
        })
        .on_press(move || {
            let st = store();
            if st.vers_pop.get_untracked().is_some_and(|p| p.app == app) {
                st.vers_pop.set(None);
                return;
            }
            let (cur, versions) = state();
            let (x, y) = origin.get_untracked();
            st.vers_pop.set(Some(VersPop { app: app.clone(), x, y: y + CHIP_H + 4.0, cur, versions }));
        })
        .label("Versions")
}

/// The dropdown, drawn while `vers_pop` is `Some`.
pub(crate) fn layer() -> impl IntoElement {
    dynamic(|| {
        let Some(p) = store().vers_pop.get() else {
            return view().into_element();
        };
        let win = window();
        let mut versions = p.versions.clone();
        versions.sort_by(|a, b| b.v.cmp(&a.v));
        let rows = versions.len() as f32;
        let h = (44.0 + rows * 46.0 + 34.0).min(360.0);
        // Under the chip, right edge on the chip's when it would overflow;
        // above it when the window runs out below.
        let x = p.x.min(win.width.0 - W - 12.0).max(12.0);
        let y = if p.y + h > win.height.0 - 12.0 { (p.y - CHIP_H - 8.0 - h).max(12.0) } else { p.y };
        let app = p.app.clone();
        view()
            .size_full()
            .absolute()
            .child(view().size_full().absolute().on_press(|| store().vers_pop.set(None)))
            .child(
                view()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(W))
                    .max_h(px(360.0))
                    .rounded(px(12.0))
                    .bg(theme::POPOVER)
                    .border(px(0.5))
                    .border_color(theme::BORDER_F)
                    .shadow_lg()
                    .overflow_hidden()
                    .on_press(|| {})
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .h(px(40.0))
                            .px(px(14.0))
                            .child(
                                text("Versions")
                                    .flex_1()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::TEXT),
                            )
                            .child(
                                text(format!("{} total", versions.len()))
                                    .text_size(px(12.0))
                                    .text_color(theme::MUTED),
                            ),
                    )
                    .child(
                        scroll().max_h(px(360.0 - 40.0 - 34.0)).child(
                            view().px(px(6.0)).children(
                                versions.into_iter().map(|vr| row(&app, vr, p.cur)),
                            ),
                        ),
                    )
                    .child(
                        text("Open windows switch to the version you pick.")
                            .text_size(px(11.0))
                            .text_color(theme::GHOST)
                            .px(px(14.0))
                            .py(px(9.0)),
                    ),
            )
            .into_element()
    })
}

fn row(app: &str, vr: AppVersion, cur: i64) -> AnyElement {
    let current = vr.v == cur;
    let hovered = signal(false);
    let (pick_app, edit_app) = (app.to_owned(), app.to_owned());
    let (v, label) = (vr.v, vr.label.clone());
    let mut sub = Store::ago(vr.created_at);
    if !sub.is_empty() {
        sub.push_str(" ago");
    }
    if let Some(parent) = vr.parent.filter(|p| *p != vr.v - 1) {
        sub.push_str(&format!(" · branched from v{parent}"));
    }
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .h(px(46.0))
        .px(px(8.0))
        .rounded(px(8.0))
        .bg(move || {
            if current {
                theme::ACTIVE
            } else if hovered.get() {
                theme::HOVER
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .child(
            view()
                .w(px(14.0))
                .items_center()
                .child(when(move || current, || icon_ink("check", 13.0, || theme::TEXT).into_element())),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(1.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            text(format!("v{v}"))
                                .text_size(px(13.0))
                                .font_family("monospace")
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::TEXT),
                        )
                        .child(
                            text(if label.is_empty() { "Untitled".to_owned() } else { label.clone() })
                                .text_size(px(13.0))
                                .text_color(if label.is_empty() { theme::GHOST } else { theme::TEXT })
                                .flex_shrink(1.0)
                                .number_of_lines(1),
                        )
                        .child(when(move || current, || {
                            text("Current")
                                .text_size(px(10.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme::DIM)
                                .bg(theme::SUBTLE)
                                .px(px(5.0))
                                .py(px(1.0))
                                .rounded(px(5.0))
                                .into_element()
                        })),
                )
                .child(text(sub).text_size(px(11.5)).text_color(theme::MUTED).number_of_lines(1)),
        )
        .child(when(
            move || hovered.get(),
            move || {
                let (app, label) = (edit_app.clone(), label.clone());
                view()
                    .size(px(24.0))
                    .rounded(px(6.0))
                    .items_center()
                    .justify_center()
                    .child(icon_sized("edit", 13.0))
                    .on_press(move || {
                        let st = store();
                        st.vers_pop.set(None);
                        st.app_rename_text.set(label.clone());
                        st.app_rename.set(Some(crate::store::AppRename {
                            name: app.clone(),
                            field: "label".into(),
                            v: Some(v),
                        }));
                    })
                    .label("Name this version")
                    .into_element()
            },
        ))
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || {
            let st = store();
            st.vers_pop.set(None);
            if !current {
                st.set_version(pick_app.clone(), v);
            }
        })
        .into_element()
}
