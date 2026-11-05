//! The card over the screen: an AppKit sheet on the window of the open.
//!
//! A sheet draws over everything in that window, so a full-screen mini app
//! can't cover it, and it has no node in nsh's tree: nothing the AI or a
//! test channel reads or presses there reaches it. Its text is the [`Card`]
//! and nothing else, and it offers only Not now and Allow.
//!
//! Allow is the person's own hand or nothing: its action makes a
//! [`PersonYes`] only for a mouse release AppKit is handing it now, on that
//! button, in the sheet, from no process ([`by_hand`]). So a synthesized
//! click, which reaches nsh's views and no other, an accessibility press,
//! which has no mouse event, and an event a process posted, which names that
//! process, do nothing. No key presses Allow: Esc is Not now, and Return does
//! nothing.
//!
//! What was measured here, on macOS 26 (Darwin 25): `kCGEventSourceUnixProcessID`
//! on a posted event is the poster's pid whatever source or state it used, and
//! setting the field to 0 before posting doesn't stick. That hardware reports 0
//! is what Apple's event system is known to do, and hasn't been measured on this
//! Mac: if it is ever wrong, Allow fails closed, and the log says what came.
//! `BUWIZ_HUB_POSTED_ALLOW=1` lets a posted click through for scripted tests. It
//! is in the `dev-agent` feature, so a release build has no such setting.

use std::cell::RefCell;

use buwiz_agent::OnceId;
use nsh_platform_macos::{
    objc2::{
        MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
        rc::Retained,
        runtime::{NSObject, NSObjectProtocol},
        sel,
    },
    objc2_app_kit::{NSAlert, NSAlertStyle, NSApplication, NSButton, NSEvent, NSEventType, NSWindow},
    objc2_foundation::{NSPointInRect, NSProcessInfo, NSString},
};

use objc2_core_graphics::{CGEvent, CGEventField};

use super::hold::{Answer, Card, PersonYes};

const ALLOW: isize = 1;
const NOT_NOW: isize = 0;
/// How old the event may be when the button's action fires.
const FRESH: f64 = 1.0;

type Done = Box<dyn FnOnce(Answer) + Send>;

/// The card up now.
struct Up {
    once: OnceId,
    alert: Retained<NSAlert>,
    parent: Retained<NSWindow>,
    _target: Retained<Buttons>,
    done: Done,
}

thread_local! {
    static UP: RefCell<Option<Up>> = const { RefCell::new(None) };
}

define_class!(
    /// The two buttons' target.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BuwizCardButtons"]
    struct Buttons;

    unsafe impl NSObjectProtocol for Buttons {}

    impl Buttons {
        #[unsafe(method(pressed:))]
        fn pressed(&self, sender: &NSButton) {
            pressed(sender);
        }
    }
);

impl Buttons {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated `init` on a fresh allocation.
        unsafe { msg_send![super(this), init] }
    }
}

/// What AppKit is handing the button.
#[derive(Clone, Copy)]
struct Press {
    /// A left mouse release.
    release: bool,
    /// Seconds old.
    age: f64,
    in_sheet: bool,
    on_button: bool,
    /// The process that posted it: 0 for none. `None` when it can't be told.
    source: Option<i64>,
}

/// Whether `p` is a person's press of the button: a mouse release, just now,
/// in the sheet, on the button, that no process posted.
fn by_hand(p: Press, posted_counts: bool) -> bool {
    let hardware = p.source == Some(0) || (posted_counts && p.source.is_some());
    p.release && (0.0..FRESH).contains(&p.age) && p.in_sheet && p.on_button && hardware
}

/// The process that posted `event`: 0 for hardware.
fn source_of(event: &NSEvent) -> (Option<i64>, i64) {
    let Some(cg) = event.CGEvent() else { return (None, 0) };
    (Some(CGEvent::integer_value_field(Some(&cg), CGEventField::EventSourceUnixProcessID)), CGEvent::integer_value_field(Some(&cg), CGEventField::EventSourceStateID))
}

/// Whether a posted click may press Allow, for scripted tests. Compiled out
/// of a release build: `scripts/release/build.sh` checks the binary.
fn posted_counts() -> bool {
    #[cfg(feature = "dev-agent")]
    if buwiz_protocol::env::var("BUWIZ_HUB_POSTED_ALLOW").as_deref() == Some("1") {
        return true;
    }
    false
}

fn pressed(sender: &NSButton) {
    let mtm = MainThreadMarker::new().expect("on the main thread");
    if sender.tag() == NOT_NOW {
        return end_up(|_| true, Answer::NotNow);
    }
    let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else { return };
    let sheet = sender.window();
    let in_sheet = event.window(mtm).is_some_and(|w| sheet.as_deref().is_some_and(|s| std::ptr::eq(&*w, s)));
    let at = sender.convertPoint_fromView(event.locationInWindow(), None);
    let age = NSProcessInfo::processInfo().systemUptime() - event.timestamp();
    let (source, state) = source_of(&event);
    let press = Press { release: event.r#type() == NSEventType::LeftMouseUp, age, in_sheet, on_button: NSPointInRect(at, sender.bounds()), source };
    if by_hand(press, posted_counts()) {
        end_up(|_| true, Answer::Allow(PersonYes::by_hand()));
    } else {
        log::warn!("agent hub: a press of Allow that no hand made changed nothing: source {source:?} state {state}");
    }
}

/// Takes the sheet down and answers, if `pick` says it is the one up.
fn end_up(pick: impl Fn(&OnceId) -> bool, answer: Answer) {
    let Some(up) = UP.with_borrow_mut(|u| u.take_if(|up| pick(&up.once))) else { return };
    up.parent.endSheet(&up.alert.window());
    (up.done)(answer);
}

/// Shows `card` on `window`; `done` hears the answer, once. Main thread.
pub fn show(once: OnceId, card: &Card, window: &NSWindow, done: Done) {
    let mtm = MainThreadMarker::new().expect("on the main thread");
    if UP.with_borrow(Option::is_some) {
        return done(Answer::Changed);
    }
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&NSString::from_str(&card.title));
    let about = format!("{}\n\nThe AI pressed this in {}. Nothing goes out unless you allow it.", card.lines.join("\n"), card.app);
    alert.setInformativeText(&NSString::from_str(&about));
    let target = Buttons::new(mtm);
    for (title, tag, key) in [("Not now", NOT_NOW, "\u{1b}"), ("Allow", ALLOW, "")] {
        let button = alert.addButtonWithTitle(&NSString::from_str(title));
        button.setTag(tag);
        button.setKeyEquivalent(&NSString::from_str(key));
        // SAFETY: the target outlives the sheet ([`Up`]), and `pressed:` takes the sender.
        unsafe {
            button.setTarget(Some(&target));
            button.setAction(Some(sel!(pressed:)));
        }
    }
    alert.beginSheetModalForWindow_completionHandler(window, None);
    UP.set(Some(Up { once, alert, parent: window.retain(), _target: target, done }));
}

/// Whether the card of `once` is up. Main thread.
pub fn is_up(once: &OnceId) -> bool {
    UP.with_borrow(|u| u.as_ref().is_some_and(|up| up.once == *once))
}

/// Takes the card of `once` down, if it is up, with `answer`. Main thread.
pub fn end(once: &OnceId, answer: Answer) {
    end_up(|o| o == once, answer);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hand() -> Press {
        Press { release: true, age: 0.01, in_sheet: true, on_button: true, source: Some(0) }
    }

    #[test]
    fn only_a_fresh_mouse_release_on_the_button_in_the_sheet_is_a_hand() {
        assert!(by_hand(hand(), false));
        // a key, or an accessibility press with whatever event came last
        assert!(!by_hand(Press { release: false, ..hand() }, false));
        assert!(!by_hand(Press { age: FRESH, ..hand() }, false), "an old click on the way to an accessibility press");
        assert!(!by_hand(Press { age: -1.0, ..hand() }, false));
        assert!(!by_hand(Press { in_sheet: false, ..hand() }, false), "a click in the window under the sheet");
        assert!(!by_hand(Press { on_button: false, ..hand() }, false), "released off the button");
    }

    #[test]
    fn a_click_a_process_posted_is_no_hand() {
        assert!(!by_hand(Press { source: Some(17897), ..hand() }, false));
        assert!(!by_hand(Press { source: None, ..hand() }, false), "can't tell is no");
    }

    #[test]
    fn the_test_setting_takes_a_posted_click_and_nothing_else() {
        assert!(by_hand(Press { source: Some(17897), ..hand() }, true));
        assert!(!by_hand(Press { source: None, ..hand() }, true));
        assert!(!by_hand(Press { source: Some(17897), release: false, ..hand() }, true));
        assert!(!by_hand(Press { source: Some(17897), on_button: false, ..hand() }, true));
    }

    /// A build without `dev-agent` can't reach the setting, whatever the
    /// environment says. Both feature sets run this.
    #[test]
    fn without_dev_agent_there_is_no_test_setting() {
        if !cfg!(feature = "dev-agent") {
            // SAFETY: tests read this variable only here.
            unsafe { std::env::set_var("BUWIZ_HUB_POSTED_ALLOW", "1") };
            assert!(!posted_counts());
        }
    }
}
