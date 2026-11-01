//! Office in Rust (plan 6.1, D3): the BetterOffice agent layer Node runs
//! as an MCP child (`server/src/office/agents/*.ts`, `mcp.ts`,
//! `create.ts`), ported onto BetterOffice's own crates — `betteroffice-
//! docx-edit` (the editing core the npm `YrsSession` wraps), `-xlsx` and
//! `-pptx-edit` — so the hub's `office_*` tools run in this process.
//!
//! - [`Office`]: a workspace on its own thread (the engines' sessions are
//!   not `Send`; Node serializes calls through one queue too), answering a
//!   tool call as mcp.ts does: arguments checked as zod checks them, then
//!   `{content, structuredContent, isError?}`.
//! - `mcp`: the same server over stdio (`buwiz-broker office-mcp --root <dir>`),
//!   Node's `mcp-main` command line, for tests and any MCP client.
//! - `routes`: `/office/*` (viewer, editor bundle, vendor files, caps) and
//!   `/file/render` for office files.
//!
//! What no crate here does: DOCX page rendering. Node draws pages with
//! render.ts (the docx package's JS layout and display list, painted by
//! `@napi-rs/canvas` with `@betteroffice/fonts`); the crate's PNG path is
//! its `raster` feature, which needs `betteroffice-docx-raster` 0.5.1 (not
//! fetched, D26) and a measured layout input. So `office_render` on a DOCX
//! answers as Node does when render.ts cannot load: `UNSUPPORTED`, and
//! `capabilities.render` is false.

pub mod args;
pub mod create;
pub mod docx;
pub mod mcp;
pub mod pptx;
pub mod proto;
pub mod prototype;
pub mod routes;
pub mod workspace;
pub mod xlsx;

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, OnceLock};

use base64::Engine as _;
use serde_json::{json, Value};

use proto::{Res, ToolError};
use workspace::{Doc, Workspace};

/// The tools' definitions and the server's instructions as Node's MCP
/// server lists them (recorded from it; office.test.mjs OFF-01 holds both
/// to it).
pub fn defs() -> &'static Value {
    static DEFS: OnceLock<Value> = OnceLock::new();
    DEFS.get_or_init(|| serde_json::from_str(include_str!("tools.json")).unwrap_or(Value::Null))
}

const WRITES: [&str; 7] = ["office_propose", "office_propose_cells", "office_review", "office_accept", "office_reject", "office_export", "office_create"];

/// A tool's answer.
pub struct Reply {
    pub value: Value,
    pub is_error: bool,
    pub image: Option<Vec<u8>>,
}

impl Reply {
    fn ok(value: Value) -> Self {
        Self { value, is_error: false, image: None }
    }

    /// mcp.ts's `failure(error)`.
    fn failure(e: &ToolError) -> Self {
        Self { value: e.body(), is_error: true, image: None }
    }

    /// The `CallToolResult` Node's server answers.
    pub fn mcp(&self) -> Value {
        let mut content = vec![json!({ "type": "text", "text": crate::js::stringify(&self.value) })];
        if let Some(png) = &self.image {
            content.push(json!({ "type": "image", "mimeType": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(png) }));
        }
        let mut out = json!({ "content": content, "structuredContent": self.value });
        if self.is_error {
            out["isError"] = json!(true);
        }
        out
    }

    /// office.mjs's `callOfficeTool` result: the blocks folded to one string.
    pub fn fold(&self) -> String {
        crate::hub::tools::mcp::fold(&self.mcp())
    }
}

/// The tool server: a workspace and mcp.ts's handlers.
pub struct Server {
    ws: Workspace,
    read_only: bool,
}

fn doc_id(args: &Value) -> &str {
    args.get("document").and_then(Value::as_str).unwrap_or("")
}

impl Server {
    pub fn new(root: &Path, read_only: bool) -> Res<Self> {
        Ok(Self { ws: Workspace::at(root)?, read_only })
    }

    /// `tools/call`.
    pub fn call(&mut self, name: &str, args: &Value) -> Reply {
        let schema = defs()["tools"].as_array().and_then(|t| t.iter().find(|t| t["name"] == name && !(self.read_only && WRITES.contains(&name))));
        let Some(schema) = schema else {
            return Reply::failure(&ToolError::new("UNKNOWN_TOOL", "Use tools/list to discover supported tools."));
        };
        let args = match args::parse(&schema["inputSchema"], args) {
            Ok(a) => a,
            Err(issues) => {
                return Reply { value: json!({ "code": "INVALID_ARGUMENT", "message": "Check the tool input schema.", "issues": issues }), is_error: true, image: None };
            }
        };
        match self.run(name, &args) {
            Ok(r) => r,
            Err(e) => Reply::failure(&e),
        }
    }

    fn run(&mut self, name: &str, a: &Value) -> Res<Reply> {
        let s = |k: &str| a.get(k).and_then(Value::as_str);
        let proposal = s("proposal");
        let value = match name {
            "office_files" => self.ws.files(s("directory"), a.get("offset").and_then(Value::as_f64))?,
            "office_open" => self.ws.open(s("path").unwrap_or(""))?,
            "office_outline" => match self.ws.get(doc_id(a))? {
                Doc::Docx(d) => d.list(a)?,
                Doc::Xlsx(d) => d.list(a)?,
                Doc::Pptx(d) => d.list(a)?,
            },
            "office_grep" => match self.ws.get(doc_id(a))? {
                Doc::Docx(d) => d.grep(a)?,
                Doc::Xlsx(d) => d.grep(a)?,
                Doc::Pptx(d) => d.grep(a)?,
            },
            "office_read" => {
                let r = s("ref").unwrap_or("");
                let doc = self.ws.get(doc_id(a))?;
                if a.get("field").is_some() && !matches!(doc, Doc::Xlsx(_)) {
                    return Err(ToolError::new("UNSUPPORTED_FORMAT", "field is an XLSX-only read option."));
                }
                match doc {
                    Doc::Docx(d) => d.read(r, a)?,
                    Doc::Xlsx(d) => d.read(r, a)?,
                    Doc::Pptx(d) => d.read(r, a)?,
                }
            }
            "office_cells" => match self.ws.get(doc_id(a))? {
                Doc::Xlsx(d) => d.read_cells(a)?,
                _ => return Err(ToolError::new("UNSUPPORTED_FORMAT", "office_cells requires an XLSX workbook.")),
            },
            "office_render" => {
                let page = a.get("page").and_then(Value::as_f64).unwrap_or(1.0);
                let (meta, png) = match self.ws.get(doc_id(a))? {
                    Doc::Docx(d) => d.render(page)?,
                    Doc::Xlsx(d) => d.render(proposal)?,
                    Doc::Pptx(d) => d.render()?,
                };
                return Ok(Reply { value: meta, is_error: false, image: Some(png) });
            }
            "office_verify" => match self.ws.get(doc_id(a))? {
                Doc::Docx(d) => d.verify(proposal)?,
                Doc::Xlsx(d) => d.verify(proposal)?,
                Doc::Pptx(d) => d.verify(proposal)?,
            },
            "office_close" => self.ws.close(doc_id(a))?,
            "office_propose" => match self.ws.get(doc_id(a))? {
                Doc::Docx(d) => d.propose(a)?,
                Doc::Xlsx(d) => d.propose()?,
                Doc::Pptx(d) => d.propose(a)?,
            },
            "office_propose_cells" => match self.ws.get(doc_id(a))? {
                Doc::Xlsx(d) => d.propose_cells(a)?,
                _ => return Err(ToolError::new("UNSUPPORTED_FORMAT", "office_propose_cells requires an XLSX workbook.")),
            },
            "office_review" => {
                let doc = self.ws.get(doc_id(a))?;
                match proposal {
                    Some(id) => match doc {
                        Doc::Docx(d) => d.review(id)?,
                        Doc::Xlsx(d) => d.review(id)?,
                        Doc::Pptx(d) => d.review(id)?,
                    },
                    None => json!({ "proposals": match doc {
                        Doc::Docx(d) => d.list_proposals()?,
                        Doc::Xlsx(d) => d.list_proposals()?,
                        Doc::Pptx(d) => d.list_proposals()?,
                    } }),
                }
            }
            "office_accept" => {
                let id = proposal.unwrap_or("");
                match self.ws.get(doc_id(a))? {
                    Doc::Docx(d) => d.accept(id, a)?,
                    Doc::Xlsx(d) => d.accept(id, a)?,
                    Doc::Pptx(d) => d.accept(id, a)?,
                }
            }
            "office_reject" => {
                let id = proposal.unwrap_or("");
                match self.ws.get(doc_id(a))? {
                    Doc::Docx(d) => d.reject(id)?,
                    Doc::Xlsx(d) => d.reject(id)?,
                    Doc::Pptx(d) => d.reject(id)?,
                }
            }
            "office_export" => self.ws.export(doc_id(a), s("path").unwrap_or(""), proposal)?,
            "office_create" => {
                let path = s("path").unwrap_or("");
                let lower = path.to_lowercase();
                let Some(format) = ["xlsx", "pptx", "docx"].into_iter().find(|f| lower.ends_with(&format!(".{f}"))) else {
                    return Err(ToolError::new("UNSUPPORTED_FORMAT", "Path must end in .docx, .xlsx, or .pptx."));
                };
                if !a.get(format).is_some_and(crate::js::truthy) {
                    return Err(ToolError::new("INVALID_ARGUMENT", format!("A .{format} path requires a {format} payload.")));
                }
                let bytes = create::bytes(format, a, create::Flavor::Tool)?;
                self.ws.create(path, &bytes)?
            }
            _ => return Err(ToolError::new("UNKNOWN_TOOL", "Use tools/list to discover supported tools.")),
        };
        Ok(Reply::ok(value))
    }

    /// `tools/list`.
    pub fn tools(&self) -> Vec<Value> {
        tool_defs(self.read_only)
    }

    pub fn dispose(&mut self) {
        self.ws.dispose();
    }
}

fn tool_defs(read_only: bool) -> Vec<Value> {
    defs()["tools"].as_array().cloned().unwrap_or_default().into_iter().filter(|t| !(read_only && WRITES.contains(&t["name"].as_str().unwrap_or("")))).collect()
}

/// The hub's office track: the in-process office the `office_*` tools
/// call, and the office routes.
pub fn install(routes: &mut crate::hub::Registry, ctx: &mut crate::hub::ctx::HubCtx) {
    ctx.ext.insert(Office::for_hub(&ctx.data));
    routes::install(routes);
}

type Job = Box<dyn FnOnce(&mut Server) + Send>;

/// The hub's office: the tool server on its own thread, started on first
/// use (Node starts its child once and keeps a failed start for good).
pub struct Office {
    root: PathBuf,
    started: Mutex<Option<Result<mpsc::Sender<Job>, String>>>,
}

impl Office {
    pub fn new(root: PathBuf) -> Self {
        Self { root, started: Mutex::new(None) }
    }

    /// The hub's: the files dir (`generated/files`, made as config.mjs makes it).
    pub fn for_hub(data: &Path) -> Arc<Self> {
        let root = data.join("generated").join("files");
        let _ = std::fs::create_dir_all(&root);
        Arc::new(Self::new(root))
    }

    fn sender(&self) -> Result<mpsc::Sender<Job>, String> {
        let mut slot = self.started.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            let (tx, rx) = mpsc::channel::<Job>();
            let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
            let root = self.root.clone();
            let spawned = std::thread::Builder::new().name("office".into()).spawn(move || {
                let mut server = match Server::new(&root, false) {
                    Ok(s) => {
                        let _ = ready_tx.send(Ok(()));
                        s
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.message));
                        return;
                    }
                };
                while let Ok(job) = rx.recv() {
                    job(&mut server);
                }
                server.dispose();
            });
            let started = match spawned {
                Ok(_) => ready_rx.recv().unwrap_or_else(|_| Err("the office thread ended".into())).map(|_| tx),
                Err(e) => Err(e.to_string()),
            };
            if let Err(e) = &started {
                eprintln!("  office mcp unavailable: {}", crate::js::slice_utf16(e, 160));
            }
            *slot = Some(started);
        }
        slot.as_ref().unwrap().clone()
    }

    /// Runs one tool call on the office thread.
    pub async fn reply(&self, name: &str, args: &Value) -> Result<Reply, String> {
        let tx = self.sender()?;
        let (done, wait) = tokio::sync::oneshot::channel();
        let (name, args) = (name.to_owned(), args.clone());
        tx.send(Box::new(move |s: &mut Server| {
            let _ = done.send(s.call(&name, &args));
        }))
        .map_err(|_| "the office thread ended".to_owned())?;
        wait.await.map_err(|_| "the office thread ended".to_owned())
    }

    /// `officeMcpTools()`: the tools in Anthropic's shape, `office_create`
    /// left out (the tool loop's own adds the file card).
    pub fn tools(&self) -> Vec<Value> {
        if self.sender().is_err() {
            return Vec::new();
        }
        tool_defs(false)
            .into_iter()
            .filter(|t| t["name"] != "office_create")
            .map(|t| json!({ "name": t["name"], "description": t.get("description").filter(|d| crate::js::truthy(d)).cloned().unwrap_or(json!("")), "input_schema": t.get("inputSchema").filter(|s| crate::js::truthy(s)).cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}})) }))
            .collect()
    }

    /// `callOfficeTool(root, name, input)`: (content, is_error).
    pub async fn call(&self, name: &str, input: &Value) -> Result<(String, bool), String> {
        let args = if input.is_object() { input.clone() } else { json!({}) };
        match self.reply(name, &args).await {
            Ok(r) => Ok((r.fold(), r.is_error)),
            Err(e) => Ok((format!("office tools unavailable — the BetterOffice MCP server failed to start ({e})"), true)),
        }
    }

    /// office.mjs's `officeCreateBytes(format, {[format]: payload})`.
    pub async fn create_bytes(format: &str, payload: &Value) -> Result<Vec<u8>, String> {
        let (format, mut input) = (format.to_owned(), serde_json::Map::new());
        input.insert(format.clone(), payload.clone());
        tokio::task::spawn_blocking(move || create::bytes(&format, &Value::Object(input), create::Flavor::Chat).map_err(|e| e.message)).await.map_err(|e| e.to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace holding Node's seed files.
    fn workspace() -> PathBuf {
        let seeds = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../server/src/office/seeds");
        let dir = std::env::temp_dir().join(format!("rox-office-test-{}", proto::uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["betteroffice-demo.docx", "showcase.xlsx", "presentation.pptx"] {
            std::fs::copy(seeds.join(f), dir.join(f)).unwrap();
        }
        dir
    }

    fn ok(s: &mut Server, name: &str, args: Value) -> Value {
        let r = s.call(name, &args);
        assert!(!r.is_error, "{name}: {}", r.value);
        r.value
    }

    #[test]
    fn office_docx_round_trip_open_propose_accept_export_reopen() {
        let dir = workspace();
        let mut s = Server::new(&dir, false).unwrap();
        let doc = ok(&mut s, "office_open", json!({"path": "betteroffice-demo.docx"}))["document"].clone();
        let g = ok(&mut s, "office_grep", json!({"document": doc, "query": "Welcome"}));
        let p = ok(&mut s, "office_propose", json!({"document": doc, "author": "Buwiz Bot", "edits": [{"match": g["matches"][0]["match"], "newText": "Hello"}]}));
        ok(&mut s, "office_accept", json!({"document": doc, "proposal": p["id"]}));
        assert_eq!(ok(&mut s, "office_export", json!({"document": doc, "path": "out.docx"}))["path"], "out.docx");
        let out = ok(&mut s, "office_open", json!({"path": "out.docx"}))["document"].clone();
        assert_eq!(ok(&mut s, "office_read", json!({"document": out, "ref": "p1"}))["text"], "Hello to BetterOffice");
        // the source is untouched and an export never overwrites
        assert_eq!(s.call("office_export", &json!({"document": doc, "path": "out.docx"})).value["code"], "EEXIST");
        let r = s.call("office_render", &json!({"document": doc}));
        assert_eq!((r.is_error, r.value["code"].as_str()), (true, Some("UNSUPPORTED")));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn office_xlsx_and_pptx_round_trips() {
        let dir = workspace();
        let mut s = Server::new(&dir, false).unwrap();
        let book = ok(&mut s, "office_open", json!({"path": "showcase.xlsx"}))["document"].clone();
        let cells = ok(&mut s, "office_cells", json!({"document": book, "sheet": "sheet:0", "range": "A1"}));
        let p = ok(&mut s, "office_propose_cells", json!({"document": book, "author": "Buwiz Bot", "edits": [{"cell": cells["cells"][0]["cell"], "input": "Q4"}]}));
        ok(&mut s, "office_accept", json!({"document": book, "proposal": p["id"]}));
        ok(&mut s, "office_export", json!({"document": book, "path": "out.xlsx"}));
        let r = s.call("office_render", &json!({"document": book}));
        assert!(!r.is_error && r.image.as_ref().is_some_and(|png| png.starts_with(b"\x89PNG")));
        let out = ok(&mut s, "office_open", json!({"path": "out.xlsx"}))["document"].clone();
        assert_eq!(ok(&mut s, "office_cells", json!({"document": out, "sheet": "sheet:0", "range": "A1"}))["cells"][0]["displayText"], "Q4");
        let deck = ok(&mut s, "office_open", json!({"path": "presentation.pptx"}))["document"].clone();
        let g = ok(&mut s, "office_grep", json!({"document": deck, "query": "WASM"}));
        let p = ok(&mut s, "office_propose", json!({"document": deck, "author": "Buwiz Bot", "edits": [{"match": g["matches"][0]["match"], "newText": "Native"}]}));
        ok(&mut s, "office_accept", json!({"document": deck, "proposal": p["id"]}));
        ok(&mut s, "office_export", json!({"document": deck, "path": "out.pptx"}));
        let out = ok(&mut s, "office_open", json!({"path": "out.pptx"}))["document"].clone();
        let g = ok(&mut s, "office_grep", json!({"document": out, "query": "Native", "caseSensitive": true}));
        assert_eq!(g["matches"].as_array().map(Vec::len), Some(1));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn office_create_and_argument_checks_answer_as_mcp_ts() {
        let dir = workspace();
        let mut s = Server::new(&dir, false).unwrap();
        for (path, key, payload) in [
            ("a.docx", "docx", json!({"paragraphs": [{"text": "Title", "style": "title"}]})),
            ("a.xlsx", "xlsx", json!({"rows": [["1", "=A1*2"]]})),
            ("a.pptx", "pptx", json!({"slides": [{"title": "Hi", "bullets": ["x"]}]})),
        ] {
            let mut args = json!({"path": path});
            args[key] = payload;
            assert_eq!(ok(&mut s, "office_create", args)["path"], path);
        }
        assert_eq!(s.call("office_create", &json!({"path": "a.docx", "docx": {"paragraphs": [{"text": "x"}]}})).value["code"], "EEXIST");
        assert_eq!(s.call("office_create", &json!({"path": "b.pptx", "docx": {"paragraphs": [{"text": "x"}]}})).value["message"], "A .pptx path requires a pptx payload.");
        let bad = s.call("office_open", &json!({"path": "a.docx", "x": 1}));
        assert_eq!(bad.value, json!({"code": "INVALID_ARGUMENT", "message": "Check the tool input schema.", "issues": [{"path": [], "message": "Unrecognized key: \"x\""}]}));
        assert_eq!(s.call("office_nope", &json!({})).value["code"], "UNKNOWN_TOOL");
        let read_only = Server::new(&dir, true).unwrap();
        assert!(!read_only.tools().iter().any(|t| t["name"] == "office_export"));
        std::fs::remove_dir_all(dir).ok();
    }
}
