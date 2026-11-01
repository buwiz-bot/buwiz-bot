//! The cards drawn while a tool's input streams (agent.mjs's
//! `streamEvent` handler, `appPlaceholderFrom`, `dropPendingApp`): a
//! generate_app gets its placeholder at once, titled and sized as its
//! layout streams; an office_create's is shaped by its path's extension; a
//! revise_app covers the card it revises as soon as its name is known.

use serde_json::{json, Value};

use crate::hub::store::{uuidv7, Turn};
use crate::hub::tools::artifact::mark_revising;
use crate::hub::tools::office::office_art;
use crate::hub::tools::{activity, Run};

/// `"<k>"\s*:\s*"((?:[^"\\]|\\.)*)"`: the first string value of key `k`.
pub fn json_str(json: &str, k: &str) -> Option<String> {
    let lead = format!("\"{k}\"");
    let mut from = 0;
    while let Some(p) = json[from..].find(&lead) {
        let at = from + p + lead.len();
        from = from + p + 1;
        let rest = json[at..].trim_start_matches(crate::js::is_space);
        let Some(rest) = rest.strip_prefix(':') else { continue };
        let rest = rest.trim_start_matches(crate::js::is_space);
        let Some(body) = rest.strip_prefix('"') else { continue };
        let mut out = String::new();
        let mut cs = body.chars();
        loop {
            match cs.next() {
                None => break,
                Some('"') => return Some(out),
                Some('\\') => match cs.next() {
                    Some(c) if !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') => {
                        out.push('\\');
                        out.push(c);
                    }
                    _ => break,
                },
                Some(c) => out.push(c),
            }
        }
    }
    None
}

/// `"<k>"\s*:\s*(\d+)`: the first integer value of key `k` (0 for none).
pub fn json_num(json: &str, k: &str) -> f64 {
    let lead = format!("\"{k}\"");
    let mut from = 0;
    while let Some(p) = json[from..].find(&lead) {
        let at = from + p + lead.len();
        from = from + p + 1;
        let rest = json[at..].trim_start_matches(crate::js::is_space);
        let Some(rest) = rest.strip_prefix(':') else { continue };
        let rest = rest.trim_start_matches(crate::js::is_space);
        let n = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        if n > 0 {
            return rest[..n].parse().unwrap_or(0.0);
        }
    }
    0.0
}

/// `v.replace(/\\(.)/g, "$1")`.
fn unescape(v: &str) -> String {
    let mut out = String::new();
    let mut cs = v.chars().peekable();
    while let Some(c) = cs.next() {
        if c == '\\' {
            if let Some(n) = cs.peek().copied().filter(|n| !matches!(n, '\n' | '\r' | '\u{2028}' | '\u{2029}')) {
                out.push(n);
                cs.next();
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// `appPlaceholderFrom(turn, json)`: the title, description and size read
/// off the streamed start of a generate_app input, onto `art`.
pub fn app_placeholder_from(art: &mut Value, json: &str) {
    if let Some(t) = json_str(json, "title").map(|t| unescape(&t)).filter(|t| !t.is_empty()) {
        art["title"] = json!(crate::js::slice_utf16(&t, 60));
    }
    if let Some(d) = json_str(json, "desc").map(|d| unescape(&d)).filter(|d| !d.is_empty()) {
        art["desc"] = json!(crate::js::slice_utf16(&d, 140));
    }
    let (w, h) = (json_num(json, "width"), json_num(json, "height"));
    if w >= 60.0 && h >= 60.0 {
        art["w"] = json!(w.min(2048.0) as i64);
        art["h"] = json!(h.min(2048.0) as i64);
    }
}

/// What a streamed tool block is becoming.
enum Watch {
    App { index: u64, json: String, turn: String },
    Office { index: u64, json: String },
    Revise { index: u64, json: String },
}

/// The placeholders of one streamed model call.
#[derive(Default)]
pub struct Stream {
    app: Option<Watch>,
    office: Option<Watch>,
    revise: Option<Watch>,
}

fn touch_turn(run: &Run, id: &str, f: impl FnOnce(&mut Turn)) {
    run.ctx.store.touch(|s| {
        if let Some(u) = s.thread_mut(&run.turn.thread).and_then(|t| t.turns.iter_mut().find(|u| u.id.as_deref() == Some(id))) {
            f(u);
        }
    });
}

fn push_turn(run: &Run, turn: Turn) {
    run.ctx.store.touch(|s| {
        if let Some(t) = s.thread_mut(&run.turn.thread) {
            t.turns.push(turn);
        }
    });
}

impl Stream {
    /// One streamed event (`stream.on("streamEvent")`); `think` sets the
    /// job's thinking phrase.
    pub fn event(&mut self, run: &Run, event: &Value, label: &str, think: &dyn Fn()) {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let index = event.get("index").and_then(Value::as_u64);
        let delta = kind == "content_block_delta" && event.pointer("/delta/type").and_then(Value::as_str) == Some("input_json_delta");
        let piece = || event.pointer("/delta/partial_json").filter(|p| crate::js::truthy(p)).map(crate::js::string_of).unwrap_or_default();
        if delta {
            if let Some(Watch::App { index: i, json, turn }) = &mut self.app {
                if Some(*i) == index {
                    json.push_str(&piece());
                    if json.encode_utf16().count() < 4000 {
                        let json = json.clone();
                        touch_turn(run, turn, |u| {
                            let mut art = u.art.clone().unwrap_or(Value::Null);
                            app_placeholder_from(&mut art, &json);
                            u.art = Some(art);
                        });
                    }
                    return;
                }
            }
            if let Some(Watch::Office { index: i, json }) = &mut self.office {
                if Some(*i) == index {
                    json.push_str(&piece());
                    if let Some(path) = json_str(json, "path") {
                        let name = path.rsplit('/').next().unwrap_or("").to_owned();
                        if let Some(id) = run.state().pending_office.clone() {
                            touch_turn(run, &id, |u| u.art = Some(office_art(&name)));
                        }
                        self.office = None;
                    }
                    return;
                }
            }
            if let Some(Watch::Revise { index: i, json }) = &mut self.revise {
                if Some(*i) == index {
                    json.push_str(&piece());
                    if let Some(name) = revise_name(json) {
                        mark_revising(run, &name, None);
                        self.revise = None;
                    }
                    return;
                }
            }
        }
        if kind != "content_block_start" {
            return;
        }
        let block = event.get("content_block").cloned().unwrap_or(Value::Null);
        let btype = block.get("type").and_then(Value::as_str).unwrap_or("");
        let bname = block.get("name").and_then(Value::as_str).unwrap_or("");
        let idx = index.unwrap_or(u64::MAX);
        if btype == "tool_use" && bname == "generate_app" && run.state().pending_app.is_none() {
            let id = uuidv7();
            push_turn(
                run,
                Turn { role: "ai".into(), id: Some(id.clone()), building: true, label: Some(label.to_owned()), art: Some(json!({"title": null, "desc": "", "w": 0, "h": 0})), ..Default::default() },
            );
            run.state().pending_app = Some(id.clone());
            self.app = Some(Watch::App { index: idx, json: String::new(), turn: id });
        }
        if btype == "tool_use" && bname == "office_create" && run.state().pending_office.is_none() {
            let id = uuidv7();
            push_turn(run, Turn { role: "ai".into(), id: Some(id.clone()), building: true, label: Some("office".into()), art: Some(office_art("")), ..Default::default() });
            run.state().pending_office = Some(id);
            self.office = Some(Watch::Office { index: idx, json: String::new() });
        }
        if btype == "tool_use" && bname == "revise_app" {
            let revise = run.state().revise.clone();
            if let Some((name, v)) = revise {
                mark_revising(run, &name, v);
            }
            self.revise = Some(Watch::Revise { index: idx, json: String::new() });
        }
        match btype {
            "tool_use" => {
                let (p, d) = activity::tool_activity(&run.ctx, bname, block.get("input").unwrap_or(&json!({})));
                run.set_job(&p, &d);
            }
            "server_tool_use" => run.set_job("searching", "Digging around"),
            "text" => run.set_job("streaming", "Typing..."),
            "thinking" | "redacted_thinking" => think(),
            _ => {}
        }
    }
}

/// `/"name"\s*:\s*"(R\d+)"/`.
fn revise_name(json: &str) -> Option<String> {
    let lead = "\"name\"";
    let mut from = 0;
    while let Some(p) = json[from..].find(lead) {
        let at = from + p + lead.len();
        from = from + p + 1;
        let rest = json[at..].trim_start_matches(crate::js::is_space);
        let Some(rest) = rest.strip_prefix(':') else { continue };
        let rest = rest.trim_start_matches(crate::js::is_space);
        let Some(body) = rest.strip_prefix("\"R") else { continue };
        let n = body.find(|c: char| !c.is_ascii_digit()).unwrap_or(body.len());
        if n > 0 && body[n..].starts_with('"') {
            return Some(format!("R{}", &body[..n]));
        }
    }
    None
}

/// `dropPendingApp(job, thread)`: a placeholder no call adopted leaves.
pub fn drop_pending(run: &Run) {
    let ids: Vec<String> = {
        let mut st = run.state();
        [st.pending_app.take(), st.pending_office.take()].into_iter().flatten().collect()
    };
    if ids.is_empty() {
        return;
    }
    run.ctx.store.touch(|s| {
        if let Some(t) = s.thread_mut(&run.turn.thread) {
            t.turns.retain(|u| !u.id.as_ref().is_some_and(|i| ids.contains(i)));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_read_the_streamed_layout() {
        let mut art = json!({"title": null, "desc": "", "w": 0, "h": 0});
        app_placeholder_from(&mut art, r#"{"title":"Tiny \"Tip\"","desc":"Spl"#);
        assert_eq!(art, json!({"title": "Tiny \"Tip\"", "desc": "", "w": 0, "h": 0}));
        app_placeholder_from(&mut art, r#"{"title":"T","desc":"Splits","layout":{"width":320,"height":20"#);
        assert_eq!(art, json!({"title": "T", "desc": "Splits", "w": 0, "h": 0}));
        app_placeholder_from(&mut art, r#"{"layout":{"width":3200,"height":200}"#);
        assert_eq!((art["w"].clone(), art["h"].clone()), (json!(2048), json!(200)));
        assert_eq!(revise_name(r#"{"name" : "R12","code""#), Some("R12".into()));
        assert_eq!(revise_name(r#"{"name":"R1"#), None);
    }
}
