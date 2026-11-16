//! `/data/reminders` through EventKit. The osascript JXA path costs ~30 s
//! per call (Apple Events, one round-trip per property), so the widget's
//! poll and every tap lagged for seconds; EventKit answers in ms.
//!
//! EventKit needs the bundle's `NSReminders*UsageDescription` text and the
//! calendars entitlement, and macOS 13 lacks the macOS 14 request call. So
//! every entry point answers with an [`Ek`]: a result, a real EventKit
//! error, or "fall back" (access denied or restricted, an access request
//! that failed, an OS without the call), which the caller turns into the
//! old osascript path. Only the pure parts (due dates, order, JSON) are
//! unit-tested; nothing here is run against the real Reminders database.

use std::cmp::Ordering;
use std::sync::atomic::{AtomicU8, Ordering as Atomic};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{Bool, NSObjectProtocol};
use objc2::{sel, Message};
use objc2_event_kit::{EKAuthorizationStatus, EKEntityType, EKEventStore, EKReminder};
use objc2_foundation::{NSArray, NSCalendar, NSCalendarUnit, NSDate, NSDateComponents, NSError, NSString};
use serde_json::{json, Value};

/// A fetch or save that has not answered by now is reported as an error.
const WAIT: Duration = Duration::from_secs(15);
/// How long one call waits for the permission prompt to be answered.
const ASK_WAIT: Duration = Duration::from_secs(4);
/// The most reminders `list` returns, as the JXA's first 50.
const LIMIT: usize = 50;

/// What an entry point found.
#[derive(Debug, PartialEq)]
pub enum Ek {
    /// The route's response body.
    Done(Value),
    /// A real EventKit failure, answered as `{error}`; no fallback.
    Failed(String),
    /// EventKit cannot be used here; the reason is for the log.
    Fallback(String),
}

impl Ek {
    /// The shape `LocalTools` answers with: `None` sends the route down the
    /// osascript path.
    pub fn into_local(self) -> Option<Result<Value, String>> {
        match self {
            Ek::Done(v) => Some(Ok(v)),
            Ek::Failed(e) => Some(Err(e)),
            Ek::Fallback(_) => None,
        }
    }
}

// ————— access —————

#[derive(Debug, PartialEq)]
enum Access {
    Granted,
    Fallback(String),
    /// The prompt is still up.
    Waiting,
}

/// What a status settles; `None` means the user was not asked yet.
fn classify(status: EKAuthorizationStatus) -> Option<Access> {
    if status == EKAuthorizationStatus::FullAccess {
        Some(Access::Granted)
    } else if status == EKAuthorizationStatus::Denied {
        Some(Access::Fallback("reminders access denied".to_owned()))
    } else if status == EKAuthorizationStatus::Restricted {
        Some(Access::Fallback("reminders access restricted".to_owned()))
    } else if status == EKAuthorizationStatus::NotDetermined {
        None
    } else {
        // write-only (macOS 14+): cannot read the list
        Some(Access::Fallback("reminders access is write-only".to_owned()))
    }
}

/// One access request for the whole process: `true` while its prompt is up.
static ASKING: Mutex<bool> = Mutex::new(false);
static ASKED: Condvar = Condvar::new();

fn status() -> EKAuthorizationStatus {
    unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Reminder) }
}

/// Starts the system's access request. macOS 14 has
/// `requestFullAccessToRemindersWithCompletion:`; on 13 that selector does
/// not exist (calling it aborts the process), so the older
/// `requestAccessToEntityType:completion:` is used there.
///
/// Blocks: EventKit copies a completion block it keeps (the Objective-C
/// convention for block arguments; ours is already on the heap, so the
/// copy is a retain). Dropping our reference after the call is therefore
/// always safe, whether the callback comes in 1 ms or never, and nothing
/// is leaked. The callback holds a clone of the store, so the store lives
/// until the callback has run or the block is released.
fn request(store: &EKEventStore) {
    let keep = store.retain();
    let block = RcBlock::new(move |_granted: Bool, _err: *mut NSError| {
        let _keep = &keep;
        *ASKING.lock().unwrap_or_else(|e| e.into_inner()) = false;
        ASKED.notify_all();
    });
    let ptr = &*block as *const block2::DynBlock<dyn Fn(Bool, *mut NSError)> as *mut _;
    if store.respondsToSelector(sel!(requestFullAccessToRemindersWithCompletion:)) {
        unsafe { store.requestFullAccessToRemindersWithCompletion(ptr) };
    } else {
        #[allow(deprecated)]
        unsafe {
            store.requestAccessToEntityType_completion(EKEntityType::Reminder, ptr)
        };
    }
}

/// Reminders access, asking at most once at a time and waiting at most
/// [`ASK_WAIT`] for the answer. The authorization status, not the callback's
/// flag, is the truth afterwards.
fn access(store: &EKEventStore) -> Access {
    if let Some(a) = classify(status()) {
        return a;
    }
    let start = {
        let mut asking = ASKING.lock().unwrap_or_else(|e| e.into_inner());
        !std::mem::replace(&mut *asking, true)
    };
    if start {
        request(store);
    }
    let deadline = Instant::now() + ASK_WAIT;
    let mut asking = ASKING.lock().unwrap_or_else(|e| e.into_inner());
    while *asking {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        asking = ASKED.wait_timeout(asking, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    let pending = *asking;
    drop(asking);
    match classify(status()) {
        Some(a) => a,
        None if pending => Access::Waiting,
        None => Access::Fallback("reminders access request failed".to_owned()),
    }
}

/// A store per call (EventKit's thread-safety for one shared store is not
/// documented, and calls come from several blocking threads), an
/// autorelease pool around the Objective-C work, access checked first.
fn with_store(f: impl FnOnce(&EKEventStore) -> Ek) -> Ek {
    autoreleasepool(|_| {
        let store = unsafe { EKEventStore::new() };
        match access(&store) {
            Access::Granted => f(&store),
            Access::Fallback(why) => Ek::Fallback(why),
            Access::Waiting => Ek::Failed(
                "waiting for Reminders permission: answer the system prompt, then try again".to_owned(),
            ),
        }
    })
}

// ————— rows —————

/// One reminder, extracted inside the fetch block (the objects are only
/// safe to read there).
#[derive(Debug, Clone, PartialEq)]
struct Row {
    name: String,
    list: String,
    due: Option<f64>,
    completed: bool,
}

fn due_ms(comps: &NSDateComponents) -> Option<f64> {
    let date = NSCalendar::currentCalendar().dateFromComponents(comps)?;
    Some(date.timeIntervalSince1970() * 1000.0)
}

fn row(rem: &EKReminder) -> Row {
    let name = unsafe { rem.title() }.to_string();
    let list = unsafe { rem.calendar() }.map(|c| unsafe { c.title() }.to_string()).unwrap_or_default();
    let due = unsafe { rem.dueDateComponents() }.as_deref().and_then(due_ms);
    Row { name, list, due, completed: unsafe { rem.isCompleted() } }
}

/// The order EventKit's unspecified one is replaced by, before the cut to
/// 50 and before a name is matched: list name, then due date (none last),
/// then title.
fn order(a: &Row, b: &Row) -> Ordering {
    a.list
        .cmp(&b.list)
        .then_with(|| match (a.due, b.due) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        })
        .then_with(|| a.name.cmp(&b.name))
}

/// Indices of the open rows, in [`order`] (stable for equal rows).
fn open_in_order(rows: &[Row]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..rows.len()).filter(|&i| !rows[i].completed).collect();
    idx.sort_by(|&a, &b| order(&rows[a], &rows[b]));
    idx
}

fn row_json(r: &Row) -> Value {
    json!({
        "name": r.name,
        "list": r.list,
        "due": r.due.map(crate::js::iso),
        // EventKit does not expose Reminders' flagged bit; kept for widgets
        "flagged": false,
    })
}

/// `{reminders}`: the first [`LIMIT`] open rows in [`order`].
fn list_json(rows: &[Row]) -> Value {
    let reminders: Vec<Value> = open_in_order(rows).into_iter().take(LIMIT).map(|i| row_json(&rows[i])).collect();
    json!({ "reminders": reminders })
}

/// The row `complete` takes: the first open one of that name in [`order`].
fn pick(rows: &[Row], name: &str) -> Option<usize> {
    open_in_order(rows).into_iter().find(|&i| rows[i].name == name)
}

// ————— fetching —————

const PENDING: u8 = 0;
const CLAIMED: u8 = 1;
const ABANDONED: u8 = 2;

/// Fetches the open reminders and runs `handle` on them inside the
/// completion block. The block copies as in [`request`], so dropping it on
/// a timeout is safe. A callback after the timeout must not act: it claims
/// the call first, and the caller, on timeout, abandons it first; whoever
/// wins decides. `Err(None)` is a timeout that left nothing running,
/// `Err(Some(_))` one where the callback had started and did not finish.
fn fetch_open<T: Send + 'static>(
    store: &EKEventStore,
    handle: impl Fn(&NSArray<EKReminder>) -> T + 'static,
) -> Result<T, Option<()>> {
    let pred = unsafe { store.predicateForIncompleteRemindersWithDueDateStarting_ending_calendars(None, None, None) };
    let (tx, rx) = mpsc::channel();
    let state = Arc::new(AtomicU8::new(PENDING));
    let seen = state.clone();
    let block = RcBlock::new(move |ptr: *mut NSArray<EKReminder>| {
        if seen.compare_exchange(PENDING, CLAIMED, Atomic::SeqCst, Atomic::SeqCst).is_err() {
            return;
        }
        let out = if ptr.is_null() { handle(&NSArray::new()) } else { handle(unsafe { &*ptr }) };
        let _ = tx.send(out);
    });
    let token = unsafe { store.fetchRemindersMatchingPredicate_completion(&pred, &block) };
    match rx.recv_timeout(WAIT) {
        Ok(v) => Ok(v),
        Err(_) => {
            if state.compare_exchange(PENDING, ABANDONED, Atomic::SeqCst, Atomic::SeqCst).is_ok() {
                unsafe { store.cancelFetchRequest(&token) };
                Err(None)
            } else {
                // the callback is mid-way: wait for what it does
                rx.recv_timeout(WAIT).map_err(|_| Some(()))
            }
        }
    }
}

/// `GET /data/reminders` → `{reminders: [{name,list,due,flagged}]}`.
pub fn list() -> Ek {
    with_store(|store| {
        let rows = fetch_open(store, |arr| arr.iter().map(|r| row(&r)).collect::<Vec<Row>>());
        match rows {
            Ok(rows) => Ek::Done(list_json(&rows)),
            Err(_) => Ek::Failed("reminders fetch timed out".to_owned()),
        }
    })
}

/// `POST /data/reminders/complete` `{name}` → `{ok: done}`: the first open
/// reminder of that name in [`order`] (the JXA's `whose({completed:false,
/// name})` took the first in Reminders' own order, which EventKit has not).
pub fn complete(name: &str) -> Ek {
    with_store(|store| {
        let wanted = name.to_owned();
        let saver = store.retain();
        let done = fetch_open(store, move |arr| -> Result<bool, String> {
            let items: Vec<Retained<EKReminder>> = arr.iter().collect();
            let rows: Vec<Row> = items.iter().map(|r| row(r)).collect();
            let Some(i) = pick(&rows, &wanted) else { return Ok(false) };
            unsafe { items[i].setCompleted(true) };
            match unsafe { saver.saveReminder_commit_error(&items[i], true) } {
                Ok(()) => Ok(true),
                Err(e) => Err(e.localizedDescription().to_string()),
            }
        });
        match done {
            Ok(Ok(done)) => Ek::Done(json!({ "ok": done })),
            Ok(Err(e)) => Ek::Done(json!({ "error": e })),
            Err(None) => Ek::Failed("reminders fetch timed out; nothing was marked done".to_owned()),
            Err(Some(())) => {
                Ek::Failed("reminders save timed out; the reminder may or may not have been marked done".to_owned())
            }
        }
    })
}

// ————— due dates —————

/// A `due` a widget sent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Due {
    /// An instant, ms since the epoch (a `Z` or `±hh:mm` was given).
    At(i64),
    /// A wall-clock time in the Mac's zone (no zone given).
    Local { year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32 },
}

fn digits(b: &[u8]) -> Option<u32> {
    if b.is_empty() || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    b.iter().try_fold(0u32, |n, d| n.checked_mul(10)?.checked_add(u32::from(d - b'0')))
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// What `new Date(due)` took, as far as widgets send it: RFC 3339 with or
/// without fractional seconds (`2026-11-13T09:00:00.000Z`, `...+08:00`), a
/// bare date (local midnight; JavaScript would say UTC), and a date-time
/// with no zone (local; `T` or a space, seconds optional).
pub fn parse_due(text: &str) -> Option<Due> {
    let b = text.trim().as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let year = i32::try_from(digits(&b[0..4])?).ok()?;
    let (month, day) = (digits(&b[5..7])?, digits(&b[8..10])?);
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    let rest = &b[10..];
    if rest.is_empty() {
        return Some(Due::Local { year, month, day, hour: 0, minute: 0, second: 0 });
    }
    if !matches!(rest[0], b'T' | b't' | b' ') || rest.len() < 6 || rest[3] != b':' {
        return None;
    }
    let (hour, minute) = (digits(&rest[1..3])?, digits(&rest[4..6])?);
    let mut at = 6;
    let (mut second, mut millis) = (0, 0i64);
    if rest.get(at) == Some(&b':') {
        second = digits(rest.get(at + 1..at + 3)?)?;
        at += 3;
        if rest.get(at) == Some(&b'.') {
            let end = rest[at + 1..].iter().position(|c| !c.is_ascii_digit()).map_or(rest.len(), |n| at + 1 + n);
            let frac = &rest[at + 1..end];
            if frac.is_empty() {
                return None;
            }
            for i in 0..3 {
                millis = millis * 10 + frac.get(i).map_or(0, |c| i64::from(c - b'0'));
            }
            at = end;
        }
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let offset_min = match &rest[at..] {
        [] => return Some(Due::Local { year, month, day, hour, minute, second }),
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), z @ ..] => {
            let (h, m) = match z {
                [h1, h2] => (digits(&[*h1, *h2])?, 0),
                [h1, h2, m1, m2] => (digits(&[*h1, *h2])?, digits(&[*m1, *m2])?),
                [h1, h2, b':', m1, m2] => (digits(&[*h1, *h2])?, digits(&[*m1, *m2])?),
                _ => return None,
            };
            if h > 23 || m > 59 {
                return None;
            }
            let n = i64::from(h * 60 + m);
            if *sign == b'-' {
                -n
            } else {
                n
            }
        }
        _ => return None,
    };
    let secs = days_from_civil(year, month, day) * 86_400
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(second)
        - offset_min * 60;
    Some(Due::At(secs * 1000 + millis))
}

fn due_components(due: Due) -> Retained<NSDateComponents> {
    match due {
        Due::Local { year, month, day, hour, minute, second } => {
            let c = NSDateComponents::new();
            c.setYear(year as isize);
            c.setMonth(month as isize);
            c.setDay(day as isize);
            c.setHour(hour as isize);
            c.setMinute(minute as isize);
            c.setSecond(second as isize);
            c
        }
        Due::At(ms) => {
            let date = NSDate::dateWithTimeIntervalSince1970(ms.div_euclid(1000) as f64);
            NSCalendar::currentCalendar().components_fromDate(
                NSCalendarUnit::Year
                    | NSCalendarUnit::Month
                    | NSCalendarUnit::Day
                    | NSCalendarUnit::Hour
                    | NSCalendarUnit::Minute
                    | NSCalendarUnit::Second,
                &date,
            )
        }
    }
}

/// `POST /data/reminders/add` `{name, list?, due?}` → `{ok,name,list}`.
pub fn add(name: &str, list: &str, due: &str) -> Ek {
    let due = if due.is_empty() {
        None
    } else {
        match parse_due(due) {
            Some(d) => Some(d),
            None => return Ek::Done(json!({ "error": "bad due date" })),
        }
    };
    with_store(|store| {
        let rem = unsafe { EKReminder::reminderWithEventStore(store) };
        unsafe { rem.setTitle(Some(&NSString::from_str(name))) };
        // `iter` yields owned items, so nothing borrows the temporary array
        let named = if list.is_empty() {
            None
        } else {
            unsafe { store.calendarsForEntityType(EKEntityType::Reminder) }
                .iter()
                .find(|c| unsafe { c.title() }.to_string() == list)
        };
        let Some(calendar) = named.or_else(|| unsafe { store.defaultCalendarForNewReminders() }) else {
            return Ek::Failed("no reminders calendar".to_owned());
        };
        unsafe { rem.setCalendar(Some(&calendar)) };
        if let Some(due) = due {
            unsafe { rem.setDueDateComponents(Some(&due_components(due))) };
        }
        match unsafe { store.saveReminder_commit_error(&rem, true) } {
            Ok(()) => Ek::Done(json!({ "ok": true, "name": name, "list": unsafe { calendar.title() }.to_string() })),
            Err(e) => Ek::Failed(e.localizedDescription().to_string()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(h: u32, mi: u32, s: u32) -> Due {
        Due::Local { year: 2026, month: 11, day: 13, hour: h, minute: mi, second: s }
    }

    fn at(s: &str) -> i64 {
        match parse_due(s) {
            Some(Due::At(ms)) => ms,
            other => panic!("{s}: {other:?}"),
        }
    }

    #[test]
    fn due_accepts_what_widgets_send() {
        let ms = at("2026-11-13T09:00:00.000Z");
        assert_eq!(crate::js::iso(ms as f64), "2026-11-13T09:00:00.000Z");
        assert_eq!(at("2026-11-13T09:00:00Z"), ms);
        assert_eq!(at("2026-11-13T09:00:00.5Z"), ms + 500);
        assert_eq!(at("2026-11-13T17:00:00+08:00"), ms);
        assert_eq!(at("2026-11-13T04:30:00-04:30"), ms);
        assert_eq!(at("2026-11-13T17:00+0800"), ms);
        assert_eq!(parse_due("2026-11-13"), Some(local(0, 0, 0)));
        assert_eq!(parse_due("2026-11-13T09:00"), Some(local(9, 0, 0)));
        assert_eq!(parse_due("2026-11-13T09:00:30"), Some(local(9, 0, 30)));
        assert_eq!(parse_due("2026-11-13 09:00:00.250"), Some(local(9, 0, 0)));
        assert_eq!(parse_due(" 2026-11-13 "), Some(local(0, 0, 0)));
        assert_eq!(at("1969-12-31T23:59:59Z"), -1000);
        assert_eq!(at("2024-02-29T00:00:00Z"), 1_709_164_800_000);
    }

    #[test]
    fn due_rejects_the_rest() {
        for bad in [
            "", "tomorrow", "2026-13-01", "2026-02-29", "2026-11-31", "2026-11-13T24:00", "2026-11-13T09:60",
            "2026-11-13T09:00:00.", "2026-11-13T09:00:00Zx", "2026-11-13T09", "2026-11-13T09:00+25:00",
            "2026-11-13T09:00:00+8", "20261113", "2026-11-13T09:00:00.000+",
        ] {
            assert_eq!(parse_due(bad), None, "{bad:?}");
        }
    }

    fn r(name: &str, list: &str, due: Option<f64>, completed: bool) -> Row {
        Row { name: name.into(), list: list.into(), due, completed }
    }

    #[test]
    fn order_is_list_then_due_then_title_with_open_only() {
        let rows = vec![
            r("b", "Work", Some(2.0), false),
            r("done", "Home", Some(0.0), true),
            r("z", "Home", None, false),
            r("a", "Work", Some(2.0), false),
            r("early", "Work", Some(1.0), false),
            r("m", "Home", Some(5.0), false),
        ];
        let names: Vec<&str> = open_in_order(&rows).into_iter().map(|i| rows[i].name.as_str()).collect();
        assert_eq!(names, ["m", "z", "early", "a", "b"]);
    }

    #[test]
    fn list_cuts_at_fifty_after_sorting() {
        let rows: Vec<Row> = (0..80).rev().map(|i| r(&format!("t{i:02}"), "L", None, i % 10 == 9)).collect();
        let v = list_json(&rows);
        let got = v["reminders"].as_array().unwrap();
        assert_eq!(got.len(), 50);
        assert_eq!(got[0]["name"], "t00");
        assert_eq!(got[49]["name"], "t54");
        assert!(got.iter().all(|x| !x["name"].as_str().unwrap().ends_with('9')));
        assert_eq!(list_json(&[]), json!({ "reminders": [] }));
    }

    #[test]
    fn complete_picks_the_first_open_of_that_name_in_order() {
        let rows = vec![
            r("call", "Work", Some(9.0), false),
            r("call", "Home", Some(7.0), true),
            r("call", "Home", Some(8.0), false),
            r("other", "Home", None, false),
        ];
        assert_eq!(pick(&rows, "call"), Some(2));
        assert_eq!(pick(&rows, "nope"), None);
        let only_done = vec![r("x", "L", None, true)];
        assert_eq!(pick(&only_done, "x"), None);
    }

    #[test]
    fn a_row_maps_to_the_widgets_json() {
        assert_eq!(
            row_json(&r("Pay rent", "Home", Some(at("2026-11-13T09:00:00Z") as f64), false)),
            json!({ "name": "Pay rent", "list": "Home", "due": "2026-11-13T09:00:00.000Z", "flagged": false })
        );
        assert_eq!(
            row_json(&r("x", "", None, false)),
            json!({ "name": "x", "list": "", "due": null, "flagged": false })
        );
    }

    #[test]
    fn statuses_settle_or_ask() {
        assert_eq!(classify(EKAuthorizationStatus::FullAccess), Some(Access::Granted));
        assert!(matches!(classify(EKAuthorizationStatus::Denied), Some(Access::Fallback(_))));
        assert!(matches!(classify(EKAuthorizationStatus::Restricted), Some(Access::Fallback(_))));
        assert!(matches!(classify(EKAuthorizationStatus::WriteOnly), Some(Access::Fallback(_))));
        assert_eq!(classify(EKAuthorizationStatus::NotDetermined), None);
    }

    #[test]
    fn outcomes_map_to_the_local_tools_answer() {
        assert_eq!(Ek::Done(json!({"ok": true})).into_local(), Some(Ok(json!({"ok": true}))));
        assert_eq!(Ek::Failed("e".into()).into_local(), Some(Err("e".to_owned())));
        assert_eq!(Ek::Fallback("denied".into()).into_local(), None);
    }
}
