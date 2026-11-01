//! `DocxAgentDocument` (agents/document.ts) over `betteroffice-docx-edit`'s
//! `EditingDoc`, the editing core the npm package's `YrsSession` wraps:
//! paragraphs come from `story_segments` as the session's `storySegments`
//! gives them, proposals are `replace_range` calls, and a save is the
//! session's paragraph save plan (each changed paragraph patched into its
//! source XML) written by `docx-parse`'s package writer.
//!
//! Node forks the session for a proposal's render/export/verify and adopts
//! the fork's update on accept; here a draft is the source seeded again with
//! every accepted edit replayed (seeding is deterministic for one client
//! id), and an accept adopts the draft it was applied to.

use docx_edit::{EditCtx, EditingDoc, SegmentContent, StoryRange};
use serde_json::{json, Map, Value};

use super::proto::{boundary, engine, find_all, integer, lone_surrogate, opt_num, slice, sorted, units, Ordered, Res, ToolError};

const MAX_TEXT: usize = 16_000;
/// Every session (and its drafts) edits as this client, so a draft seeded
/// from the same bytes holds the same paragraph ids.
const CLIENT: u64 = 0x5245_4458;

#[derive(Clone)]
struct Run {
    start: usize,
    end: usize,
    formatting: Value,
    protected: bool,
}

#[derive(Clone)]
struct Para {
    r: String,
    revision: u64,
    story: String,
    para_id: String,
    paragraph: usize,
    text: Vec<u16>,
    style: Option<String>,
    heading_level: Option<u64>,
    runs: Vec<Run>,
    fingerprint: String,
}

#[derive(Clone)]
struct Change {
    r: String,
    start: usize,
    old: String,
    new: String,
}

struct HeldProposal {
    value: Map<String, Value>,
    changes: Vec<Change>,
    /// ref → (story, paraId, fingerprint)
    targets: Ordered<(String, String, String)>,
}

impl HeldProposal {
    fn status(&self) -> &str {
        self.value.get("status").and_then(Value::as_str).unwrap_or("")
    }
}

struct MatchTarget {
    r: String,
    revision: u64,
    old: String,
    start: usize,
}

struct Cursor {
    epoch: u64,
    signature: String,
    paragraph: usize,
    offset: usize,
}

/// One accepted replacement, in paragraph coordinates, for replay.
#[derive(Clone)]
struct Applied {
    story: String,
    para_id: String,
    start: usize,
    end: usize,
    old: String,
    text: String,
    author: Option<(String, String)>,
}

pub struct Docx {
    pub name: String,
    source: Vec<u8>,
    doc: EditingDoc,
    applied: Vec<Applied>,
    epoch: u64,
    indexed: Option<u64>,
    paragraphs: Vec<Para>,
    identities: Ordered<Para>,
    proposals: Ordered<HeldProposal>,
    matches: Ordered<MatchTarget>,
    cursors: Ordered<Cursor>,
    next_ref: u64,
    next_proposal: u64,
    next_cursor: u64,
    next_match: u64,
    closed: bool,
}

fn seeded(bytes: &[u8]) -> Res<EditingDoc> {
    let doc = EditingDoc::new(CLIENT);
    docx_edit::seed_from_docx(&doc, bytes).map_err(engine)?;
    Ok(doc)
}

/// A segment's yrs attributes as the session's JSON gives them.
fn attrs<T: serde::Serialize>(a: &T) -> Value {
    sorted(serde_json::to_value(a).unwrap_or(Value::Null))
}

/// `visibleFormatting(attributes)`.
fn visible_formatting(attributes: &Value) -> Value {
    const KEYS: [&str; 14] =
        ["bold", "italic", "underline", "strike", "textColor", "color", "fontFamily", "fontFamilyAscii", "fontFamilyHAnsi", "highlight", "verticalAlign", "hyperlink", "ins", "del"];
    let mut out = Map::new();
    let Some(map) = attributes.as_object() else { return Value::Object(out) };
    for (k, v) in map {
        if KEYS.contains(&k.as_str()) {
            let shown = if super::proto::json_len(v) <= 500 { v.clone() } else { json!("(formatting value exceeds preview limit)") };
            out.insert(k.clone(), shown);
        }
    }
    if let Some(size) = map.get("fontSize").filter(|s| s.is_object()) {
        if let Some(n) = size.get("size").and_then(Value::as_f64) {
            out.insert("fontSizePt".into(), json!(n / 2.0));
        }
        if let Some(n) = size.get("sizeCs").and_then(Value::as_f64) {
            out.insert("complexScriptFontSizePt".into(), json!(n / 2.0));
        }
    }
    Value::Object(out)
}

/// The story index of `offset` in paragraph `para_id` (wasm's `loc_index`).
fn loc(doc: &EditingDoc, story: &str, para_id: &str, offset: usize) -> Res<u32> {
    let mut at = 0u32;
    let mut start = 0u32;
    for seg in doc.story_segments(story).map_err(engine)? {
        match &seg.content {
            SegmentContent::Text(t) => at += t.encode_utf16().count() as u32,
            SegmentContent::Pilcrow(p) => {
                if p.para_id == para_id {
                    let len = at - start;
                    if offset as u32 > len {
                        return Err(engine(format!("offset {offset} exceeds the length {len} of paragraph {para_id:?}")));
                    }
                    return Ok(start + offset as u32);
                }
                at += 1;
                start = at;
            }
            SegmentContent::OtherEmbed { .. } => at += 1,
        }
    }
    Err(engine(format!("no paragraph {para_id:?} in story {story:?}")))
}

fn apply(doc: &EditingDoc, a: &Applied) -> Res<()> {
    let start = loc(doc, &a.story, &a.para_id, a.start)?;
    let end = loc(doc, &a.story, &a.para_id, a.end)?;
    let ctx = match &a.author {
        Some((name, date)) => EditCtx::local(name.clone(), date.clone()).suggesting(),
        None => EditCtx::local(String::new(), String::new()),
    };
    doc.replace_range(&ctx, StoryRange::new(a.story.as_str(), start, end), &a.text).map_err(engine)?;
    Ok(())
}

/// `Date.parse(date)` is finite: the ISO forms zod's `datetime()` admits.
fn date_ok(date: &str) -> bool {
    super::args::iso_datetime(date)
}

impl Docx {
    pub fn open(bytes: &[u8], name: &str) -> Res<Self> {
        let doc = seeded(bytes)?;
        Ok(Self {
            name: name.to_owned(),
            source: bytes.to_vec(),
            doc,
            applied: Vec::new(),
            epoch: 0,
            indexed: None,
            paragraphs: Vec::new(),
            identities: Ordered::default(),
            proposals: Ordered::default(),
            matches: Ordered::default(),
            cursors: Ordered::default(),
            next_ref: 1,
            next_proposal: 1,
            next_cursor: 1,
            next_match: 1,
            closed: false,
        })
    }

    fn index(&mut self) -> Res<()> {
        if self.closed {
            return Err(ToolError::new("CLOSED", "This document is closed. Open it again."));
        }
        if self.indexed == Some(self.epoch) {
            return Ok(());
        }
        let mut stories: Vec<String> = self.doc.list_stories().stories.into_iter().map(|s| s.story).collect();
        stories.sort();
        stories.dedup();
        let mut paragraphs = Vec::new();
        let mut active = Vec::new();
        for story in stories {
            let headings: Vec<(String, u64)> = self.doc.paragraph_headings(&story).map_err(engine)?.into_iter().map(|(id, h)| (id, h.outline_level as u64 + 1)).collect();
            let mut text: Vec<u16> = Vec::new();
            let mut runs = Vec::new();
            let mut segments: Vec<Value> = Vec::new();
            let mut ordinal = 0;
            for seg in self.doc.story_segments(&story).map_err(engine)? {
                let attributes = attrs(&seg.attributes);
                match &seg.content {
                    SegmentContent::Pilcrow(p) => {
                        segments.push(json!({ "attributes": attributes, "kind": "pilcrow", "paraId": p.para_id, "properties": attrs(&p.values) }));
                        let key = crate::js::stringify(&json!([story, p.para_id]));
                        active.push(key.clone());
                        let fingerprint = crate::js::stringify(&Value::Array(std::mem::take(&mut segments)));
                        let previous = self.identities.get(&key).cloned();
                        let r = match &previous {
                            Some(prev) => prev.r.clone(),
                            None => {
                                let r = format!("p{}", self.next_ref);
                                self.next_ref += 1;
                                r
                            }
                        };
                        ordinal += 1;
                        let para = Para {
                            r,
                            revision: previous.as_ref().map_or(1, |prev| prev.revision + u64::from(prev.fingerprint != fingerprint)),
                            story: story.clone(),
                            para_id: p.para_id.clone(),
                            paragraph: ordinal,
                            text: std::mem::take(&mut text),
                            style: p.values.get("pStyle").and_then(|s| serde_json::to_value(s).ok()).and_then(|v| v.as_str().map(str::to_owned)),
                            heading_level: headings.iter().find(|(id, _)| *id == p.para_id).map(|(_, l)| *l),
                            runs: std::mem::take(&mut runs),
                            fingerprint,
                        };
                        self.identities.set(key, para.clone());
                        paragraphs.push(para);
                    }
                    content => {
                        let (value, embed) = match content {
                            SegmentContent::Text(t) => {
                                segments.push(json!({ "attributes": attributes, "kind": "text", "text": t }));
                                (units(t), false)
                            }
                            SegmentContent::OtherEmbed { kind, payload } => {
                                segments.push(json!({ "attributes": attributes, "embedKind": kind, "kind": "embed", "payload": attrs(payload) }));
                                (vec![0xfffc], true)
                            }
                            SegmentContent::Pilcrow(_) => unreachable!(),
                        };
                        let truthy = |k: &str| attributes.get(k).is_some_and(crate::js::truthy);
                        runs.push(Run { start: text.len(), end: text.len() + value.len(), formatting: visible_formatting(&attributes), protected: embed || truthy("ins") || truthy("del") });
                        text.extend(value);
                    }
                }
            }
        }
        self.identities.retain(|k, _| active.iter().any(|a| a == k));
        self.paragraphs = paragraphs;
        self.indexed = Some(self.epoch);
        Ok(())
    }

    fn reference(p: &Para) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("ref".into(), json!(p.r));
        m.insert("revision".into(), json!(p.revision));
        m.insert("story".into(), json!(p.story));
        m.insert("paragraph".into(), json!(p.paragraph));
        m
    }

    fn get(&mut self, r: &str) -> Res<Para> {
        self.index()?;
        self.paragraphs.iter().find(|p| p.r == r).cloned().ok_or_else(|| ToolError::with("UNKNOWN_REF", "Paragraph missing. Use list or grep to find a current ref.", json!({ "ref": r })))
    }

    fn check_story(&self, story: Option<&str>) -> Res<()> {
        if let Some(s) = story.filter(|s| !s.is_empty()) {
            if !self.paragraphs.iter().any(|p| p.story == s) {
                return Err(ToolError::with("UNKNOWN_STORY", "Story missing. Use overview to list stories.", json!({ "story": s })));
            }
        }
        Ok(())
    }

    pub fn overview(&mut self) -> Res<Value> {
        self.index()?;
        let mut stories: Vec<(String, usize)> = Vec::new();
        for p in &self.paragraphs {
            match stories.iter_mut().find(|(s, _)| *s == p.story) {
                Some(e) => e.1 += 1,
                None => stories.push((p.story.clone(), 1)),
            }
        }
        let entries: Vec<Value> = stories.iter().take(30).map(|(story, n)| json!({ "story": story, "paragraphs": n })).collect();
        Ok(json!({
            "name": self.name,
            "format": "docx",
            "version": self.epoch,
            "paragraphs": self.paragraphs.len(),
            "stories": entries,
            "nextOffset": if entries.len() < stories.len() { json!(entries.len()) } else { Value::Null },
            "capabilities": { "grep": "literal", "proposals": true, "render": false, "export": true },
            "workflow": "grep for the exact text to replace; read its ref for context; propose with match and newText; review; render; verify; export.",
        }))
    }

    pub fn list(&mut self, args: &Value) -> Res<Value> {
        self.index()?;
        let offset = integer(opt_num(args, "offset", 0.0), 0.0, 9007199254740991.0, "offset")? as usize;
        let limit = integer(opt_num(args, "limit", 30.0), 1.0, 100.0, "limit")? as usize;
        let story = args.get("story").and_then(Value::as_str);
        self.check_story(story)?;
        let story = story.filter(|s| !s.is_empty());
        let headings = args.get("headingsOnly").is_some_and(crate::js::truthy);
        let entries: Vec<&Para> = self.paragraphs.iter().filter(|p| story.is_none_or(|s| p.story == s) && (!headings || p.heading_level.is_some())).collect();
        let items: Vec<Value> = entries
            .iter()
            .skip(offset)
            .take(limit)
            .map(|p| {
                let mut m = Self::reference(p);
                m.insert("style".into(), json!(p.style));
                m.insert("headingLevel".into(), json!(p.heading_level));
                m.insert("text".into(), json!(slice(&p.text, 0, 120)));
                m.insert("length".into(), json!(p.text.len()));
                Value::Object(m)
            })
            .collect();
        Ok(json!({ "version": self.epoch, "items": items, "nextOffset": if offset + limit < entries.len() { json!(offset + limit) } else { Value::Null }, "total": entries.len() }))
    }

    pub fn grep(&mut self, args: &Value) -> Res<Value> {
        self.index()?;
        let query = args.get("query").and_then(Value::as_str).unwrap_or("");
        if query.is_empty() || query.encode_utf16().count() > 1000 || query.contains(['\r', '\n', '\u{fffc}']) {
            return Err(ToolError::new("INVALID_ARGUMENT", "query must be 1–1000 characters of literal text within a paragraph."));
        }
        let limit = integer(opt_num(args, "limit", 20.0), 1.0, 100.0, "limit")? as usize;
        let story = args.get("story").and_then(Value::as_str);
        self.check_story(story)?;
        let case = args.get("caseSensitive").and_then(Value::as_bool);
        let signature = crate::js::stringify(&json!([query, case.unwrap_or(false), story]));
        let cursor = args.get("cursor").and_then(Value::as_str);
        let at = cursor.and_then(|c| self.cursors.get(c)).map(|c| (c.epoch, c.signature.clone(), c.paragraph, c.offset));
        if cursor.is_some() && at.as_ref().is_none_or(|(e, s, _, _)| *e != self.epoch || *s != signature) {
            return Err(ToolError::new("STALE_CURSOR", "Search changed or the document was edited. Repeat grep without cursor."));
        }
        let story = story.filter(|s| !s.is_empty());
        let mut matches = Vec::new();
        let mut size = 0;
        let first = at.as_ref().map_or(0, |a| a.2);
        for i in first..self.paragraphs.len() {
            let p = self.paragraphs[i].clone();
            if story.is_some_and(|s| p.story != s) {
                continue;
            }
            let from = if at.as_ref().is_some_and(|a| a.2 == i) { at.as_ref().unwrap().3 } else { 0 };
            for (start, end) in find_all(&p.text, query, case.unwrap_or(false), from) {
                if p.runs.iter().any(|r| r.protected && r.start < end && r.end > start) {
                    continue;
                }
                if matches.len() == limit || (!matches.is_empty() && size + (end - start) + 160 > MAX_TEXT) {
                    let c = format!("c{}", self.next_cursor);
                    self.next_cursor += 1;
                    if self.cursors.len() >= 64 {
                        self.cursors.delete_first();
                    }
                    self.cursors.set(c.clone(), Cursor { epoch: self.epoch, signature, paragraph: i, offset: start });
                    return Ok(json!({ "matches": matches, "nextCursor": c, "version": self.epoch }));
                }
                let context_start = start.saturating_sub(80);
                let context = slice(&p.text, context_start, end + 80);
                size += context.encode_utf16().count();
                let id = format!("m{}", self.next_match);
                self.next_match += 1;
                if self.matches.len() >= 1024 {
                    self.matches.delete_first();
                }
                let text = slice(&p.text, start, end);
                self.matches.set(id.clone(), MatchTarget { r: p.r.clone(), revision: p.revision, old: text.clone(), start });
                let mut m = Self::reference(&p);
                m.insert("match".into(), json!(id));
                m.insert("start".into(), json!(start));
                m.insert("end".into(), json!(end));
                m.insert("text".into(), json!(text));
                m.insert("context".into(), json!(context));
                m.insert("contextStart".into(), json!(context_start));
                matches.push(Value::Object(m));
            }
        }
        Ok(json!({ "matches": matches, "nextCursor": null, "version": self.epoch }))
    }

    pub fn read(&mut self, r: &str, args: &Value) -> Res<Value> {
        let p = self.get(r)?;
        let len = p.text.len();
        let mut start = integer(opt_num(args, "start", 0.0), 0.0, len as f64, "start")? as usize;
        let length = integer(opt_num(args, "length", 4000.0), 1.0, MAX_TEXT as f64, "length")? as usize;
        if !boundary(&p.text, start) {
            start -= 1;
        }
        let mut end = len.min(start + length);
        let visible: Vec<&Run> = p.runs.iter().filter(|r| r.start < end && r.end > start).collect();
        if visible.len() > 100 {
            end = end.min(visible[99].end);
        }
        if !boundary(&p.text, end) {
            end -= 1;
        }
        if end == start && end < len {
            end += 2;
        }
        let runs: Vec<Value> = p
            .runs
            .iter()
            .filter(|r| r.start < end && r.end > start)
            .map(|r| json!({ "start": start.max(r.start), "end": end.min(r.end), "formatting": r.formatting, "protected": r.protected }))
            .collect();
        let mut m = Self::reference(&p);
        m.insert("text".into(), json!(slice(&p.text, start, end)));
        m.insert("start".into(), json!(start));
        m.insert("end".into(), json!(end));
        m.insert("length".into(), json!(len));
        m.insert("nextStart".into(), if end < len { json!(end) } else { Value::Null });
        m.insert("style".into(), json!(p.style));
        m.insert("runs".into(), Value::Array(runs));
        Ok(Value::Object(m))
    }

    pub fn propose(&mut self, input: &Value) -> Res<Value> {
        self.index()?;
        let author = input.get("author").and_then(Value::as_str).unwrap_or("");
        let note = input.get("note").and_then(Value::as_str);
        let edits = input.get("edits").and_then(Value::as_array).cloned().unwrap_or_default();
        let u = |s: &str| s.encode_utf16().count();
        if author.is_empty() || u(author) > 200 || note.map_or(0, u) > 2000 || edits.is_empty() || edits.len() > 32 {
            return Err(ToolError::new("INVALID_ARGUMENT", "Supply an author (1–200 characters), note (up to 2000), and 1–32 edits."));
        }
        if self.proposals.len() >= 64 {
            let closed = self.proposals.iter().find(|(_, h)| h.status() != "pending").map(|(k, _)| k.clone());
            match closed {
                Some(k) => {
                    self.proposals.delete(&k);
                }
                None => return Err(ToolError::new("PROPOSAL_LIMIT", "Reject or discard a pending proposal before staging more than 64.")),
            }
        }
        let mut changes: Vec<Change> = Vec::new();
        let mut shown = Vec::new();
        let mut targets: Ordered<(String, String, String)> = Ordered::default();
        let mut size = 0;
        for edit in &edits {
            let Some(target) = self.matches.get(edit["match"].as_str().unwrap_or("")) else {
                return Err(ToolError::new("UNKNOWN_MATCH", "Match expired or belongs to another document. Run grep again and copy its match ID."));
            };
            let (r, revision, old, start) = (target.r.clone(), target.revision, target.old.clone(), target.start);
            let new_text = edit["newText"].as_str().unwrap_or("").to_owned();
            let p = self.get(&r)?;
            if p.revision != revision {
                return Err(stale(&[p.r.clone()]));
            }
            if new_text.contains(['\r', '\n', '\u{fffc}']) || super::proto::xml_control(&new_text) {
                return Err(ToolError::new("INVALID_ARGUMENT", "Edits replace plain text inside one paragraph; paragraph breaks, embeds, and control characters are unsupported."));
            }
            if lone_surrogate(&units(&new_text)) {
                return Err(ToolError::new("INVALID_ARGUMENT", "newText must contain complete Unicode characters."));
            }
            let (old_len, new_len) = (u(&old), u(&new_text));
            size += old_len + new_len;
            if size > MAX_TEXT {
                return Err(ToolError::new("EDIT_LIMIT", format!("Combined oldText and newText must fit in {MAX_TEXT} UTF-16 units.")));
            }
            if start + old_len > p.text.len() || slice(&p.text, start, start + old_len) != old {
                return Err(ToolError::with("TEXT_MISMATCH", "oldText does not match at start. Run grep and use its match ID instead of calculating offsets.", json!({ "ref": p.r })));
            }
            let end = start + old_len;
            if !boundary(&p.text, start) || !boundary(&p.text, end) {
                return Err(ToolError::new("INVALID_RANGE", "Text ranges must not split a Unicode character."));
            }
            if old == new_text {
                return Err(ToolError::new("NO_CHANGE", "oldText and newText are identical."));
            }
            if p.runs.iter().any(|run| run.protected && (run.start < end && run.end > start || start == end && run.start <= start && start < run.end)) {
                return Err(ToolError::new("PROTECTED_CONTENT", "This range contains an embed or an existing tracked change. Choose an ordinary text range."));
            }
            if changes.iter().any(|c| c.r == p.r && start <= c.start + u(&c.old) && end >= c.start) {
                return Err(ToolError::new("OVERLAPPING_EDITS", "Edits within one proposal must have separate, non-touching ranges."));
            }
            let mut m = Self::reference(&p);
            m.insert("start".into(), json!(start));
            m.insert("oldText".into(), json!(old));
            m.insert("newText".into(), json!(new_text));
            shown.push(Value::Object(m));
            changes.push(Change { r: p.r.clone(), start, old, new: new_text });
            targets.set(p.r.clone(), (p.story.clone(), p.para_id.clone(), p.fingerprint.clone()));
        }
        let id = format!("proposal{}", self.next_proposal);
        self.next_proposal += 1;
        let mut value = Map::new();
        value.insert("id".into(), json!(id));
        value.insert("author".into(), json!(author));
        value.insert("note".into(), json!(note.unwrap_or("")));
        value.insert("status".into(), json!("pending"));
        value.insert("changes".into(), Value::Array(shown));
        value.insert("staleRefs".into(), json!([]));
        let out = Value::Object(value.clone());
        self.proposals.set(id, HeldProposal { value, changes, targets });
        Ok(out)
    }

    fn proposal(&mut self, id: &str) -> Res<()> {
        self.index()?;
        if !self.proposals.has(id) {
            return Err(ToolError::with("UNKNOWN_PROPOSAL", "Proposal missing. Stage a proposal first.", json!({ "id": id })));
        }
        Ok(())
    }

    fn stale_refs(&mut self, id: &str) -> Res<Vec<String>> {
        self.index()?;
        let held = self.proposals.get(id).unwrap();
        Ok(held.targets.iter().filter(|(r, t)| self.paragraphs.iter().find(|p| p.r == *r).map(|p| &p.fingerprint) != Some(&t.2)).map(|(r, _)| r.clone()).collect())
    }

    fn pending(&mut self, id: &str) -> Res<()> {
        self.proposal(id)?;
        let status = self.proposals.get(id).unwrap().status().to_owned();
        if status != "pending" {
            return Err(ToolError::new("PROPOSAL_CLOSED", format!("Proposal is {status}. Stage a new proposal.")));
        }
        let stale_refs = self.stale_refs(id)?;
        if !stale_refs.is_empty() {
            return Err(stale(&stale_refs));
        }
        Ok(())
    }

    pub fn review(&mut self, id: &str) -> Res<Value> {
        self.proposal(id)?;
        let pending = self.proposals.get(id).unwrap().status() == "pending";
        let refs = if pending { self.stale_refs(id)? } else { Vec::new() };
        let mut v = self.proposals.get(id).unwrap().value.clone();
        v.insert("staleRefs".into(), json!(refs));
        Ok(Value::Object(v))
    }

    pub fn list_proposals(&mut self) -> Res<Value> {
        self.index()?;
        Ok(Value::Array(
            self.proposals
                .values()
                .map(|h| {
                    let note = h.value.get("note").and_then(Value::as_str).unwrap_or("");
                    json!({ "id": h.value["id"], "author": h.value["author"], "note": crate::js::slice_utf16(note, 120), "status": h.status(), "changes": h.changes.len() })
                })
                .collect(),
        ))
    }

    /// The proposal's replacements, applied last-first per paragraph.
    fn edits_of(&self, id: &str, author: Option<(String, String)>) -> Vec<Applied> {
        let held = self.proposals.get(id).unwrap();
        let mut changes = held.changes.clone();
        changes.sort_by(|a, b| a.r.cmp(&b.r).then(b.start.cmp(&a.start)));
        changes
            .iter()
            .map(|c| {
                let (story, para_id, _) = held.targets.get(&c.r).cloned().unwrap_or_default();
                Applied { story, para_id, start: c.start, end: c.start + c.old.encode_utf16().count(), old: c.old.clone(), text: c.new.clone(), author: author.clone() }
            })
            .collect()
    }

    /// A copy of the session: the source seeded again with every accepted
    /// edit replayed (Node's `fork`).
    fn fork(&mut self) -> Res<EditingDoc> {
        self.index()?;
        let draft = seeded(&self.source)?;
        for a in &self.applied {
            apply(&draft, a)?;
        }
        Ok(draft)
    }

    pub fn accept(&mut self, id: &str, args: &Value) -> Res<Value> {
        self.pending(id)?;
        let tracked = args.get("tracked").is_some_and(crate::js::truthy);
        let date = args.get("date").and_then(Value::as_str);
        if tracked && !date.is_some_and(date_ok) {
            return Err(ToolError::new("INVALID_ARGUMENT", "Tracked acceptance requires an ISO date."));
        }
        let draft = self.fork()?;
        self.pending(id)?;
        let author = tracked.then(|| (self.proposals.get(id).unwrap().value["author"].as_str().unwrap_or("").to_owned(), date.unwrap_or("").to_owned()));
        let edits = self.edits_of(id, author);
        for a in &edits {
            apply(&draft, a)?;
        }
        self.pending(id)?;
        self.doc = draft;
        self.applied.extend(edits);
        self.epoch += 1;
        let held = self.proposals.get_mut(id).unwrap();
        held.value.insert("status".into(), json!("accepted"));
        Ok(json!({ "id": id, "status": "accepted", "tracked": tracked, "changes": held.changes.len() }))
    }

    pub fn reject(&mut self, id: &str) -> Res<Value> {
        self.proposal(id)?;
        let held = self.proposals.get_mut(id).unwrap();
        if held.status() != "pending" {
            return Err(ToolError::new("PROPOSAL_CLOSED", format!("Proposal is {}.", held.status())));
        }
        held.value.insert("status".into(), json!("rejected"));
        Ok(json!({ "id": id, "status": "rejected" }))
    }

    /// `render(page, proposal)`: Node renders through its canvas renderer
    /// (render.ts); this broker has none (see `office/mod.rs`), as Node
    /// itself answers when render.ts fails to load.
    pub fn render(&mut self, page: f64) -> Res<(Value, Vec<u8>)> {
        integer(page, 1.0, 100000.0, "page")?;
        Err(ToolError::new("UNSUPPORTED", "No renderer configured. Supply a DocumentRenderer or use the MCP server."))
    }

    pub fn export(&mut self, id: Option<&str>) -> Res<Vec<u8>> {
        if let Some(id) = id {
            self.pending(id)?;
        }
        let draft = self.fork()?;
        let mut edits = self.applied.clone();
        if let Some(id) = id {
            self.pending(id)?;
            for a in self.edits_of(id, None) {
                apply(&draft, &a)?;
                edits.push(a);
            }
        }
        save(&draft, &self.source, &edits)
    }

    pub fn verify(&mut self, id: Option<&str>) -> Res<Value> {
        let bytes = self.export(id)?;
        let mut reopened = Docx::open(&bytes, "document.docx")?;
        let paragraphs = reopened.overview()?["paragraphs"].clone();
        Ok(json!({ "reopened": true, "bytes": bytes.len(), "paragraphs": paragraphs, "checks": ["save", "reopen"], "unchecked": ["visual fidelity", "semantic correctness"] }))
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.proposals.clear();
        self.cursors.clear();
        self.matches.clear();
        self.identities.clear();
        self.paragraphs.clear();
    }
}

fn stale(refs: &[String]) -> ToolError {
    ToolError::with("STALE_TARGET", "Target text or formatting changed. Read these refs and propose again.", json!({ "refs": refs }))
}

/// Saves a session seeded from `source`: the package as parsed, with the
/// session's paragraph save plan (each changed paragraph spliced into its
/// part, patched into its source XML where a patch expresses the change)
/// written by docx-parse's S13 package writer. The writer takes a changed
/// paragraph no patch expresses from the model, so the edits are made on
/// the parsed model too (`model::replace`).
fn save(doc: &EditingDoc, source: &[u8], edits: &[Applied]) -> Res<Vec<u8>> {
    use sha2::{Digest, Sha256};
    let plan = doc.paragraph_save_plan();
    let mut spliced = Vec::new();
    for part in &plan.spliced_parts {
        if !part.inserted.is_empty() || !part.removed.is_empty() {
            return Err(ToolError::new("UNSUPPORTED", "Paragraphs were added or removed; this save writes text edits only."));
        }
        spliced.push(json!({
            "part": part.part,
            "sha256": part.sha256,
            "paragraphs": part.paragraphs.iter().map(|(o, _)| o).collect::<Vec<_>>(),
            "changed": part.changed,
            "paragraphPatches": part.paragraph_patches,
            "legacyParagraphs": part.legacy_paragraphs,
            "tablePatches": part.table_patches,
        }));
    }
    let envelope = docx_parse::parse_docx_s9_wire(source, docx_parse::S9ParseOptions { source_ordinals: true, ..Default::default() }).map_err(engine)?;
    let package = envelope.document.package;
    let mut document = serde_json::to_value(&package.document).map_err(engine)?;
    let mut headers = serde_json::to_value(package.header_entries.clone().unwrap_or_default()).map_err(engine)?;
    let mut footers = serde_json::to_value(package.footer_entries.clone().unwrap_or_default()).map_err(engine)?;
    // where each session paragraph came from: its part and w:p ordinal
    let identities = doc.paragraph_identities();
    let mut next_revision = 90_000;
    for a in edits {
        let source = identities.paragraphs.iter().find_map(|p| match &p.paragraph {
            docx_edit::ParagraphRef::Session { story, para_id } if *story == a.story && *para_id == a.para_id => p.source.as_ref(),
            _ => None,
        });
        let Some(source) = source else {
            return Err(ToolError::new("UNSUPPORTED", "The edited paragraph has no source paragraph to save into."));
        };
        let container = if let Some(rid) = a.story.strip_prefix("hf:") {
            let entry = headers.as_array_mut().into_iter().flatten().chain(footers.as_array_mut().into_iter().flatten()).find(|e| e[0] == rid);
            match entry {
                Some(e) => &mut e[1]["content"],
                None => return Err(ToolError::new("UNSUPPORTED", "The edited header or footer is not in the package.")),
            }
        } else if source.part_uri.trim_start_matches('/') == "word/document.xml" {
            &mut document["content"]
        } else {
            return Err(ToolError::new("UNSUPPORTED", "Edits in this story cannot be saved yet."));
        };
        let Some(paragraph) = model::find(container, source.paragraph_ordinal) else {
            return Err(ToolError::new("UNSUPPORTED", "The edited paragraph is not in the parsed package."));
        };
        let author = a.author.as_ref().map(|(name, date)| {
            next_revision += 2;
            (name.as_str(), date.as_str(), next_revision)
        });
        model::replace(paragraph, a.start, a.end, &a.old, &a.text, author)?;
    }
    if let Some(sections) = &package.document.sections {
        let content = document["content"].as_array().cloned().unwrap_or_default();
        let sections: Vec<Value> = sections.iter().map(|s| json!({ "id": s.id, "properties": s.properties, "content": content.get(s.content_start..s.content_end).unwrap_or(&[]) })).collect();
        document["sections"] = Value::Array(sections);
    }
    let request = json!({
        "determinism": { "seed": format!("{:x}", Sha256::digest(source)), "now": "1970-01-01T00:00:00.000Z" },
        "document": document,
        "headerEntries": headers,
        "footerEntries": footers,
        "footnotes": package.footnotes.unwrap_or_default(),
        "endnotes": package.endnotes.unwrap_or_default(),
        "footnoteSeparators": package.footnote_separators.unwrap_or_default(),
        "endnoteSeparators": package.endnote_separators.unwrap_or_default(),
        "relationshipEntries": package.relationship_entries,
        "numbering": package.numbering,
        "options": { "updateModifiedDate": false },
        "paragraphIds": {
            "assignments": plan.assignments.iter().map(|(part, ordinal, id)| json!({ "part": part, "ordinal": ordinal, "paraId": id })).collect::<Vec<_>>(),
            "patchedParts": plan.patched_parts.iter().map(|(part, ids)| json!({ "part": part, "paraIds": ids })).collect::<Vec<_>>(),
            "splicedParts": spliced,
        },
    });
    let request: docx_parse::serializer::S13SaveRequest = serde_json::from_value(request).map_err(engine)?;
    docx_parse::serializer::write_docx_s13(request, source).map_err(engine)
}

/// Text replacements on the parsed document model (its JSON form): the
/// same edits the session took, in the same paragraph coordinates (text in
/// UTF-16 units, a tab or break one unit, bookmarks and range marks none).
mod model {
    use serde_json::{json, Value};

    use super::{units, Res, ToolError};

    fn unsupported() -> ToolError {
        ToolError::new("UNSUPPORTED", "This edit touches content the save cannot rewrite (a field, embed or nested structure).")
    }

    /// The paragraph with `sourceOrdinal` `ordinal`, anywhere in `blocks`.
    pub fn find(blocks: &mut Value, ordinal: u32) -> Option<&mut Value> {
        for b in blocks.as_array_mut()? {
            match b.get("type").and_then(Value::as_str) {
                Some("paragraph") if b.get("sourceOrdinal").and_then(Value::as_f64) == Some(ordinal as f64) => return Some(b),
                Some("table") => {
                    for row in b.get_mut("rows").and_then(Value::as_array_mut).into_iter().flatten() {
                        for cell in row.get_mut("cells").and_then(Value::as_array_mut).into_iter().flatten() {
                            if let Some(p) = find(&mut cell["content"], ordinal) {
                                return Some(p);
                            }
                        }
                    }
                }
                Some("blockSdt") => {
                    if let Some(p) = find(&mut b["content"], ordinal) {
                        return Some(p);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// A text node: (top-level item index, run path, content index, start unit, text units).
    struct Leaf {
        item: usize,
        child: Option<(&'static str, usize)>,
        node: usize,
        start: usize,
        text: Vec<u16>,
    }

    fn run_units(run: &Value, item: usize, child: Option<(&'static str, usize)>, at: &mut usize, leaves: &mut Vec<Leaf>) -> Res<()> {
        for (node, c) in run.get("content").and_then(Value::as_array).into_iter().flatten().enumerate() {
            match c.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => {
                    let text = units(c.get("text").and_then(Value::as_str).unwrap_or(""));
                    let len = text.len();
                    leaves.push(Leaf { item, child, node, start: *at, text });
                    *at += len;
                }
                "tab" | "break" | "noBreakHyphen" | "softHyphen" => *at += 1,
                _ => return Err(unsupported()),
            }
        }
        Ok(())
    }

    fn leaves(paragraph: &Value) -> Res<Vec<Leaf>> {
        let mut out = Vec::new();
        let mut at = 0;
        for (item, c) in paragraph.get("content").and_then(Value::as_array).into_iter().flatten().enumerate() {
            match c.get("type").and_then(Value::as_str).unwrap_or("") {
                "run" => run_units(c, item, None, &mut at, &mut out)?,
                kind @ ("hyperlink" | "insertion" | "deletion") => {
                    let key = if kind == "hyperlink" { "children" } else { "content" };
                    for (k, child) in c.get(key).and_then(Value::as_array).into_iter().flatten().enumerate() {
                        match child.get("type").and_then(Value::as_str).unwrap_or("") {
                            "run" => run_units(child, item, Some((key, k)), &mut at, &mut out)?,
                            "bookmarkStart" | "bookmarkEnd" => {}
                            _ => return Err(unsupported()),
                        }
                    }
                }
                "bookmarkStart" | "bookmarkEnd" | "commentRangeStart" | "commentRangeEnd" | "commentRange" | "moveFromRangeStart" | "moveToRangeStart" | "moveFromRangeEnd" | "moveToRangeEnd"
                | "permStart" | "permEnd" | "proofErr" => {}
                _ => return Err(unsupported()),
            }
        }
        Ok(out)
    }

    fn run_at<'a>(paragraph: &'a mut Value, leaf: &Leaf) -> &'a mut Value {
        let item = &mut paragraph["content"][leaf.item];
        match leaf.child {
            Some((key, k)) => &mut item[key][k],
            None => item,
        }
    }

    /// Replaces `[start, end)` (which holds `old`) with `text`; the new text
    /// takes the formatting of the first replaced unit. With an author, the
    /// old text stays as a Word deletion and the new is an insertion.
    pub fn replace(paragraph: &mut Value, start: usize, end: usize, old: &str, text: &str, author: Option<(&str, &str, u64)>) -> Res<()> {
        let leaves = leaves(paragraph)?;
        // the range must be text only, and hold `old`
        let covering: Vec<&Leaf> = leaves.iter().filter(|l| l.start < end && l.start + l.text.len() > start).collect();
        let held: Vec<u16> = covering.iter().flat_map(|l| l.text[start.max(l.start) - l.start..end.min(l.start + l.text.len()) - l.start].to_vec()).collect();
        if held != units(old) {
            return Err(unsupported());
        }
        let first = covering.first().ok_or_else(unsupported)?;
        if let Some((name, date, id)) = author {
            // tracked: only a range inside one top-level run is written as Word's ins/del
            if covering.len() != 1 || first.child.is_some() {
                return Err(unsupported());
            }
            let run = paragraph["content"][first.item].clone();
            let formatting = run.get("formatting").cloned();
            let nodes = run["content"].as_array().cloned().unwrap_or_default();
            let (a, b) = (start - first.start, end - first.start);
            let piece = |content: Vec<Value>| {
                let mut r = json!({ "type": "run", "content": content });
                if let Some(f) = &formatting {
                    r["formatting"] = f.clone();
                }
                r
            };
            let text_node = |u: &[u16]| json!({ "type": "text", "text": String::from_utf16_lossy(u) });
            let mut before: Vec<Value> = nodes[..first.node].to_vec();
            let mut after: Vec<Value> = nodes[first.node + 1..].to_vec();
            if a > 0 {
                before.push(text_node(&first.text[..a]));
            }
            if b < first.text.len() {
                after.insert(0, text_node(&first.text[b..]));
            }
            let mut replacement = Vec::new();
            if !before.is_empty() {
                replacement.push(piece(before));
            }
            replacement.push(json!({ "type": "deletion", "info": { "id": id, "author": name, "date": date }, "content": [piece(vec![text_node(&first.text[a..b])])] }));
            if !text.is_empty() {
                replacement.push(json!({ "type": "insertion", "info": { "id": id + 1, "author": name, "date": date }, "content": [piece(vec![json!({ "type": "text", "text": text })])] }));
            }
            if !after.is_empty() {
                replacement.push(piece(after));
            }
            let items = paragraph["content"].as_array_mut().unwrap();
            items.splice(first.item..first.item + 1, replacement);
            return Ok(());
        }
        // plain: the first leaf takes the new text in place of its part;
        // the others lose theirs
        let mut first_done = false;
        for l in covering.iter().rev() {
            let (a, b) = (start.max(l.start) - l.start, end.min(l.start + l.text.len()) - l.start);
            let mut t = l.text[..a].to_vec();
            if l.start == first.start && l.item == first.item && l.child == first.child && l.node == first.node {
                t.extend(units(text));
                first_done = true;
            }
            t.extend_from_slice(&l.text[b..]);
            run_at(paragraph, l)["content"][l.node]["text"] = json!(String::from_utf16_lossy(&t));
        }
        if !first_done {
            return Err(unsupported());
        }
        Ok(())
    }
}
