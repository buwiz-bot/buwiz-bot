//! The card over the screen: an AppKit sheet on the window of the open.
//!
//! A sheet draws over everything in that window, so a full-screen mini app
//! can't cover it, and it has no node in nsh's tree: nothing the AI or a
//! test channel reads or presses there reaches it. Its text is the [`Card`]
//! and nothing else, every line of it, in a view that scrolls when there are
//! too many to fit, and it offers only Not now and Allow. A card that doesn't
//! fit keeps Allow off until its last line has been on screen ([`note_scroll`]):
//! a yes covers what the person could read. One that fits has it on at once.
//!
//! Allow is the person's own hand or nothing: its action makes a
//! [`PersonYes`] only for a mouse release AppKit is handing it now, on that
//! button, in the sheet, from no process, on a card read to its end
//! ([`by_hand`]). So a synthesized
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
    objc2_app_kit::{NSAlert, NSAlertStyle, NSApplication, NSButton, NSEvent, NSEventType, NSFont, NSScrollView, NSTextView, NSWindow},
    objc2_foundation::{NSPoint, NSPointInRect, NSProcessInfo, NSRect, NSSize, NSString},
};

use objc2_core_graphics::{CGEvent, CGEventField};

use super::hold::{Answer, Card, PersonYes};

const ALLOW: isize = 1;
const NOT_NOW: isize = 0;
/// The card's text: how wide, and how tall before it scrolls.
const TEXT_WIDTH: f64 = 360.0;
const TEXT_MAX_HEIGHT: f64 = 300.0;
const ROW_HEIGHT: f64 = 18.0;
/// Narrow characters to a row at [`TEXT_WIDTH`], for the widest Latin capitals.
/// Low on purpose: a card a little too tall is safe, and one that looks shorter
/// than it is would leave out the hint to scroll.
const ROW_UNITS: usize = 26;
/// How old the event may be when the button's action fires.
const FRESH: f64 = 1.0;
/// How close to the text's end counts as there, in points: less than a line.
const END_SLACK: f64 = 2.0;

type Done = Box<dyn FnOnce(Answer) + Send>;

/// The card up now.
struct Up {
    once: OnceId,
    alert: Retained<NSAlert>,
    parent: Retained<NSWindow>,
    _target: Retained<Buttons>,
    done: Done,
    allow: Retained<NSButton>,
    scroll: Retained<NSScrollView>,
    /// The text's end has been on screen.
    read: bool,
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
    /// The card has been read to its end.
    read: bool,
}

/// Whether `p` is a person's press of the button: a mouse release, just now,
/// in the sheet, on the button, on a card read to its end, that no process
/// posted.
fn by_hand(p: Press, posted_counts: bool) -> bool {
    let hardware = p.source == Some(0) || (posted_counts && p.source.is_some());
    p.release && (0.0..FRESH).contains(&p.age) && p.in_sheet && p.on_button && p.read && hardware
}

/// Whether the text's end shows: `shown_to` is how far down it the view
/// reaches, and `height` how tall it stands.
fn end_shows(shown_to: f64, height: f64) -> bool {
    shown_to >= height - END_SLACK
}

/// How tall `scroll`'s text stands, laid out now: the view's own frame grows
/// only when AppKit gets to it, and a card that reads as short has Allow on.
fn laid_out_height(scroll: &NSScrollView) -> Option<f64> {
    let text = scroll.documentView()?.downcast::<NSTextView>().ok()?;
    // SAFETY: both belong to `text`, which outlives them here.
    let (manager, container) = unsafe { (text.layoutManager()?, text.textContainer()?) };
    manager.ensureLayoutForTextContainer(&container);
    Some(manager.usedRectForTextContainer(&container).size.height + 2.0 * text.textContainerInset().height)
}

/// Whether the end of `scroll`'s text is on screen. What can't be measured
/// isn't.
fn at_end(scroll: &NSScrollView) -> bool {
    let shown = scroll.documentVisibleRect();
    laid_out_height(scroll).is_some_and(|height| end_shows(shown.origin.y + shown.size.height, height))
}

/// Whether a card of `lines` has Allow on as it goes up, and whether it does
/// once its view is scrolled to the end: for the test on real AppKit views,
/// which have to be on the main thread (`tests/card_scroll.rs`).
pub fn allow_on(mtm: MainThreadMarker, lines: &[String]) -> Option<(bool, bool)> {
    let view = lines_view(mtm, lines, text_height(lines).min(TEXT_MAX_HEIGHT))?;
    let first = at_end(&view);
    let text = view.documentView()?;
    text.scrollPoint(NSPoint::new(0.0, laid_out_height(&view)?));
    Some((first, at_end(&view)))
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
    let read = UP.with_borrow(|u| u.as_ref().is_some_and(|up| up.read));
    let press = Press { release: event.r#type() == NSEventType::LeftMouseUp, age, in_sheet, on_button: NSPointInRect(at, sender.bounds()), source, read };
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

/// About how tall `lines` stand at [`TEXT_WIDTH`].
fn text_height(lines: &[String]) -> f64 {
    let units = |l: &String| l.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum::<usize>();
    let rows: usize = lines.iter().map(|l| units(l).div_ceil(ROW_UNITS).max(1)).sum();
    rows as f64 * ROW_HEIGHT + 8.0
}

/// The card's lines, all of them, in a view that scrolls past `height`.
/// `None` when it can't hold them: then there is no card.
fn lines_view(mtm: MainThreadMarker, lines: &[String], height: f64) -> Option<Retained<NSScrollView>> {
    let scroll = NSTextView::scrollableTextView(mtm);
    scroll.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(TEXT_WIDTH, height)));
    scroll.setDrawsBackground(false);
    let text = scroll.documentView()?.downcast::<NSTextView>().ok()?;
    text.setDrawsBackground(false);
    text.setEditable(false);
    text.setSelectable(false);
    text.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    let all = lines.join("\n");
    text.setString(&NSString::from_str(&all));
    (text.string().to_string() == all).then_some(scroll)
}

/// Shows `card` on `window`; `done` hears the answer, once. Main thread.
pub fn show(once: OnceId, card: &Card, window: &NSWindow, done: Done) {
    let mtm = MainThreadMarker::new().expect("on the main thread");
    if UP.with_borrow(Option::is_some) {
        return done(Answer::Changed);
    }
    let height = text_height(&card.lines);
    let Some(view) = lines_view(mtm, &card.lines, height.min(TEXT_MAX_HEIGHT)) else { return done(Answer::NoCard) };
    let read = at_end(&view);
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&NSString::from_str(&card.title));
    let scrolls = if read { "" } else { " Scroll to the end to allow it." };
    let about = format!("The AI pressed this in {}. Nothing goes out unless you allow it.{scrolls}", card.app);
    alert.setInformativeText(&NSString::from_str(&about));
    alert.setAccessoryView(Some(&view));
    let target = Buttons::new(mtm);
    let mut allow = None;
    for (title, tag, key) in [("Not now", NOT_NOW, "\u{1b}"), ("Allow", ALLOW, "")] {
        let button = alert.addButtonWithTitle(&NSString::from_str(title));
        button.setTag(tag);
        button.setKeyEquivalent(&NSString::from_str(key));
        // SAFETY: the target outlives the sheet ([`Up`]), and `pressed:` takes the sender.
        unsafe {
            button.setTarget(Some(&target));
            button.setAction(Some(sel!(pressed:)));
        }
        if tag == ALLOW {
            button.setEnabled(read);
            allow = Some(button);
        }
    }
    let Some(allow) = allow else { return done(Answer::NoCard) };
    alert.beginSheetModalForWindow_completionHandler(window, None);
    UP.set(Some(Up { once, alert, parent: window.retain(), _target: target, done, allow, scroll: view, read }));
}

/// Looks whether the end of the card of `once` is on screen, and turns Allow
/// on when it is: it stays on. Main thread, as the card is watched.
pub fn note_scroll(once: &OnceId) {
    UP.with_borrow_mut(|u| {
        if let Some(up) = u.as_mut().filter(|up| up.once == *once && !up.read && at_end(&up.scroll)) {
            up.read = true;
            up.allow.setEnabled(true);
        }
    });
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

    #[test]
    fn a_tall_card_stands_taller_than_a_short_one_and_a_long_line_counts_its_rows() {
        let line = |n: usize| "x".repeat(n);
        assert!(text_height(&[line(10)]) < TEXT_MAX_HEIGHT);
        assert_eq!(text_height(&[line(ROW_UNITS * 3)]), text_height(&[line(1), line(1), line(1)]));
        assert!(text_height(&vec![line(10); 50]) > TEXT_MAX_HEIGHT);
    }

    /// Wide glyphs take more of a row: their rows count double, so a card of
    /// them reads as tall and gets its hint.
    #[test]
    fn wide_glyphs_stand_taller_than_their_count() {
        let wide = |n: usize| "東".repeat(n);
        assert_eq!(text_height(&[wide(ROW_UNITS)]), text_height(&["x".repeat(ROW_UNITS * 2)]));
        assert!(text_height(&vec![wide(ROW_UNITS * 2); 5]) > TEXT_MAX_HEIGHT, "what doesn't fit says so");
    }

    fn hand() -> Press {
        Press { release: true, age: 0.01, in_sheet: true, on_button: true, source: Some(0), read: true }
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
    fn a_press_on_a_card_not_read_to_its_end_is_no_hand() {
        assert!(!by_hand(Press { read: false, ..hand() }, false));
        assert!(!by_hand(Press { read: false, source: Some(17897), ..hand() }, true), "not the test setting's either");
    }

    #[test]
    fn the_end_shows_only_at_the_end() {
        assert!(end_shows(300.0, 300.0), "a card that fits");
        assert!(end_shows(400.0, 300.0), "a card shorter than its view");
        assert!(!end_shows(300.0, 1440.0), "a long one, from the top");
        assert!(!end_shows(1420.0, 1440.0), "a line short");
        assert!(end_shows(1439.5, 1440.0), "the last line, to the rounding");
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
