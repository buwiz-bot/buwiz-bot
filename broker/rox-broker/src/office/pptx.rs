//! `PptxAgentPresentation` (agents/pptx.ts) over `betteroffice-pptx-edit`'s
//! `DeckSession`: `read_content`, `validate_edits` and `apply_edits` take
//! and give the JSON the wasm binding's `readContent`, `validateEdits` and
//! `applyEdits` do, so Node's requests and answers carry over.

use pptx_edit::{DeckSession, EditCtx, EditRequest, ReadRequest, ShapeDraft};
use serde_json::{json, Map, Value};

use super::proto::{engine, find_all, integer, opt_num, plain_text, slice, sorted, text_window, units, unwrap, Ordered, Res, ToolError};
use super::prototype::{Engine, Grep, Proto};

/// A client id as `openPresentation` draws one: random, positive, safe.
fn client_id() -> u64 {
    let b = rox_v1::keys::random::<8>();
    (u64::from_le_bytes(b) % 9_007_199_254_740_990) + 1
}

fn outcome<T: serde::Serialize>(r: Result<T, pptx_edit::EditRefusal>) -> Res<Value> {
    let s = pptx_edit::outcome_json(&r).map_err(engine)?;
    unwrap(sorted(serde_json::from_str(&s).map_err(engine)?))
}

struct Deck {
    session: DeckSession,
    source: Vec<u8>,
}

fn request(steps: &[Value], version: &str) -> Res<EditRequest> {
    serde_json::from_value(json!({ "expectVersion": version, "steps": steps, "source": "agent" })).map_err(engine)
}

impl Engine for Deck {
    fn format(&self) -> &'static str {
        "pptx"
    }
    fn current_version(&self) -> String {
        self.session.version().to_string()
    }
    fn validate(&self, steps: &[Value], version: &str) -> Res<()> {
        outcome(self.session.validate_edits(&request(steps, version)?).map_err(engine)?).map(|_| ())
    }
    fn apply(&mut self, steps: &[Value], version: &str) -> Res<Value> {
        let r = outcome(self.session.apply_edits(&request(steps, version)?).map_err(engine)?)?;
        let len = |v: &Value| v.as_array().map_or(0, Vec::len);
        Ok(json!({ "applied": r["applied"], "version": r["version"], "changedSlides": len(&r["changedSlides"]), "changedStories": len(&r["changedStories"]) }))
    }
    fn save(&self, steps: Option<&[Value]>) -> Res<Vec<u8>> {
        let Some(steps) = steps else { return self.session.save().map_err(engine) };
        let draft = DeckSession::open_from_update_with_source(&self.session.encode_state_as_update_v1(), &self.source, client_id()).map_err(engine)?;
        let version = draft.version().to_string();
        outcome(draft.apply_edits(&request(steps, &version)?).map_err(engine)?)?;
        draft.save().map_err(engine)
    }
    fn reopen(&self, bytes: &[u8]) -> Res<()> {
        DeckSession::open(bytes, client_id()).map(|_| ()).map_err(engine)
    }
}

struct Para {
    r: String,
    slide: i64,
    story: Value,
    start: usize,
    end: usize,
    editable: Value,
}

#[derive(Clone)]
struct Held {
    r: String,
    slide: i64,
    target: Value,
    start: usize,
    paragraph_start: usize,
    text: String,
    version: String,
}

pub struct Pptx {
    pub name: String,
    deck: Deck,
    proto: Proto,
    refs: Ordered<String>,
    matches: Ordered<Held>,
    next_match: u64,
    next_ref: u64,
}

impl Pptx {
    pub fn open(bytes: &[u8], name: &str) -> Res<Self> {
        let session = DeckSession::open(bytes, client_id()).map_err(engine)?;
        Ok(Self { name: name.to_owned(), deck: Deck { session, source: bytes.to_vec() }, proto: Proto::default(), refs: Ordered::default(), matches: Ordered::default(), next_match: 1, next_ref: 1 })
    }

    fn content(&self) -> Res<Value> {
        self.proto.alive()?;
        let req: ReadRequest = serde_json::from_value(json!({})).map_err(engine)?;
        outcome(self.deck.session.read_content(&req).map_err(engine)?)
    }

    fn paragraphs(&mut self, read: &Value) -> Vec<Para> {
        let slides = read["slides"].as_array().cloned().unwrap_or_default();
        let mut active = Vec::new();
        let mut out = Vec::new();
        for story in read["stories"].as_array().cloned().unwrap_or_default() {
            for p in story["paragraphs"].as_array().cloned().unwrap_or_default() {
                let key = crate::js::stringify(&json!([story["storyId"], p["paragraphId"]]));
                active.push(key.clone());
                let r = match self.refs.get(&key) {
                    Some(r) => r.clone(),
                    None => {
                        let r = format!("p{}", self.next_ref);
                        self.next_ref += 1;
                        self.refs.set(key, r.clone());
                        r
                    }
                };
                let slide = slides.iter().position(|s| s["id"] == story["slideId"]).map_or(0, |i| i as i64 + 1);
                out.push(Para { r, slide, story: story.clone(), start: p["start"].as_u64().unwrap_or(0) as usize, end: p["end"].as_u64().unwrap_or(0) as usize, editable: p["editable"].clone() });
            }
        }
        self.refs.retain(|k, _| active.iter().any(|a| a == k));
        out
    }

    fn reference(p: &Para) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("ref".into(), json!(p.r));
        m.insert("slide".into(), json!(p.slide));
        m.insert("slideId".into(), p.story["slideId"].clone());
        m.insert("shapeId".into(), p.story["shapeId"].clone());
        m.insert("story".into(), p.story["storyId"].clone());
        m
    }

    fn check_story(story: Option<&str>, read: &Value) -> Res<()> {
        if let Some(s) = story.filter(|s| !s.is_empty()) {
            if !read["stories"].as_array().is_some_and(|all| all.iter().any(|x| x["storyId"] == s)) {
                return Err(ToolError::new("UNKNOWN_STORY", "Use a story from outline."));
            }
        }
        Ok(())
    }

    pub fn overview(&mut self) -> Res<Value> {
        let read = self.content()?;
        let paragraphs = self.paragraphs(&read).len();
        Ok(json!({
            "name": self.name,
            "format": "pptx",
            "version": read["version"],
            "slides": read["slides"].as_array().map_or(0, Vec::len),
            "paragraphs": paragraphs,
            "capabilities": { "grep": "literal", "proposals": "paragraph-local text replacements", "render": false, "export": true, "prototype": true },
            "workflow": "outline lists slide text; grep for exact text; read its ref; propose with match and newText; review; verify; export. This prototype edits slide text, not notes, layout, or media.",
        }))
    }

    pub fn list(&mut self, args: &Value) -> Res<Value> {
        if args.get("headingsOnly").is_some_and(crate::js::truthy) {
            return Err(ToolError::new("UNSUPPORTED", "PPTX outline lists slide paragraphs; omit headingsOnly."));
        }
        let read = self.content()?;
        let story = args.get("story").and_then(Value::as_str);
        Self::check_story(story, &read)?;
        let offset = integer(opt_num(args, "offset", 0.0), 0.0, 9007199254740991.0, "offset")? as usize;
        let limit = integer(opt_num(args, "limit", 30.0), 1.0, 100.0, "limit")? as usize;
        let story = story.filter(|s| !s.is_empty());
        let items: Vec<Para> = self.paragraphs(&read).into_iter().filter(|p| story.is_none_or(|s| p.story["storyId"] == s)).collect();
        let shown: Vec<Value> = items
            .iter()
            .skip(offset)
            .take(limit)
            .map(|p| {
                let text = units(p.story["text"].as_str().unwrap_or(""));
                let mut m = Self::reference(p);
                m.insert("text".into(), json!(slice(&text, p.start, p.end.min(p.start + 120))));
                m.insert("length".into(), json!(p.end as i64 - p.start as i64));
                m.insert("editable".into(), p.editable.clone());
                Value::Object(m)
            })
            .collect();
        Ok(json!({ "version": read["version"], "items": shown, "total": items.len(), "nextOffset": if offset + limit < items.len() { json!(offset + limit) } else { Value::Null } }))
    }

    pub fn grep(&mut self, args: &Value) -> Res<Value> {
        let g = Grep::from(args);
        self.proto.check_query(&g)?;
        let read = self.content()?;
        Self::check_story(g.story, &read)?;
        let story = g.story.filter(|s| !s.is_empty());
        let paragraphs = self.paragraphs(&read);
        let mut hits: Vec<(usize, usize, String)> = Vec::new();
        'outer: for (i, p) in paragraphs.iter().enumerate() {
            if story.is_some_and(|s| p.story["storyId"] != s) {
                continue;
            }
            let all = units(p.story["text"].as_str().unwrap_or(""));
            let text = &all[p.start.min(all.len())..p.end.min(all.len())];
            for (a, b) in find_all(text, g.query, g.case_sensitive.unwrap_or(false), 0) {
                hits.push((i, p.start + a, slice(text, a, b)));
                if hits.len() > 10000 {
                    break 'outer;
                }
            }
        }
        let clipped = hits.len() > 10000;
        let shown: Vec<Value> = hits
            .iter()
            .take(10000)
            .map(|(i, start, text)| {
                let p = &paragraphs[*i];
                let all = units(p.story["text"].as_str().unwrap_or(""));
                let tl = text.encode_utf16().count();
                let mut m = Self::reference(p);
                m.insert("start".into(), json!(start - p.start));
                m.insert("text".into(), json!(text));
                m.insert("context".into(), json!(slice(&all, p.start.max(start.saturating_sub(80)), p.end.min(start + tl + 80))));
                Value::Object(m)
            })
            .collect();
        let mut page = self.proto.search_page(&self.deck, &g, &shown, clipped)?;
        let version = read["version"].as_str().unwrap_or("").to_owned();
        let matches: Vec<Value> = page["matches"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|mut hit| {
                let start = hit["start"].as_u64().unwrap_or(0) as usize;
                let (i, _, _) = hits.iter().find(|(i, s, _)| paragraphs[*i].r == hit["ref"].as_str().unwrap_or("") && s - paragraphs[*i].start == start).cloned().unwrap_or((0, 0, String::new()));
                let p = &paragraphs[i];
                let id = format!("{}:match{}", self.proto.prefix, self.next_match);
                self.next_match += 1;
                if self.matches.len() >= 1024 {
                    self.matches.delete_first();
                }
                let target = json!({ "slideId": p.story["slideId"], "shapeId": p.story["shapeId"], "storyId": p.story["storyId"] });
                self.matches.set(
                    id.clone(),
                    Held { r: p.r.clone(), slide: p.slide, target, start: p.start + start, paragraph_start: p.start, text: hit["text"].as_str().unwrap_or("").to_owned(), version: version.clone() },
                );
                hit["match"] = json!(id);
                hit
            })
            .collect();
        page.insert("matches".into(), Value::Array(matches));
        Ok(Value::Object(page))
    }

    pub fn read(&mut self, r: &str, args: &Value) -> Res<Value> {
        let read = self.content()?;
        let paragraphs = self.paragraphs(&read);
        let Some(p) = paragraphs.iter().find(|p| p.r == r) else {
            return Err(ToolError::new("UNKNOWN_REF", "Use a ref from outline or grep."));
        };
        let all = units(p.story["text"].as_str().unwrap_or(""));
        let text = &all[p.start.min(all.len())..p.end.min(all.len())];
        let mut m = Self::reference(p);
        m.insert("version".into(), read["version"].clone());
        let window = text_window(text, args.get("start").and_then(Value::as_f64), args.get("length").and_then(Value::as_f64))?;
        for (k, v) in window.as_object().unwrap() {
            m.insert(k.clone(), v.clone());
        }
        m.insert("editable".into(), p.editable.clone());
        Ok(Value::Object(m))
    }

    pub fn propose(&mut self, input: &Value) -> Res<Value> {
        self.proto.alive()?;
        let edits = input["edits"].as_array().cloned().unwrap_or_default();
        if edits.is_empty() || edits.len() > 32 {
            return Err(ToolError::new("INVALID_ARGUMENT", "Supply 1–32 replacements."));
        }
        let version = self.deck.current_version();
        let mut changes = Vec::new();
        let mut steps = Vec::new();
        for edit in &edits {
            let Some(id) = edit.get("match").and_then(Value::as_str) else {
                return Err(ToolError::new("UNSUPPORTED", "PPTX replacements require a match ID from grep."));
            };
            let Some(hit) = self.matches.get(id).cloned() else {
                return Err(ToolError::new("UNKNOWN_MATCH", "Match expired or belongs to another deck. Grep again."));
            };
            if hit.version != version {
                return Err(ToolError::new("STALE_TARGET", "Presentation changed. Grep for fresh matches."));
            }
            let new_text = edit["newText"].as_str().unwrap_or("");
            plain_text(new_text)?;
            if new_text.contains(['\r', '\n', '\u{fffc}']) || new_text.encode_utf16().count() > 16000 {
                return Err(ToolError::new("INVALID_ARGUMENT", "Replacement must be plain text in one paragraph, up to 16000 UTF-16 units."));
            }
            changes.push(json!({ "ref": hit.r, "slide": hit.slide, "story": hit.target["storyId"], "start": hit.start - hit.paragraph_start, "oldText": hit.text, "newText": new_text }));
            let mut target = Map::new();
            target.insert("kind".into(), json!("range"));
            for (k, v) in hit.target.as_object().unwrap() {
                target.insert(k.clone(), v.clone());
            }
            target.insert("start".into(), json!(hit.start));
            target.insert("end".into(), json!(hit.start + hit.text.encode_utf16().count()));
            steps.push(json!({ "op": "replaceText", "target": target, "text": new_text, "expect": { "text": hit.text } }));
        }
        self.proto.stage(&self.deck, input, changes, steps, &version)
    }

    pub fn render(&self) -> Res<(Value, Vec<u8>)> {
        self.proto.render_unsupported("pptx").map(|v| (v, Vec::new()))
    }
    pub fn review(&self, id: &str) -> Res<Value> {
        self.proto.review(&self.deck, id)
    }
    pub fn list_proposals(&self) -> Res<Value> {
        self.proto.list_proposals(&self.deck)
    }
    pub fn accept(&mut self, id: &str, args: &Value) -> Res<Value> {
        self.proto.accept(&mut self.deck, id, args)
    }
    pub fn reject(&mut self, id: &str) -> Res<Value> {
        self.proto.reject(id)
    }
    pub fn export(&self, id: Option<&str>) -> Res<Vec<u8>> {
        self.proto.export(&self.deck, id)
    }
    pub fn verify(&self, id: Option<&str>) -> Res<Value> {
        self.proto.verify(&self.deck, id)
    }
    pub fn close(&mut self) {
        if self.proto.closed {
            return;
        }
        self.proto.close();
        self.refs.clear();
        self.matches.clear();
    }
}

/// EMU rectangles for a 4:3 seed slide (9144000 x 6858000 EMU).
const TITLE_RECT: (i64, i64, i64, i64) = (457200, 274320, 8229600, 822960);
const BODY_RECT: (i64, i64, i64, i64) = (457200, 1371600, 8229600, 5029200);

fn rect(r: (i64, i64, i64, i64)) -> Value {
    json!({ "x": r.0, "y": r.1, "width": r.2, "height": r.3 })
}

/// create.ts's `createPptxBytes`: each slide inserted into the blank seed
/// with a title box, a bullets box and its notes.
pub fn create(seed: &[u8], slides: &[Value]) -> Res<Vec<u8>> {
    let session = DeckSession::open(seed, client_id()).map_err(engine)?;
    let ctx = EditCtx::local("wasm");
    for (index, slide) in slides.iter().enumerate() {
        let receipt = session.insert_slide(&ctx, index as u32, None).map_err(engine)?;
        let slide_id = serde_json::to_value(&receipt).map_err(engine)?["slideId"].as_str().unwrap_or("").to_owned();
        let title = slide.get("title").and_then(Value::as_str).unwrap_or("");
        if !title.is_empty() {
            let draft: ShapeDraft = serde_json::from_value(json!({ "name": "Title", "rect": rect(TITLE_RECT), "text": title, "style": { "fontSizePt": 32, "bold": true } })).map_err(engine)?;
            session.add_text_box(&ctx, &slide_id, &draft).map_err(engine)?;
        }
        let bullets: Vec<&str> = slide.get("bullets").and_then(Value::as_array).map(|b| b.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        if !bullets.is_empty() {
            let draft: ShapeDraft = serde_json::from_value(json!({ "name": "Body", "rect": rect(BODY_RECT), "text": bullets.join("\n"), "style": { "fontSizePt": 18 } })).map_err(engine)?;
            session.add_text_box(&ctx, &slide_id, &draft).map_err(engine)?;
        }
        let notes = slide.get("notes").and_then(Value::as_str).unwrap_or("");
        if !notes.is_empty() {
            session.set_slide_notes(&ctx, &slide_id, notes).map_err(engine)?;
        }
    }
    session.save().map_err(engine)
}
