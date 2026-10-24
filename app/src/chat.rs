//! The transcript (`TurnCell` in `shell.tsx`): a `virtual_list` over the
//! open thread's turns — user bubbles on the right, agent markdown on the
//! left, reply quotes, reactions, and generated-app cards.

use nsh_core::ui::prelude::*;
use nsh_core::{TouchEvent, TouchPhase};

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
    store().chat_list.replace(Some(list.clone()));
    view().flex_1().child(
        virtual_list(
            move || store().turns.get().len(),
            // `TurnCell` — every row lives in the shared centered column
            // `maxWidth 992 + paddingHorizontal 16`, the same edges the
            // composer's card meets (shell.tsx:617, App.tsx:2188).
            move |at| {
                view()
                    .items_center()
                    .child(
                        view()
                            .w_full()
                            .max_w(px(992.0))
                            .px_4()
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
                // An anchored pill would float off its bar once the
                // content moves — dismiss it (App.tsx:2126).
                st.close_pill(-1);
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
    let reaction = t.reaction.clone();
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
            // The bubble: `bubbleFill` (the bot's color), radius 18,
            // padding 7×12, `maxWidth 560` (shell.tsx:438). Inside it
            // `OnFill` holds — the quote row and Markdown keep the dark
            // ramp in both themes.
            view().flex_row().justify_end().child(
                view()
                    .max_w(px(560.0))
                    .bg(move || theme::bubble_fill())
                    .rounded(px(18.0))
                    .px_3()
                    .py(px(7.0))
                    .when(!reaction.is_empty(), |v| {
                        v.child(tapback(i, reaction.clone(), false))
                    })
                    .child(fill_quote(reply))
                    .child(markdown(move || body.clone(), || false, || true)),
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

/// `badge` (shell.tsx:375) — the iMessage tapback: a 36pt knockout disc
/// painted in the page color centered on the bubble's outer top corner,
/// holding a 30pt chip with the emoji. `right` picks the corner (user →
/// left, agent → right); pressing it clears the reaction.
fn tapback(i: i64, reaction: String, right: bool) -> impl IntoElement {
    view()
        .absolute()
        .top(px(-27.0))
        .z_index(5)
        .when(right, |v| v.right(px(-18.0)))
        .when(!right, |v| v.left(px(-18.0)))
        .size(px(36.0))
        .rounded(px(18.0))
        .items_center()
        .justify_center()
        // The knockout: Grok punches a page-colored disc through the
        // bubble — `tc("#161616")`.
        .bg(theme::tc("#161616"))
        .child(
            view()
                .size(px(30.0))
                .rounded(px(15.0))
                .items_center()
                .justify_center()
                // `appearance() === "dark" ? "#242424" : "#ffffff"` /
                // `"#353535" : "#d4d4d8"` — literal ternaries, not `tc`.
                .bg(theme::tone(
                    theme::hsla(0.0, 0.0, 1.0, 1.0),
                    theme::hsla(0.0, 0.0, 0.141, 1.0),
                ))
                .border_1()
                .border_color(theme::tone(
                    theme::hsla(0.667, 0.087, 0.859, 1.0),
                    theme::hsla(0.0, 0.0, 0.208, 1.0),
                ))
                .child(text(reaction).text_size(px(14.0)))
                .on_press(move || store().react(i, String::new())),
        )
}

/// The quote row inside a filled user bubble (shell.tsx:452): 💬 + the
/// replied text whitespace-squashed to 80 chars in `OnFill` whites;
/// pressing jumps to the reply's turn.
fn fill_quote(reply: Option<crate::store::Reply>) -> impl IntoElement {
    dynamic(move || {
        let Some(r) = reply.clone() else {
            return view().into_element();
        };
        let q: String = r.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let q: String = q.chars().take(80).collect();
        let i = r.i;
        view()
            .flex_row()
            .items_center()
            .gap(px(5.0))
            .mb(px(2.0))
            .on_press(move || {
                let st = store();
                let pos = st.turns.get_untracked().iter().position(|t| t.i == i);
                let list = st.chat_list.borrow().clone();
                if let (Some(pos), Some(list)) = (pos, list) {
                    list.scroll_to_index(pos as u32, percentage(0.0), px(0.0), true);
                }
            })
            .child(
                // `💬︎` (shell.tsx:455) — the VS15 text glyph CoreText
                // draws as a color emoji; the stroked balloon icon
                // matches what Grok's font renders.
                view()
                    .items_center()
                    .child(crate::icons::icon_ink("quoteBubble", 12.0, || {
                        theme::ON_FILL_ICON
                    })),
            )
            .child(
                text(format!("“{q}”"))
                    .text_size(px(13.0))
                    .line_height(px(18.0))
                    .text_color(theme::ON_FILL_QUOTE)
                    .number_of_lines(1),
            )
            .into_element()
    })
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
    // `msgBar` — the emoji pill anchors to its measured rect
    // (shell.tsx:315's `ref`).
    let bar_ref = element_ref();
    view()
        .py(px(3.0))
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
        // The agent bubble (shell.tsx:484): `agentFill` + a hairline,
        // radius 18, padding 16×10, `maxWidth 560`, hugging its content
        // on the row's left edge; the tapback knocks out top-right.
        .child(
            view().flex_row().w_full().justify_start().child(
                view()
                    .max_w(px(560.0))
                    .bg(theme::AGENT_BUBBLE)
                    .rounded(px(18.0))
                    .border_1()
                    .border_color(theme::AGENT_BORDER)
                    .px_4()
                    .py(px(10.0))
                    .when(!reaction.is_empty(), |v| {
                        v.child(tapback(i, reaction.clone(), true))
                    })
                    .child(markdown(move || body.clone(), move || streaming, || false)),
            ),
        )
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
        .child(
            // `bar(false, true)` — react / reply / ⋯ (shell.tsx:504), shown
            // only while the row is hovered (or its pill is open).
            view()
                .flex_row()
                .gap(px(2.0))
                .pt_1()
                .element_ref(&bar_ref)
                .opacity(move || {
                    if store()
                        .pill
                        .get()
                        .as_ref()
                        .map_or(false, |a| a.i == i && a.slot == "msg")
                    {
                        1.0
                    } else {
                        bar_opacity()
                    }
                })
                .child(react_act(i, false, bar_ref, "msg".to_owned()))
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

/// `openPicker` (shell.tsx:281) — the ☺ button measures `bar` and opens
/// the shared emoji pill under (or over) it. `right` anchors the pill's
/// right edge to the bar's (artifact bars sit on the right); `slot` is
/// the owning bar's name — `"msg"` or `"art:<path>"` — and reopening the
/// same slot toggles it closed.
fn react_act(i: i64, right: bool, bar: ElementRef, slot: String) -> impl IntoElement {
    act("☺", "react", move || {
        let bar = bar.clone();
        let slot = slot.clone();
        spawn(async move {
            let st = store();
            if st
                .pill
                .get_untracked()
                .as_ref()
                .map_or(false, |a| a.i == i && a.slot == slot)
            {
                st.close_pill(i);
                return;
            }
            let Some(m) = bar.measure().await else {
                return;
            };
            let current = st
                .turns
                .get_untracked()
                .iter()
                .find(|t| t.i == i)
                .map(|t| t.reaction.clone())
                .unwrap_or_default();
            let win = window();
            let flip =
                m.page.y.0 + m.frame.size.height.0 + 205.0 > win.height.0 - 80.0;
            st.open_pill(crate::store::PillAnchor {
                x: m.page.x.0,
                y: m.page.y.0,
                w: m.frame.size.width.0,
                h: m.frame.size.height.0,
                flip,
                right,
                i,
                slot,
                current,
            });
        });
    })
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
    // active bot's color when set, else the neutral 18%-white fill.
    let tint = theme::bubble_fill_of(
        &store()
            .active_bot_untracked()
            .map(|b| b.color)
            .unwrap_or_default(),
    );
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
    let art_bar = element_ref();
    // `"art:" + (f?.path || "gallery")` (shell.tsx:339) — each artifact's
    // bar owns its own pill slot.
    let slot = format!("art:{}", f.as_ref().map(|f| f.path.clone()).unwrap_or_else(|| "gallery".to_owned()));
    let slot2 = slot.clone();
    let reply_label = f
        .as_ref()
        .map(|f| format!("📎 {}", f.name))
        .unwrap_or_else(|| msg.chars().take(120).collect());
    view()
        .self_start()
        .gap_1()
        .max_w(percentage(1.0))
        .on_hover_in(move || {
            if !store().chat_scrolling.get_untracked() {
                hov.set(true);
            }
        })
        .on_hover_out(move || hov.set(false))
        .child(node)
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(px(2.0))
                .element_ref(&art_bar)
                .opacity(move || {
                    if store()
                        .pill
                        .get()
                        .as_ref()
                        .map_or(false, |a| a.i == i && a.slot == slot2)
                    {
                        1.0
                    } else if hov.get() && !store().chat_scrolling.get() {
                        1.0
                    } else {
                        0.0
                    }
                })
                .child(react_act(i, true, art_bar, slot))
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

// ---------------------------------------------------------------------------
// Emoji pill
// ---------------------------------------------------------------------------

/// `EMOJI` (shell.tsx:114) — the pill's quick row.
const EMOJI: &[&str] = &["👍", "❤️", "😂", "😮", "👎"];
/// `EMOJI_GRID` (shell.tsx:115) — the plus button's full pick.
const EMOJI_GRID: &[&str] = &[
    "😀","😄","😁","🤣","😂","🙂","😉","😊","😇","🥰","😍","🤩","😘","😋","😛","🤪","😜","😝","🤑","🤗",
    "🤭","🤫","🤔","🫡","😐","😑","😶","😏","😒","🙄","😬","🤥","😌","😔","😪","🤤","😴","😷","🤒","🤕",
    "🤢","🤮","🥵","🥶","🥴","😵","🤯","🤠","🥳","😎","🤓","🧐","😕","😟","🙁","😮","😯","😲","😳","🥺",
    "😦","😧","😨","😰","😥","😢","😭","😱","😖","😣","😞","😓","😩","😫","🥱","😤","😡","😠","🤬","😈",
    "❤️","🧡","💛","💚","💙","💜","🖤","🤍","🤎","💔","💕","💖","💗","💯","✨","⭐","🌟","💫","🔥","🎉",
    "👍","👎","👏","🙌","🙏","🤝","💪","🫶","👌","✌️","🤞","🫰","🤙","👋","✋","🖐️","🖖","👆","👇","☝️",
    "👽","🤖","💀","🎃","🚀","🎮","🎯","🏆","🥇","🍕","🍔","🍩","☕","🍺","⚡","🌈","☀️","🌙","❄️","🐛",
];

/// `EmojiPill` (shell.tsx:124) — the quick-react row; the emoji-plus
/// toggle expands the full grid. Picking the current emoji clears it.
pub(crate) fn emoji_pill(i: i64, current: String) -> impl IntoElement {
    let grid = signal(false);
    let current_grid = current.clone();
    // `dot` — a 30pt circle: current → `#f5f5f52b`, hover → `#f5f5f514` +
    // a 1.25 scale pop (120ms).
    let dot = move |e: &'static str| {
        let h = signal(false);
        let (h1, h2, h3) = (h.clone(), h.clone(), h.clone());
        let e2 = e.to_owned();
        let cur = current.clone();
        let cur2 = current.clone();
        view()
            .size(px(30.0))
            .rounded(px(15.0))
            .items_center()
            .justify_center()
            .bg(move || {
                if cur == e2 {
                    theme::tc("#f5f5f52b")
                } else if h1.get() {
                    theme::tc("#f5f5f514")
                } else {
                    AdaptiveColor::TRANSPARENT
                }
            })
            .scale(move || if h2.get() { 1.25_f32 } else { 1.0 })
            .on_hover_in(move || h.set(true))
            .on_hover_out(move || h3.set(false))
            .on_press(move || {
                let st = store();
                st.react(i, if cur2 == e { String::new() } else { e.to_owned() });
                st.close_pill(i);
            })
            .child(text(e).text_size(px(17.0)))
    };
    let plus_hov = signal(false);
    let (ph1, ph2) = (plus_hov.clone(), plus_hov.clone());
    let (grid_bg, grid_open, grid_when) = (grid.clone(), grid.clone(), grid.clone());
    view()
        .gap(px(2.0))
        .px(px(6.0))
        .py(px(5.0))
        .rounded(px(20.0))
        // `appearance() === "dark" ? "#262626" : "#ffffff"`.
        .bg(theme::tone(
            theme::hsla(0.0, 0.0, 1.0, 1.0),
            theme::hsla(0.0, 0.0, 0.149, 1.0),
        ))
        .border_1()
        .border_color(theme::tc("#f5f5f51f"))
        .child(
            view()
                .flex_row()
                .gap(px(2.0))
                .children(EMOJI.iter().map(|e| dot(e).into_element()))
                .child(
                    view()
                        .size(px(32.0))
                        .rounded(px(16.0))
                        .items_center()
                        .justify_center()
                        .bg(move || {
                            if grid_bg.get() || plus_hov.get() {
                                theme::tc("#f5f5f514")
                            } else {
                                AdaptiveColor::TRANSPARENT
                            }
                        })
                        .scale(move || if ph1.get() { 1.25_f32 } else { 1.0 })
                        .on_hover_in(move || ph2.set(true))
                        .on_hover_out(move || plus_hov.set(false))
                        .on_press(move || grid_open.update(|v| *v = !*v))
                        .child(crate::icons::icon_ink("emojiPlus", 20.0, || {
                            theme::tc("#f5f5f5a8")
                        })),
                ),
        )
        .child(
            when(move || grid_when.get(), move || {
                scroll()
                    .max_h(px(160.0))
                    .w(px(226.0))
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .gap(px(2.0))
                            .children(
                                EMOJI_GRID
                                    .iter()
                                    .map(|e| {
                                        let h = signal(false);
                                        let (h1, h2) = (h.clone(), h.clone());
                                        let e2 = e.to_owned();
                                        let cur = current_grid.clone();
                                        let cur2 = current_grid.clone();
                                        view()
                                            .size(px(30.0))
                                            .rounded(px(15.0))
                                            .items_center()
                                            .justify_center()
                                            .bg(move || {
                                                if cur == e2 {
                                                    theme::tc("#f5f5f52b")
                                                } else if h1.get() {
                                                    theme::tc("#f5f5f514")
                                                } else {
                                                    AdaptiveColor::TRANSPARENT
                                                }
                                            })
                                            .on_hover_in(move || h.set(true))
                                            .on_hover_out(move || h2.set(false))
                                            .on_press(move || {
                                                let st = store();
                                                st.react(i, if cur2 == *e { String::new() } else { e.to_string() });
                                                st.close_pill(i);
                                            })
                                            .child(text(*e).text_size(px(17.0)))
                                            .into_element()
                                    })
                                    .collect::<Vec<_>>(),
                            ),
                    )
                    .into_element()
            }),
        )
}

/// The pill's app-level overlay (App.tsx:3056): anchored to the bar's
/// measured rect — `left:x` or `right:`-anchored, `top:y+h+6`, flipped to
/// `bottom` when it would clip the window's lower edge. `box-none` lets
/// every other surface take hovers and presses like a native menu; the
/// pill's own touch marks `pillGuard` so the root's click-away handler
/// (hot.rs) doesn't read it as outside.
pub(crate) fn pill_layer() -> impl IntoElement {
    dynamic(move || {
        let st = store();
        let Some(a) = st.pill.get() else {
            return view().into_element();
        };
        let win = window();
        view()
            .size_full()
            .absolute()
            .flex_col()
            .pointer_events(PointerEvents::BoxNone)
            // Flow-positioned like the palette's card: margins in a
            // column (bottom-anchored when flipped), a row inside for
            // left/right anchoring — `right` and `bottom` map to padding
            // of the full-size outer.
            .when(a.flip, |v| v.justify_end())
            .child(
                view()
                    .flex_row()
                    .w_full()
                    .when(a.right, |v| v.justify_end())
                    .when(!a.right, |v| v.pl(px(a.x)))
                    .when(a.right, |v| {
                        v.pr(px((win.width.0 - a.x - a.w).max(0.0)))
                    })
                    .when(!a.flip, |v| v.mt(px(a.y + a.h + 6.0)))
                    .when(a.flip, |v| {
                        v.mb(px(win.height.0 - a.y + 6.0))
                    })
                    .on_touch(|e: &TouchEvent| {
                        if e.phase == TouchPhase::Start {
                            store().pill_guard.set(true);
                        }
                    })
                    .child(emoji_pill(a.i, a.current.clone())),
            )
            .into_element()
    })
}
