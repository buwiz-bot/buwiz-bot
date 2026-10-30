//! `PrototypeDocument` (agents/prototype.ts): the proposals, review,
//! accept/reject, export/verify and grep pages the xlsx and pptx agents
//! share, over an [`Engine`] each format implements on its crate.

use serde_json::{json, Map, Value};

use super::proto::{integer, json_len, uuid, Ordered, Res, ToolError};

/// What a format gives `PrototypeDocument` (its abstract members).
pub trait Engine {
    fn format(&self) -> &'static str;
    fn current_version(&self) -> String;
    fn validate(&self, steps: &[Value], version: &str) -> Res<()>;
    fn apply(&mut self, steps: &[Value], version: &str) -> Res<Value>;
    fn save(&self, steps: Option<&[Value]>) -> Res<Vec<u8>>;
    fn reopen(&self, bytes: &[u8]) -> Res<()>;
}

struct Held {
    value: Map<String, Value>,
    steps: Vec<Value>,
}

impl Held {
    fn status(&self) -> &str {
        self.value.get("status").and_then(Value::as_str).unwrap_or("")
    }
    fn version(&self) -> &str {
        self.value.get("version").and_then(Value::as_str).unwrap_or("")
    }
}

struct Cursor {
    signature: String,
    version: String,
    offset: usize,
}

pub struct Proto {
    proposals: Ordered<Held>,
    cursors: Ordered<Cursor>,
    next_id: u64,
    pub prefix: String,
    pub closed: bool,
}

impl Default for Proto {
    fn default() -> Self {
        Self { proposals: Ordered::default(), cursors: Ordered::default(), next_id: 1, prefix: uuid(), closed: false }
    }
}

/// The grep arguments `searchPage` signs a cursor with.
pub struct Grep<'a> {
    pub query: &'a str,
    pub case_sensitive: Option<bool>,
    pub story: Option<&'a str>,
    pub limit: Option<f64>,
    pub cursor: Option<&'a str>,
}

impl Grep<'_> {
    pub fn from(args: &Value) -> Grep<'_> {
        Grep {
            query: args.get("query").and_then(Value::as_str).unwrap_or(""),
            case_sensitive: args.get("caseSensitive").and_then(Value::as_bool),
            story: args.get("story").and_then(Value::as_str),
            limit: args.get("limit").and_then(Value::as_f64),
            cursor: args.get("cursor").and_then(Value::as_str),
        }
    }
}

impl Proto {
    pub fn alive(&self) -> Res<()> {
        if self.closed {
            return Err(ToolError::new("CLOSED", "This document is closed. Open it again."));
        }
        Ok(())
    }

    /// The next of the ids proposals and cursors share.
    pub fn next(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn stage(&mut self, e: &dyn Engine, input: &Value, changes: Vec<Value>, steps: Vec<Value>, version: &str) -> Res<Value> {
        self.alive()?;
        let author = input.get("author").and_then(Value::as_str).unwrap_or("");
        let note = input.get("note").and_then(Value::as_str);
        let units = |s: &str| s.encode_utf16().count();
        if author.is_empty() || units(author) > 200 || note.map_or(0, units) > 2000 || changes.is_empty() || changes.len() > 32 {
            return Err(ToolError::new("INVALID_ARGUMENT", "Supply an author (1–200 characters), note (up to 2000), and 1–32 edits."));
        }
        if json_len(&Value::Array(changes.clone())) > 32000 {
            return Err(ToolError::new("EDIT_LIMIT", "Combined proposal changes must fit in 32000 UTF-16 units."));
        }
        e.validate(&steps, version)?;
        if self.proposals.len() >= 64 {
            let old = self.proposals.iter().find(|(_, held)| held.status() != "pending").map(|(k, _)| k.clone());
            match old {
                Some(k) => {
                    self.proposals.delete(&k);
                }
                None => return Err(ToolError::new("PROPOSAL_LIMIT", "Reject or discard a proposal before staging more than 64.")),
            }
        }
        let id = format!("proposal{}", self.next());
        let mut value = Map::new();
        value.insert("id".into(), json!(id));
        value.insert("author".into(), json!(author));
        value.insert("note".into(), json!(note.unwrap_or("")));
        value.insert("status".into(), json!("pending"));
        value.insert("version".into(), json!(version));
        value.insert("changes".into(), Value::Array(changes));
        value.insert("stale".into(), json!(false));
        let out = Value::Object(value.clone());
        self.proposals.set(id, Held { value, steps });
        Ok(out)
    }

    fn held(&self, id: &str) -> Res<&Held> {
        self.alive()?;
        self.proposals.get(id).ok_or_else(|| ToolError::new("UNKNOWN_PROPOSAL", "Stage a proposal first."))
    }

    fn pending(&self, e: &dyn Engine, id: &str) -> Res<&Held> {
        let held = self.held(id)?;
        if held.status() != "pending" {
            return Err(ToolError::new("PROPOSAL_CLOSED", format!("Proposal is {}.", held.status())));
        }
        if held.version() != e.current_version() {
            return Err(ToolError::new("STALE_TARGET", "The document changed. Read fresh targets and create a new proposal."));
        }
        Ok(held)
    }

    pub fn review(&self, e: &dyn Engine, id: &str) -> Res<Value> {
        let held = self.held(id)?;
        let stale = held.status() == "pending" && held.version() != e.current_version();
        let mut v = held.value.clone();
        v.insert("stale".into(), json!(stale));
        Ok(Value::Object(v))
    }

    pub fn list_proposals(&self, e: &dyn Engine) -> Res<Value> {
        self.alive()?;
        let current = e.current_version();
        Ok(Value::Array(
            self.proposals
                .values()
                .map(|held| {
                    let note = held.value.get("note").and_then(Value::as_str).unwrap_or("");
                    json!({
                        "id": held.value["id"],
                        "author": held.value["author"],
                        "note": crate::js::slice_utf16(note, 120),
                        "status": held.status(),
                        "changes": held.value.get("changes").and_then(Value::as_array).map_or(0, Vec::len),
                        "stale": held.status() == "pending" && held.version() != current,
                    })
                })
                .collect(),
        ))
    }

    pub fn accept(&mut self, e: &mut dyn Engine, id: &str, args: &Value) -> Res<Value> {
        if args.get("tracked").is_some_and(crate::js::truthy) || args.get("date").is_some_and(crate::js::truthy) {
            return Err(ToolError::new("UNSUPPORTED", "Tracked acceptance is available for DOCX only. Omit tracked and date."));
        }
        let held = self.pending(e, id)?;
        let (steps, version) = (held.steps.clone(), held.version().to_owned());
        let receipt = e.apply(&steps, &version)?;
        if let Some(held) = self.proposals.get_mut(id) {
            held.value.insert("status".into(), json!("accepted"));
        }
        Ok(json!({ "id": id, "status": "accepted", "receipt": receipt }))
    }

    pub fn reject(&mut self, id: &str) -> Res<Value> {
        let status = self.held(id)?.status().to_owned();
        if status != "pending" {
            return Err(ToolError::new("PROPOSAL_CLOSED", format!("Proposal is {status}.")));
        }
        if let Some(held) = self.proposals.get_mut(id) {
            held.value.insert("status".into(), json!("rejected"));
        }
        Ok(json!({ "id": id, "status": "rejected" }))
    }

    /// The steps of a pending proposal (`pendingSteps`).
    pub fn pending_steps(&self, e: &dyn Engine, id: &str) -> Res<Vec<Value>> {
        Ok(self.pending(e, id)?.steps.clone())
    }

    pub fn export(&self, e: &dyn Engine, id: Option<&str>) -> Res<Vec<u8>> {
        self.alive()?;
        match id {
            Some(id) => {
                let steps = self.pending(e, id)?.steps.clone();
                e.save(Some(&steps))
            }
            None => e.save(None),
        }
    }

    pub fn verify(&self, e: &dyn Engine, id: Option<&str>) -> Res<Value> {
        let bytes = self.export(e, id)?;
        e.reopen(&bytes)?;
        let mut unchecked = vec!["visual fidelity", "semantic correctness"];
        if e.format() == "xlsx" {
            unchecked.push("formula correctness");
        }
        Ok(json!({ "reopened": true, "bytes": bytes.len(), "checks": ["save", "reopen"], "unchecked": unchecked }))
    }

    /// The default `render`: no PNG for this format.
    pub fn render_unsupported(&self, format: &str) -> Res<Value> {
        self.alive()?;
        Err(ToolError::new("UNSUPPORTED", format!("PNG rendering is not available in the {} prototype. Review the proposal changes and verify its export.", format.to_uppercase())))
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.proposals.clear();
        self.cursors.clear();
    }

    pub fn check_query(&self, g: &Grep) -> Res<()> {
        self.alive()?;
        if g.query.is_empty() || g.query.encode_utf16().count() > 1000 || g.query.contains(['\r', '\n']) {
            return Err(ToolError::new("INVALID_ARGUMENT", "query must be 1–1000 characters within one paragraph or cell."));
        }
        Ok(())
    }

    /// `searchPage(options, hits, clipped)`: one page of `hits` and the
    /// cursor for the next.
    pub fn search_page(&mut self, e: &dyn Engine, g: &Grep, hits: &[Value], clipped: bool) -> Res<Map<String, Value>> {
        let limit = integer(g.limit.unwrap_or(20.0), 1.0, 100.0, "limit")? as usize;
        let signature = crate::js::stringify(&json!([g.query, g.case_sensitive, g.story]));
        let version = e.current_version();
        let cursor = g.cursor.and_then(|c| self.cursors.get(c));
        if g.cursor.is_some() && cursor.is_none_or(|c| c.version != version || c.signature != signature) {
            return Err(ToolError::new("STALE_CURSOR", "Search changed or the document was edited. Repeat grep without cursor."));
        }
        let offset = cursor.map_or(0, |c| c.offset);
        let mut matches = Vec::new();
        let mut size = 0;
        for hit in hits.iter().skip(offset).take(limit) {
            let cost = json_len(hit) + 100;
            if !matches.is_empty() && size + cost > 16000 {
                break;
            }
            matches.push(hit.clone());
            size += cost;
        }
        let mut next = Value::Null;
        if offset + matches.len() < hits.len() {
            let n = self.next();
            let c = format!("{}:cursor{n}", self.prefix);
            if self.cursors.len() >= 64 {
                self.cursors.delete_first();
            }
            self.cursors.set(c.clone(), Cursor { signature, version: version.clone(), offset: offset + matches.len() });
            next = json!(c);
        }
        let mut out = Map::new();
        out.insert("matches".into(), Value::Array(matches));
        out.insert("nextCursor".into(), next);
        out.insert("version".into(), json!(version));
        out.insert("truncated".into(), json!(clipped));
        if clipped {
            out.insert("warning".into(), json!("Search stopped at 10000 matches. Narrow query or story to search beyond this cap."));
        }
        Ok(out)
    }
}
