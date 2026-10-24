//! The transcript (`TurnCell` in `shell.tsx`): a `virtual_list` over the
//! open thread's turns — user bubbles on the right, agent markdown on the
//! left, reply quotes, reactions, and generated-app cards.

use nsh_core::ui::prelude::*;

use crate::live::store;
use crate::md::markdown;
use crate::store::{Attach, Turn};
use crate::theme;
use crate::widgets::app_card;

/// The scrollable conversation. Opens on the latest turn (the TSX's
/// `openThread` scrollToEnd + `revealed`), `follow_end` pins the bottom
/// while a reply streams in.
pub(crate) fn transcript() -> impl IntoElement {
    let st = store();
    let list = element_ref();
    // Poll turns while the thread is open.
    set_interval(std::time::Duration::from_millis(1500), move || st.refresh_turns());
    // Mount hidden, snap to the end, reveal (App.tsx:289-300): the first
    // snap lands pre-frame via `initialScrollIndex`; these converge to the
    // true end as unmeasured rows measure — the command's pin retargets.
    let revealed = signal(true);
    {
        let list = list.clone();
        let revealed = revealed.clone();
        effect(move || {
            store().active_id.get();
            revealed.set(false);
            let list = list.clone();
            let revealed = revealed.clone();
            spawn(async move {
                timer(std::time::Duration::from_millis(150)).await;
                list.scroll_to_end(false);
                timer(std::time::Duration::from_millis(400)).await;
                list.scroll_to_end(false);
                revealed.set(true);
            });
        });
    }
    view().flex_1().child(
        virtual_list(
            move || store().turns.get().len(),
            // `TurnCell` — every row lives in the shared centered column
            // `min(960, width-32)` so the transcript's edges meet the
            // composer's card (App.tsx:2127).
            move |at| {
                view()
                    .items_center()
                    .child(
                        view()
                            .w_full()
                            .max_w(px(960.0))
                            .child(turn_row(
                                store().turns.get_untracked()[at.index()].clone(),
                            )),
                    )
            },
        )
        .keys(move || {
            store().turns.get().iter().map(|t| t.i.to_string()).collect::<Vec<_>>()
        })
        .follow_end(px(40.0))
        .element_ref(&list)
        // `chatScrolling` (shell.tsx:88) — true from the first scroll
        // event until ~160ms after the last; hover handlers read it to
        // skip their signal writes (every write is a commit).
        .on_scroll({
            let epoch = std::rc::Rc::new(std::cell::Cell::new(0u64));
            move |_: &ScrollEvent| {
                let st = store();
                if !st.chat_scrolling.get_untracked() {
                    st.chat_scrolling.set(true);
                }
                let e = epoch.get().wrapping_add(1);
                epoch.set(e);
                let epoch = epoch.clone();
                spawn(async move {
                    timer(std::time::Duration::from_millis(160)).await;
                    if epoch.get() == e {
                        store().chat_scrolling.set(false);
                    }
                });
            }
        })
        .flex_1()
        .px_4()
        .pt(px(56.0)) // under the top overlay bar
        // `paddingBottom: dockH() + 24` — the transcript clears the
        // floating composer (App.tsx:2132).
        .pb(move || px(crate::composer::dock_h().get() + 24.0)),
    )
}

/// The empty state when no thread is open or no turns exist.
pub(crate) fn empty_state() -> impl IntoElement {
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .child(
            text("Rox")
                .text_size(px(28.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::GHOST),
        )
        .child(
            text("Ask anything.")
                .text_size(px(13.0))
                .text_color(theme::GHOST)
                .pt_1(),
        )
}

fn turn_row(t: Turn) -> impl IntoElement {
    if t.role == "user" {
        user_row(t).into_element()
    } else {
        agent_row(t).into_element()
    }
}

// ---------------------------------------------------------------------------
// User bubble
// ---------------------------------------------------------------------------

fn user_row(t: Turn) -> impl IntoElement {
    let body = t.text.clone();
    let reply = t.reply.clone();
    let atts = t.attachments.clone();
    let i = t.i;
    // shell.tsx:425 — the row's `hover` gates the action bar; during a
    // scroll `chatScrolling` skips the writes (every write is a commit).
    let hov = signal(false);
    let hov2 = hov.clone();
    let bar_opacity = move || {
        if hov.get() && !store().chat_scrolling.get() {
            1.0f32
        } else {
            0.0
        }
    };
    view()
        .py(px(3.0))
        .items_end()
        .gap_1()
        .on_hover_in(move || {
            if !store().chat_scrolling.get_untracked() {
                hov2.set(true);
            }
        })
        // `hover_out` always clears — skipping it during a scroll leaves
        // `hov` stuck true and the bar lit at rest once scrolling ends.
        .on_hover_out(move || hov.set(false))
        // attached images/files sit above the bubble (shell.tsx:427):
        // right-aligned wrap row, `FileTile` 72 for files, 120 for media
        .when(!atts.is_empty(), |v| {
            v.child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .justify_end()
                    .gap(px(6.0))
                    .max_w(px(560.0))
                    .children(atts.iter().map(|a| {
                        crate::files::file_tile(
                            a,
                            if a.kind == "image" || a.kind == "video" {
                                120.0
                            } else {
                                72.0
                            },
                        )
                        .into_element()
                    })),
            )
        })
        .child(
            view().flex_row().justify_end().child(
                view()
                    .max_w(px(560.0))
                    .bg(theme::BUBBLE)
                    .rounded(px(18.0))
                    .px_3()
                    .py(px(7.0))
                    .child(quote_block(reply))
                    .child(markdown(move || body.clone(), || false)),
            ),
        )
        // `bar(true, false)` (shell.tsx:472) — right-aligned, no react on
        // your own bubbles: reply + ⋯ only.
        .child(
            view()
                .flex_row()
                .justify_end()
                .gap(px(2.0))
                .opacity(bar_opacity)
                .child(act("↩", "reply", move || {
                    let st = store();
                    let tr = st.turns.get_untracked();
                    if let Some(tt) = tr.iter().find(|x| x.i == i) {
                        st.reply_to.set(Some(crate::store::ReplyTo {
                            i: tt.i,
                            name: tt.name.clone(),
                            text: tt.text.chars().take(120).collect(),
                        }));
                    }
                }))
                .child(more_act(i)),
        )
}

// ---------------------------------------------------------------------------
// Agent row
// ---------------------------------------------------------------------------

/// `ArtPlaceholder` — the gray box a building artifact renders before its
/// module exists (shell.tsx:188): title, optional `art.desc`, and an
/// aspect-ratio box shimmering `t.status` with a `w×h` corner label.
fn art_placeholder(t: &Turn) -> impl IntoElement {
    let art = t.art.clone().unwrap_or_default();
    let w = (if art.w > 0.0 { art.w } else { 480.0 } as f64).min(560.0) as f32;
    let ar = if art.w > 0.0 && art.h > 0.0 { art.w / art.h } else { 4.0 / 3.0 } as f32;
    let title = if !art.title.is_empty() {
        art.title.clone()
    } else if t.label == "image" {
        "Image".to_owned()
    } else {
        "Mini app".to_owned()
    };
    let desc = art.desc.clone();
    let status = if t.status.is_empty() { "Loading…".to_owned() } else { t.status.clone() };
    let dims = if art.w > 0.0 && art.h > 0.0 {
        Some(format!("{}×{}", art.w, art.h))
    } else {
        None
    };
    view()
        .w(px(w))
        .max_w(percentage(100.0))
        .bg(theme::CARD)
        .rounded(px(16.0))
        .border_1()
        .border_color(theme::BORDER_F)
        .p_3()
        .child(
            text(title)
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::TEXT)
                .number_of_lines(1),
        )
        .when(!desc.is_empty(), |v| {
            v.child(
                text(desc.clone())
                    .text_size(px(12.0))
                    .text_color(theme::FAINT)
                    .pt(px(2.0))
                    .number_of_lines(1),
            )
        })
        .child(
            view()
                .w(px(w - 24.0))
                .h(px(((w - 24.0) / ar).max(1.0)))
                .mt(px(10.0))
                .rounded(px(10.0))
                .bg(theme::CARD2)
                .items_center()
                .justify_center()
                .child(
                    view()
                        .w(px(170.0))
                        .child(crate::statusfx::shimmer_at(
                            move || status.clone(),
                            12.0,
                            theme::FAINT,
                            theme::TEXT,
                        )),
                )
                .when_some(dims, |v, d| {
                    v.child(
                        text(d)
                            .absolute()
                            .right(px(8.0))
                            .bottom(px(6.0))
                            .text_size(px(10.5))
                            .text_color(theme::GHOST),
                    )
                }),
        )
}

fn agent_row(t: Turn) -> impl IntoElement {
    let name = t.name.clone();
    let caption = t.caption.clone();
    let body = t.text.clone();
    let streaming = t.building;
    let i = t.i;
    let reaction = t.reaction.clone();
    let has_card = t.art.is_some();
    // shell.tsx:254 `hov` — gates the action bar's opacity; the writes are
    // skipped while `chatScrolling` (every write is a commit).
    let hov = signal(false);
    let hov2 = hov.clone();
    let bar_opacity = move || {
        if hov.get() && !store().chat_scrolling.get() {
            1.0f32
        } else {
            0.0
        }
    };
    view()
        .py(px(3.0))
        .max_w(px(640.0))
        .on_hover_in(move || {
            if !store().chat_scrolling.get_untracked() {
                hov2.set(true);
            }
        })
        .on_hover_out(move || hov.set(false))
        .child(
            // Sender line: name + caption.
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .when(!name.is_empty() || !caption.is_empty(), |v| {
                    v.child(
                        text(name.clone())
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::FAINT),
                    )
                    .child(
                        text(caption.clone())
                            .text_size(px(11.0))
                            .text_color(theme::GHOST)
                            .number_of_lines(1),
                    )
                }),
        )
        .child(reply_quote(&t))
        .child(markdown(move || body.clone(), move || streaming))
        .child(files_section(&t))
        // A building artifact renders its declared placeholder — before
        // the module exists, the card is a gray box shimmering the job's
        // phase (`ArtPlaceholder` in shell.tsx). A fetched module that is
        // still compiling (`d === "loading"`) shows it too, until
        // `mini_source` lands or `mini_err` reports.
        .when(has_card && t.building, |v| v.child(art_placeholder(&t)))
        .when(has_card && !t.building, |v| {
            let app = t.art.clone().map(|a| a.app).unwrap_or_default();
            let t2 = t.clone();
            let t3 = t.clone();
            v.child(
                when(
                    move || {
                        store().mini_source(&app).get().is_empty()
                            && store().mini_err(&app).get().is_empty()
                    },
                    move || art_placeholder(&t2).into_element(),
                )
                .otherwise(move || app_card(t3.art.clone().unwrap()).into_element()),
            )
        })
        // The reaction badge (shell.tsx:377's tapback circle) — always
        // visible, so it lives outside the hover-gated bar.
        .when(!reaction.is_empty(), move |v| {
            v.child(
                view().items_end().child(
                    text(reaction.clone())
                        .text_size(px(12.0))
                        .px_2()
                        .py(px(2.0))
                        .bg(theme::CARD2)
                        .rounded(px(10.0)),
                ),
            )
        })
        .child(
            // `bar(false, true)` — react / reply / ⋯ (shell.tsx:504), shown
            // only while the row is hovered.
            view()
                .flex_row()
                .gap(px(2.0))
                .pt_1()
                .opacity(bar_opacity)
                .child(act("☺", "react", move || {
                    store().react(i, "👍".to_owned());
                }))
                .child(act("↩", "reply", move || {
                    let st = store();
                    let tr = st.turns.get_untracked();
                    if let Some(tt) = tr.iter().find(|x| x.i == i) {
                        st.reply_to.set(Some(crate::store::ReplyTo {
                            i: tt.i,
                            name: tt.name.clone(),
                            text: tt.text.chars().take(120).collect(),
                        }));
                    }
                }))
                .child(more_act(i)),
        )
}

/// `Act` (shell.tsx:94) — a bare 24pt text glyph; the hovered one gets a
/// rounded plate. No icon drawing: Grok's glyphs are text.
fn act(
    glyph: &'static str,
    label: &'static str,
    on: impl Fn() + 'static,
) -> impl IntoElement {
    let hovered = signal(false);
    view()
        .label(label)
        .size(px(24.0))
        .rounded(px(6.0))
        .items_center()
        .justify_center()
        .bg(move || if hovered.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .child(text(glyph).text_size(px(15.0)).text_color(theme::DIM))
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || on())
}

/// `more()` (shell.tsx:301) — the ⋯ button opening the turn's menu
/// (Copy / Delete) anchored under itself.
fn more_act(i: i64) -> impl IntoElement {
    let node_ref = element_ref();
    act("⋯", "more", move || {
        let node_ref = node_ref.clone();
        spawn(async move {
            if let Some(m) = node_ref.measure().await {
                crate::ctx::open_at(
                    m.page.x.0,
                    m.page.y.0 + m.frame.size.height.0,
                    crate::ctx::msg_items(i),
                );
            }
        });
    })
}

fn reply_quote(t: &Turn) -> impl IntoElement {
    quote_block(t.reply.clone())
}

fn quote_block(reply: Option<crate::store::Reply>) -> impl IntoElement {
    dynamic(move || {
        let Some(r) = reply.clone() else {
            return view().into_element();
        };
        view()
            .border_l(px(2.0))
            .border_l_color(theme::BORDER_S)
            .pl_2()
            .mb_1()
            .child(
                text(r.name)
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::MUTED),
            )
            .child(
                text(r.text)
                    .text_size(px(12.0))
                    .text_color(theme::MUTED)
                    .number_of_lines(2),
            )
            .into_element()
    })
}

/// `t.files` (shell.tsx:507): images/videos group into `MediaGallery`;
/// audio gets `AudioCard`, everything else `FileCard` — each wrapped in
/// `artifact` (hover action bar: react / reply / ⋯ menu).
fn files_section(t: &Turn) -> impl IntoElement {
    let files = t.files.clone();
    if files.is_empty() {
        return view().into_element();
    }
    let i = t.i;
    let msg = t.text.clone();
    // the AudioCard waveform's played tint is the user bubble fill — the
    // active bot's color when set, else the neutral fill.
    let tint = store()
        .active_bot_untracked()
        .and_then(|b| b.color.parse::<Hsla>().ok())
        .map(AdaptiveColor::from)
        .unwrap_or(theme::BUBBLE);
    let media: Vec<Attach> = files
        .iter()
        .filter(|f| f.kind == "image" || f.kind == "video")
        .cloned()
        .collect();
    let rest: Vec<Attach> = files
        .iter()
        .filter(|f| f.kind != "image" && f.kind != "video")
        .cloned()
        .collect();
    view()
        .gap_2()
        .w_full()
        .max_w(px(560.0))
        .when(!media.is_empty(), |v| {
            v.child(artifact(crate::files::media_gallery(media).into_element(), i, None, &msg))
        })
        .children(rest.iter().map(|f| {
            let card = if f.kind == "audio" {
                crate::files::audio_card(f, tint).into_element()
            } else {
                crate::files::file_card(f).into_element()
            };
            artifact(card, i, Some(f.clone()), &msg).into_element()
        }))
        .into_element()
}

/// `artifact(node, f)` (shell.tsx:331): the card with its own hover bar —
/// ☺ react, ↩ reply (files travel with the reply), ⋯ menu.
fn artifact(node: AnyElement, i: i64, f: Option<Attach>, msg: &str) -> impl IntoElement {
    let hov = signal(false);
    let items = crate::ctx::artifact_items(i, f.as_ref(), msg);
    let node_ref = element_ref();
    let reply_label = f
        .as_ref()
        .map(|f| format!("📎 {}", f.name))
        .unwrap_or_else(|| msg.chars().take(120).collect());
    view()
        .self_start()
        .gap_1()
        .max_w(percentage(1.0))
        .on_hover_in(move || hov.set(true))
        .on_hover_out(move || hov.set(false))
        .child(node)
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(px(2.0))
                .opacity(move || if hov.get() { 1.0_f32 } else { 0.0 })
                .child(
                    view()
                        .label("React")
                        .size(px(26.0))
                        .rounded(px(6.0))
                        .items_center()
                        .justify_center()
                        .child(text("☺").text_size(px(13.0)).text_color(theme::MUTED))
                        .on_press(move || store().react(i, "👍".to_owned())),
                )
                .child(
                    view()
                        .label("Reply")
                        .size(px(26.0))
                        .rounded(px(6.0))
                        .items_center()
                        .justify_center()
                        .child(text("↩").text_size(px(13.0)).text_color(theme::MUTED))
                        .on_press(move || {
                            let st = store();
                            let name = st
                                .active_bot_untracked()
                                .map(|b| b.name)
                                .unwrap_or_default();
                            st.reply_to.set(Some(crate::store::ReplyTo {
                                i,
                                name,
                                text: reply_label.clone(),
                            }));
                        }),
                )
                .child(
                    view()
                        .label("More")
                        .element_ref(&node_ref)
                        .size(px(26.0))
                        .rounded(px(6.0))
                        .items_center()
                        .justify_center()
                        .child(text("⋯").text_size(px(13.0)).text_color(theme::MUTED))
                        .on_press(move || {
                            let items = items.clone();
                            spawn(async move {
                                if let Some(m) = node_ref.measure().await {
                                    crate::ctx::open_at(
                                        m.page.x.0,
                                        m.page.y.0 + m.frame.size.height.0,
                                        items,
                                    );
                                }
                            });
                        }),
                ),
        )
}
