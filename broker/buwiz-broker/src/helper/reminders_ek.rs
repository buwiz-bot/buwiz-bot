//! `/data/reminders` through EventKit. The osascript JXA path costs ~30 s
//! per call — Apple Events, one round-trip per property — so the widget's
//! poll and every tap lagged for seconds; EventKit answers in ms. Callers
//! fall back to the JXA when these return `Err` only where there is no
//! EventKit (never on macOS); a denied grant is an error body, as JXA's
//! is.

use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_event_kit::{EKAuthorizationStatus, EKEventStore, EKEntityType, EKReminder};
use objc2_foundation::{
    NSCalendar, NSDateComponents, NSError, NSISO8601DateFormatter, NSString,
};
use serde_json::{json, Value};

const WAIT: Duration = Duration::from_secs(15);

/// One reminder row, extracted inside the fetch block (the objects are
/// only safe to read there).
struct Row {
    name: String,
    list: String,
    due: Option<f64>,
    completed: bool,
}

fn store() -> Result<Retained<EKEventStore>, String> {
    Ok(unsafe { EKEventStore::new() })
}

/// Ask for Reminders access once (the system prompt), then remember the
/// answer in TCC. `Err` text becomes the route's `{error}` body.
fn granted(store: &EKEventStore) -> Result<(), String> {
    let status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Reminder) };
    match status {
        EKAuthorizationStatus::FullAccess => return Ok(()),
        EKAuthorizationStatus::Denied | EKAuthorizationStatus::Restricted => {
            return Err(
                "reminders access denied — grant it in System Settings → Privacy & Security → Reminders"
                    .to_owned(),
            )
        }
        _ => {}
    }
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |granted: Bool, _err: *mut NSError| {
        let _ = tx.send(granted.as_bool());
    });
    let ptr = &*block as *const block2::DynBlock<dyn Fn(Bool, *mut NSError)> as *mut _;
    unsafe { store.requestFullAccessToRemindersWithCompletion(ptr) };
    // the callback may still land after a timeout: never free the block
    std::mem::forget(block);
    match rx.recv_timeout(Duration::from_secs(300)) {
        Ok(true) => Ok(()),
        Ok(false) => Err(
            "reminders access denied — grant it in System Settings → Privacy & Security → Reminders"
                .to_owned(),
        ),
        Err(_) => Err("reminders access request timed out".to_owned()),
    }
}

fn due_ms(comps: &NSDateComponents) -> Option<f64> {
    let date = NSCalendar::currentCalendar().dateFromComponents(comps)?;
    Some(date.timeIntervalSince1970() * 1000.0)
}

fn row(rem: &EKReminder) -> Row {
    let name = unsafe { rem.title() }.to_string();
    let list = unsafe { rem.calendar() }
        .map(|c| unsafe { c.title() }.to_string())
        .unwrap_or_default();
    let due = unsafe { rem.dueDateComponents() }.as_deref().and_then(due_ms);
    Row { name, list, due, completed: unsafe { rem.isCompleted() } }
}

/// Every reminder as plain rows; the ObjC reads happen inside the block.
fn fetch(store: &EKEventStore) -> Result<Vec<Row>, String> {
    let pred = unsafe { store.predicateForRemindersInCalendars(None) };
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |ptr: *mut objc2_foundation::NSArray<EKReminder>| {
        let mut rows = Vec::new();
        if !ptr.is_null() {
            for rem in unsafe { &*ptr }.iter() {
                rows.push(row(&rem));
            }
        }
        let _ = tx.send(rows);
    });
    unsafe { store.fetchRemindersMatchingPredicate_completion(&pred, &block) };
    rx.recv_timeout(WAIT).map_err(|_| "reminders fetch timed out".to_owned())
}

/// `GET /data/reminders` → `{reminders: [{name,list,due,flagged}]}`.
pub fn list() -> Result<Value, String> {
    let store = store()?;
    granted(&store)?;
    let rows = fetch(&store)?;
    let reminders: Vec<Value> = rows
        .iter()
        .filter(|r| !r.completed)
        .take(50)
        .map(|r| {
            json!({
                "name": r.name,
                "list": r.list,
                "due": r.due.map(crate::js::iso),
                // EventKit does not expose Reminders' flagged bit
                "flagged": false,
            })
        })
        .collect();
    Ok(json!({ "reminders": reminders }))
}

/// `POST /data/reminders/complete` `{name}` → `{ok: done}`: the first open
/// reminder of that name, as the JXA's `whose({completed:false,name})`.
pub fn complete(name: &str) -> Result<Value, String> {
    let store = store()?;
    granted(&store)?;
    let pred = unsafe { store.predicateForRemindersInCalendars(None) };
    let (tx, rx) = mpsc::channel::<Result<bool, String>>();
    let wanted = name.to_owned();
    let store2 = store.clone();
    let block = RcBlock::new(move |ptr: *mut objc2_foundation::NSArray<EKReminder>| {
        let mut done = Ok(false);
        if !ptr.is_null() {
            'outer: for rem in unsafe { &*ptr }.iter() {
                if unsafe { rem.isCompleted() } || unsafe { rem.title() }.to_string() != wanted {
                    continue;
                }
                unsafe { rem.setCompleted(true) };
                done = match unsafe { store2.saveReminder_commit_error(&rem, true) } {
                    Ok(()) => Ok(true),
                    Err(e) => Err(e.localizedDescription().to_string()),
                };
                break 'outer;
            }
        }
        let _ = tx.send(done);
    });
    unsafe { store.fetchRemindersMatchingPredicate_completion(&pred, &block) };
    match rx.recv_timeout(WAIT).map_err(|_| "reminders fetch timed out".to_owned())? {
        Ok(done) => Ok(json!({ "ok": done })),
        Err(e) => Ok(json!({ "error": e })),
    }
}

/// `POST /data/reminders/add` `{name, list?, due?}` → `{ok,name,list}`.
pub fn add(name: &str, list: &str, due: &str) -> Result<Value, String> {
    let store = store()?;
    granted(&store)?;
    let rem = unsafe { EKReminder::reminderWithEventStore(&store) };
    let title = NSString::from_str(name);
    unsafe { rem.setTitle(Some(&title)) };
    let calendar = if list.is_empty() {
        None
    } else {
        unsafe { store.calendarsForEntityType(EKEntityType::Reminder) }
            .iter()
            .find(|c| unsafe { c.title() }.to_string() == list)
    };
    let calendar = calendar.or_else(|| unsafe { store.defaultCalendarForNewReminders() });
    let Some(calendar) = calendar else {
        return Err("no reminders calendar".to_owned());
    };
    unsafe { rem.setCalendar(Some(&calendar)) };
    if !due.is_empty() {
        let ns_due = NSString::from_str(due);
        let date = NSISO8601DateFormatter::new()
            .dateFromString(&ns_due)
            .ok_or_else(|| "bad due date".to_owned())?;
        let comps = NSCalendar::currentCalendar().components_fromDate(
            objc2_foundation::NSCalendarUnit::Year
                | objc2_foundation::NSCalendarUnit::Month
                | objc2_foundation::NSCalendarUnit::Day
                | objc2_foundation::NSCalendarUnit::Hour
                | objc2_foundation::NSCalendarUnit::Minute
                | objc2_foundation::NSCalendarUnit::Second,
            &date,
        );
        unsafe { rem.setDueDateComponents(Some(&comps)) };
    }
    unsafe { store.saveReminder_commit_error(&rem, true) }
        .map_err(|e| e.localizedDescription().to_string())?;
    Ok(json!({ "ok": true, "name": name, "list": unsafe { calendar.title() }.to_string() }))
}
