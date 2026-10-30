//! The office tools (tools.mjs `toolOffice`, `officeArt`, `cardOfficeFile`;
//! plan D31): `office_create` writes its file through the BetterOffice
//! child's own `office_create` (the same payloads) and lands it as a file
//! card; every other `office_*` call goes to the child as is, and an
//! export's new file becomes a card too.

use std::sync::Arc;

use serde_json::{json, Value};

use super::{Out, Run};
use crate::helper::office::Office;
use crate::hub::agent::files::file_record;
use crate::hub::ctx::HubCtx;
use crate::hub::store::{uuidv7, Turn};
use crate::js;

/// The hub's office child, when installed.
pub fn office(ctx: &HubCtx) -> Option<Arc<Office>> {
    ctx.ext.get::<Arc<Office>>().cloned()
}

/// `officeMcpTools()`.
pub fn tools(ctx: &HubCtx) -> Vec<Value> {
    office(ctx).map(|o| o.tools()).unwrap_or_default()
}

/// `officeArt(name)`: the placeholder shaped like its file.
pub fn office_art(name: &str) -> Value {
    let lower = name.to_lowercase();
    let (kind, w, h) = if lower.ends_with(".ppt") || lower.ends_with(".pptx") {
        ("presentation", 560, 315)
    } else if [".xlsx", ".xlsm", ".xls", ".csv"].iter().any(|e| lower.ends_with(e)) {
        ("spreadsheet", 480, 360)
    } else if lower.ends_with(".pdf") {
        ("pdf", 425, 550)
    } else {
        ("document", 425, 550)
    };
    json!({"title": if name.is_empty() { Value::Null } else { json!(name) }, "desc": "", "w": w, "h": h, "kind": kind})
}

/// `outFile(rel)`: a path inside the files dir, else `None`.
pub fn out_file(ctx: &HubCtx, rel: &str) -> Option<std::path::PathBuf> {
    let dir = ctx.store.gen_dir().join("files");
    let p = normalize(&dir.join(rel));
    p.starts_with(&dir).then_some(p).filter(|p| *p != dir)
}

/// `path.join`'s `..` and `.` folding.
fn normalize(p: &std::path::Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// A building turn's index in the thread, by id.
fn index_of(run: &Run, id: &str) -> bool {
    run.ctx.store.read(|s| s.thread(&run.turn.thread).is_some_and(|t| t.turn(id).is_some()))
}

fn drop_turn(run: &Run, id: &str) {
    run.ctx.store.touch(|s| {
        if let Some(t) = s.thread_mut(&run.turn.thread) {
            t.turns.retain(|u| u.id.as_deref() != Some(id));
        }
    });
    run.ctx.jobs.with(run.job, |j| {
        if j.turn.as_deref() == Some(id) {
            j.turn = None;
        }
    });
}

/// `office_create` and the child's `office_*` tools.
pub async fn tool(run: &Run, name: &str, input: &Value) -> Result<Out, String> {
    let ctx = &run.ctx;
    if name == "office_create" {
        let file = input.get("path").filter(|p| js::truthy(p)).map(js::string_of).unwrap_or_default().rsplit('/').next().unwrap_or("").to_owned();
        let lower = file.to_lowercase();
        let format = [".xlsx", ".pptx", ".docx"].iter().find(|e| lower.ends_with(*e)).map(|e| &e[1..]);
        // the placeholder drawn while the payload streamed becomes the
        // file's turn
        let pre = run.state().pending_office.take().filter(|id| index_of(run, id));
        let Some(format) = format else {
            if let Some(p) = &pre {
                drop_turn(run, p);
            }
            return Ok(Out::err("path must end in .docx, .xlsx, or .pptx".into()));
        };
        let Some(payload) = input.get(format).filter(|p| js::truthy(p)) else {
            if let Some(p) = &pre {
                drop_turn(run, p);
            }
            return Ok(Out::err(format!("a .{format} path requires a {format} payload")));
        };
        let id = pre.clone().unwrap_or_else(uuidv7);
        ctx.store.touch(|s| {
            let Some(t) = s.thread_mut(&run.turn.thread) else { return };
            let art = office_art(&file);
            match t.turns.iter_mut().find(|u| u.id.as_deref() == Some(id.as_str())) {
                Some(u) => {
                    u.role = "ai".into();
                    u.building = true;
                    u.status = Some("Putting the file together".into());
                    u.label = Some("office".into());
                    u.art = Some(art);
                }
                None => t.turns.push(Turn {
                    role: "ai".into(),
                    id: Some(id.clone()),
                    building: true,
                    status: Some("Putting the file together".into()),
                    label: Some("office".into()),
                    art: Some(art),
                    ..Default::default()
                }),
            }
        });
        ctx.jobs.with(run.job, |j| j.turn = Some(id.clone()));
        let rel = format!("o{}", crate::hub::library::groups::now36());
        let dir = ctx.store.gen_dir().join("files").join(&rel);
        let _ = std::fs::create_dir_all(&dir);
        let key = format!("{rel}/{file}");
        let made = match office(ctx) {
            None => Err("the BetterOffice MCP server is not installed".to_owned()),
            Some(o) => match o
                .call("office_create", &{
                    let mut args = serde_json::Map::new();
                    args.insert("path".into(), json!(key));
                    args.insert(format.into(), payload.clone());
                    Value::Object(args)
                })
                .await
            {
                Ok((_, false)) => Ok(()),
                Ok((text, true)) => Err(text),
                Err(e) => Err(e),
            },
        };
        if let Err(e) = made {
            drop_turn(run, &id);
            return Ok(Out::err(format!("office_create failed: {}", js::slice_utf16(&e, 400))));
        }
        let record = file_record(ctx, &dir.join(&file), &key).await;
        ctx.store.write(|s| {
            if let Some(u) = s.thread_mut(&run.turn.thread).and_then(|t| t.turns.iter_mut().find(|u| u.id.as_deref() == Some(id.as_str()))) {
                u.files = Some(json!([record]));
                u.art = None;
                u.building = false;
                u.status = None;
            }
        })?;
        ctx.jobs.with(run.job, |j| {
            if j.turn.as_deref() == Some(id.as_str()) {
                j.turn = None;
            }
        });
        ctx.store.write_index();
        println!("✓ office {format} → {key}");
        return Ok(Out::ok(format!("created {key} — the file card is in the reply. To revise it, open this path with office_open and export the result.")));
    }
    let Some(o) = office(ctx) else {
        return Ok(Out::err("office tools unavailable — the BetterOffice MCP server failed to start (not installed)".into()));
    };
    let (content, is_error) = o.call(name, input).await?;
    let r = Out { content: json!(content), is_error: Some(is_error) };
    if is_error {
        return Ok(r);
    }
    // office_export's result names the file it wrote: card it
    if name == "office_export" {
        if let Some(rel) = serde_json::from_str::<Value>(&content).ok().and_then(|v| v.get("path").and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned)) {
            if let Some(p) = out_file(ctx, &rel).filter(|p| p.exists()) {
                let record = file_record(ctx, &p, &rel).await;
                let turn = Turn { role: "ai".into(), label: Some("office".into()), files: Some(json!([record])), ..Default::default() };
                ctx.store.append_turn(&run.turn.thread, turn)?;
            }
        }
    }
    Ok(r)
}
