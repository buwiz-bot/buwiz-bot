//! `/data/timeclock`: clock-in and clock-out rows in `timeclock`
//! (server/src/routes/data.mjs). Times are `toISOString()` strings; minutes
//! and "today" are counted as Node counts them, in local time.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::OptionalExtension;
use serde_json::{json, Map, Value};

use super::{Answer, Leaf};
use crate::js;

type Rows = Vec<(Option<String>, Option<String>)>;

fn rows(leaf: &Leaf) -> Result<Rows, String> {
    leaf.db(|db| {
        let mut stmt = db.prepare("SELECT clock_in AS cin, clock_out AS cout FROM timeclock ORDER BY id DESC LIMIT 25")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    })
}

pub fn get(leaf: &Leaf) -> Answer {
    match rows(leaf) {
        Ok(rows) => Answer::json(200, js::stringify(&Value::Object(summary(&rows, now_ms(), local_midnight_ms(now_ms()))))),
        Err(e) => Answer::thrown(&e),
    }
}

pub fn clock_in(leaf: &Leaf) -> Answer {
    let open = leaf.db(|db| db.query_row("SELECT id FROM timeclock WHERE clock_out IS NULL", [], |_| Ok(())).optional());
    let open = match open {
        Ok(open) => open.is_some(),
        Err(e) => return Answer::thrown(&e),
    };
    if !open {
        if let Err(e) = leaf.db(|db| db.execute("INSERT INTO timeclock(clock_in) VALUES(?)", [crate::js::iso(now_ms())])) {
            return Answer::thrown(&e);
        }
    }
    with_summary(leaf, json!({ "ok": true, "already": open }))
}

pub fn clock_out(leaf: &Leaf) -> Answer {
    if let Err(e) = leaf.db(|db| db.execute("UPDATE timeclock SET clock_out=? WHERE clock_out IS NULL", [crate::js::iso(now_ms())])) {
        return Answer::thrown(&e);
    }
    with_summary(leaf, json!({ "ok": true }))
}

/// `{...head, ...tcJson()}`
fn with_summary(leaf: &Leaf, head: Value) -> Answer {
    let rows = match rows(leaf) {
        Ok(rows) => rows,
        Err(e) => return Answer::thrown(&e),
    };
    let mut out = head.as_object().cloned().unwrap_or_default();
    out.extend(summary(&rows, now_ms(), local_midnight_ms(now_ms())));
    Answer::json(200, js::stringify(&Value::Object(out)))
}

/// `tcJson()`: in or out, since when, today's minutes, the last 10 sessions.
fn summary(rows: &Rows, now: f64, day_start: f64) -> Map<String, Value> {
    let ms = |s: &Option<String>| s.as_deref().map(parse_iso).unwrap_or(f64::NAN);
    let minutes = |cin: &Option<String>, cout: &Option<String>| {
        let end = match cout.as_deref() {
            Some(s) if !s.is_empty() => parse_iso(s),
            _ => now,
        };
        // Math.max(0, Math.round(x)): NaN stays NaN
        let m = ((end - ms(cin)) / 60000.0 + 0.5).floor();
        if m.is_nan() {
            m
        } else {
            m.max(0.0)
        }
    };
    let open = rows.iter().find(|(_, cout)| cout.as_deref().unwrap_or("").is_empty());
    let today: f64 = rows.iter().filter(|(cin, _)| ms(cin) >= day_start).map(|(cin, cout)| minutes(cin, cout)).sum();
    let sessions: Vec<Value> = rows
        .iter()
        .take(10)
        .map(|(cin, cout)| json!({ "in": cin, "out": cout, "min": number(minutes(cin, cout)) }))
        .collect();
    let mut m = Map::new();
    m.insert("status".into(), json!(if open.is_some() { "in" } else { "out" }));
    m.insert("since".into(), open.map(|(cin, _)| json!(cin)).unwrap_or(Value::Null));
    m.insert("todayMin".into(), number(today));
    m.insert("sessions".into(), Value::Array(sessions));
    m
}

fn number(f: f64) -> Value {
    serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null)
}

fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

/// Local midnight of the day `now` falls in (`setHours(0, 0, 0, 0)`).
fn local_midnight_ms(now: f64) -> f64 {
    let secs = (now / 1000.0).floor() as libc::time_t;
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return f64::NAN;
        }
        tm.tm_hour = 0;
        tm.tm_min = 0;
        tm.tm_sec = 0;
        tm.tm_isdst = -1;
        libc::mktime(&mut tm) as f64 * 1000.0
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `new Date(s).getTime()` for `toISOString()` output
/// (`YYYY-MM-DDTHH:MM:SS[.sss]Z`); NaN for anything else.
fn parse_iso(s: &str) -> f64 {
    let b = s.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> { s.get(from..from + len).filter(|t| t.bytes().all(|c| c.is_ascii_digit())).and_then(|t| t.parse().ok()) };
    let shape = b.len() >= 20 && b[4] == b'-' && b[7] == b'-' && b[10] == b'T' && b[13] == b':' && b[16] == b':' && b[b.len() - 1] == b'Z';
    let parts = (|| Some((num(0, 4)?, num(5, 2)?, num(8, 2)?, num(11, 2)?, num(14, 2)?, num(17, 2)?)))();
    let (Some((y, mo, d, h, mi, sec)), true) = (parts, shape) else { return f64::NAN };
    let millis = match &s[19..s.len() - 1] {
        "" => 0,
        frac if frac.starts_with('.') && frac.len() > 1 && frac[1..].bytes().all(|c| c.is_ascii_digit()) => {
            let digits: String = frac[1..].chars().chain("00".chars()).take(3).collect();
            digits.parse().unwrap_or(0)
        }
        _ => return f64::NAN,
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 24 || mi > 59 || sec > 59 {
        return f64::NAN;
    }
    ((days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec) * 1000 + millis) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_times_round_trip() {
        for s in ["1970-01-01T00:00:00.000Z", "2026-10-29T03:04:05.678Z", "2000-02-29T23:59:59.999Z", "1969-12-31T23:59:59.000Z"] {
            assert_eq!(crate::js::iso(parse_iso(s)), s);
        }
        assert_eq!(parse_iso("2026-10-29T03:04:05Z"), parse_iso("2026-10-29T03:04:05.000Z"));
        assert_eq!(parse_iso("2026-10-29T03:04:05.6Z"), parse_iso("2026-10-29T03:04:05.600Z"));
        assert!(parse_iso("yesterday").is_nan());
        assert_eq!(parse_iso("2026-10-29T00:00:00.000Z"), 1_793_232_000_000.0);
    }

    #[test]
    fn the_summary_counts_as_node_does() {
        let t = |s: &str| Some(s.to_owned());
        let now = parse_iso("2026-10-29T12:00:00.000Z");
        let day = parse_iso("2026-10-29T00:00:00.000Z");
        let rows: Rows = vec![
            (t("2026-10-29T11:00:00.000Z"), None),
            (t("2026-10-29T08:00:00.000Z"), t("2026-10-29T09:30:29.000Z")),
            (t("2026-10-28T08:00:00.000Z"), t("2026-10-28T09:00:00.000Z")),
            (t("2026-10-28T10:00:00.000Z"), t("2026-10-28T09:00:00.000Z")),
        ];
        let m = summary(&rows, now, day);
        assert_eq!(
            js::stringify(&Value::Object(m)),
            r#"{"status":"in","since":"2026-10-29T11:00:00.000Z","todayMin":150,"sessions":[{"in":"2026-10-29T11:00:00.000Z","out":null,"min":60},{"in":"2026-10-29T08:00:00.000Z","out":"2026-10-29T09:30:29.000Z","min":90},{"in":"2026-10-28T08:00:00.000Z","out":"2026-10-28T09:00:00.000Z","min":60},{"in":"2026-10-28T10:00:00.000Z","out":"2026-10-28T09:00:00.000Z","min":0}]}"#
        );
        let bad = summary(&vec![(t("junk"), None)], now, day);
        assert_eq!(js::stringify(&Value::Object(bad)), r#"{"status":"in","since":"junk","todayMin":0,"sessions":[{"in":"junk","out":null,"min":null}]}"#);
    }
}
