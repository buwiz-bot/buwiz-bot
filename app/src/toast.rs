//! Toasts: a short message over the window, top centre, gone after a few
//! seconds — what a command or a token couldn't do (slash.rs), kept out of
//! the composer. nsh-base's `ToastStack` lays them out and moves them; its
//! `ToastManager` keeps their lifecycle (enter, timeout, hover pause, exit).

use std::cell::{Cell, RefCell};
use std::time::Duration;

use nsh_base::{
    Anchor, ToastManager, ToastMotion, ToastOptions, ToastStackState, ToastTransitionStatus, toast,
    toast_stack,
};
use nsh_core::{time::now, ui::prelude::*};

use crate::theme::tc;

type Manager = Stored<ToastManager<u64, String>>;
type Shown = Signal<Vec<(u64, String, ToastTransitionStatus)>>;

thread_local! {
    /// The mounted layer's manager and what it shows (`show` before the
    /// layer mounts drops the message).
    static LAYER: RefCell<Option<(Manager, Shown)>> = const { RefCell::new(None) };
    static NEXT: Cell<u64> = const { Cell::new(1) };
}

/// Shows `msg` for four seconds.
pub(crate) fn show(msg: impl Into<String>) {
    let msg = msg.into();
    LAYER.with_borrow(|l| {
        let Some((manager, shown)) = *l else { return };
        let id = NEXT.replace(NEXT.get() + 1);
        manager.with(|m| {
            m.push(
                id,
                msg,
                ToastOptions::timeout(Some(Duration::from_secs(4))),
                now(),
            )
        });
        sync(manager, shown);
    });
}

/// The three newest toasts into `shown`.
fn sync(manager: Manager, shown: Shown) {
    shown.set(manager.with(|m| m.visible(3).map(|(id, t, s)| (*id, t.clone(), s)).collect()));
}

/// The toasts, over everything.
pub(crate) fn layer() -> impl IntoElement {
    let manager: Manager = stored(ToastManager::new(ToastMotion::sonner()));
    let shown: Shown = signal(Vec::new());
    let state = ToastStackState::new();
    set_interval(Duration::from_millis(50), move || {
        if manager.with(|m| m.advance(now(), state.is_expanded()).changed) {
            sync(manager, shown);
        }
    });
    LAYER.with_borrow_mut(|l| *l = Some((manager, shown)));
    // the stack picks the edge it grows from; centring it under the top bar is ours
    view()
        .absolute()
        .top(px(60.0))
        .left(px(0.0))
        .right(px(0.0))
        .items_center()
        .pointer_events(PointerEvents::BoxNone)
        .child(
            toast_stack()
                .placement(Anchor::TopCenter)
                .state(state)
                .w(px(360.0))
                .child(each(
                    shown,
                    |(id, _, _)| *id,
                    move |item, _| {
                        toast()
                            .transition_status(move || item.get().2)
                            .accessibility_label(move || Some(SharedString::from(item.get().1)))
                            .on_dismiss_request(move || {
                                manager.with(|m| m.dismiss(&item.get().0, now()));
                            })
                            .rounded_xl()
                            .px_4()
                            .py_3()
                            .bg(tc("#1f1f1f"))
                            .border(px(0.5))
                            .border_color(tc("#f5f5f51a"))
                            .shadow_lg()
                            .transition_all()
                            .duration(200.0)
                            .starting_class("opacity-0 -translate-y-4")
                            .ending_class("opacity-0 scale-95")
                            .child(
                                text(move || item.get().1)
                                    .number_of_lines(3)
                                    .text_size(px(13.0))
                                    .line_height(px(18.0))
                                    .text_color(tc("#f5f5f5")),
                            )
                    },
                )),
        )
}
