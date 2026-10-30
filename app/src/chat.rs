//! The transcript (`TurnCell` in `shell.tsx`): a `virtual_list` over the
//! open thread's turns — user bubbles on the right, agent markdown on the
//! left, reply quotes, reactions, and generated-app cards.

use nsh_core::prelude::*;
use nsh_core::{TouchEvent, TouchPhase};

use crate::live::store;
use crate::md::markdown;
use crate::store::{Attach, RoutineNotice, Turn};
use crate::theme;
use crate::widgets::app_card;

/// A turn's height before it is measured (points).
const ROW_ESTIMATE: f32 = 150.0;

/// Where a freshly mounted transcript opens.
#[derive(Clone, Copy)]
enum Opening {
    /// A link's turn (row, turn i), centered.
    Jump(usize, i64),
    /// Where the chat was left: the row at the viewport's top and how far
    /// into it.
    Mark(usize, f32),
    /// The latest turn.
    End,
}

/// Whether the transcript a live mini app sits in is the shown chat:
/// `app_card` mounts its app only then.
#[derive(Clone, Copy)]
pub(crate) struct ChatShown(pub(crate) Memo<bool>);

/// The scrollable conversation of chat `mine`. The last few chats' are
/// kept alive (`Store::alive`) and only shown or hidden: switching back
/// shows the list exactly where it was — nothing re-laid out, no jump to
/// the end and back — and fades it in (160ms, native). A chat that fell
/// out of that cache mounts again at the spot it was left (`ScrollMark`),
/// a link's turn, or the end. `follow_end` pins the bottom while a reply
/// streams in.
pub(crate) fn transcript(mine: String) -> impl IntoElement {
    let st = store();
    let list = element_ref();
    let active = {
        let mine = mine.clone();
        computed(move || store().turns_of.get() == mine)
    };
    // Poll turns while this chat is the one shown.
    set_interval(std::time::Duration::from_millis(1500), move || {
        if active.get_untracked() {
            st.refresh_turns();
        }
    });
    // This chat's own turns: they follow `turns` only while it is shown —
    // hidden, it keeps its rows (another chat's turns never reach its
    // list: that shrank it, clamped its scroll and flashed old rows).
    let rows = signal(if active.get_untracked() { st.turns.get_untracked() } else { Vec::new() });
    // The rows the list reports in view (`None` before its first report):
    // a row's live mini app holds while out of it.
    let seen = signal(None::<(usize, usize)>);
    effect(move || {
        if active.get() {
            let t = store().turns.get();
            untracked(|| rows.set(t));
        }
    });
    // The first viewable row and the last scroll position: what the spot
    // the chat is left at is saved from, once scrolling settles.
    let first_row = std::rc::Rc::new(std::cell::Cell::new(None::<usize>));
    // the last row the list reports in view (the message field covers
    // the bottom of it) and the viewport's height
    let last_row = std::rc::Rc::new(std::cell::Cell::new(None::<usize>));
    let view_h = std::rc::Rc::new(std::cell::Cell::new(0.0f32));
    let last_scroll = std::rc::Rc::new(std::cell::Cell::new((0.0f32, false)));
    // Where it opens: it mounts when its chat becomes the shown one.
    let opening = opening_for(&mine);
    let shown = signal(false);
    // The fade runs natively (a motion's opacity on the display link).
    let fade = motion();
    fade.value(MotionChannel::Opacity).set(0.0);
    // Turn count and end state when the chat was hidden: what came in
    // meanwhile is offered by the "New messages" pill.
    let left = std::rc::Rc::new(std::cell::Cell::new(None::<(i64, bool)>));
    let first = std::rc::Rc::new(std::cell::Cell::new(true));
    {
        let list = list.clone();
        let last_scroll = last_scroll.clone();
        effect(move || {
            let on = active.get();
            untracked(|| {
                let st = store();
                if !on {
                    if !first.get() && shown.get_untracked() {
                        let last = rows.with(|t| t.last().map_or(-1, |t| t.i));
                        left.set(Some((last, last_scroll.get().1)));
                    }
                    fade.value(MotionChannel::Opacity).set(0.0);
                    return;
                }
                st.chat_list.replace(Some(list.clone()));
                if first.replace(false) {
                    open_at(opening, list.clone(), shown, fade);
                    return;
                }
                // Shown again: where it was, unless a link names a turn.
                if let Some((pos, i)) = st.take_jump() {
                    list.scroll_to_index(pos as u32, percentage(0.5), px(0.0), false);
                    st.flash(i);
                    st.new_below.set(None);
                } else if let Some((last, at_end)) = left.take() {
                    let fresh = rows.with(|t| t.iter().find(|t| t.i > last).map(|t| t.i));
                    if at_end && fresh.is_some() {
                        list.scroll_to_end(false);
                        st.new_below.set(None);
                    } else {
                        st.new_below.set(fresh);
                    }
                }
                let _ = fade.value(MotionChannel::Opacity).timing(1.0, 160.0, Easing::EaseOut);
            });
        });
    }
    view()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .bind_motion(&fade)
        // Hidden chats take no input.
        .pointer_events(move || if active.get() { PointerEvents::Auto } else { PointerEvents::None })
        .child(new_messages_pill(list.clone()))
        .child({
        let cell_chat = mine.clone();
        virtual_list(
            move || rows.with(|t| t.len()),
            // `TurnCell` — every row lives in the shared centered column
            // `maxWidth 992 + paddingHorizontal 16`, the same edges the
            // composer's card meets (shell.tsx:617, App.tsx:2188).
            move |at| {
                let chat = cell_chat.clone();
                view()
                    .items_center()
                    .child(
                        view()
                            .w_full()
                            .max_w(px(992.0))
                            .px_4()
                            // The cell's row is bound, not its index's:
                            // recycled cells re-render for their new
                            // index, and a turn's streamed text lands in
                            // place (gallery's `item`/`dynamic` pair).
                            .child({
                                // The row's own turn, compared: a change to
                                // any other turn (a streamed chunk, a
                                // reaction, a poll) no longer rebuilds this
                                // row — its markdown, its live mini app.
                                // Its own reaction neither: the badge
                                // redraws in place (`reaction_badge`); a
                                // rebuilt row was measured anew and the
                                // rows under it shifted.
                                let row = computed(move || {
                                    rows.with(|t| t.get(at.index()).cloned()).map(
                                        |mut t| {
                                            t.reaction.clear();
                                            // A building card's phase and
                                            // a card's revise flag neither:
                                            // the placeholders read them
                                            // live, not off the row.
                                            t.status.clear();
                                            t.revising = false;
                                            t
                                        },
                                    )
                                });
                                dynamic(move || {
                                    let Some(t) = row.get() else {
                                        return view().into_element();
                                    };
                                    // (provided here: the cells are the
                                    // list's, not under this component)
                                    provide_context(ChatShown(active));
                                    // out of the viewport, or under a
                                    // full-screen app
                                    provide_context(crate::mini::OutOfSight(std::rc::Rc::new(move || {
                                        let off = seen.with(|r| r.is_some_and(|(a, b)| !(a..=b).contains(&at.index())));
                                        off || !store().mini_full.with(Vec::is_empty)
                                    })));
                                    turn_row(t, &chat).into_element()
                                })
                            }),
                    )
            },
        )
        .keys(move || {
            rows.with(|t| t.iter().map(|t| t.i.to_string()).collect::<Vec<_>>())
        })
        .when(!matches!(opening, Opening::End), |l| {
            l.initial_scroll_index(match opening {
                Opening::Jump(pos, _) | Opening::Mark(pos, _) => pos,
                Opening::End => 0,
            })
        })
        .follow_end(px(40.0))
        // Rows not measured yet are placed at this height: nsh's 50pt
        // default had a fresh chat's viewport hold ~20 of them, all bound
        // (and their views made) at once, then most dropped once measured.
        .estimated_item_size(px(ROW_ESTIMATE))
        .element_ref(&list)
        // `chatScrolling` (shell.tsx:88) — true from the first scroll
        // event until ~160ms after the last; hover handlers read it to
        // skip their signal writes (every write is a commit).
        .on_viewable_items_change({
            let first_row = first_row.clone();
            let last_row = last_row.clone();
            move |v: &ViewableItems| {
                first_row.set(v.viewable.map(|(first, _)| first));
                last_row.set(v.viewable.map(|(_, last)| last));
                if v.viewable.is_some() && seen.get_untracked() != v.viewable {
                    seen.set(v.viewable);
                }
            }
        })
        .on_scroll({
            let epoch = std::rc::Rc::new(std::cell::Cell::new(0u64));
            let mine = mine.clone();
            let list = list.clone();
            let first_row = first_row.clone();
            let last_row = last_row.clone();
            let view_h = view_h.clone();
            let last_scroll = last_scroll.clone();
            move |e: &ScrollEvent| {
                let st = store();
                let y = e.offset.y.0;
                view_h.set(e.size.height.0);
                let at_end = y + e.size.height.0 >= e.content_size.height.0 - 48.0;
                last_scroll.set((y, at_end));
                if at_end && st.new_below.get_untracked().is_some() {
                    st.new_below.set(None);
                }
                // An anchored pill would float off its bar once the
                // content moves — dismiss it (App.tsx:2126); a message's
                // menu too.
                st.close_pill(-1);
                if st.ctx_owner.get_untracked().is_some() && st.ctx.get_untracked().is_some() {
                    st.ctx.set(None);
                }
                if !st.chat_scrolling.get_untracked() {
                    st.chat_scrolling.set(true);
                }
                let e = epoch.get().wrapping_add(1);
                epoch.set(e);
                let epoch = epoch.clone();
                let (mine, list, first_row, last_scroll) =
                    (mine.clone(), list.clone(), first_row.clone(), last_scroll.clone());
                let last_row = last_row.clone();
                let view_h = view_h.clone();
                spawn(async move {
                    timer(std::time::Duration::from_millis(160)).await;
                    if epoch.get() != e {
                        return;
                    }
                    let st = store();
                    st.chat_scrolling.set(false);
                    // Settled: save where this chat is — the first viewable
                    // row and how far into it, or the end — for when it is
                    // opened again.
                    if !shown.get_untracked() {
                        return;
                    }
                    // The message position follows a scroll the keys didn't
                    // make: the bottom message in view, ring hidden. The
                    // view ends at the message field's top (the list runs on
                    // under it): the last row starting 24pt above that.
                    let keyed = st.sel_stepped.get().is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(900));
                    if !keyed && st.active_id.get_untracked() == mine {
                        let cutoff = last_scroll.get().0 + view_h.get() - crate::composer::dock_h().get_untracked();
                        let (first, last) = (first_row.get().unwrap_or(0), last_row.get().unwrap_or(0));
                        let mut pick = None;
                        for r in (first..=last).rev() {
                            if list.item_offset(r).await.is_some_and(|top| top.0 + 24.0 <= cutoff) {
                                pick = Some(r);
                                break;
                            }
                        }
                        if let Some(i) = pick.and_then(|r| st.turns.with_untracked(|t| t.get(r).map(|t| t.i))) {
                            st.set_sel(i, false);
                        }
                    }
                    let (y, at_end) = last_scroll.get();
                    let last = st.turns.with_untracked(|t| t.last().map_or(-1, |t| t.i));
                    let row = first_row.get();
                    let into = match row {
                        Some(r) => list.item_offset(r).await.map(|at| y - at.0),
                        None => None,
                    };
                    let i = row.and_then(|r| st.turns.with_untracked(|t| t.get(r).map(|t| t.i)));
                    if let (Some(i), Some(into)) = (i, into) {
                        st.scroll_marks
                            .borrow_mut()
                            .insert(mine, crate::store::ScrollMark { i, into, at_end, last });
                    } else if at_end {
                        st.scroll_marks.borrow_mut().insert(
                            mine,
                            crate::store::ScrollMark { i: last, into: 0.0, at_end, last },
                        );
                    }
                });
            }
        })
        .flex_1()
        // `contentContainerStyle` (App.tsx:2132) — the padding and the
        // 20pt row gap are content insets, not view padding: rows slide
        // under the top bar while scrolling, and the last row ends 24pt
        // above the floating composer.
        .content_padding_top(px(crate::pane::TOP + 16.0))
        .content_padding_bottom(move || px(crate::composer::dock_h().get() + 24.0))
        .row_gap(px(20.0))
        })
}

/// A transcript's first showing: placed while invisible — a link's turn,
/// where the chat was left (`ScrollMark`), or the end — then faded in,
/// and settled once more as rows near the target finish measuring.
fn opening_for(mine: &str) -> Opening {
    let st = store();
    untracked(|| {
        if let Some((pos, i)) = st.take_jump() {
            st.new_below.set(None);
            return Opening::Jump(pos, i);
        }
        let mark = st.scroll_marks.borrow().get(mine).copied();
        let turns = st.turns.get_untracked();
        let Some(m) = mark.filter(|m| !m.at_end) else {
            st.new_below.set(None);
            return Opening::End;
        };
        let Some(pos) = turns.iter().position(|t| t.i == m.i) else {
            st.new_below.set(None);
            return Opening::End;
        };
        st.new_below.set(turns.iter().find(|t| t.i > m.last).map(|t| t.i));
        Opening::Mark(pos, m.into)
    })
}

/// Places a newly mounted transcript at `opening` (see [`opening_for`]).
fn open_at(opening: Opening, list: ElementRef, shown: Signal<bool>, fade: Motion) {
    let st = store();
    if let Opening::Jump(_, i) = opening {
        st.flash(i);
    }
    // A navigation after this (a link landing just now) owns the position
    // from then on: the remaining snaps stand down.
    let epoch = st.nav_epoch.get();
    spawn(async move {
        for (wait, reveal) in [(16, false), (64, true), (300, false)] {
            timer(std::time::Duration::from_millis(wait)).await;
            if store().nav_epoch.get() != epoch {
                shown.set(true);
                fade.value(MotionChannel::Opacity).set(1.0);
                break;
            }
            match opening {
                Opening::Jump(pos, _) => list.scroll_to_index(pos as u32, percentage(0.5), px(0.0), false),
                Opening::Mark(pos, into) => list.scroll_to_index(pos as u32, percentage(0.0), px(-into), false),
                Opening::End => list.scroll_to_end(false),
            }
            if reveal {
                shown.set(true);
                let _ = fade.value(MotionChannel::Opacity).timing(1.0, 160.0, Easing::EaseOut);
            }
        }
    });
}

/// "↓ New messages": turns that arrived while the chat was away, below
/// where it reopened; pressing scrolls to the first of them.
fn new_messages_pill(list: ElementRef) -> impl IntoElement {
    when(
        || store().new_below.get().is_some(),
        move || {
            let list = list.clone();
            view()
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
                .bottom(move || px(crate::composer::dock_h().get() + 12.0))
                .z_index(20)
                .items_center()
                .pointer_events(PointerEvents::BoxNone)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(12.0))
                        .py(px(6.0))
                        .rounded(px(999.0))
                        .bg(theme::PILL_BG)
                        .border(px(0.5))
                        .border_color(theme::BORDER_S)
                        .child(text("↓ New messages").text_size(px(12.0)).text_color(theme::TEXT))
                        .on_press(move || {
                            let st = store();
                            let Some(i) = st.new_below.get_untracked() else { return };
                            let pos = st.turns.with_untracked(|t| t.iter().position(|t| t.i == i));
                            if let Some(pos) = pos {
                                list.scroll_to_index(pos as u32, percentage(0.0), px(0.0), true);
                            }
                            st.new_below.set(None);
                        }),
                )
                .into_element()
        },
    )
}

/// The empty state when no thread is open or no turns exist: Grok
/// Shell's heading alone — no suggested prompts. A routine's thread
/// before it found anything: its name, and that new items land here (its
/// quiet line sits over the composer).
pub(crate) fn empty_state() -> impl IntoElement {
    dynamic(|| match store().active_routine() {
        Some(r) => view()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .child(text(r.name).text_size(px(22.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT))
            .child(
                text("Nothing new yet. What it finds shows up here.")
                    .text_size(px(14.0))
                    .text_color(theme::MUTED),
            )
            .into_element(),
        None => view()
            .flex_1()
            .items_center()
            .justify_center()
            .child(
                text("What's on the agenda today?")
                    .text_size(px(22.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT),
            )
            .into_element(),
    })
}

/// A routine thread's quiet line, centered over the composer: "Checks
/// every 5 min · last checked 3:42 PM · nothing new", or its error. A run
/// with nothing new adds no turn; this is where it shows. The plugins'
/// list (where it reads from) is pulled again on opening the thread and
/// every minute while it is open.
pub(crate) fn routine_status_line() -> impl IntoElement {
    effect(|| {
        if store().active_routine().is_some() {
            untracked(|| store().v1_refresh(false));
        }
    });
    set_interval(std::time::Duration::from_secs(60), || {
        if untracked(|| store().active_routine().is_some()) {
            store().v1_refresh(false);
        }
    });
    dynamic(|| match store().active_routine_status() {
        Some((line, failed)) => view()
            .flex_row()
            .w_full()
            .justify_center()
            .pb(px(6.0))
            .child(
                text(line)
                    .text_size(px(12.0))
                    .text_color(if failed { theme::DOWN } else { theme::MUTED })
                    .number_of_lines(2)
                    .max_w(px(560.0)),
            )
            .into_element(),
        None => view().into_element(),
    })
}

/// One turn of chat `chat` (its bot colors the user's bubbles).
fn turn_row(t: Turn, chat: &str) -> impl IntoElement {
    if let Some(n) = t.notice.clone() {
        notice_row(n).into_element()
    } else if let Some(n) = t.agents_notice.clone() {
        agents_notice_row(n).into_element()
    } else if let Some(c) = t.consent.clone() {
        consent_row(c).into_element()
    } else if t.role == "notice" {
        // a notice of a kind this build doesn't know
        view().into_element()
    } else if let Some(m) = t.message.clone() {
        message_row(t.i, m).into_element()
    } else if t.role == "user" && !t.instruction_from.is_empty() {
        instruction_row(t).into_element()
    } else if let (true, Some(r)) = (t.role == "user", t.reply_from.clone()) {
        reply_row(t, r).into_element()
    } else if t.role == "user" {
        user_row(t, chat.to_owned()).into_element()
    } else if let Some(r) = t.reply_from.clone() {
        // an answer made for another agent's request, its cards as usual
        let mut t = t;
        t.text = r.body(&t.text);
        view().gap_1().child(reply_caption(r)).child(agent_row(t)).into_element()
    } else {
        agent_row(t).into_element()
    }
}

// ---------------------------------------------------------------------------
// Agents messaging each other (agents.rs)
// ---------------------------------------------------------------------------

/// A small accent action in a centered notice line ("Open", "Resume").
fn notice_act(word: &'static str, aria: String, on: impl Fn() + 'static) -> impl IntoElement {
    let hov = signal(false);
    view()
        .label(aria)
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(on)
        .child(
            text(word)
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || if hov.get() { theme::TEXT } else { theme::ACCENT }),
        )
}

fn notice_dot() -> impl IntoElement {
    text("·").text_size(px(12.0)).text_color(theme::FAINT)
}

/// An agents' notice: one centered, dim line as a routine's run is —
/// "Piccolo → Boss · Q3 payments · 2 messages · Open". A paused
/// conversation's adds Resume while it is paused; a line inside the
/// conversation it names has no Open.
fn agents_notice_row(n: crate::agents::AgentsNotice) -> impl IntoElement {
    use crate::agents::NoticeKind;
    let st = store();
    let here = untracked(|| {
        let id = st.active_id.get();
        st.threads.with(|t| t.iter().any(|t| t.id == id && (t.id == n.thread || t.uid == n.thread)))
    });
    let open = n.opens() && !here;
    let (thread, message, label) = (n.thread.clone(), n.message.clone(), n.open_label());
    let conv = n.conversation.clone();
    view().flex_row().w_full().justify_center().child(
        view()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .max_w(px(620.0))
            // wraps in a narrow column instead of running off its edges
            .flex_shrink(1.0)
            .flex_wrap()
            .justify_center()
            .label(n.text.clone())
            .child(
                text(n.text.clone())
                    .text_size(px(12.0))
                    .text_color(theme::MUTED)
                    .text_center()
                    .number_of_lines(2)
                    .flex_shrink(1.0),
            )
            .when(n.kind == NoticeKind::Paused && !conv.is_empty(), |v| {
                v.child(dynamic(move || {
                    let c = conv.clone();
                    if store().agents_paused(&c) {
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(notice_dot())
                            .child(notice_act("Resume", "Resume the conversation".to_owned(), move || store().agents_resume(c.clone())))
                            .into_element()
                    } else if store().agents_resumed.with(|l| l.contains(&c)) {
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(notice_dot())
                            .child(text("Resumed").text_size(px(12.0)).text_color(theme::FAINT))
                            .into_element()
                    } else {
                        view().into_element()
                    }
                }))
            })
            .when(open, |v| {
                v.child(notice_dot()).child(notice_act("Open", label, move || {
                    store().go_to_key(&thread, Some(message.clone()).filter(|m| !m.is_empty()))
                }))
            }),
    )
}

/// A participant's avatar: its bot's mark, or for an outside endpoint
/// (no bot) a neutral disc with its initials.
pub(crate) fn peer_avatar(id: &str, name: &str, size: f32) -> AnyElement {
    match store().bots.with_untracked(|b| b.iter().find(|b| b.id == id).cloned()) {
        Some(b) => crate::avatar::bot_avatar(crate::avatar::BotAvatarProps {
            box_: Some(size),
            paused: true,
            ..crate::avatar::BotAvatarProps::bot(&b, size / 0.82)
        })
        .into_element(),
        None => view()
            .size(px(size))
            .rounded(px(size / 2.0))
            .items_center()
            .justify_center()
            .bg(theme::SUBTLE)
            .border(px(0.5))
            .border_color(theme::BORDER_S)
            .label(name.to_owned())
            .child(
                text(crate::agents::initials(name))
                    .text_size(px((size * 0.42).round()))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::DIM),
            )
            .into_element(),
    }
}

/// One message in an agents' conversation: the sender's avatar and name
/// over a bubble of Markdown; its tickets as chips, its `data` collapsed
/// under Details.
fn message_row(i: i64, m: crate::agents::AgentMessage) -> impl IntoElement {
    let body = m.text.clone();
    let to = match m.to.as_slice() {
        [] => String::new(),
        [one] => format!("to {}", one.name),
        many => format!("to {} agents", many.len()),
    };
    let data = m.data.clone();
    let open = signal(false);
    let hov = signal(false);
    view()
        .items_start()
        .gap(px(4.0))
        .label(format!("{} · {}", m.header(), m.text.chars().take(80).collect::<String>()))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(peer_avatar(&m.from.id, &m.from.name, 22.0))
                .child(text(m.from.name.clone()).text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT))
                .when(!to.is_empty(), |v| v.child(text(to).text_size(px(12.0)).text_color(theme::MUTED).number_of_lines(1))),
        )
        .child(
            view().flex_row().w_full().justify_start().pl(px(28.0)).child(
                view()
                    .max_w(px(560.0))
                    .flex_shrink(1.0)
                    .min_w(px(BUBBLE_MIN))
                    .bg_linear_to_b()
                    .gradient_from(theme::AGENT_BUBBLE_TOP)
                    .gradient_to(theme::AGENT_BUBBLE_BOTTOM)
                    .rounded(px(18.0))
                    .border(px(0.5))
                    .border_color(theme::AGENT_RIM)
                    .px(px(11.5))
                    .py(px(6.5))
                    .child(highlight_ring(i, 18.0))
                    .child(markdown(move || store().v1_display(&body), || false, || false)),
            ),
        )
        .when(!m.attachments.is_empty(), |v| {
            v.child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(6.0))
                    .pl(px(28.0))
                    .max_w(px(588.0))
                    .children(m.attachments.iter().map(|tok| {
                        let t = tok.clone();
                        view()
                            .flex_row()
                            .items_center()
                            .h(px(22.0))
                            .px(px(8.0))
                            .rounded(px(11.0))
                            .bg(theme::ACCENT_TINT)
                            .label(move || format!("Attached: {}", store().v1_plain(&t)))
                            .child({
                                let t = tok.clone();
                                text(move || store().v1_plain(&t)).number_of_lines(1).text_size(px(12.0)).text_color(theme::ACCENT)
                            })
                            .into_element()
                    })),
            )
        })
        .when(!data.is_empty(), |v| {
            v.child(
                view()
                    .pl(px(28.0))
                    .gap(px(4.0))
                    .child(
                        view()
                            .flex_row()
                            .label(move || if open.get() { "Hide details".to_owned() } else { "Show details".to_owned() })
                            .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                            .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                            .on_press(move || open.update(|o| *o = !*o))
                            .child(
                                text(move || if open.get() { "Details ▾" } else { "Details ›" })
                                    .text_size(px(12.0))
                                    .text_color(move || if hov.get() { theme::TEXT } else { theme::MUTED }),
                            ),
                    )
                    .child(when(open, move || {
                        view()
                            .max_w(px(560.0))
                            .px_3()
                            .py(px(8.0))
                            .rounded(px(10.0))
                            .bg(theme::SUBTLE)
                            .border(px(0.5))
                            .border_color(theme::BORDER_S)
                            .child(text(data.clone()).text_size(px(12.0)).line_height(px(17.0)).text_color(theme::DIM))
                            .into_element()
                    })),
            )
        })
}

/// "Reply from Boss · Open" (or "For Piccolo's request · Open") over a
/// turn another agent's message brought: Open shows the conversation at
/// that message.
fn reply_caption(r: crate::agents::ReplyFrom) -> impl IntoElement {
    let (thread, message) = (r.thread.clone(), r.message.clone());
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(text(r.caption()).text_size(px(12.0)).text_color(theme::MUTED).number_of_lines(1))
        .when(!thread.is_empty(), |v| {
            v.child(notice_dot()).child(notice_act("Open", "Open the conversation".to_owned(), move || {
                store().go_to_key(&thread, Some(message.clone()).filter(|m| !m.is_empty()))
            }))
        })
}

/// A user turn that is another agent's reply: captioned, in a quiet
/// bubble (the user never typed it), as a routine's instruction is.
fn reply_row(t: Turn, r: crate::agents::ReplyFrom) -> impl IntoElement {
    let body = r.body(&t.text);
    view()
        .items_start()
        .gap(px(4.0))
        .child(reply_caption(r))
        .when(!body.trim().is_empty(), |v| {
            v.child(
                view()
                    .max_w(px(560.0))
                    .flex_shrink(1.0)
                    .bg(theme::SUBTLE)
                    .border(px(0.5))
                    .border_color(theme::BORDER_S)
                    .rounded(px(12.0))
                    .px_3()
                    .py(px(7.0))
                    .child(highlight_ring(t.i, 12.0))
                    .child(markdown(move || store().v1_display(&body), || false, || false)),
            )
        })
}

// ---------------------------------------------------------------------------
// Routine runs
// ---------------------------------------------------------------------------

/// A routine's run in the chat it reports to: one centered, dim line —
/// "BIR payments ran · 2 new items · Open" — not a bubble. Open shows the
/// routine's thread at that run.
fn notice_row(n: RoutineNotice) -> impl IntoElement {
    let failed = !n.error.is_empty();
    let hov = signal(false);
    let (thread, turn) = (n.thread.clone(), n.turn.clone());
    view().flex_row().w_full().justify_center().child(
        view()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .max_w(px(560.0))
            .child(
                text(n.line())
                    .text_size(px(12.0))
                    .text_color(if failed { theme::DOWN } else { theme::MUTED })
                    .number_of_lines(2)
                    .flex_shrink(1.0),
            )
            .when(!thread.is_empty(), |v| {
                v.child(text("·").text_size(px(12.0)).text_color(theme::FAINT)).child(
                    view()
                        .label(format!("Open {}", n.name))
                        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                        .on_press(move || {
                            store().go_to_key(&thread, Some(turn.clone()).filter(|t| !t.is_empty()))
                        })
                        .child(
                            text("Open")
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || if hov.get() { theme::TEXT } else { theme::ACCENT }),
                        ),
                )
            }),
    )
}

/// A routine instance the agent made that waits for the user (5.9): the
/// question, Enable (switches it on, consenting to what it reads and
/// changes) and Not now (deletes it); once answered, a quiet line.
fn consent_row(c: crate::store::ConsentNotice) -> impl IntoElement {
    let inst = c.instance.clone();
    let answered = move || {
        let local = store().routine_consented.with(|l| l.iter().find(|(i, _)| *i == inst).map(|(_, s)| s.clone()));
        local
    };
    let (yes_hov, no_hov) = (signal(false), signal(false));
    let (c1, c2, c3) = (c.clone(), c.clone(), c.clone());
    let answer = move |c: &crate::store::ConsentNotice, state: &str| {
        let st = store();
        st.routine_consented.update(|l| l.push((c.instance.clone(), state.to_owned())));
        let action = if state == "enabled" { crate::mini_v1::host_api::InstanceAction::Enable } else { crate::mini_v1::host_api::InstanceAction::Delete };
        let extra = if state == "enabled" { serde_json::json!({"approve": true}) } else { serde_json::json!({}) };
        st.v1_instance(&c.plugin, &c.routine, &c.instance, action, extra);
    };
    view().flex_row().w_full().justify_center().child(dynamic(move || {
        let local = answered();
        if let Some(done) = c1.answered(local.as_deref()) {
            return text(format!("{} · {done}", c1.name))
                .text_size(px(12.0))
                .text_color(theme::MUTED)
                .number_of_lines(1)
                .into_element();
        }
        let (c2, c3) = (c2.clone(), c3.clone());
        let (yes, no) = (format!("Enable {}", c2.name), format!("Don't enable {}", c3.name));
        view()
            .max_w(px(560.0))
            .gap(px(8.0))
            .px_3()
            .py(px(10.0))
            .rounded(px(12.0))
            .bg(theme::SUBTLE)
            .border(px(0.5))
            .border_color(theme::BORDER_S)
            .label(format!("Enable {}?", c1.name))
            .child(text(c1.text.clone()).text_size(px(13.0)).text_color(theme::TEXT))
            .child(
                view()
                    .flex_row()
                    .gap_2()
                    .child(
                        text("Enable")
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::TEXT)
                            .px_3()
                            .py(px(5.0))
                            .rounded(px(8.0))
                            .bg(move || if yes_hov.get() { theme::ACTIVE } else { theme::HOVER })
                            .on_mouse_enter(move |_: &MouseEvent| yes_hov.set(true))
                            .on_mouse_leave(move |_: &MouseEvent| yes_hov.set(false))
                            .label(yes)
                            .on_press(move || answer(&c2, "enabled")),
                    )
                    .child(
                        text("Not now")
                            .text_size(px(12.0))
                            .text_color(move || if no_hov.get() { theme::TEXT } else { theme::MUTED })
                            .px_3()
                            .py(px(5.0))
                            .rounded(px(8.0))
                            .on_mouse_enter(move |_: &MouseEvent| no_hov.set(true))
                            .on_mouse_leave(move |_: &MouseEvent| no_hov.set(false))
                            .label(no)
                            .on_press(move || answer(&c3, "declined")),
                    ),
            )
            .into_element()
    }))
}

/// An item's instruction in a routine's thread: what the routine gave the
/// bot, captioned "Instruction from <routine>" — the user never typed it,
/// so it is no bubble of theirs.
fn instruction_row(t: Turn) -> impl IntoElement {
    let body = t.text.clone();
    view()
        .items_start()
        .gap(px(4.0))
        .child(
            text(format!("Instruction from {}", t.instruction_from))
                .text_size(px(12.0))
                .text_color(theme::MUTED)
                .number_of_lines(1),
        )
        .child(
            view()
                .max_w(px(560.0))
                .flex_shrink(1.0)
                .bg(theme::SUBTLE)
                .border(px(0.5))
                .border_color(theme::BORDER_S)
                .rounded(px(12.0))
                .px_3()
                .py(px(7.0))
                .child(highlight_ring(t.i, 12.0))
                .child(markdown(move || store().v1_display(&body), || false, || false)),
        )
}

// ---------------------------------------------------------------------------
// User bubble
// ---------------------------------------------------------------------------

fn user_row(t: Turn, chat: String) -> impl IntoElement {
    // The bubble takes this chat's bot's color, not the selected chat's
    // (a hidden transcript keeps its own).
    let body = t.text.clone();
    let reply = t.reply.clone();
    // Text narrower than the circle's inside (a one-letter "a") sits
    // centered in it, as in Grok Bot; anything wider stays left-aligned
    // (centering wide text let it overflow the bubble).
    let tiny = reply.is_none()
        && !body.contains('\n')
        && measure_text("400 15px -apple-system, system-ui, sans-serif", body.trim()).0 < BUBBLE_MIN - 24.0;
    // A replied-to card travels as a module:// attachment for the model; the
    // quote already shows it, so it gets no tile of its own.
    let atts: Vec<_> = t.attachments.iter().filter(|a| !a.url.starts_with("module://")).cloned().collect();
    // what the model got with it (10.3): the chat's v1 attachments
    let v1_atts = if t.v1_attachments.is_empty() { store().v1_sent_with(&chat, &t.text) } else { t.v1_attachments.clone() };
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
        .items_end()
        .gap_1()
        .on_mouse_enter(move |_: &MouseEvent| {
            if !store().chat_scrolling.get_untracked() {
                hov2.set(true);
            }
        })
        // `hover_out` always clears — skipping it during a scroll leaves
        // `hov` stuck true and the bar lit at rest once scrolling ends.
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
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
        .when(!v1_atts.is_empty(), |v| v.child(crate::mini_v1::ui::sent_attachments(v1_atts)))
        .child(
            // The bubble: `bubbleFill` (the bot's color), radius 18,
            // padding 7×12, `maxWidth 560` (shell.tsx:438). Inside it
            // `OnFill` holds — the quote row and Markdown keep the dark
            // ramp in both themes.
            // `flex-row w-full` (shell.tsx:434): a full-width row gives the
            // bubble a definite width to shrink into; a hugging row let a
            // long message run off the column's left edge unwrapped.
            view().flex_row().w_full().justify_end().child(
                view()
                    .max_w(px(560.0))
                    // Never narrower than tall: a one-letter message is a
                    // 36pt circle, as in Grok Bot.
                    .min_w(px(BUBBLE_MIN))
                    .when(tiny, |v| v.items_center())
                    // `flexShrink: 1` — wrap inside a narrow column
                    // instead of running under the pane.
                    .flex_shrink(1.0)
                    .bg(move || theme::bubble_fill_for(&chat))
                    .rounded(px(18.0))
                    .px_3()
                    .py(px(7.0))
                    .child(highlight_ring(i, 18.0))
                    .child(reaction_badge(i, "msg".to_owned(), false))
                    .child(fill_quote(reply))
                    .child(markdown(move || store().v1_display(&body), || false, || true)),
            ),
        )
        // `bar(true, false)` (shell.tsx:472) — right-aligned, no react on
        // your own bubbles: reply + ⋯ only (`bar_slot`).
        .child(bar_slot(
            true,
            move || bar_opacity() > 0.5 || store().ctx_open_for(i, "msg"),
            vec![
                act("↩", "reply", move || store().reply_to_turn(i)).into_element(),
                turn_more(i).into_element(),
            ],
        ))
}

/// `badge` (shell.tsx:375) — the iMessage tapback: a 36pt knockout disc
/// painted in the page color centered on the bubble's outer top corner,
/// holding a 30pt chip with the emoji. `right` picks the corner (user →
/// left, agent → right). Slot `slot`'s reaction (`"msg"` or an
/// artifact's), redrawn in place when it changes.
pub(crate) fn reaction_badge(i: i64, slot: String, right: bool) -> impl IntoElement {
    dynamic(move || {
        let r = store().reaction_of(i, &slot);
        if r.is_empty() {
            // Absolute as the badge is: a column's `gap` skips both, so a
            // reaction coming or going moves nothing.
            view().absolute().into_element()
        } else {
            tapback_in(i, slot.clone(), r, right).into_element()
        }
    })
}

/// The badge for `reaction` on `slot`; pressing it clears that slot's.
fn tapback_in(i: i64, slot: String, reaction: String, right: bool) -> impl IntoElement {
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
                .on_press(move || store().react_slot(i, &slot, String::new())),
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
        let q: String = store().v1_plain(&r.text).split_whitespace().collect::<Vec<_>>().join(" ");
        let q: String = q.chars().take(80).collect();
        let i = r.i;
        view()
            .flex_row()
            .items_center()
            .gap(px(5.0))
            .mb(px(2.0))
            .on_press(move || {
                let st = store();
                st.go_to(&st.active_id.get_untracked(), Some(i));
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

/// `ArtPlaceholder` — what a building artifact renders before its module
/// exists (shell.tsx:188), drawn as ChatGPT's generation placeholder: a
/// dot grid at the output's declared `w×h`, fitted to the column as the
/// finished card will be (`min(1, 560/w, 560/h)`). An image is the grid
/// alone; a mini app keeps its card's title/desc header over it, as an
/// office file does its file name over a page / slide / sheet grid. The
/// turn id keys the grid's progress, so a rebuilt row never restarts it.
fn art_placeholder(t: &Turn) -> impl IntoElement {
    let art = t.art.clone().unwrap_or_default();
    let image = t.label == "image";
    let dw = if art.w > 0.0 { art.w as f32 } else { 480.0 };
    let dh = if art.h > 0.0 { art.h as f32 } else { (dw * 0.75).round() };
    // A mini app's card padding around the grid; a bare image has none.
    let pad = if image { 0.0 } else { 24.0 };
    // The measured column width; 0 until the first layout.
    let avail = signal(0.0f32);
    let fit = move || {
        let f = 1.0f32.min(560.0 / dw).min(560.0 / dh);
        let a = avail.get();
        if a > 0.0 { f.min(((a - pad) / dw).max(0.0)) } else { f }
    };
    let iw = move || (dw * fit()).round().max(140.0);
    let ih = move || (dh * iw() / dw).round();
    let tau = if image { crate::dotgrid::TAU_IMAGE } else { crate::dotgrid::TAU_APP };
    let grid = view()
        .w(move || px(iw()))
        .h(move || px(ih()))
        .child(crate::dotgrid::dot_grid(t.id.clone(), tau));
    let body = if image {
        grid.into_element()
    } else {
        let title = match (art.title.is_empty(), art.kind.as_str()) {
            (false, _) => art.title.clone(),
            (true, "presentation") => "Presentation".to_owned(),
            (true, "spreadsheet") => "Spreadsheet".to_owned(),
            (true, "document" | "pdf") => "Document".to_owned(),
            _ => "Mini app".to_owned(),
        };
        app_placeholder(title, art.desc.clone(), grid)
            .w(move || px(iw() + pad))
            .max_w(percentage(1.0))
            .bg(theme::CARD)
            .rounded(px(16.0))
            .border_1()
            .border_color(theme::BORDER_F)
            .into_element()
    };
    view()
        .w_full()
        .items_start()
        .on_layout(move |b: &Bounds<Points>| {
            if (b.size.width.0 - avail.get_untracked()).abs() >= 0.5 {
                avail.set(b.size.width.0);
            }
        })
        .child(body)
}

/// A mini app's placeholder: its title (and `desc`) over `grid`, the dot
/// grid at the size it is given, padded as a card. A building widget's
/// card is one ([`art_placeholder`]); an inline card's hold puts one over
/// the app it holds (`mini::mini_app_inline`).
pub(crate) fn app_placeholder(title: String, desc: String, grid: View) -> View {
    view()
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
                text(desc)
                    .text_size(px(12.0))
                    .text_color(theme::FAINT)
                    .pt(px(2.0))
                    .number_of_lines(1),
            )
        })
        .child(grid.mt(px(10.0)))
}

fn agent_row(t: Turn) -> impl IntoElement {
    let caption = t.caption.clone();
    let body = t.text.clone();
    let streaming = t.building;
    let i = t.i;
    let has_card = t.art.is_some();
    // a reply with no text (only files or Drive links, or nothing left)
    // shows no bubble — an empty one otherwise sits above its cards
    let bubble = !has_card && !(body.trim().is_empty() && !streaming);
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
    // `bar(false, true)` — react / reply / ⋯ (shell.tsx:503): under a
    // message bubble here; a card mounts it in its own row instead
    // (`CardShell`'s bar, shell.tsx:1103).
    let bar = move || -> AnyElement {
        // `bar(false, true)` — react / reply / ⋯ (shell.tsx:503), under
        // the bubble before the artifacts, shown only while the row is
        // hovered (or its pill is open). The bar (the pill's anchor)
        // stays mounted either way.
        let shown = move || {
            bar_opacity() > 0.5
                || store()
                    .pill
                    .get()
                    .as_ref()
                    .is_some_and(|a| a.i == i && a.slot == "msg")
                || store().ctx_open_for(i, "msg")
        };
        bar_slot(
            false,
            shown,
            vec![
                react_act(i, "msg".to_owned()).into_element(),
                act("↩", "reply", move || store().reply_to_turn(i)).into_element(),
                turn_more(i).into_element(),
            ],
        )
        .into_element()
    };
    view()
        // shell.tsx:481 — the agent column's `gap: 4` spaces the bubble,
        // its bar, and the artifacts uniformly; the 20pt turn separation
        // is the list's `row_gap`, not per-row padding.
        .gap_1()
        .on_mouse_enter(move |_: &MouseEvent| {
            if !store().chat_scrolling.get_untracked() {
                hov2.set(true);
            }
        })
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        // shell.tsx:479 — a turn naming a module (`tt.name`) is an app
        // card and nothing else: no sender line, no message bubble (the
        // module name and an empty bubble were leaking through).
        // Only real children: an empty view still takes the column's 4pt
        // gap, where Grok's `<If>` renders nothing (bot rows sat ~4.5pt
        // looser than Grok's).
        .when(!has_card && t.reply.is_some(), |v| v.child(reply_quote(&t)))
        // The agent bubble (shell.tsx:484): `agentFill` + a hairline,
        // radius 18, padding 16×10, `maxWidth 560`, hugging its content
        // on the row's left edge; the tapback knocks out top-right.
        .when(bubble, |v| v.child(
            view().flex_row().w_full().justify_start().child(
                view()
                    .max_w(px(560.0))
                    // `flexShrink: 1` — wrap inside a narrow column
                    // instead of running under the pane.
                    .flex_shrink(1.0)
                    .min_w(px(BUBBLE_MIN))
                    // Grok Bot's `.sand-message`: a faint top-down gradient
                    // under a 0.5pt ring (drawn inset there: the padding
                    // gives back its half points), 12×7 like the user's.
                    .bg_linear_to_b()
                    .gradient_from(theme::AGENT_BUBBLE_TOP)
                    .gradient_to(theme::AGENT_BUBBLE_BOTTOM)
                    .rounded(px(18.0))
                    .border(px(0.5))
                    .border_color(theme::AGENT_RIM)
                    .px(px(11.5))
                    .py(px(6.5))
                    .child(highlight_ring(i, 18.0))
                    .child(reaction_badge(i, "msg".to_owned(), true))
                    .child(markdown(move || store().v1_display(&body), move || streaming, || false)),
            ),
        ))
        // A building artifact renders its declared placeholder — before
        // the module exists, the card is the dot grid filling in at the
        // output's size (`ArtPlaceholder` in shell.tsx). A fetched module that is
        // still compiling (`d === "loading"`) shows it too, until
        // `mini_source` lands or `mini_err` reports.
        .when(has_card && t.building, |v| v.child(art_placeholder(&t)))
        .when(has_card && !t.building, |v| {
            let app = t.art.clone().map(|a| a.app).unwrap_or_default();
            let t2 = t.clone();
            let shell = crate::widgets::CardShell {
                id: t.id.clone(),
                art: t.art.clone().unwrap_or_default(),
                label: t.label.clone(),
                v: t.v,
                caption,
                i,
                bar: Some(std::rc::Rc::new(bar)),
            };
            v.child(
                when(
                    move || {
                        store().mini_source(&app).get().is_empty()
                            && store().mini_err(&app).get().is_empty()
                    },
                    move || art_placeholder(&t2).into_element(),
                )
                .otherwise(move || app_card(shell.clone()).into_element()),
            )
        })
        // Interactive cards (approvals, choices, forms, plugin needs) in
        // document order, under the words they follow.
        .when(!t.cards.is_empty(), |v| v.child(crate::cards::section(&t)))
        // The dev hook's local turn has nothing on the server to react to.
        .when(!has_card && i < crate::store::LOCAL_TURN_BASE, |v| v.child(bar()))
        .when(!t.files.is_empty(), |v| v.child(files_section(&t)))
        .when(!t.links.is_empty(), |v| v.child(links_section(&t)))
        // the answer's next steps (9.8): a tap sends one as your message
        .when(!t.suggest.is_empty(), |v| v.child(crate::mini_v1::ui::suggest_chips(t.suggest.clone())))

}

/// Grok Bot's highlighted message (a jump's target): a 3px ring in the
/// text color just outside the bubble (`box-shadow: 0 0 0 3px`), which
/// fades out (`Store::flash`); the message ⌘↑ / ⌘↓ selected keeps one in
/// the accent color. Mounted only on that turn.
pub(crate) fn highlight_ring(i: i64, radius: f32) -> impl IntoElement {
    when(
        move || store().highlight.get() == Some(i) || store().sel_shown.get() && store().sel_msg.get() == Some(i),
        move || {
            if store().sel_shown.get_untracked() && store().sel_msg.get_untracked() == Some(i) {
                let ring = element_ref();
                store().sel_ring.set(Some(ring));
                return view()
                    .element_ref(&ring)
                    .absolute()
                    .top(px(-3.0))
                    .bottom(px(-3.0))
                    .left(px(-3.0))
                    .right(px(-3.0))
                    .rounded(px(radius + 3.0))
                    .border(px(3.0))
                    .border_color(theme::ACCENT)
                    .into_element();
            }
            view()
                .absolute()
                .top(px(-3.0))
                .bottom(px(-3.0))
                .left(px(-3.0))
                .right(px(-3.0))
                .rounded(px(radius + 3.0))
                .border(px(3.0))
                .border_color(theme::TEXT)
                .opacity(move || if store().highlight_on.get() { 1.0f32 } else { 0.0 })
                .transition_opacity()
                .duration(400.0)
                .into_element()
        },
    )
}

/// A one-line bubble's height (7 + 22 + 7): no bubble is narrower.
const BUBBLE_MIN: f32 = 36.0;

/// A message's hover bar: always mounted at its height (rows keep their
/// size), made invisible while not `shown` — a hover only flips a style.
/// Building its buttons on hover put views in and out of the window on
/// every enter and leave, 8–13 ms a frame for 5–14 views; a style flip
/// costs well under one. The bar is the slot itself and each button a
/// single text, so it adds 2–3 views to a row.
fn bar_slot(right: bool, shown: impl Fn() -> bool + 'static, acts: Vec<AnyElement>) -> View {
    view()
        .flex_row()
        .when(right, |v| v.justify_end())
        .gap(px(2.0))
        .h(px(ACT))
        .style_when(move || !shown(), |s| s.invisible())
        .children(acts)
}

/// `Act` (shell.tsx:94) — a bare 24pt text glyph; the hovered one gets a
/// rounded plate. No icon drawing: Grok's glyphs are text, and the glyph
/// is the button (one view, centered by its 24pt line).
fn act(
    glyph: &'static str,
    label: &'static str,
    on: impl Fn() + 'static,
) -> impl IntoElement {
    act_at(glyph, label, move |_, _| on())
}

/// [`act`] told where it is: `on(x, y)` gets the button's top-left corner
/// in page coordinates, from the pointer's last move over it — the
/// platform's own hit test. `measure()` read stale positions inside the
/// transcript's recycled cells: menus opened over the wrong row, or not
/// at all.
fn act_at(
    glyph: &'static str,
    label: &'static str,
    on: impl Fn(f32, f32) + 'static,
) -> impl IntoElement {
    let hovered = signal(false);
    let origin = std::rc::Rc::new(std::cell::Cell::new((0.0f32, 0.0f32)));
    let at = origin.clone();
    text(glyph)
        .label(label)
        .size(px(ACT))
        .rounded(px(6.0))
        .text_center()
        .text_size(px(15.0))
        .line_height(px(ACT))
        .number_of_lines(1)
        .text_color(theme::DIM)
        .bg(move || if hovered.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_mouse_move(move |e: &MouseEvent| {
            at.set((
                e.page_position.x.0 - e.position.x.0,
                e.page_position.y.0 - e.position.y.0,
            ));
        })
        .on_press(move || {
            let (x, y) = origin.get();
            on(x, y)
        })
}

/// An [`act`]'s side; a bar is three of them 2pt apart.
const ACT: f32 = 24.0;

/// `openPicker` (shell.tsx:281) — the ☺ button opens the shared emoji
/// pill under (or over) its bar, which starts at the ☺ (bars are
/// left-aligned). `slot` is the owning bar's name — `"msg"` or
/// `"art:<path>"` — and reopening the same slot toggles it closed.
fn react_act(i: i64, slot: String) -> impl IntoElement {
    act_at("☺", "react", move |x, y| {
        let st = store();
        if st
            .pill
            .get_untracked()
            .as_ref()
            .is_some_and(|a| a.i == i && a.slot == slot)
        {
            st.close_pill(i);
            return;
        }
        let current = untracked(|| st.reaction_of(i, &slot));
        let flip = y + ACT + 205.0 > window().height.0 - 80.0;
        st.open_pill(crate::store::PillAnchor {
            x,
            y,
            w: ACT * 3.0 + 4.0,
            h: ACT,
            flip,
            right: false,
            i,
            slot: slot.clone(),
            current,
        });
    })
}

/// `more()` (shell.tsx:301) — the ⋯ button opening `items()` under
/// itself; bar `(i, slot)` stays up while the menu is open.
fn more_act(i: i64, slot: String, items: impl Fn() -> Vec<crate::store::CtxItem> + 'static) -> impl IntoElement {
    act_at("⋯", "more", move |x, y| {
        let st = store();
        st.close_pill(-1);
        crate::ctx::open_near(x, y, y + ACT, items());
        st.ctx_owner.set(Some((i, slot.clone())));
    })
}

/// The ⋯ of a message or card bar: Copy / Delete, or a card's actions.
fn turn_more(i: i64) -> impl IntoElement {
    more_act(i, "msg".to_owned(), move || {
        let card = store()
            .turns
            .get_untracked()
            .iter()
            .find(|t| t.i == i)
            .and_then(|t| t.art.as_ref().map(|a| a.app.clone()));
        match card {
            Some(app) => crate::ctx::card_items(i, &app),
            None => crate::ctx::msg_items(i),
        }
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
            v.child(artifact(
                crate::files::media_gallery(media).into_element(),
                i,
                None,
                &msg,
            ))
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

/// `t.links`: the Drive files the reply made or changed, one card each
/// (opens in the browser).
fn links_section(t: &Turn) -> impl IntoElement {
    view()
        .gap_2()
        .w_full()
        .max_w(px(560.0))
        // a v1 answer's cards (9.6, 9.8): one main, two small, links after;
        // they age into chips after five of your messages
        .child({
            let cards: Vec<_> = t.links.iter().filter(|l| l.provider == "rox-app").cloned().collect();
            let i = t.i;
            // the person's messages shown after this one (local replies sit
            // among the server's turns, so the order counts, not `i`)
            let user_after = move || {
                store().turns.with(|ts| ts.iter().skip_while(|x| x.i != i).filter(|x| x.role == "user").count())
            };
            crate::mini_v1::ui::answer_cards(&t.id, cards, user_after)
        })
        .children(t.links.iter().filter(|l| l.provider != "rox-app").map(|l| crate::files::link_card(l).into_element()))
}

/// `artifact(node, f)` (shell.tsx:331): the card with its own hover bar —
/// ☺ react, ↩ reply (files travel with the reply), ⋯ menu — and the
/// turn's tapback knocked out at its top-right (`badge("right")`).
fn artifact(
    node: AnyElement,
    i: i64,
    f: Option<Attach>,
    msg: &str,
) -> impl IntoElement {
    let hov = signal(false);
    let items = crate::ctx::artifact_items(i, f.as_ref(), msg);
    // `"art:" + (f?.path || "gallery")` (shell.tsx:339) — each artifact's
    // bar owns its own pill slot.
    let slot = format!("art:{}", f.as_ref().map(|f| f.path.clone()).unwrap_or_else(|| "gallery".to_owned()));
    let slot2 = slot.clone();
    // The file itself goes along with a reply on it (App.tsx:580).
    let reply_file = f.as_ref().map(|f| f.path.clone()).filter(|p| !p.is_empty());
    let reply_label = f
        .as_ref()
        .map(|f| format!("📎 {}", f.name))
        .unwrap_or_else(|| msg.chars().take(120).collect());
    view()
        .self_start()
        .gap_1()
        .max_w(percentage(1.0))
        .on_mouse_enter(move |_: &MouseEvent| {
            if !store().chat_scrolling.get_untracked() {
                hov.set(true);
            }
        })
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .child(node)
        // This artifact's own reaction (`slot`), not the message's: a
        // reply's files each take theirs.
        .child(reaction_badge(i, slot.clone(), true))
        // The bar is the pill's anchor; shown while hovered, or its pill
        // open.
        .child(
            // On the left: the artifact's tapback sits at its top-right,
            // and a bar under that corner ran into the next one's.
            bar_slot(
                false,
                move || {
                    store()
                        .pill
                        .get()
                        .as_ref()
                        .is_some_and(|a| a.i == i && a.slot == slot2)
                        || store().ctx_open_for(i, &slot2)
                        || (hov.get() && !store().chat_scrolling.get())
                },
                vec![
                    react_act(i, slot.clone()).into_element(),
                    act("↩", "reply", move || {
                        let st = store();
                        let name = st
                            .active_bot_untracked()
                            .map(|b| b.name)
                            .unwrap_or_default();
                        st.reply_to.set(Some(crate::store::ReplyTo {
                            i,
                            name,
                            text: reply_label.clone(),
                            files: reply_file.iter().cloned().collect(),
                        }));
                    })
                    .into_element(),
                    more_act(i, slot, move || items.clone()).into_element(),
                ],
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
pub(crate) fn emoji_pill(i: i64, slot: String, current: String) -> impl IntoElement {
    let grid = signal(false);
    let slot_grid = slot.clone();
    let current_grid = current.clone();
    // `dot` — a 30pt circle: current → `#f5f5f52b`, hover → `#f5f5f514` +
    // a 1.25 scale pop (120ms).
    let dot = move |e: &'static str| {
        let h = signal(false);
        let (h1, h2, h3) = (h.clone(), h.clone(), h.clone());
        let e2 = e.to_owned();
        let cur = current.clone();
        let cur2 = current.clone();
        let slot = slot.clone();
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
            .on_mouse_enter(move |_: &MouseEvent| h.set(true))
            .on_mouse_leave(move |_: &MouseEvent| h3.set(false))
            .on_press(move || {
                let st = store();
                st.react_slot(i, &slot, if cur2 == e { String::new() } else { e.to_owned() });
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
                        .on_mouse_enter(move |_: &MouseEvent| ph2.set(true))
                        .on_mouse_leave(move |_: &MouseEvent| plus_hov.set(false))
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
                                        let slot_grid = slot_grid.clone();
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
                                            .on_mouse_enter(move |_: &MouseEvent| h.set(true))
                                            .on_mouse_leave(move |_: &MouseEvent| h2.set(false))
                                            .on_press(move || {
                                                let st = store();
                                                st.react_slot(i, &slot_grid, if cur2 == *e { String::new() } else { e.to_string() });
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
                    .child(emoji_pill(a.i, a.slot.clone(), a.current.clone())),
            )
            .into_element()
    })
}
