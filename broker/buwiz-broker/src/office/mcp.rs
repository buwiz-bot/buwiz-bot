//! `buwiz-broker office-mcp [--root <dir>] [--read-only]`: the office tools
//! over stdio, as Node's `mcp-main.ts` serves them (newline-delimited
//! JSON-RPC, MCP's `initialize`, `tools/list`, `tools/call`).

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{json, Value};

use super::{defs, Server};

fn write(out: &mut impl Write, v: &Value) {
    let _ = writeln!(out, "{}", crate::js::stringify(v));
    let _ = out.flush();
}

/// One request's answer, or `None` for a notification.
fn answer(server: &mut Server, msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match msg.get("method").and_then(Value::as_str).unwrap_or("") {
        "initialize" => json!({
            "protocolVersion": params.get("protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
            "capabilities": { "tools": {} },
            "serverInfo": defs()["serverInfo"],
            "instructions": defs()["instructions"],
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": server.tools() }),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").filter(|a| !a.is_null()).cloned().unwrap_or_else(|| json!({}));
            server.call(name, &args).mcp()
        }
        _ => return Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "Method not found" } })),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

pub fn cli(args: &[String]) -> i32 {
    let mut root = std::env::var("GEN_OFFICE_ROOT").ok().filter(|r| !r.trim().is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("generated/files"));
    let mut read_only = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                i += 1;
                if let Some(r) = args.get(i) {
                    root = PathBuf::from(r);
                }
            }
            "--read-only" => read_only = true,
            "-h" | "--help" => {
                eprintln!("Usage: office-mcp [--root <directory>] [--read-only]\n\nBuwiz Bot's bundled BetterOffice stdio MCP server for DOCX, XLSX, and PPTX create/read/edit/export.");
                return 0;
            }
            _ => {}
        }
        i += 1;
    }
    let mut server = match Server::new(&root, read_only) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("office-mcp: {}", e.message);
            return 1;
        }
    };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                write(&mut out, &json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("Parse error: {e}") } }));
                continue;
            }
        };
        if let Some(reply) = answer(&mut server, &msg) {
            write(&mut out, &reply);
        }
    }
    server.dispose();
    0
}
