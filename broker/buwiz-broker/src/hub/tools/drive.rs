//! Google Drive files the AI made or changed (drive.mjs, the tool loop's
//! half): a successful google-workspace write leaves a link on the reply
//! (`{provider: "gdrive", id, name, mime, link, action, at, account?}`, one
//! per file, merged within the turn); names come from the write's own
//! result or input, else from what reads in the same reply showed, else
//! from an earlier turn's link of the same file. The Library's side
//! (`driveType`) is the store's (`store::json::drive_type`).

use serde_json::{json, Map, Value};

use super::{LinkTurn, Out, Run};
use crate::js;

pub const DRIVE_SERVER: &str = "google-workspace";

const DOC: &str = "application/vnd.google-apps.document";
const SHEET: &str = "application/vnd.google-apps.spreadsheet";
const SLIDES: &str = "application/vnd.google-apps.presentation";

/// workspace-mcp's write tools → (action, the mime they make).
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

fn id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// `[\w-]{10,}` at the start of `s`, and what follows it.
fn id_at(s: &str) -> Option<(&str, &str)> {
    let n = s.find(|c: char| !id_char(c)).unwrap_or(s.len());
    (n >= 10).then(|| (&s[..n], &s[n..]))
}

/// A regex `.`: not a line terminator.
fn dot(c: char) -> bool {
    !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// `<lead>(ID)<tail>`: the first id after `lead` that `tail` follows.
fn id_between(text: &str, lead: &str, tail: &str) -> String {
    let mut from = 0;
    while let Some(p) = text[from..].find(lead) {
        let at = from + p + lead.len();
        if let Some((id, rest)) = id_at(&text[at..]) {
            if rest.starts_with(tail) {
                return id.to_owned();
            }
        }
        from = from + p + 1;
    }
    String::new()
}

/// `<lead>(.*?)<tail>`: the shortest run on one line between them.
fn lazy_between(text: &str, lead: &str, tail: &str) -> String {
    let mut from = 0;
    while let Some(p) = text[from..].find(lead) {
        let at = from + p + lead.len();
        let line_end = text[at..].find(|c: char| !dot(c)).map(|n| at + n).unwrap_or(text.len());
        if let Some(q) = text[at..].find(tail).filter(|q| at + q <= line_end) {
            return text[at..at + q].to_owned();
        }
        from = from + p + 1;
    }
    String::new()
}

/// `<lead>(.+)`: the rest of the line, at least one character.
fn rest_of_line(text: &str, lead: &str) -> String {
    let mut from = 0;
    while let Some(p) = text[from..].find(lead) {
        let at = from + p + lead.len();
        let end = text[at..].find(|c: char| !dot(c)).map(|n| at + n).unwrap_or(text.len());
        if end > at {
            return text[at..end].to_owned();
        }
        from = from + p + 1;
    }
    String::new()
}

/// `<lead>(\S+)` (with `https?:` when `url`).
fn word_after(text: &str, lead: &str, url: bool) -> String {
    let mut from = 0;
    while let Some(p) = text[from..].find(lead) {
        let at = from + p + lead.len();
        let end = text[at..].find(char::is_whitespace).map(|n| at + n).unwrap_or(text.len());
        let w = &text[at..end];
        if !w.is_empty() && (!url || ((w.starts_with("http:") && w.len() > 5) || (w.starts_with("https:") && w.len() > 6))) {
            return w.to_owned();
        }
        from = from + p + 1;
    }
    String::new()
}

fn input_s(input: &Value, k: &str) -> String {
    input.get(k).filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default()
}

/// `driveWrite(tool, input, text)`: the file a successful write touched,
/// read off its result, or `None`.
pub fn drive_write(tool: &str, input: &Value, text: &str) -> Option<Map<String, Value>> {
    let (mut action, fixed) = write_of(tool)?;
    let head = text.trim_start_matches(char::is_whitespace);
    let error_word = head.len() >= 5 && head[..5].eq_ignore_ascii_case("error") && !head[5..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
    let lower = text.to_lowercase();
    if error_word || lower.contains("resumable upload session") || lower.contains("upload url (no authorization") {
        return None;
    }
    let (mut id, mut name, mut mime, mut link) = (String::new(), String::new(), fixed.to_owned(), String::new());
    let or = |a: String, b: String| if a.is_empty() { b } else { a };
    match tool {
        "create_drive_file" => {
            id = id_between(text, "(ID: ", ")");
            name = or(lazy_between(text, "created file '", "' (ID:"), input_s(input, "file_name"));
            mime = input_s(input, "mime_type");
            if mime.contains("folder") {
                return None;
            }
        }
        "import_to_google_doc" | "import_to_google_sheets" | "import_to_google_slides" => {
            id = ["Document ID: ", "Spreadsheet ID: ", "Presentation ID: ", "File ID: "]
                .iter()
                .filter_map(|lead| {
                    let got = id_between(text, lead, "");
                    (!got.is_empty()).then(|| (text.find(&format!("{lead}{got}")).unwrap_or(usize::MAX), got))
                })
                .min()
                .map(|(_, g)| g)
                .unwrap_or_default();
            name = or(lazy_between(text, "Successfully imported '", "' as"), input_s(input, "file_name"));
        }
        "copy_drive_file" => {
            id = id_between(text, "New file ID: ", "");
            name = rest_of_line(text, "New file name: ");
            mime = word_after(text, "File type: ", false);
            if mime == "Unknown" {
                mime.clear();
            }
            link = word_after(text, "View copied file: ", true);
        }
        "create_doc" => {
            id = id_between(text, "(ID: ", ")");
            name = or(lazy_between(text, "Created Google Doc '", "' (ID:"), input_s(input, "title"));
        }
        "create_spreadsheet" => {
            id = or(id_between(text, "ID: ", " |"), id_between(text, "ID: ", ""));
            name = or(lazy_between(text, "created spreadsheet '", "' for"), input_s(input, "title"));
            link = word_after(text, "URL: ", true);
        }
        "create_presentation" => {
            id = id_between(text, "Presentation ID: ", "");
            name = or(rest_of_line(text, "- Title: "), input_s(input, "title"));
            link = word_after(text, "URL: ", true);
        }
        "export_doc_to_pdf" => {
            // `saved to Drive as '.*?' \(ID: (ID)`
            let n = lazy_between(text, "saved to Drive as '", "' (ID:");
            if !text.contains("saved to Drive as '") {
                id.clear();
            } else {
                id = id_between(text, &format!("saved to Drive as '{n}' (ID: "), "");
            }
            name = n;
            link = word_after(text, "PDF: ", true);
        }
        "update_drive_file" => {
            id = or(id_between(text, "File ID: ", ""), input_s(input, "file_id"));
            name = or(rest_of_line(text, "Successfully updated file: "), input_s(input, "name"));
            mime = input_s(input, "mime_type");
            link = word_after(text, "View file: ", true);
            let changes: Vec<&str> = text.split('\n').filter(|l| l.trim_start_matches(char::is_whitespace).starts_with('•')).collect();
            let is_name = |l: &str| l.find('•').is_some_and(|i| l[i + '•'.len_utf8()..].trim_start_matches(char::is_whitespace).starts_with("Name:"));
            if !changes.is_empty() && changes.iter().all(|l| is_name(l)) {
                action = "renamed";
            }
            if text.contains("No changes were made") || changes.iter().any(|l| l.contains("moved to trash")) {
                return None;
            }
        }
        _ => {
            let raw = ["document_id", "spreadsheet_id", "presentation_id", "file_id"].iter().map(|k| input_s(input, k)).find(|v| !v.is_empty()).unwrap_or_default();
            id = or(id_between(&raw, "/d/", ""), raw);
        }
    }
    if link.is_empty() {
        let hit = ["https://docs.google.com/", "https://drive.google.com/"]
            .iter()
            .filter_map(|lead| {
                let p = text.find(lead)?;
                let end = text[p..].find(char::is_whitespace).map(|n| p + n).unwrap_or(text.len());
                (end > p + lead.len()).then_some((p, end))
            })
            .min();
        if let Some((p, end)) = hit {
            link = text[p..end].to_owned();
        }
    }
    let link = link.trim_end_matches([')', '.', ',']).to_owned();
    if id.is_empty() || !id.chars().all(id_char) || id.len() < 10 {
        return None;
    }
    let name = name.trim_matches(js::is_space);
    let name = name.strip_prefix(['\'', '"']).unwrap_or(name);
    let name = name.strip_suffix(['\'', '"']).unwrap_or(name);
    let link = if !link.is_empty() && link != "#" && !link.contains("No link available") && !link.contains("N/A") { link } else { String::new() };
    let mut m = Map::new();
    m.insert("provider".into(), json!("gdrive"));
    m.insert("id".into(), json!(id));
    m.insert("name".into(), json!(name));
    m.insert("mime".into(), json!(mime));
    m.insert("link".into(), json!(link));
    m.insert("action".into(), json!(action));
    Some(m)
}

/// `harvestFiles(text, known)`: what a result shows of files, by id.
pub fn harvest_files(text: &str, known: &mut Vec<(String, Value)>) {
    let set = |known: &mut Vec<(String, Value)>, id: &str, v: Value| match known.iter_mut().find(|(k, _)| k == id) {
        Some(x) => x.1 = v,
        None => known.push((id.to_owned(), v)),
    };
    for line in text.split('\n') {
        let mut hit = None;
        for (q, _) in line.match_indices('"') {
            let rest = &line[q + 1..];
            let Some(close) = rest.find('"') else { break };
            if close == 0 {
                continue;
            }
            let Some(after) = rest[close + 1..].strip_prefix(" (ID: ") else { continue };
            let Some((id, tail)) = id_at(after) else { continue };
            let mime = tail.strip_prefix(", Type: ").map(|t| t.split(|c: char| c == ',' || c == ')' || c.is_whitespace()).next().unwrap_or("")).filter(|m| !m.is_empty()).map(str::to_owned);
            hit = Some((rest[..close].to_owned(), id.to_owned(), mime));
            break;
        }
        let Some((name, id, mime)) = hit else { continue };
        let link = word_after(line, "Link: ", false);
        let link = if link.starts_with("http://") && link.len() > 7 || link.starts_with("https://") && link.len() > 8 { link } else { String::new() };
        let prev = known.iter().find(|(k, _)| *k == id).map(|(_, v)| v.clone());
        let old = |k: &str| prev.as_ref().and_then(|p| p.get(k)).map(js::string_of).unwrap_or_default();
        let mime = mime.unwrap_or_else(|| old("mime"));
        let link = if link.is_empty() { old("link") } else { link };
        set(known, &id, json!({"name": name, "mime": mime, "link": link}));
    }
    if let Some(p) = text.find("- Title: ") {
        let at = p + "- Title: ".len();
        let end = text[at..].find(|c: char| !dot(c)).map(|n| at + n).unwrap_or(text.len());
        if end > at {
            if let Some((id, _)) = text[end..].strip_prefix("\n- Presentation ID: ").and_then(id_at) {
                set(known, id, json!({"name": text[at..end].trim_matches(js::is_space), "mime": SLIDES, "link": ""}));
            }
        }
    }
}

/// `recordLink(turn, l)`: merged by file id; a created file stays created.
pub fn record_link(lt: &mut LinkTurn, l: Map<String, Value>) {
    let now = json!(crate::hub::store::now_ms());
    let cur = lt.links.iter_mut().find(|x| x.get("provider") == l.get("provider") && x.get("id") == l.get("id"));
    let Some(cur) = cur else {
        let mut l = l;
        let at = l.get("at").filter(|a| js::truthy(a)).cloned().unwrap_or(now);
        l.insert("at".into(), at);
        lt.links.push(Value::Object(l));
        return;
    };
    for k in ["name", "mime", "link", "account"] {
        if let Some(v) = l.get(k).filter(|v| js::truthy(v)) {
            cur[k] = v.clone();
        }
    }
    if cur.get("action").and_then(Value::as_str) != Some("created") {
        cur["action"] = l.get("action").cloned().unwrap_or(Value::Null);
    }
    cur["at"] = l.get("at").filter(|a| js::truthy(a)).cloned().unwrap_or(now);
}

fn link_for(id: &str, mime: &str) -> String {
    match mime {
        DOC => format!("https://docs.google.com/document/d/{id}/edit"),
        SHEET => format!("https://docs.google.com/spreadsheets/d/{id}/edit"),
        SLIDES => format!("https://docs.google.com/presentation/d/{id}/edit"),
        _ => format!("https://drive.google.com/file/d/{id}/view"),
    }
}

/// `noteDriveCall(job, thread, tool, input, result)`.
pub fn note_drive_call(run: &Run, tool: &str, input: &Value, result: &Out) {
    if result.is_error == Some(true) {
        return;
    }
    let text = if js::truthy(&result.content) { js::string_of(&result.content) } else { String::new() };
    let prior = |id: &str| -> Value {
        run.ctx.store.read(|s| {
            s.thread(&run.turn.thread)
                .and_then(|t| t.turns.iter().rev().find_map(|u| u.links.as_ref().and_then(Value::as_array).and_then(|ls| ls.iter().find(|l| l.get("id").and_then(Value::as_str) == Some(id)).cloned())))
                .unwrap_or(json!({}))
        })
    };
    let mut st = run.state();
    harvest_files(&text, &mut st.drive_seen);
    let Some(mut l) = drive_write(tool, input, &text) else { return };
    let id = js::string_of(&l["id"]);
    let seen = st.drive_seen.iter().find(|(k, _)| *k == id).map(|(_, v)| v.clone()).unwrap_or(json!({}));
    let prior = prior(&id);
    for k in ["name", "mime"] {
        if !js::truthy(&l[k]) {
            let v = [seen.get(k), prior.get(k)].into_iter().flatten().find(|v| js::truthy(v)).cloned().unwrap_or(json!(""));
            l.insert(k.into(), v);
        }
    }
    if !js::truthy(&l["link"]) {
        let v = [seen.get("link"), prior.get("link")].into_iter().flatten().find(|v| js::truthy(v)).cloned().unwrap_or_else(|| json!(link_for(&id, &js::string_of(&l["mime"]))));
        l.insert("link".into(), v);
    }
    if let Some(e) = input.get("user_google_email").filter(|e| js::truthy(e)) {
        l.insert("account".into(), json!(js::string_of(e)));
    } else if let Some(a) = prior.get("account").filter(|a| js::truthy(a)) {
        l.insert("account".into(), a.clone());
    }
    record_link(st.link_turn.get_or_insert_with(Default::default), l);
}

#[cfg(test)]
mod tests {
    use super::*;

    const X: &str = "1AbCdEfGhIjKlMnOp";

    #[test]
    fn drive_writes_read_each_tools_result() {
        let w = drive_write("create_drive_file", &json!({}), &format!("Successfully created file 'Q3.pdf' (ID: {X}) in folder root. Link: https://drive.google.com/file/d/{X}/view")).unwrap();
        assert_eq!(js::stringify(&Value::Object(w)), format!(r#"{{"provider":"gdrive","id":"{X}","name":"Q3.pdf","mime":"","link":"https://drive.google.com/file/d/{X}/view","action":"created"}}"#));
        assert!(drive_write("create_drive_file", &json!({"mime_type": "application/vnd.google-apps.folder"}), &format!("created file 'F' (ID: {X})")).is_none());
        assert!(drive_write("create_doc", &json!({}), "Error: nope").is_none());
        let w =
            drive_write("create_spreadsheet", &json!({}), &format!("Successfully created spreadsheet 'Budget' for a@b.co. ID: {X} | URL: https://docs.google.com/spreadsheets/d/{X}/edit | Sheets: 1"))
                .unwrap();
        assert_eq!((w["name"].as_str(), w["mime"].as_str(), w["link"].as_str()), (Some("Budget"), Some(SHEET), Some(format!("https://docs.google.com/spreadsheets/d/{X}/edit").as_str())));
        let w = drive_write("update_drive_file", &json!({}), &format!("✅ Successfully updated file: B\n   File ID: {X}\n  • Name: 'A' → 'B'\nView file: https://drive.google.com/x.")).unwrap();
        assert_eq!((w["action"].as_str(), w["link"].as_str()), (Some("renamed"), Some("https://drive.google.com/x")));
        let w = drive_write("modify_doc_text", &json!({"document_id": format!("https://docs.google.com/document/d/{X}/edit")}), "Done").unwrap();
        assert_eq!((w["id"].as_str(), w["mime"].as_str(), w["action"].as_str()), (Some(X), Some(DOC), Some("edited")));
        let mut known = Vec::new();
        harvest_files(&format!("- Name: \"Notes\" (ID: {X}, Type: text/plain, Size: 2) Link: https://drive.google.com/n\n- Title: Deck\n- Presentation ID: {X}x"), &mut known);
        assert_eq!(
            known,
            vec![(X.to_owned(), json!({"name": "Notes", "mime": "text/plain", "link": "https://drive.google.com/n"})), (format!("{X}x"), json!({"name": "Deck", "mime": SLIDES, "link": ""}))]
        );
    }
}
