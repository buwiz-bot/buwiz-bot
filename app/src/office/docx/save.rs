//! The package side of a Word document: its pictures, its lists, a legacy
//! `.doc` brought in through office_oxide, and the save.

use std::io::Cursor;

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

/// The document as edited, as `.docx` bytes.
///
/// The package is parsed again (with each paragraph's place in its part)
/// and its body rebuilt from the engine's stories: a paragraph the engine
/// kept unchanged stays as parsed, a changed or new one takes its text and
/// run formats from the engine and its paragraph properties from the one
/// it came from, a table keeps its structure with each cell rebuilt the
/// same way. docx-parse's package writer then writes it, every part the
/// body doesn't touch as it was, and unchanged paragraphs from their
/// source XML.
pub(crate) fn docx(source: &[u8], doc: &docx_edit::EditingDoc) -> Result<Vec<u8>, String> {
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    let options = docx_parse::S9ParseOptions { source_ordinals: true, ..Default::default() };
    let envelope = docx_parse::parse_docx_s9_wire(source, options).map_err(|e| e.to_string())?;
    let package = envelope.document.package;
    let mut document = serde_json::to_value(&package.document).map_err(|e| e.to_string())?;
    // where each engine paragraph came from: its w:p ordinal in the main part
    let mut sources: std::collections::HashMap<(String, String), u32> = Default::default();
    for p in doc.paragraph_identities().paragraphs {
        if let (docx_edit::ParagraphRef::Session { story, para_id }, Some(src)) = (&p.paragraph, &p.source)
            && src.part_uri.trim_start_matches('/') == "word/document.xml"
        {
            sources.insert((story.clone(), para_id.clone()), src.paragraph_ordinal);
        }
    }
    let plan = doc.paragraph_save_plan();
    let changed: std::collections::HashSet<u32> =
        plan.spliced_parts.iter().chain(&plan.section_spliced_parts).filter(|p| p.part.trim_start_matches('/') == "word/document.xml").flat_map(|p| p.changed.iter().copied()).collect();
    let content = document["content"].as_array().cloned().unwrap_or_default();
    let ctx = Rebuild { doc, sources: &sources, changed: &changed };
    document["content"] = Value::Array(ctx.story("body", &content)?);
    // sections are carried by their paragraphs' section properties
    if let Some(o) = document.as_object_mut() {
        o.remove("sections");
    }
    let request = json!({
        "determinism": { "seed": format!("{:x}", Sha256::digest(source)), "now": "1970-01-01T00:00:00.000Z" },
        "document": document,
        "headerEntries": package.header_entries.unwrap_or_default(),
        "footerEntries": package.footer_entries.unwrap_or_default(),
        "footnotes": package.footnotes.unwrap_or_default(),
        "endnotes": package.endnotes.unwrap_or_default(),
        "footnoteSeparators": package.footnote_separators.unwrap_or_default(),
        "endnoteSeparators": package.endnote_separators.unwrap_or_default(),
        "relationshipEntries": package.relationship_entries,
        "numbering": package.numbering,
        "options": { "updateModifiedDate": false },
    });
    let request: docx_parse::serializer::S13SaveRequest = serde_json::from_value(request).map_err(|e| e.to_string())?;
    docx_parse::serializer::write_docx_s13(request, source).map_err(|e| e.to_string())
}

struct Rebuild<'a> {
    doc: &'a docx_edit::EditingDoc,
    sources: &'a std::collections::HashMap<(String, String), u32>,
    changed: &'a std::collections::HashSet<u32>,
}

/// The parsed paragraphs of `blocks` by their ordinal, tables' included.
fn by_ordinal(blocks: &[serde_json::Value], out: &mut std::collections::HashMap<u64, serde_json::Value>) {
    for b in blocks {
        match b["type"].as_str() {
            Some("paragraph") => {
                if let Some(o) = b["sourceOrdinal"].as_u64() {
                    out.insert(o, b.clone());
                }
            }
            Some("table") => {
                for row in b["rows"].as_array().into_iter().flatten() {
                    for cell in row["cells"].as_array().into_iter().flatten() {
                        by_ordinal(cell["content"].as_array().map(Vec::as_slice).unwrap_or(&[]), out);
                    }
                }
            }
            _ => {
                if let Some(c) = b["content"].as_array() {
                    by_ordinal(c, out);
                }
            }
        }
    }
}

/// A yrs value as JSON.
fn any_json(v: &yrs::Any) -> serde_json::Value {
    let mut s = String::new();
    v.to_json(&mut s);
    serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)
}

/// A text's attributes (the engine's marks) as the model's run formatting:
/// the names that differ mapped, then only what the model reads kept.
fn run_formatting(attrs: &std::collections::BTreeMap<String, yrs::Any>) -> Option<serde_json::Value> {
    use serde_json::{Value, json};
    let mut fmt = serde_json::Map::new();
    for (k, v) in attrs {
        let j = any_json(v);
        if j.is_null() || j == Value::Bool(false) || k == "ins" || k == "del" || k.starts_with('_') {
            continue;
        }
        match k.as_str() {
            "fontSize" => {
                if let Some(n) = j.get("size").and_then(Value::as_f64).or(j.as_f64()) {
                    fmt.insert("fontSize".into(), json!(n));
                }
                if let Some(n) = j.get("sizeCs").and_then(Value::as_f64) {
                    fmt.insert("fontSizeCs".into(), json!(n));
                }
            }
            "textColor" => {
                fmt.insert("color".into(), j);
            }
            "superscript" => {
                fmt.insert("vertAlign".into(), json!("superscript"));
            }
            "subscript" => {
                fmt.insert("vertAlign".into(), json!("subscript"));
            }
            "underline" if j == Value::Bool(true) => {
                fmt.insert("underline".into(), json!({ "style": "single" }));
            }
            _ => {
                fmt.insert(k.clone(), j);
            }
        }
    }
    // what the model can't read goes
    let keys: Vec<String> = fmt.keys().cloned().collect();
    for k in keys {
        let one = json!({ k.clone(): fmt[&k].clone() });
        if serde_json::from_value::<docx_parse::TextFormatting>(one).is_err() {
            fmt.remove(&k);
        }
    }
    (!fmt.is_empty()).then_some(Value::Object(fmt))
}

fn run_json(text: &str, attrs: &std::collections::BTreeMap<String, yrs::Any>) -> serde_json::Value {
    use serde_json::json;
    let mut run = json!({ "type": "run", "content": [{ "type": "text", "text": text, "preserveSpace": true }] });
    if let Some(f) = run_formatting(attrs) {
        run["formatting"] = f;
    }
    for (key, kind) in [("ins", "insertion"), ("del", "deletion")] {
        if let Some(v @ yrs::Any::Map(_)) = attrs.get(key) {
            let mut info = any_json(v);
            if let Some(id) = info["id"].as_str().and_then(|i| i.rsplit(':').next()).and_then(|n| n.parse::<u64>().ok()) {
                info["id"] = json!(id);
            }
            if key == "del" {
                run["content"][0]["type"] = json!("delText");
            }
            return json!({ "type": kind, "info": info, "content": [run] });
        }
    }
    run
}

/// The runs of `p` (parsed) that hold something besides text: a picture, a
/// note mark, a tab, a field — each item as a run of its own.
fn atoms(p: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    fn walk(items: &[serde_json::Value], out: &mut Vec<serde_json::Value>) {
        for item in items {
            match item["type"].as_str() {
                Some("run") => {
                    for c in item["content"].as_array().into_iter().flatten() {
                        if c["type"] != "text" && c["type"] != "delText" {
                            let mut one = item.clone();
                            one["content"] = serde_json::json!([c]);
                            out.push(one);
                        }
                    }
                }
                _ => {
                    if let Some(c) = item["content"].as_array() {
                        walk(c, out);
                    }
                }
            }
        }
    }
    walk(p["content"].as_array().map(Vec::as_slice).unwrap_or(&[]), &mut out);
    out
}

impl Rebuild<'_> {
    /// `story`'s blocks, from the parsed `blocks` it was seeded from.
    fn story(&self, story: &str, blocks: &[serde_json::Value]) -> Result<Vec<serde_json::Value>, String> {
        use serde_json::{Value, json};
        let segments = self.doc.story_segments(story).map_err(|e| e.to_string())?;
        let mut parsed = std::collections::HashMap::new();
        by_ordinal(blocks, &mut parsed);
        let mut tables = blocks.iter().filter(|b| b["type"] == "table");
        let mut out: Vec<Value> = Vec::new();
        let mut runs: Vec<Value> = Vec::new();
        let mut embeds: Vec<(String, usize)> = Vec::new();
        let mut table_index = 0;
        let mut last_para: Option<Value> = None;
        for s in segments {
            match s.content {
                docx_edit::SegmentContent::Text(t) => runs.push(run_json(&t, &s.attributes)),
                docx_edit::SegmentContent::OtherEmbed { kind, .. } if kind == "table" => {
                    let Some(t) = tables.next() else { continue };
                    let mut t = t.clone();
                    for (r, row) in t["rows"].as_array_mut().into_iter().flatten().enumerate() {
                        for (c, cell) in row["cells"].as_array_mut().into_iter().flatten().enumerate() {
                            let id = format!("{story}:t{table_index}:r{r}c{c}");
                            let content = cell["content"].as_array().cloned().unwrap_or_default();
                            if let Ok(rebuilt) = self.story(&id, &content) {
                                cell["content"] = Value::Array(rebuilt);
                            }
                        }
                    }
                    table_index += 1;
                    out.push(t);
                }
                docx_edit::SegmentContent::OtherEmbed { kind, .. } => {
                    embeds.push((kind, runs.len()));
                    runs.push(Value::Null);
                }
                docx_edit::SegmentContent::Pilcrow(props) => {
                    let ordinal = self.sources.get(&(story.to_owned(), props.para_id.clone())).copied();
                    let source = ordinal.and_then(|o| parsed.get(&(o as u64)));
                    let mut items = std::mem::take(&mut runs);
                    let marks = std::mem::take(&mut embeds);
                    if let (Some(src), Some(o)) = (source, ordinal)
                        && !self.changed.contains(&o)
                    {
                        out.push(src.clone());
                        last_para = Some(src.clone());
                        continue;
                    }
                    // a changed or new paragraph: its properties from its source
                    // (or the one before it), its content from the engine
                    let mut p = source.cloned().or_else(|| last_para.clone()).unwrap_or_else(|| json!({ "type": "paragraph", "content": [] }));
                    let from = source.map(atoms).unwrap_or_default();
                    for (k, (kind, at)) in marks.iter().enumerate() {
                        items[*at] = match kind.as_str() {
                            "tab" => json!({ "type": "run", "content": [{ "type": "tab" }] }),
                            "hardBreak" | "break" | "lineBreak" => json!({ "type": "run", "content": [{ "type": "break" }] }),
                            _ => from.get(k).cloned().unwrap_or(Value::Null),
                        };
                    }
                    items.retain(|i| !i.is_null());
                    p["content"] = Value::Array(items);
                    if let Some(o) = p.as_object_mut() {
                        o.insert("paraId".to_owned(), json!(props.para_id));
                        o.remove("sourceOrdinal");
                        o.remove("textId");
                    }
                    // the list it is in
                    let fmt = p["formatting"].as_object().cloned().unwrap_or_default();
                    let mut fmt = Value::Object(fmt);
                    match props.values.get("numPr") {
                        Some(v) => fmt["numPr"] = any_json(v),
                        None => {
                            if let Some(o) = fmt.as_object_mut() {
                                o.remove("numPr");
                            }
                        }
                    }
                    // its style and alignment as the engine holds them
                    for (key, model) in [("pStyle", "styleId"), ("alignment", "alignment")] {
                        match props.values.get(key).map(any_json) {
                            Some(Value::String(v)) if !v.is_empty() => fmt[model] = json!(v),
                            _ => {
                                if let Some(o) = fmt.as_object_mut() {
                                    o.remove(model);
                                }
                            }
                        }
                    }
                    if source.is_none()
                        && let Some(o) = fmt.as_object_mut()
                    {
                        // a new paragraph doesn't start another section
                        o.remove("sectionProperties");
                    }
                    p["formatting"] = fmt;
                    if source.is_none()
                        && let Some(o) = p.as_object_mut()
                    {
                        o.remove("sectionProperties");
                    }
                    last_para = Some(p.clone());
                    out.push(p);
                }
            }
        }
        Ok(out)
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
