//! Google Drive files the AI made or changed, as Library artifacts
//! (drive.mjs). A successful google-workspace write leaves a link on the AI
//! turn that made it:
//!   turn.links = [{provider: "gdrive", id, name, mime, link, action, at, account?}]
//! one per file id, action "created" | "edited" | "renamed". `artifacts`
//! turns them into `gdrive:<id>` records.
//!
//! For the tool loop (5.4): after each google-workspace call, call
//! [`note_drive_call`] with the job's [`DriveNotes`]; when the reply's
//! turn is made, `DriveNotes::link_turn`'s `links` go on it.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex_lite::Regex;
use serde_json::{json, Map, Value};

pub use crate::hub::store::json::drive_type;
use crate::hub::store::Thread;
use crate::js;

pub const DRIVE_SERVER: &str = "google-workspace";

const DOC: &str = "application/vnd.google-apps.document";
const SHEET: &str = "application/vnd.google-apps.spreadsheet";
const SLIDES: &str = "application/vnd.google-apps.presentation";

/// workspace-mcp's write tools: what each does to a file, and the mime of
/// the file it makes when the tool fixes it.
fn write_of(tool: &str) -> Option<(&'static str, &'static str)> {
    Some(match tool {
        "create_drive_file" | "copy_drive_file" => ("created", ""),
        "import_to_google_doc" | "create_doc" => ("created", DOC),
        "import_to_google_sheets" | "create_spreadsheet" => ("created", SHEET),
        "import_to_google_slides" | "create_presentation" => ("created", SLIDES),
        "export_doc_to_pdf" => ("created", "application/pdf"),
        "update_drive_file" => ("edited", ""),
        "modify_doc_text"
        | "find_and_replace_doc"
        | "insert_doc_elements"
        | "insert_doc_image"
        | "update_paragraph_style"
        | "update_doc_headers_footers"
        | "batch_update_doc"
        | "create_table_with_data"
        | "manage_doc_tab" => ("edited", DOC),
        "modify_sheet_values"
        | "format_sheet_range"
        | "create_sheet"
        | "manage_sheet_tab"
        | "append_table_rows"
        | "resize_sheet_dimensions"
        | "move_sheet_rows"
        | "manage_conditional_formatting"
        | "manage_named_range" => ("edited", SHEET),
        "batch_update_presentation" => ("edited", SLIDES),
        _ => return None,
    })
}

/// `linkFor(id, mime)`: the file's link when a result carries none.
pub fn link_for(id: &str, mime: &str) -> String {
    match mime {
        DOC => format!("https://docs.google.com/document/d/{id}/edit"),
        SHEET => format!("https://docs.google.com/spreadsheets/d/{id}/edit"),
        SLIDES => format!("https://docs.google.com/presentation/d/{id}/edit"),
        _ => format!("https://drive.google.com/file/d/{id}/view"),
    }
}

const ID: &str = r"[\w-]{10,}";

fn re(src: &str) -> Regex {
    Regex::new(src).expect("a drive pattern")
}

macro_rules! rx {
    ($src:expr) => {{
        static R: OnceLock<Regex> = OnceLock::new();
        R.get_or_init(|| re(&$src))
    }};
}

/// `text.match(re)?.[1] || ""`.
fn pick(text: &str, r: &Regex) -> String {
    r.captures(text).and_then(|c| c.get(1)).map(|m| m.as_str().to_owned()).unwrap_or_default()
}

fn input_str(input: &Value, k: &str) -> String {
    input.get(k).filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default()
}

fn or(a: String, b: impl FnOnce() -> String) -> String {
    if a.is_empty() {
        b()
    } else {
        a
    }
}

/// `driveWrite(tool, input, text)`: the file a successful write touched,
/// read off its result text (`{provider, id, name, mime, link, action}`),
/// or `None` for a write that touched none.
pub fn drive_write(tool: &str, input: &Value, text: &str) -> Option<Value> {
    let (mut action, fixed) = write_of(tool)?;
    if rx!(r"(?i)^\s*Error\b").is_match(text) || rx!(r"(?i)Resumable upload session|Upload URL \(no Authorization").is_match(text) {
        return None;
    }
    let (mut name, mut mime, mut link) = (String::new(), fixed.to_owned(), String::new());
    let id: String;
    match tool {
        "create_drive_file" => {
            id = pick(text, rx!(format!(r"\(ID: ({ID})\)")));
            name = or(pick(text, rx!(r"created file '(.*?)' \(ID:")), || input_str(input, "file_name"));
            mime = input_str(input, "mime_type");
            if mime.contains("folder") {
                return None;
            }
        }
        "import_to_google_doc" | "import_to_google_sheets" | "import_to_google_slides" => {
            id = pick(text, rx!(format!(r"(?:Document|Spreadsheet|Presentation|File) ID: ({ID})")));
            name = or(pick(text, rx!(r"Successfully imported '(.*?)' as")), || input_str(input, "file_name"));
        }
        "copy_drive_file" => {
            id = pick(text, rx!(format!(r"New file ID: ({ID})")));
            name = pick(text, rx!(r"New file name: (.+)"));
            mime = pick(text, rx!(r"File type: (\S+)"));
            if mime == "Unknown" {
                mime = String::new();
            }
            link = pick(text, rx!(r"View copied file: (https?:\S+)"));
        }
        "create_doc" => {
            id = pick(text, rx!(format!(r"\(ID: ({ID})\)")));
            name = or(pick(text, rx!(r"Created Google Doc '(.*?)' \(ID:")), || input_str(input, "title"));
        }
        "create_spreadsheet" => {
            id = or(pick(text, rx!(format!(r"ID: ({ID}) \|"))), || pick(text, rx!(format!(r"ID: ({ID})"))));
            name = or(pick(text, rx!(r"created spreadsheet '(.*?)' for")), || input_str(input, "title"));
            link = pick(text, rx!(r"URL: (https?:\S+)"));
        }
        "create_presentation" => {
            id = pick(text, rx!(format!(r"Presentation ID: ({ID})")));
            name = or(pick(text, rx!(r"- Title: (.+)")), || input_str(input, "title"));
            link = pick(text, rx!(r"URL: (https?:\S+)"));
        }
        "export_doc_to_pdf" => {
            id = pick(text, rx!(format!(r"saved to Drive as '.*?' \(ID: ({ID})")));
            name = pick(text, rx!(r"saved to Drive as '(.*?)' \(ID:"));
            link = pick(text, rx!(r"PDF: (https?:\S+)"));
        }
        "update_drive_file" => {
            id = or(pick(text, rx!(format!(r"File ID: ({ID})"))), || input_str(input, "file_id"));
            name = or(pick(text, rx!(r"Successfully updated file: (.+)")), || input_str(input, "name"));
            mime = input_str(input, "mime_type");
            link = pick(text, rx!(r"View file: (https?:\S+)"));
            let changes: Vec<&str> = text.split('\n').filter(|l| rx!(r"^\s*•").is_match(l)).collect();
            if !changes.is_empty() && changes.iter().all(|l| rx!(r"•\s*Name:").is_match(l)) {
                action = "renamed";
            }
            if text.contains("No changes were made") {
                return None;
            }
            if changes.iter().any(|l| l.contains("moved to trash")) {
                return None;
            }
        }
        _ => {
            let raw = ["document_id", "spreadsheet_id", "presentation_id", "file_id"].iter().map(|k| input_str(input, k)).find(|s| !s.is_empty()).unwrap_or_default();
            id = rx!(format!(r"/d/({ID})")).captures(&raw).and_then(|c| c.get(1)).map(|m| m.as_str().to_owned()).unwrap_or(raw);
        }
    }
    if link.is_empty() {
        link = pick(text, rx!(r"(https://(?:docs|drive)\.google\.com/\S+)"));
    }
    let link = link.trim_end_matches([')', '.', ',']).to_owned();
    if id.is_empty() || !rx!(format!(r"^{ID}$")).is_match(&id) {
        return None;
    }
    let name = js::slice_utf16(&name, usize::MAX);
    let name = name.trim_matches(|c: char| js::is_space(c) || matches!(c, '\n' | '\r'));
    let name = name.strip_prefix(['\'', '"']).unwrap_or(name);
    let name = name.strip_suffix(['\'', '"']).unwrap_or(name);
    let link = if !link.is_empty() && link != "#" && !rx!(r"No link available|N/A").is_match(&link) { link } else { String::new() };
    Some(json!({"provider": "gdrive", "id": id, "name": name, "mime": mime, "link": link, "action": action}))
}

/// What a google-workspace result shows of files (`harvestFiles`): id →
/// `{name, mime, link}`, added to `known`.
pub fn harvest_files(text: &str, known: &mut HashMap<String, Value>) {
    for line in text.split('\n') {
        let Some(m) = rx!(format!(r#""([^"]+)" \(ID: ({ID})(?:, Type: ([^,)\s]+))?"#)).captures(line) else { continue };
        let link = pick(line, rx!(r"Link: (https?://\S+)"));
        let id = m[2].to_owned();
        let prev = known.get(&id).cloned();
        let prev_of = |k: &str| prev.as_ref().and_then(|p| p.get(k)).filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
        let mime = m.get(3).map(|x| x.as_str().to_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| prev_of("mime"));
        let link = if link.is_empty() { prev_of("link") } else { link };
        known.insert(id, json!({"name": &m[1], "mime": mime, "link": link}));
    }
    if let Some(p) = rx!(format!(r"- Title: (.+)\n- Presentation ID: ({ID})")).captures(text) {
        known.insert(p[2].to_owned(), json!({"name": p[1].trim(), "mime": SLIDES, "link": ""}));
    }
}

/// `recordLink(turn, l)`: merges link `l` into `links` by file id (the
/// newest name, mime, link and account win where known; a file the turn
/// created stays "created").
pub fn record_link(links: &mut Vec<Value>, l: &Value, now: f64) {
    let at = l.get("at").filter(|v| js::truthy(v)).cloned().unwrap_or_else(|| super::appmeta::num(now));
    let same = |x: &Value| x.get("provider") == l.get("provider") && x.get("id") == l.get("id");
    let Some(cur) = links.iter_mut().find(|x| same(x)).and_then(Value::as_object_mut) else {
        let mut m = l.as_object().cloned().unwrap_or_default();
        m.insert("at".into(), at);
        links.push(Value::Object(m));
        return;
    };
    for k in ["name", "mime", "link", "account"] {
        if let Some(v) = l.get(k).filter(|v| js::truthy(v)) {
            cur.insert(k.into(), v.clone());
        }
    }
    if cur.get("action").and_then(Value::as_str) != Some("created") {
        cur.insert("action".into(), l.get("action").cloned().unwrap_or(Value::Null));
    }
    cur.insert("at".into(), at);
}

/// `priorLink(thread, id)`: what an earlier turn knows of file `id`.
fn prior_link(thread: Option<&Thread>, id: &str) -> Option<Value> {
    thread?.turns.iter().rev().find_map(|t| t.links.as_ref().and_then(Value::as_array).and_then(|ls| ls.iter().find(|x| x.get("id").and_then(Value::as_str) == Some(id)).cloned()))
}

/// A job's Drive bookkeeping (Node's `job.driveSeen` and `job.linkTurn`).
#[derive(Clone, Debug, Default)]
pub struct DriveNotes {
    pub seen: HashMap<String, Value>,
    /// the links the reply's turn gets (`job.linkTurn.links`), `None` until one is made
    pub link_turn: Option<Vec<Value>>,
}

/// `noteDriveCall(job, thread, tool, input, result)`: a read's files are
/// noted; a successful write's file is recorded for the reply.
pub fn note_drive_call(notes: &mut DriveNotes, thread: Option<&Thread>, tool: &str, input: &Value, content: &str, is_error: bool, now: f64) {
    if is_error {
        return;
    }
    harvest_files(content, &mut notes.seen);
    let Some(mut l) = drive_write(tool, input, content) else { return };
    let id = l["id"].as_str().unwrap_or_default().to_owned();
    let seen = notes.seen.get(&id).cloned().unwrap_or_else(|| json!({}));
    let prior = prior_link(thread, &id).unwrap_or_else(|| json!({}));
    let s = |v: &Value, k: &str| v.get(k).filter(|x| js::truthy(x)).map(js::string_of).unwrap_or_default();
    for k in ["name", "mime"] {
        if s(&l, k).is_empty() {
            l[k] = json!(or(s(&seen, k), || s(&prior, k)));
        }
    }
    if s(&l, "link").is_empty() {
        let mime = s(&l, "mime");
        l["link"] = json!(or(or(s(&seen, "link"), || s(&prior, "link")), || link_for(&id, &mime)));
    }
    match input.get("user_google_email").filter(|v| js::truthy(v)) {
        Some(a) => l["account"] = json!(js::string_of(a)),
        None => {
            if let Some(a) = prior.get("account").filter(|v| js::truthy(v)) {
                l["account"] = a.clone();
            }
        }
    }
    record_link(notes.link_turn.get_or_insert_with(Vec::new), &l, now);
}

/// A link object as `Map`.
pub fn as_map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_writes_read_off_results() {
        let text = "Successfully updated file: Plan v2\nFile ID: 1AbCdEfGhIjK\n• Name: Plan v2\nView file: https://docs.google.com/document/d/1AbCdEfGhIjK/edit?renamed=1";
        let l = drive_write("update_drive_file", &json!({"file_id": "1AbCdEfGhIjK", "name": "Plan v2"}), text).unwrap();
        assert_eq!(
            js::stringify(&l),
            r#"{"provider":"gdrive","id":"1AbCdEfGhIjK","name":"Plan v2","mime":"","link":"https://docs.google.com/document/d/1AbCdEfGhIjK/edit?renamed=1","action":"renamed"}"#
        );
        assert!(drive_write("update_drive_file", &json!({}), "Error: nope").is_none());
        assert!(drive_write("search_drive_files", &json!({}), "x").is_none());
        let made = drive_write("create_doc", &json!({"title": "T"}), "Created Google Doc 'Notes' (ID: 1234567890ab) for x. Link: https://docs.google.com/document/d/1234567890ab/edit.").unwrap();
        assert_eq!((made["name"].as_str(), made["mime"].as_str(), made["link"].as_str()), (Some("Notes"), Some(DOC), Some("https://docs.google.com/document/d/1234567890ab/edit")));
        let edit = drive_write("modify_doc_text", &json!({"document_id": "https://docs.google.com/document/d/abcdefghijkl/edit"}), "done").unwrap();
        assert_eq!(edit["id"], "abcdefghijkl");
    }

    #[test]
    fn drive_notes_merge_what_reads_and_earlier_turns_knew() {
        let mut notes = DriveNotes::default();
        note_drive_call(
            &mut notes,
            None,
            "search_drive_files",
            &json!({}),
            "- Name: \"Budget\" (ID: abcdefghijkl, Type: application/vnd.google-apps.spreadsheet) Link: https://x.example/b",
            false,
            1.0,
        );
        note_drive_call(&mut notes, None, "modify_sheet_values", &json!({"spreadsheet_id": "abcdefghijkl", "user_google_email": "a@b.c"}), "ok", false, 2.0);
        let links = notes.link_turn.unwrap();
        assert_eq!(
            js::stringify(&links[0]),
            r#"{"provider":"gdrive","id":"abcdefghijkl","name":"Budget","mime":"application/vnd.google-apps.spreadsheet","link":"https://x.example/b","action":"edited","account":"a@b.c","at":2}"#
        );
    }
}
