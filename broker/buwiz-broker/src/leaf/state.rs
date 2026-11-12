//! `/state`: one JSON blob per widget or Rox store, in `kv`
//! (server/src/routes/apps.mjs). Accepts `/state/<name>`, `/state?name=`,
//! and bodies of `{name, state}` or the bare state.

use std::time::{Duration, Instant};

use rusqlite::OptionalExtension;
use serde_json::Value;

use super::{Answer, Leaf};
use crate::js;

/// The name in the URL: the path after `/state/`, decoded, else `?name=`.
/// The path wins: an inline widget is granted only its own
/// `/state/<name>` path (the app's `widget_policy`), and a query must not
/// re-aim that grant at another key. `Err` where Node's
/// decodeURIComponent throws.
fn state_name(url: &str) -> Result<String, ()> {
    let path = js::pathname(url);
    let rest = path.strip_prefix("/state").map(|r| r.strip_prefix('/').unwrap_or(r)).unwrap_or(&path);
    let from_path = js::decode_uri_component(rest)?;
    if !from_path.is_empty() {
        return Ok(from_path);
    }
    Ok(js::search_param(url, "name").unwrap_or_default())
}

/// `/^[\w.-]+$/`
fn valid(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// The longest a `?wait=` holds a `/state` GET (Node's cap too).
const MAX_WAIT: Duration = Duration::from_secs(30);

/// `?wait=<ms>`: how long a GET may hold for a change, capped at
/// [`MAX_WAIT`]; absent or invalid means answer now.
fn wait(url: &str) -> Duration {
    js::search_param(url, "wait")
        .and_then(|w| w.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms).min(MAX_WAIT))
        .unwrap_or_default()
}

/// FNV-1a over the stored text: the `v` a widget echoes back as `?v=` so a
/// waiting GET answers at once when the state moved on since its last look.
fn version(data: Option<&str>) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data.unwrap_or("").as_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn read(leaf: &Leaf, name: &str) -> Result<Option<String>, String> {
    leaf.db(|db| db.query_row("SELECT CAST(data AS TEXT) FROM kv WHERE name=?", [name], |r| r.get::<_, Option<String>>(0)).optional())
        .map(|row| row.flatten())
}

pub fn get(leaf: &Leaf, url: &str) -> Answer {
    let Ok(name) = state_name(url) else {
        return Answer::bare(500, "{\"error\":\"URI malformed\"}");
    };
    if !valid(&name) {
        return Answer::json(200, "{\"state\":null}");
    }
    let mut data = match read(leaf, &name) {
        Ok(d) => d,
        Err(e) => return Answer::thrown(&e),
    };
    // `?wait=` turns the GET into a long-poll: hold until the stored text's
    // version leaves `?v=` (or, without `v`, any write lands), or `wait`
    // runs out — whichever first. A `v` that already differs answers now,
    // closing the gap between a surface's last read and this request.
    let hold = wait(url);
    if !hold.is_zero() {
        let baseline = js::search_param(url, "v").unwrap_or_else(|| version(data.as_deref()));
        if baseline == version(data.as_deref()) {
            // `wake` stays held from the re-read to the sleep, so a POST's
            // notify can't land between them unseen; a write wakes every
            // waiter and each re-checks its own name.
            let mut guard = leaf.wake.lock().unwrap_or_else(|e| e.into_inner());
            let deadline = Instant::now() + hold;
            data = match read(leaf, &name) {
                Ok(d) => d,
                Err(e) => return Answer::thrown(&e),
            };
            while version(data.as_deref()) == baseline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let (g, res) = leaf.cv.wait_timeout(guard, remaining).unwrap_or_else(|e| e.into_inner());
                guard = g;
                if res.timed_out() {
                    break;
                }
                match read(leaf, &name) {
                    Ok(d) => data = d,
                    Err(e) => return Answer::thrown(&e),
                }
            }
        }
    }
    let state = data
        .as_deref()
        .and_then(|d| serde_json::from_str::<Value>(d).ok())
        .unwrap_or(Value::Null);
    Answer::json(200, format!("{{\"state\":{},\"v\":\"{}\"}}", js::stringify(&state), version(data.as_deref())))
}

pub fn post(leaf: &Leaf, url: &str, body: &str) -> Answer {
    // errors are caught by the handler, which sets no content type on them
    let bad = |error: &str| Answer::refused(400, false, error);
    let parsed = match super::parse_body(body) {
        Ok(v) => v,
        Err(e) => return bad(&e),
    };
    let Ok(url_name) = state_name(url) else { return bad("URI malformed") };
    let name = if !url_name.is_empty() {
        Value::String(url_name)
    } else {
        match &parsed {
            Value::Null => return bad("Cannot read properties of null (reading 'name')"),
            Value::Object(m) => m.get("name").cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        }
    };
    let state = match &parsed {
        Value::Null => return bad("Cannot read properties of null (reading 'state')"),
        Value::Object(m) if m.contains_key("state") => m["state"].clone(),
        _ => parsed.clone(),
    };
    let shown = if js::truthy(&name) { js::string_of(&name) } else { String::new() };
    if !valid(&shown) {
        return bad("bad module name");
    }
    let data = if state.is_null() { "{}".to_owned() } else { js::stringify(&state) };
    // node:sqlite binds a string as text and a number as a real; nothing else
    let key: rusqlite::types::Value = match &name {
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        Value::Number(n) => rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0)),
        _ => return bad("Provided value cannot be bound to SQLite parameter 1."),
    };
    match leaf.db(|db| db.execute("INSERT OR REPLACE INTO kv(name,data) VALUES(?,?)", rusqlite::params![key, data])) {
        Ok(_) => {
            let _guard = leaf.wake.lock().unwrap_or_else(|e| e.into_inner());
            leaf.cv.notify_all();
            Answer::json(200, "{\"ok\":true}")
        }
        Err(e) => bad(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_comes_from_the_path_then_the_query() {
        assert_eq!(state_name("/state/rox-zoom"), Ok("rox-zoom".to_owned()));
        assert_eq!(state_name("/state?name=a&x=1"), Ok("a".to_owned()));
        assert_eq!(state_name("/state/?name=a"), Ok("a".to_owned()));
        assert_eq!(state_name("/state/b?name="), Ok("b".to_owned()));
        // a widget granted only /state/R28 cannot re-aim it with a query
        assert_eq!(state_name("/state/R28?name=rox-zoom"), Ok("R28".to_owned()));
        assert_eq!(state_name("/state"), Ok(String::new()));
        assert_eq!(state_name("/statefoo"), Ok("foo".to_owned()));
        assert_eq!(state_name("/state/leaf%2Dd"), Ok("leaf-d".to_owned()));
        assert_eq!(state_name("/state/.."), Ok("/".to_owned()));
        assert_eq!(state_name("/state/%E0%A4%A"), Err(()));
        assert!(valid("rox-main.bot_1"));
        assert!(!valid("no spaces") && !valid("") && !valid("é"));
    }

    /// A broker data dir no other test here has (they run in parallel).
    fn scratch_leaf() -> Leaf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("buwiz-leaf-state-{}-{}-{n}", std::process::id(), crate::hub::store::now_ms()));
        std::fs::create_dir_all(dir.join("generated")).unwrap();
        Leaf::new(dir, Vec::new())
    }

    fn body(a: &Answer) -> String {
        String::from_utf8_lossy(&a.body).into_owned()
    }

    /// The `v` a GET answered with.
    fn v_of(a: &Answer) -> String {
        let b = body(a);
        let i = b.find("\"v\":\"").unwrap() + 5;
        b[i..].split('"').next().unwrap().to_owned()
    }

    fn post(leaf: &Leaf, n: u64) {
        let a = super::post(leaf, "/state/w", &format!("{{\"n\":{n}}}"));
        assert_eq!(a.status, 200, "{}", body(&a));
    }

    #[test]
    fn a_wait_answers_at_once_when_v_is_stale() {
        let leaf = scratch_leaf();
        post(&leaf, 1);
        let v1 = v_of(&super::get(&leaf, "/state/w"));
        post(&leaf, 2);
        let t = std::time::Instant::now();
        let a = super::get(&leaf, &format!("/state/w?wait=30000&v={v1}"));
        assert!(t.elapsed() < Duration::from_secs(2), "stale v held the GET");
        assert!(body(&a).contains("\"n\":2"), "{}", body(&a));
        assert_ne!(v_of(&a), v1);
    }

    #[test]
    fn a_wait_holds_until_a_write_lands() {
        let leaf = scratch_leaf();
        post(&leaf, 1);
        let v1 = v_of(&super::get(&leaf, "/state/w"));
        let t = std::time::Instant::now();
        let mut held = None;
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(300));
                post(&leaf, 2);
            });
            held = Some(super::get(&leaf, &format!("/state/w?wait=10000&v={v1}")));
        });
        let a = held.unwrap();
        assert!(t.elapsed() < Duration::from_secs(5), "the write did not wake the GET");
        assert!(body(&a).contains("\"n\":2"), "{}", body(&a));
    }

    #[test]
    fn a_wait_runs_out_when_nothing_is_written() {
        let leaf = scratch_leaf();
        post(&leaf, 1);
        let v1 = v_of(&super::get(&leaf, "/state/w"));
        let t = std::time::Instant::now();
        let a = super::get(&leaf, &format!("/state/w?wait=400&v={v1}"));
        let e = t.elapsed();
        assert!(e >= Duration::from_millis(300) && e < Duration::from_secs(5), "{e:?}");
        assert_eq!(v_of(&a), v1);
    }
}
