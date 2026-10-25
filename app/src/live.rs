//! The bridge between `ra-jit-live` and nsh's engine thread.
//!
//! The daemon answers each edit with a patch: the functions that differ,
//! linked against this executable and written where [`Live::start`]
//! reserved room beside it. `install` polls for patches inside an engine
//! task — a point where no reactive work is half done — and on each one
//! applied, snapshots the [`keep`] signals and bumps `remount`. The
//! `dynamic` region under `remount` then rebuilds: components under it
//! never match an old element, so each is created again and its `view`
//! runs the new code. What `keep` marked holds its value through the
//! swap; nothing else is carried: a patch that changes a type the program
//! can see is answered `Restart` by the daemon, not applied.
//!
//! The app's model does not ride `keep`: [`store`] anchors the whole
//! `Store` — every signal of the chat state — under the root owner, so a
//! remount redraws against the same data (a draft is not lost, a thread
//! stays open). This file is the dependency the anchor lives in: it is
//! never re-emitted by a patch, so its map is only ever the
//! executable's one, on every platform.

use std::{
    any::Any,
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    time::Duration,
};

use nsh_core::ui::prelude::*;

use crate::store::Store;

/// How often the engine polls for a patch: the apply itself is under a
/// millisecond, so this sets save-to-screen latency more than cost.
const POLL: Duration = Duration::from_millis(25);

// ---------------------------------------------------------------------------
// State anchor — how hot code reaches THE store.
//
// A `static` in patchable code is hopeless twice over: a local one has no
// name the splice can match, so each patch emits a private copy of it (the
// "empty board" bug), and a named one is written back to its initializer
// on every apply. A `thread_local` shares on macOS but cannot splice on
// the iOS simulator at all. `ra_jit_live::anchor` is the way out: a
// non-generic function in a dependency — code a patch never re-emits — so
// its map is only ever the executable's one, on every platform.
// ---------------------------------------------------------------------------

/// Called once from `App`'s body — under the root owner, which never
/// remounts. `store()` anchors the model while that owner is current, so
/// its signals live as long as the app.
pub(crate) fn init() {
    let _ = store();
}

/// The one model the whole process has — every patch generation reads and
/// writes the same signals, so the conversation survives remounts, not
/// just values.
pub(crate) fn store() -> &'static Store {
    let at = ra_jit_live::anchor("rox.store", || {
        Box::leak(Box::new(Store::new())) as *const Store as usize
    });
    unsafe { &*(at as *const Store) }
}

thread_local! {
    /// The live session (patch room behind the image, the watcher's patch
    /// file). Started once, with the app.
    static LIVE: RefCell<Option<ra_jit_live::Live>> = const { RefCell::new(None) };
    /// What reads each `keep` signal now: refreshed as components mount.
    static READERS: RefCell<HashMap<&'static str, Rc<dyn Fn() -> Box<dyn Any>>>> =
        RefCell::new(HashMap::new());
    /// What the last snapshot left: read once by the next `keep` of the
    /// name, which registers its own reader instead.
    static KEPT: RefCell<HashMap<&'static str, Box<dyn Any>>> = RefCell::new(HashMap::new());
}

/// A signal that survives a hot remount.
///
/// `keep("counter.count", || 0)` reads like `signal(0)` except: before the
/// hot area rebuilds, its value is written into a table under `name`, and
/// the `keep` of the name in the rebuilt tree starts from that value. The
/// name is the identity — keep the same name (and type) across edits, or
/// the old value is dropped and `init` runs.
///
/// The snapshot runs before the rebuild (nsh reconciles new elements
/// before it releases the old ones, so an `on_cleanup` on the outgoing
/// instance would run too late to be read by its replacement).
pub fn keep<T: PartialEq + Clone + 'static>(name: &'static str, init: impl FnOnce() -> T) -> Signal<T> {
    let restored = KEPT.with(|kept| {
        kept.borrow_mut()
            .remove(name)
            .and_then(|value| value.downcast::<T>().ok())
            .map(|value| *value)
    });
    let sig = signal(restored.unwrap_or_else(init));
    READERS.with(|readers| {
        readers.borrow_mut().insert(
            name,
            Rc::new(move || Box::new(sig.get_untracked()) as Box<dyn Any>),
        );
    });
    sig
}

/// Starts `ra_jit_live::Live` and polls it on the engine thread.
///
/// - `Outcome::Applied`: snapshots the `keep` signals, bumps `remount`
///   (which rebuilds the hot area), and reports it on `note`.
/// - `Outcome::Restart` / `Outcome::Declined`: reports on `note`; the app
///   runs on as it was.
///
/// Call it once, under the root, which is never remounted itself: the
/// `Live` it holds and the interval it spawns belong to that owner.
pub fn install(remount: Signal<u64>, note: Signal<String>) {
    // `note` is `<state>:<message>` — `live`, `applied`, `restart`,
    // `declined` or `off` (no ra-jit daemon: a plain `cargo run`) — the
    // pill shows the state as a dot and the message on hover.
    LIVE.with(|live| {
        if live.borrow().is_none() {
            *live.borrow_mut() = ra_jit_live::Live::start().ok();
        }
        note.set(if live.borrow().is_some() {
            "live:ra-jit live".to_owned()
        } else {
            "off:ra-jit is not connected (plain cargo run)".to_owned()
        });
    });
    set_interval(POLL, move || {
        let outcome =
            LIVE.with(|live| live.borrow_mut().as_mut().and_then(ra_jit_live::Live::poll));
        let Some(outcome) = outcome else {
            return;
        };
        match outcome {
            ra_jit_live::Outcome::Applied { .. } => {
                let line = outcome.to_string();
                snapshot();
                remount.update(|v| *v = v.wrapping_add(1));
                note.set(format!("applied:{line}"));
            }
            ra_jit_live::Outcome::Restart(_) => note.set(format!("restart:{outcome}")),
            ra_jit_live::Outcome::Declined(_) => note.set(format!("declined:{outcome}")),
        }
    });
}

/// Reads every `keep` signal into the table its replacement will read.
fn snapshot() {
    READERS.with(|readers| {
        let readers = readers.borrow();
        KEPT.with(|kept| {
            let mut kept = kept.borrow_mut();
            for (name, read) in readers.iter() {
                kept.insert(name, read());
            }
        });
    });
}
