//! The package side of a Word document: its pictures, its lists, a legacy
//! `.doc` brought in through office_oxide, and the save.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;

use serde_json::{Value, json};

/// The package's parts (name, bytes).
fn parts(source: &[u8]) -> Vec<(String, Vec<u8>)> {
    ooxml_opc::unzip_parts(source).unwrap_or_default()
}

/// `(Id, Target)` of every relationship in a `.rels` part.
fn relationships(xml: &str) -> Vec<(String, String)> {
    let attr = |tag: &str, name: &str| -> Option<String> {
        let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        Some(tag[at..].split('"').next()?.to_owned())
    };
    xml.split("<Relationship").skip(1).filter_map(|tag| Some((attr(tag, "Id")?, attr(tag, "Target")?))).collect()
}

/// The parts pictures can come from: media and the relationships of the
/// story parts.
pub(crate) fn media(source: &[u8]) -> Vec<(String, Vec<u8>)> {
    parts(source).into_iter().filter(|(n, _)| n.starts_with("word/")).filter(|(n, _)| n.ends_with(".rels") || !n.ends_with(".xml")).collect()
}

/// A part's path from a relationship target in `word/`.
fn resolve(target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_owned();
    }
    let mut out: Vec<&str> = vec!["word"];
    for seg in target.split('/') {
        match seg {
            ".." => {
                out.pop();
            }
            "." | "" => {}
            s => out.push(s),
        }
    }
    out.join("/")
}

/// The bytes of the picture `rel_id` names in `part` (`None`: the body; a
/// header's or footer's relationship id; `notes:footnote`), or a `data:` URL's.
pub(crate) fn picture(_source: &[u8], media: &[(String, Vec<u8>)], part: Option<&str>, rel_id: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    if let Some(data) = rel_id.strip_prefix("data:") {
        let (_, b64) = data.split_once(";base64,")?;
        return base64::engine::general_purpose::STANDARD.decode(b64).ok();
    }
    let get = |name: &str| media.iter().find(|(n, _)| n == name).map(|(_, b)| b);
    let doc_rels = relationships(&String::from_utf8_lossy(get("word/_rels/document.xml.rels")?));
    let rels_of = |part: &str| {
        let name = part.rsplit('/').next().unwrap_or(part);
        get(&format!("word/_rels/{name}.rels")).map(|b| relationships(&String::from_utf8_lossy(b))).unwrap_or_default()
    };
    let rels = match part {
        None => doc_rels,
        Some(p) if p.starts_with("notes:") => rels_of(if p.ends_with("endnote") { "endnotes.xml" } else { "footnotes.xml" }),
        Some(rid) => {
            let target = doc_rels.iter().find(|(id, _)| id == rid)?.1.clone();
            rels_of(&target)
        }
    };
    let target = rels.iter().find(|(id, _)| id == rel_id)?.1.clone();
    get(&resolve(&target)).cloned()
}

/// A legacy `.doc` as a `.docx` (office_oxide reads its text, tables and
/// headings; character formats beyond those are lost).
pub(crate) fn doc_to_docx(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let doc = office_oxide::Document::from_reader(Cursor::new(bytes.to_vec()), office_oxide::DocumentFormat::Doc).map_err(|e| format!("Couldn't read the .doc file: {e}"))?;
    let ir = doc.to_ir();
    let mut out = Cursor::new(Vec::new());
    office_oxide::create::create_from_ir_to_writer(&ir, office_oxide::DocumentFormat::Docx, &mut out).map_err(|e| format!("Couldn't convert the .doc file: {e}"))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relationship_targets_resolve() {
        let rels = relationships(r#"<Relationships><Relationship Id="rId1" Type="x" Target="media/image1.png"/><Relationship Target="../a.png" Id="rId2"/></Relationships>"#);
        assert_eq!(rels, vec![("rId1".to_owned(), "media/image1.png".to_owned()), ("rId2".to_owned(), "../a.png".to_owned())]);
        assert_eq!(resolve("media/image1.png"), "word/media/image1.png");
        assert_eq!(resolve("../customXml/a.png"), "customXml/a.png");
    }

    #[test]
    fn a_body_picture_is_found() {
        let bytes = super::super::fixture::document(1);
        let m = media(&bytes);
        assert_eq!(picture(&bytes, &m, None, "rIdImage"), Some(super::super::fixture::png()));
        assert_eq!(picture(&bytes, &m, None, "rIdNope"), None);
    }
}

/// The `numId` of a list whose first level is bullets (or numbers, when
/// `numbered`), from the package's numbering part; 0 when it has none.
pub(crate) fn list_num(source: &[u8], numbered: bool) -> u32 {
    let parts = parts(source);
    let Some((_, xml)) = parts.iter().find(|(n, _)| n == "word/numbering.xml") else { return 0 };
    let xml = String::from_utf8_lossy(xml);
    let attr = |tag: &str, name: &str| -> Option<String> {
        let at = tag.find(&format!("{name}=\""))? + name.len() + 2;
        Some(tag[at..].split('"').next()?.to_owned())
    };
    // abstract lists by their first level's format
    let mut kinds = std::collections::HashMap::new();
    for a in xml.split("<w:abstractNum ").skip(1) {
        let Some(id) = attr(a, "w:abstractNumId") else { continue };
        let lvl0 = a.split("<w:lvl ").nth(1).unwrap_or("");
        let fmt = lvl0.split("<w:numFmt ").nth(1).and_then(|t| attr(t, "w:val")).unwrap_or_default();
        kinds.insert(id, fmt);
    }
    for n in xml.split("<w:num ").skip(1) {
        let (Some(num), Some(abs)) = (attr(n, "w:numId"), n.split("<w:abstractNumId ").nth(1).and_then(|t| attr(t, "w:val"))) else { continue };
        let fmt = kinds.get(&abs).map(String::as_str).unwrap_or("");
        if (fmt == "bullet") != numbered && !fmt.is_empty() && fmt != "none" {
            return num.parse().unwrap_or(0);
        }
    }
    0
}

/// A paragraph's list attribute: list `num` at level `lvl`.
pub(crate) fn num_pr(num: u32, lvl: u32) -> yrs::Any {
    let mut m = std::collections::HashMap::new();
    m.insert("numId".to_owned(), yrs::Any::Number(num as f64));
    m.insert("ilvl".to_owned(), yrs::Any::Number(lvl as f64));
    yrs::Any::Map(std::sync::Arc::new(m))
}

/// The list a paragraph's list attribute names.
pub(crate) fn num_id(v: &yrs::Any) -> Option<u32> {
    let yrs::Any::Map(m) = v else { return None };
    match m.get("numId")? {
        yrs::Any::Number(n) => Some(*n as u32),
        yrs::Any::BigInt(n) => Some(*n as u32),
        yrs::Any::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// What the web editor keeps outside the engine and saves with it (its
/// document state): whether a save wrote every part whole, which every
/// later one then does, and the watermark the layout draws.
#[derive(Debug, Default)]
pub(crate) struct State {
    pub whole: bool,
    /// The watermark set in the editor: `Some(None)` when it was removed.
    pub watermark: Option<Option<Value>>,
}

/// Now, as the save stamps it (UTC, milliseconds).
pub(crate) fn now() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let (secs, ms) = (t.as_secs() as i64, t.subsec_millis());
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil from days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// The document as edited, as `.docx` bytes: the web editor's save
/// (`writeSessionSave` over `yrsToDocument`, `@betteroffice/docx` 0.4.3).
///
/// The package is parsed again and the engine's stories projected onto it
/// ([`super::project`]), with what the editor keeps beside the stories: the
/// final section's properties and the headers' metadata (the engine's part
/// metadata and section margins), the watermark, and the comments. Its
/// writer then keeps every unchanged part and paragraph as its source
/// bytes; once the document's parts or the comments' ids changed, the web
/// writes every part whole, from then on.
pub(crate) fn docx(source: &[u8], doc: &docx_edit::EditingDoc, state: &mut State, now: &str) -> Result<Vec<u8>, String> {
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(source));
    let options = docx_parse::S9ParseOptions { source_ordinals: true, determinism_seed: Some(digest.clone()), ..Default::default() };
    let envelope = docx_parse::parse_docx_s9_wire(source, options).map_err(|e| e.to_string())?;
    let original = serde_json::to_value(&envelope.document.package).map_err(|e| e.to_string())?;
    let before = Keys::of(&original);
    let mut package = original.clone();
    document_state(doc, &mut package, state);
    let (comments, session_ids) = comments(doc, &original);
    package["document"]["comments"] = Value::Array(comments.clone());
    // captured before the projection, as the web does
    let identities = doc.paragraph_identities();
    let plan = doc.paragraph_save_plan();
    let mut projected = super::project::Projector::new(doc, &package, &session_ids).package(&package)?;
    let replies = reply_marks(&mut projected["document"]["content"], &comments);
    let after = Keys::of(&projected);
    let comments_changed = after.comments != before.comments;
    let whole = state.whole || after.metadata != before.metadata || before.ids.iter().any(|i| !after.ids.contains(i));
    state.whole = whole;
    let body_part = identities.paragraphs.iter().find_map(|p| match (&p.paragraph, &p.source) {
        (docx_edit::ParagraphRef::Session { story, .. }, Some(s)) if story == "body" => Some(s.part_uri.trim_start_matches('/').to_owned()),
        _ => None,
    });
    let patched_kept = |part: &str| !whole && (!comments_changed || part != "word/comments.xml") && (!replies || body_part.as_deref().is_some_and(|b| b != part));
    let spliced: Vec<&docx_edit::SplicedPart> = if whole { Vec::new() } else { plan.spliced_parts.iter().collect() };
    drop_synthetic(&mut projected, &identities);
    set_ordinals(&mut projected, &spliced);
    let mut keys = Vec::new();
    super::project::take_session_keys(&mut projected, &mut keys);
    if let Some(d) = projected["document"].as_object_mut() {
        // the writer reads sections from their paragraphs
        d.remove("sections");
    }
    use docx_parse::serializer::paragraph_ids::{S13ParagraphId, S13ParagraphIds, S13PatchedPart, S13SpliceAnchor, S13SplicedPart};
    let ids = S13ParagraphIds {
        section_edits: None,
        assignments: plan.assignments.iter().map(|(part, ordinal, para_id)| S13ParagraphId { part: part.clone(), ordinal: *ordinal, para_id: para_id.clone() }).collect(),
        patched_parts: plan.patched_parts.iter().filter(|(part, _)| patched_kept(part)).map(|(part, para_ids)| S13PatchedPart { part: part.clone(), para_ids: para_ids.clone() }).collect(),
        spliced_parts: spliced
            .iter()
            .map(|p| S13SplicedPart {
                part: p.part.clone(),
                sha256: p.sha256.clone(),
                paragraphs: p.paragraphs.iter().map(|(o, _)| *o).collect(),
                changed: p.changed.clone(),
                inserted: p
                    .inserted
                    .iter()
                    .map(|a| match a {
                        docx_edit::SpliceAnchor::Before(o) => S13SpliceAnchor::Before(*o),
                        docx_edit::SpliceAnchor::After(o) => S13SpliceAnchor::After(*o),
                    })
                    .collect(),
                removed: p.removed.clone(),
                paragraph_patches: p.paragraph_patches.clone(),
                legacy_paragraphs: p.legacy_paragraphs.clone(),
                table_patches: p.table_patches.clone(),
            })
            .collect(),
    };
    let request = json!({
        "determinism": { "seed": digest, "now": now },
        "document": projected["document"],
        "headerEntries": projected["headerEntries"],
        "footerEntries": projected["footerEntries"],
        "footnotes": projected["footnotes"],
        "endnotes": projected["endnotes"],
        "footnoteSeparators": projected["footnoteSeparators"],
        "endnoteSeparators": projected["endnoteSeparators"],
        "relationshipEntries": projected["relationshipEntries"],
        "options": { "updateModifiedDate": true },
    });
    let mut request = request;
    if let Some(o) = request.as_object_mut() {
        o.retain(|_, v| !v.is_null());
        if !projected["numbering"].is_null() {
            o.insert("numbering".into(), projected["numbering"].clone());
        }
    }
    let mut request: docx_parse::serializer::S13SaveRequest = serde_json::from_value(request).map_err(|e| format!("save request: {e}"))?;
    request.paragraph_ids = Some(ids);
    let bytes = docx_parse::serializer::write_docx_s13(request, source).map_err(|e| e.to_string())?;
    record_saved(doc, &identities, &plan, &keys, &bytes);
    Ok(bytes)
}

/// `editorSaveKeys`: what tells a save whether the document's parts or its
/// comments changed.
struct Keys {
    metadata: String,
    comments: String,
    ids: Vec<String>,
}

impl Keys {
    fn of(package: &Value) -> Keys {
        use super::project::canon;
        let without = |v: &Value, keys: &[&str]| {
            let mut v = v.clone();
            if let Some(o) = v.as_object_mut() {
                for k in keys {
                    o.remove(*k);
                }
            }
            v
        };
        let entries = |k: &str| Value::Array(package[k].as_array().into_iter().flatten().map(|e| json!([e[0], without(&e[1], &["content"])])).collect());
        let notes = |k: &str| Value::Array(package[k].as_array().into_iter().flatten().map(|n| without(n, &["content", "verbatimXml", "sourceOrdinal"])).collect());
        let d = &package["document"];
        let metadata = json!({
            "finalSectionProperties": d["finalSectionProperties"],
            "sections": d["sections"].as_array().map(|s| s.iter().map(|s| without(s, &["content"])).collect::<Vec<_>>()),
            "headers": entries("headerEntries"),
            "footers": entries("footerEntries"),
            "footnotes": notes("footnotes"),
            "endnotes": notes("endnotes"),
            "relationships": package["relationshipEntries"],
        });
        let list = d["comments"].as_array().cloned().unwrap_or_default();
        Keys { metadata: canon(&metadata), comments: canon(&Value::Array(list.clone())), ids: list.iter().map(|c| canon(&c["id"])).collect() }
    }
}

/// The document's state beside its stories, as the engine holds it: the
/// final section's properties (its part metadata's and margin overrides,
/// as Page Setup sets them), the headers' and footers' metadata, and the
/// watermark set in the editor.
fn document_state(doc: &docx_edit::EditingDoc, package: &mut Value, state: &State) {
    let merge = |into: &mut Value, patch: &Value| {
        if let Some(p) = patch.as_object() {
            if !into.is_object() {
                *into = json!({});
            }
            for (k, v) in p {
                into[k] = v.clone();
            }
        }
    };
    let meta: Value = serde_json::from_str(&doc.part_metadata_json()).unwrap_or(Value::Null);
    let margins: Value = serde_json::from_str(&doc.section_margins_json()).unwrap_or(Value::Null);
    let fin = &mut package["document"]["finalSectionProperties"];
    if let Some(p) = meta.pointer("/body/finalSectionProperties").filter(|p| p.is_object()) {
        merge(fin, p);
    }
    if let Some(p) = margins.get("final") {
        merge(fin, p);
    }
    for (entries, kind) in [("headerEntries", "headers"), ("footerEntries", "footers")] {
        for m in meta[kind].as_array().into_iter().flatten() {
            let Some(id) = m[0].as_str() else { continue };
            let Some(entry) = package[entries].as_array_mut().and_then(|e| e.iter_mut().find(|e| e[0] == id)) else { continue };
            for (k, v) in m[1].as_object().into_iter().flatten().filter(|(k, _)| *k != "content") {
                entry[1][k] = v.clone();
            }
            if m[1].get("watermark").is_none()
                && let Some(o) = entry[1].as_object_mut()
            {
                o.remove("watermark");
            }
        }
    }
    if let Some(w) = &state.watermark {
        set_watermark(package, w.as_ref());
    }
}

const HEADER_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";

/// `setDocumentWatermark`: the watermark on every header, the headers it
/// needs made (a default one, and first and even ones where the sections
/// use them); `None` takes it off.
fn set_watermark(package: &mut Value, w: Option<&Value>) {
    let headers = package["headerEntries"].as_array_mut();
    let Some(w) = w else {
        for e in headers.into_iter().flatten() {
            if let Some(o) = e[1].as_object_mut() {
                o.remove("watermark");
            }
        }
        return;
    };
    let mut headers: Vec<Value> = headers.map(|h| h.clone()).unwrap_or_default();
    for e in &mut headers {
        e[1]["watermark"] = w.clone();
    }
    let d = &package["document"];
    let props: Vec<&Value> = d["sections"].as_array().into_iter().flatten().map(|s| &s["properties"]).filter(|p| p.is_object()).chain(Some(&d["finalSectionProperties"]).filter(|p| p.is_object())).collect();
    let kinds: Vec<String> = headers.iter().filter_map(|h| h[1]["hdrFtrType"].as_str().map(str::to_owned)).collect();
    let none = headers.is_empty();
    let first = props.iter().any(|p| super::project::truthy(&p["titlePg"])) && !kinds.iter().any(|k| k == "first");
    let even = props.iter().any(|p| super::project::truthy(&p["evenAndOddHeaders"])) && !kinds.iter().any(|k| k == "even");
    if !none && !first && !even {
        package["headerEntries"] = Value::Array(headers);
        return;
    }
    let mut rels: Vec<Value> = package["relationshipEntries"].as_array().cloned().unwrap_or_default();
    let mut targets: HashSet<String> = rels.iter().filter_map(|r| r[1]["target"].as_str()).map(|t| t.trim_start_matches('/').trim_start_matches("word/").to_lowercase()).collect();
    let mut made: HashMap<String, String> = HashMap::new();
    let mut add = |kind: &str, rels: &mut Vec<Value>, headers: &mut Vec<Value>| -> String {
        if let Some(id) = made.get(kind) {
            return id.clone();
        }
        let mut o = 1;
        while targets.contains(&format!("header{o}.xml")) {
            o += 1;
        }
        let target = format!("header{o}.xml");
        targets.insert(target.clone());
        let cap = format!("{}{}", kind[..1].to_uppercase(), &kind[1..]);
        let mut id = format!("rIdWmHdr{cap}");
        let mut b = 1;
        while rels.iter().any(|r| r[0] == id.as_str()) {
            id = format!("rIdWmHdr{cap}{b}");
            b += 1;
        }
        rels.push(json!([id, { "id": id, "type": HEADER_REL, "target": target }]));
        headers.push(json!([id, { "type": "header", "hdrFtrType": kind, "content": [], "watermark": w }]));
        made.insert(kind.to_owned(), id.clone());
        id
    };
    let mut refs = |p: &mut Value, rels: &mut Vec<Value>, headers: &mut Vec<Value>| {
        let mut with = |p: &mut Value, kind: &str, rels: &mut Vec<Value>, headers: &mut Vec<Value>| {
            if !p.is_object() {
                *p = json!({});
            }
            let list = p["headerReferences"].as_array().cloned().unwrap_or_default();
            if list.iter().any(|r| r["type"] == kind) {
                return;
            }
            let id = add(kind, rels, headers);
            let mut list = list;
            list.push(json!({ "type": kind, "rId": id }));
            p["headerReferences"] = Value::Array(list);
        };
        if none {
            with(p, "default", rels, headers);
        }
        if first && super::project::truthy(&p["titlePg"]) {
            with(p, "first", rels, headers);
        }
        if even && super::project::truthy(&p["evenAndOddHeaders"]) {
            with(p, "even", rels, headers);
        }
    };
    let mut fin = package["document"]["finalSectionProperties"].clone();
    if fin.is_object() || none {
        refs(&mut fin, &mut rels, &mut headers);
    }
    package["document"]["finalSectionProperties"] = fin;
    if let Some(sections) = package["document"]["sections"].as_array_mut() {
        for s in sections {
            if s["properties"].is_object() {
                refs(&mut s["properties"], &mut rels, &mut headers);
            }
        }
    }
    package["headerEntries"] = Value::Array(headers);
    package["relationshipEntries"] = Value::Array(rels);
}

/// The comments to save, as the web editor's comment list holds them, and
/// the engine's id of each the model numbers differently.
///
/// The parsed comments take what the editor set on them (author, date,
/// words, as the web's `Er` does; its resolved state and the comment it
/// answers, as its sidebar does); a comment the engine no longer holds, or
/// whose words have no place left (and its replies), goes, as the
/// sidebar's delete and orphan sweep take it out; new ones follow,
/// numbered after the last.
fn comments(doc: &docx_edit::EditingDoc, original: &Value) -> (Vec<Value>, HashMap<i64, String>) {
    use super::project::{any_value, jnum};
    let source = original["document"]["comments"].as_array().cloned().unwrap_or_default();
    let mut engine = doc.list_comments().unwrap_or_default();
    let anchored = |id: &str| doc.resolve_comment(id).is_ok_and(|r| !r.is_empty());
    // a parent with nothing anchored goes with its replies
    let gone: HashSet<String> = engine.iter().filter(|c| c.parent_id.is_none() && !anchored(&c.id)).map(|c| c.id.clone()).collect();
    engine.retain(|c| !gone.contains(&c.id) && !c.parent_id.as_ref().is_some_and(|p| gone.contains(p)));
    let key = |id: &Value| id.as_f64().map(jnum).map(|v| v.to_string()).unwrap_or_default();
    let mut used: HashSet<i64> = source.iter().filter_map(|c| c["id"].as_f64()).map(|n| n as i64).collect();
    let mut next = used.iter().copied().max().unwrap_or(-1).max(-1) + 1;
    let mut numbers: HashMap<String, i64> = HashMap::new();
    let mut out: Vec<Value> = Vec::new();
    let mut parents: Vec<(usize, Option<String>)> = Vec::new();
    for c in &source {
        let id = key(&c["id"]);
        let Some(at) = engine.iter().position(|e| e.id == id) else { continue };
        let u = engine.remove(at);
        numbers.insert(u.id.clone(), c["id"].as_f64().unwrap_or(0.0) as i64);
        let body = any_value(&u.body);
        let mut c = c.clone();
        if !u.author.is_empty() || !u.date.is_empty() || !body.is_null() {
            if !u.author.is_empty() {
                c["author"] = json!(u.author);
            }
            if !u.date.is_empty() {
                c["date"] = json!(u.date);
            }
            if !body.is_null() {
                c["content"] = Value::Array(comment_content(&body));
            }
        }
        if u.done {
            c["done"] = json!(true);
        }
        parents.push((out.len(), u.parent_id.clone()));
        out.push(c);
    }
    let mut session_ids = HashMap::new();
    for u in engine {
        let mut n: i64 = u.id.parse().unwrap_or(-1);
        if n < 0 || used.contains(&n) {
            while used.contains(&next) {
                next += 1;
            }
            n = next;
        }
        used.insert(n);
        if n.to_string() != u.id {
            session_ids.insert(n, u.id.clone());
        }
        numbers.insert(u.id.clone(), n);
        let mut c = json!({ "id": n, "author": u.author, "content": comment_content(&any_value(&u.body)) });
        if !u.date.is_empty() {
            c["date"] = json!(u.date);
        }
        if u.done {
            c["done"] = json!(true);
        }
        parents.push((out.len(), u.parent_id.clone()));
        out.push(c);
    }
    for (at, parent) in parents {
        if let Some(p) = parent.and_then(|p| numbers.get(&p).copied()) {
            out[at]["parentId"] = json!(p);
        }
    }
    (out, session_ids)
}

/// `Ct`: a comment's words as paragraphs.
fn comment_content(body: &Value) -> Vec<Value> {
    if let Some(a) = body.as_array().filter(|a| !a.is_empty() && a.iter().all(|p| p["type"] == "paragraph")) {
        return a.clone();
    }
    let text = body.as_str().unwrap_or("");
    let content = if text.is_empty() { json!([]) } else { json!([{ "type": "run", "formatting": {}, "content": [{ "type": "text", "text": text }] }]) };
    vec![json!({ "type": "paragraph", "formatting": {}, "content": content })]
}

/// `injectReplyRangeMarkers`: a reply with no range of its own marks its
/// parent's words.
fn reply_marks(content: &mut Value, comments: &[Value]) -> bool {
    let has = |id: &Value| {
        fn find(v: &Value, id: &Value) -> bool {
            match v {
                Value::Array(a) => a.iter().any(|v| find(v, id)),
                Value::Object(o) => (o.get("type").and_then(Value::as_str) == Some("commentRangeStart") && o.get("id") == Some(id)) || o.values().any(|v| find(v, id)),
                _ => false,
            }
        }
        find(content, id)
    };
    let mut replies: HashMap<String, Vec<Value>> = HashMap::new();
    for c in comments.iter().filter(|c| !c["parentId"].is_null() && !has(&c["id"])) {
        replies.entry(c["parentId"].to_string()).or_default().push(c["id"].clone());
    }
    if replies.is_empty() {
        return false;
    }
    fn walk(blocks: &mut Value, replies: &HashMap<String, Vec<Value>>, done: &mut bool) {
        for l in blocks.as_array_mut().into_iter().flatten() {
            match l["type"].as_str() {
                Some("paragraph") => {
                    let items = l["content"].as_array().cloned().unwrap_or_default();
                    if !items.iter().any(|n| matches!(n["type"].as_str(), Some("commentRangeStart" | "commentRangeEnd"))) {
                        continue;
                    }
                    let mut g = Vec::new();
                    for n in items {
                        let kind = n["type"].as_str().unwrap_or("").to_owned();
                        let ids = replies.get(&n["id"].to_string()).cloned();
                        g.push(n);
                        if matches!(kind.as_str(), "commentRangeStart" | "commentRangeEnd")
                            && let Some(ids) = ids
                        {
                            for d in ids {
                                g.push(json!({ "type": kind, "id": d }));
                                *done = true;
                            }
                        }
                    }
                    l["content"] = Value::Array(g);
                }
                Some("table") => {
                    for row in l["rows"].as_array_mut().into_iter().flatten() {
                        for cell in row["cells"].as_array_mut().into_iter().flatten() {
                            walk(&mut cell["content"], replies, done);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let mut done = false;
    walk(content, &replies, &mut done);
    done
}

/// `Be` over every story: each paragraph (in tables and block content
/// controls too) through `f`, which may drop it.
fn each_paragraph(package: &mut Value, f: &mut dyn FnMut(&mut Value) -> bool) {
    fn blocks(v: &mut Value, f: &mut dyn FnMut(&mut Value) -> bool) {
        let Some(list) = v.as_array_mut() else { return };
        list.retain_mut(|b| match b["type"].as_str() {
            Some("paragraph") => f(b),
            Some("table") => {
                for row in b["rows"].as_array_mut().into_iter().flatten() {
                    for cell in row["cells"].as_array_mut().into_iter().flatten() {
                        blocks(&mut cell["content"], f);
                    }
                }
                true
            }
            Some("blockSdt") => {
                blocks(&mut b["content"], f);
                true
            }
            _ => true,
        });
    }
    blocks(&mut package["document"]["content"], f);
    for k in ["headerEntries", "footerEntries"] {
        for e in package[k].as_array_mut().into_iter().flatten() {
            blocks(&mut e[1]["content"], f);
        }
    }
    for k in ["footnotes", "endnotes"] {
        for n in package[k].as_array_mut().into_iter().flatten() {
            blocks(&mut n["content"], f);
        }
    }
}

fn session_key(p: &Value) -> Option<&str> {
    p.get(super::project::SESSION_KEY).and_then(Value::as_str)
}

/// `Nr`: empty paragraphs the editor added on its own don't save.
fn drop_synthetic(package: &mut Value, identities: &docx_edit::ParagraphIdentities) {
    let synthetic: HashSet<&str> = identities
        .paragraphs
        .iter()
        .filter(|p| p.origin == docx_edit::ParagraphOrigin::Synthetic)
        .filter_map(|p| match &p.paragraph {
            docx_edit::ParagraphRef::Session { para_id, .. } => Some(para_id.as_str()),
            _ => None,
        })
        .collect();
    if synthetic.is_empty() {
        return;
    }
    each_paragraph(package, &mut |p| !(p["content"].as_array().is_some_and(Vec::is_empty) && session_key(p).is_some_and(|k| synthetic.contains(k))));
}

/// `Br`: each paragraph a spliced part holds takes its source ordinal.
fn set_ordinals(package: &mut Value, spliced: &[&docx_edit::SplicedPart]) {
    let ordinals: HashMap<&str, u32> = spliced.iter().flat_map(|p| p.paragraphs.iter().map(|(o, k)| (k.as_str(), *o))).collect();
    if ordinals.is_empty() {
        return;
    }
    each_paragraph(package, &mut |p| {
        if let Some(&o) = session_key(p).and_then(|k| ordinals.get(k)) {
            p["sourceOrdinal"] = json!(o);
        }
        true
    });
}

/// The paragraph ids the save wrote, recorded in the engine so the next
/// save keeps them (the web's `recordSavedParagraphIds`).
fn record_saved(doc: &docx_edit::EditingDoc, identities: &docx_edit::ParagraphIdentities, plan: &docx_edit::ParagraphSavePlan, keys: &[(String, Option<String>)], bytes: &[u8]) {
    let Ok(written) = docx_parse::paragraph_identity::paragraph_ids_by_part(bytes) else { return };
    let wrote = |part: &str, id: &str| written.get(part).is_some_and(|ids| ids.iter().any(|w| *w == id.to_uppercase()));
    let mut saved = Vec::new();
    for (key, para_id) in keys {
        let Some(para_id) = para_id else { continue };
        let persisted = identities.paragraphs.iter().find_map(|p| match &p.paragraph {
            docx_edit::ParagraphRef::Session { para_id: s, .. } if s == key && p.ooxml_para_id.is_some() => p.source_story.as_ref(),
            _ => None,
        });
        if let Some(story) = persisted
            && wrote(&story.part_uri, para_id)
        {
            saved.push((key.clone(), para_id.clone()));
        }
    }
    for (part, ordinal, para_id) in &plan.assignments {
        if wrote(&format!("/{part}"), para_id) {
            saved.push((format!("/{part}#{ordinal}"), para_id.clone()));
        }
    }
    if !saved.is_empty() {
        doc.record_saved_paragraph_ids(&saved);
    }
}
/// The document as HTML, for the print panel (AppKit lays it out): its
/// paragraphs with their bold, italic and underline, headings, and tables.
pub(crate) fn html(doc: &docx_edit::EditingDoc, index: &super::index::Index) -> String {
    fn esc(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }
    fn story(doc: &docx_edit::EditingDoc, index: &super::index::Index, id: &str, out: &mut String) {
        let Ok(segments) = doc.story_segments(id) else { return };
        let mut para = String::new();
        let mut tables = 0u32;
        for s in segments {
            match s.content {
                docx_edit::SegmentContent::Text(t) => {
                    let on = |k: &str| matches!(s.attributes.get(k), Some(yrs::Any::Bool(true)));
                    let mut t = esc(&t);
                    if on("bold") {
                        t = format!("<b>{t}</b>");
                    }
                    if on("italic") {
                        t = format!("<i>{t}</i>");
                    }
                    if s.attributes.get("underline").is_some_and(|v| !matches!(v, yrs::Any::Null | yrs::Any::Bool(false))) {
                        t = format!("<u>{t}</u>");
                    }
                    if matches!(s.attributes.get("del"), Some(yrs::Any::Map(_))) {
                        t = format!("<s>{t}</s>");
                    }
                    para += &t;
                }
                docx_edit::SegmentContent::Pilcrow(p) => {
                    let style = match p.values.get("pStyle") {
                        Some(yrs::Any::String(s)) => s.to_string(),
                        _ => String::new(),
                    };
                    let tag = match style.strip_prefix("Heading").and_then(|n| n.parse::<u8>().ok()) {
                        Some(n) if (1..=6).contains(&n) => format!("h{n}"),
                        _ if p.values.contains_key("numPr") => "li".to_owned(),
                        _ => "p".to_owned(),
                    };
                    *out += &format!("<{tag}>{}</{tag}>\n", std::mem::take(&mut para));
                }
                docx_edit::SegmentContent::OtherEmbed { kind, .. } if kind == "table" => {
                    // the cells' stories, row by row
                    let prefix = format!("{id}:t{tables}:");
                    tables += 1;
                    let mut cells: Vec<(u32, u32, String)> = Vec::new();
                    for p in index.paras.iter().filter(|p| p.story.starts_with(&prefix)) {
                        if let Some((_, _, r, c)) = super::index::cell_of(&p.story)
                            && !cells.iter().any(|x| x.0 == r && x.1 == c)
                        {
                            cells.push((r, c, p.story.clone()));
                        }
                    }
                    *out += "<table border=\"1\" cellspacing=\"0\" cellpadding=\"4\">";
                    let mut row = None;
                    for (r, _, cell) in cells {
                        if row != Some(r) {
                            if row.is_some() {
                                *out += "</tr>";
                            }
                            *out += "<tr>";
                            row = Some(r);
                        }
                        *out += "<td>";
                        story(doc, index, &cell, out);
                        *out += "</td>";
                    }
                    *out += "</tr></table>\n";
                }
                docx_edit::SegmentContent::OtherEmbed { .. } => {}
            }
        }
    }
    let mut out = String::from("<html><head><meta charset=\"utf-8\"></head><body>");
    story(doc, index, "body", &mut out);
    out += "</body></html>";
    out
}
