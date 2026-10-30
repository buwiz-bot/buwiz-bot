//! `XlsxAgentWorkbook` (agents/xlsx.ts) over `betteroffice-xlsx`: the
//! workbook's JSON batch API (`read_cells_json`, `find_text_json`,
//! `validate_edits_json`, `apply_edits_json`) is the contract the wasm
//! binding exposes as `readCells`, `findText`, `validateEdits` and
//! `applyEdits`, so requests and answers pass through as Node's do.

use betteroffice_xlsx::{CalculationOptions, CellInput, CellRef, RenderOptions, SheetId, Workbook};
use serde_json::{json, Map, Value};

use super::proto::{engine, integer, opt_num, plain_text, sorted, text_window, units, unwrap, Ordered, Res, ToolError};
use super::prototype::{Engine, Grep, Proto};

struct Book {
    wb: Workbook,
}

fn open(bytes: &[u8]) -> Res<Workbook> {
    Workbook::open(bytes).map_err(engine)
}

impl Book {
    fn call(&self, f: impl FnOnce(&Workbook, &str) -> betteroffice_xlsx::Result<String>, req: &Value) -> Res<Value> {
        let out = f(&self.wb, &crate::js::stringify(req)).map_err(engine)?;
        unwrap(sorted(serde_json::from_str(&out).map_err(engine)?))
    }
    fn read(&self, req: &Value) -> Res<Value> {
        self.call(|w, r| w.read_cells_json(r), req)
    }
}

fn request(steps: &[Value], version: &str) -> Value {
    json!({ "expectVersion": version, "steps": steps, "source": "agent" })
}

impl Engine for Book {
    fn format(&self) -> &'static str {
        "xlsx"
    }
    fn current_version(&self) -> String {
        self.wb.version().as_str().to_owned()
    }
    fn validate(&self, steps: &[Value], version: &str) -> Res<()> {
        self.call(|w, r| w.validate_edits_json(r), &request(steps, version)).map(|_| ())
    }
    fn apply(&mut self, steps: &[Value], version: &str) -> Res<Value> {
        let out = self.wb.apply_edits_json(&crate::js::stringify(&request(steps, version))).map_err(engine)?;
        let r = unwrap(sorted(serde_json::from_str(&out).map_err(engine)?))?;
        let len = |v: &Value| v.as_array().map_or(0, Vec::len);
        let calc = &r["calculation"];
        Ok(json!({
            "applied": r["applied"],
            "version": r["version"],
            "changedSheets": len(&r["changedSheets"]),
            "calculation": { "changedCells": len(&calc["changed"]), "cycleCells": len(&calc["cycleCells"]), "limitedCells": len(&calc["limitedCells"]), "truncated": calc["truncated"] },
        }))
    }
    fn save(&self, steps: Option<&[Value]>) -> Res<Vec<u8>> {
        let Some(steps) = steps else { return self.wb.save().map_err(engine) };
        let mut draft = Book { wb: open(&self.wb.save().map_err(engine)?)? };
        let version = draft.current_version();
        draft.apply(steps, &version)?;
        draft.wb.save().map_err(engine)
    }
    fn reopen(&self, bytes: &[u8]) -> Res<()> {
        open(bytes).map(|_| ())
    }
}

#[derive(Clone)]
struct Target {
    sheet: String,
    a1: String,
    before: Value,
    version: String,
}

pub struct Xlsx {
    pub name: String,
    book: Book,
    proto: Proto,
    cells: Ordered<Target>,
    refs: Ordered<(String, String)>,
    next_cell: u64,
}

/// `address(row, col)`: A1 from zero-based indexes.
fn address(row: usize, col: usize) -> String {
    let mut letters = String::new();
    let mut n = col + 1;
    while n > 0 {
        letters.insert(0, (b'A' + ((n - 1) % 26) as u8) as char);
        n = (n - 1) / 26;
    }
    format!("{letters}{}", row + 1)
}

/// `corners(range)`.
fn corners(range: &str) -> Res<((usize, usize), (usize, usize))> {
    let bad = || ToolError::new("INVALID_RANGE", "Use one A1 cell or a rectangular A1:B2 range, without a sheet prefix.");
    let cell = |s: &str| -> Option<(usize, usize)> {
        let s = s.strip_prefix('$').unwrap_or(s);
        let split = s.find(|c: char| !c.is_ascii_alphabetic())?;
        let (letters, rest) = s.split_at(split);
        let digits = rest.strip_prefix('$').unwrap_or(rest);
        if letters.is_empty() || letters.len() > 3 || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) || digits.starts_with('0') {
            return None;
        }
        let col = letters.to_ascii_uppercase().bytes().fold(0usize, |n, c| n * 26 + (c - b'A' + 1) as usize) - 1;
        let row: f64 = digits.parse().ok()?;
        Some(((row - 1.0).min(usize::MAX as f64) as usize, col))
    };
    let (a, b) = match range.split_once(':') {
        Some((a, b)) => (cell(a).ok_or_else(bad)?, cell(b).ok_or_else(bad)?),
        None => {
            let c = cell(range).ok_or_else(bad)?;
            (c, c)
        }
    };
    if a.0 > b.0 || a.1 > b.1 || b.0 >= 1048576 || b.1 >= 16384 {
        return Err(ToolError::new("INVALID_RANGE", "Range must be ordered and inside the Excel grid."));
    }
    Ok((a, b))
}

impl Xlsx {
    pub fn open(bytes: &[u8], name: &str) -> Res<Self> {
        Ok(Self { name: name.to_owned(), book: Book { wb: open(bytes)? }, proto: Proto::default(), cells: Ordered::default(), refs: Ordered::default(), next_cell: 1 })
    }

    fn sheets(&self) -> Res<Value> {
        self.book.read(&json!({ "ranges": [] }))
    }

    pub fn overview(&self) -> Res<Value> {
        self.proto.alive()?;
        let read = self.sheets()?;
        Ok(json!({
            "name": self.name,
            "format": "xlsx",
            "version": read["version"],
            "sheets": read["sheets"].as_array().map_or(0, Vec::len),
            "capabilities": { "grep": "literal, case-sensitive", "proposals": "cell inputs and formulas", "render": false, "export": true, "prototype": true },
            "workflow": "outline lists sheet IDs; grep finds displayed values; cells reads A1 ranges and returns cell handles; proposeCells with cell and input (prefix = for a formula); review; verify; export.",
        }))
    }

    pub fn list(&mut self, args: &Value) -> Res<Value> {
        self.proto.alive()?;
        if args.get("headingsOnly").is_some_and(crate::js::truthy) {
            return Err(ToolError::new("UNSUPPORTED", "XLSX outline lists sheets; omit headingsOnly."));
        }
        let offset = integer(opt_num(args, "offset", 0.0), 0.0, 9007199254740991.0, "offset")? as usize;
        let limit = integer(opt_num(args, "limit", 30.0), 1.0, 100.0, "limit")? as usize;
        let read = self.sheets()?;
        let story = args.get("story").and_then(Value::as_str).filter(|s| !s.is_empty());
        let all = read["sheets"].as_array().cloned().unwrap_or_default();
        if let Some(s) = story {
            if !all.iter().any(|x| x["sheetId"] == s) {
                return Err(ToolError::new("UNKNOWN_STORY", "Use a sheetId from outline."));
            }
        }
        // the sheet entries as the engine Node runs (xlsx 0.3) gives them:
        // 0.5 adds `visibility`
        let sheets: Vec<Value> = all
            .into_iter()
            .filter(|x| story.is_none_or(|s| x["sheetId"] == s))
            .map(|mut x| {
                if let Some(m) = x.as_object_mut() {
                    m.remove("visibility");
                }
                x
            })
            .collect();
        Ok(json!({
            "version": read["version"],
            "items": sheets.iter().skip(offset).take(limit).cloned().collect::<Vec<_>>(),
            "nextOffset": if offset + limit < sheets.len() { json!(offset + limit) } else { Value::Null },
            "total": sheets.len(),
        }))
    }

    /// `readTargets(positions)`.
    fn read_targets(&self, positions: &[(String, String)]) -> Res<(String, Value, Vec<Target>)> {
        let ranges: Vec<Value> = positions.iter().map(|(sheet, a1)| json!({ "sheetId": sheet, "range": { "kind": "a1", "a1": a1 } })).collect();
        let read = self.book.read(&json!({ "ranges": ranges }))?;
        let version = read["version"].as_str().unwrap_or("").to_owned();
        let targets =
            positions.iter().enumerate().map(|(i, (sheet, a1))| Target { sheet: sheet.clone(), a1: a1.clone(), before: read["ranges"][i]["cells"][0][0].clone(), version: version.clone() }).collect();
        Ok((version, read["sheets"].clone(), targets))
    }

    fn ref_of(&mut self, sheet: &str, a1: &str) -> String {
        let r = format!("{sheet}!{a1}");
        if self.refs.len() >= 10000 && !self.refs.has(&r) {
            self.refs.delete_first();
        }
        self.refs.set(r.clone(), (sheet.to_owned(), a1.to_owned()));
        r
    }

    fn hold(&mut self, target: &Target) -> String {
        let cell = format!("{}:cell{}", self.proto.prefix, self.next_cell);
        self.next_cell += 1;
        if self.cells.len() >= 1024 {
            self.cells.delete_first();
        }
        self.cells.set(cell.clone(), target.clone());
        cell
    }

    fn visible(&mut self, t: &Target) -> Value {
        let before = &t.before;
        let kind = before["value"]["kind"].as_str().unwrap_or("");
        let text_value = before["value"]["value"].as_str().unwrap_or("");
        let value = if kind == "text" { json!({ "kind": "text", "value": crate::js::slice_utf16(text_value, 400) }) } else { before["value"].clone() };
        let formula = before.get("formula").and_then(Value::as_str);
        let display = before["displayText"].as_str().unwrap_or("");
        let u = |s: &str| s.encode_utf16().count();
        let truncated = u(display) > 400 || formula.map_or(0, u) > 400 || kind == "text" && u(text_value) > 400;
        json!({
            "ref": self.ref_of(&t.sheet, &t.a1),
            "cell": self.hold(t),
            "sheet": t.sheet,
            "a1": t.a1,
            "version": t.version,
            "value": value,
            "formula": formula.map_or(Value::Null, |f| json!(crate::js::slice_utf16(f, 400))),
            "displayText": crate::js::slice_utf16(display, 400),
            "truncated": truncated,
        })
    }

    pub fn read_cells(&mut self, args: &Value) -> Res<Value> {
        self.proto.alive()?;
        let sheet = args["sheet"].as_str().unwrap_or("").to_owned();
        let range = args["range"].as_str().unwrap_or("");
        let (start, end) = corners(range)?;
        let width = end.1 - start.1 + 1;
        let total = width * (end.0 - start.0 + 1);
        let offset = integer(opt_num(args, "offset", 0.0), 0.0, total as f64, "offset")? as usize;
        let limit = integer(opt_num(args, "limit", 50.0), 1.0, 100.0, "limit")? as usize;
        let positions: Vec<(String, String)> = (offset..total.min(offset + limit)).map(|i| (sheet.clone(), address(start.0 + i / width, start.1 + i % width))).collect();
        let (version, sheets, targets) = self.read_targets(&positions)?;
        if !sheets.as_array().is_some_and(|s| s.iter().any(|x| x["sheetId"] == sheet.as_str())) {
            return Err(ToolError::new("UNKNOWN_STORY", "Use a sheetId from outline."));
        }
        let mut cells = Vec::new();
        let mut size = 0;
        for t in &targets {
            let visible = self.visible(t);
            let cost = super::proto::json_len(&visible);
            if !cells.is_empty() && size + cost > 16000 {
                break;
            }
            size += cost;
            cells.push(visible);
        }
        Ok(
            json!({ "version": version, "sheet": sheet, "range": range, "cells": cells, "total": total, "nextOffset": if offset + cells.len() < total { json!(offset + cells.len()) } else { Value::Null } }),
        )
    }

    pub fn grep(&mut self, args: &Value) -> Res<Value> {
        let g = Grep::from(args);
        self.proto.check_query(&g)?;
        if g.case_sensitive == Some(false) {
            return Err(ToolError::new("UNSUPPORTED", "XLSX search is case-sensitive. Omit caseSensitive or set it to true."));
        }
        let mut req = Map::new();
        req.insert("text".into(), json!(g.query));
        if let Some(s) = g.story {
            req.insert("sheetIds".into(), json!([s]));
        }
        req.insert("limit".into(), json!(10000));
        let found = self.book.call(|w, r| w.find_text_json(r), &Value::Object(req))?;
        let hits: Vec<Value> = found["matches"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|hit| {
                let (sheet, a1) = (hit["cell"]["sheetId"].as_str().unwrap_or(""), hit["cell"]["a1"].as_str().unwrap_or(""));
                let text = hit["text"].as_str().unwrap_or("");
                json!({ "ref": self.ref_of(sheet, a1), "sheet": sheet, "a1": a1, "text": crate::js::slice_utf16(text, 400), "length": text.encode_utf16().count() })
            })
            .collect();
        let truncated = found["truncated"].as_bool().unwrap_or(false);
        let mut page = self.proto.search_page(&self.book, &g, &hits, truncated)?;
        let positions: Vec<(String, String)> = page["matches"].as_array().unwrap().iter().map(|m| (m["sheet"].as_str().unwrap_or("").to_owned(), m["a1"].as_str().unwrap_or("").to_owned())).collect();
        let (_, _, targets) = self.read_targets(&positions)?;
        let matches: Vec<Value> = page["matches"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .zip(targets)
            .map(|(mut hit, t)| {
                hit["cell"] = json!(self.hold(&t));
                hit
            })
            .collect();
        page.insert("matches".into(), Value::Array(matches));
        Ok(Value::Object(page))
    }

    pub fn read(&mut self, r: &str, args: &Value) -> Res<Value> {
        self.proto.alive()?;
        let Some((sheet, a1)) = self.refs.get(r).cloned() else {
            return Err(ToolError::new("UNKNOWN_REF", "Use a ref from grep or readCells."));
        };
        let (_, _, targets) = self.read_targets(&[(sheet, a1)])?;
        let t = &targets[0];
        let field = args.get("field").and_then(Value::as_str).unwrap_or("displayText");
        let text = match field {
            "formula" => t.before.get("formula").and_then(Value::as_str).unwrap_or("").to_owned(),
            "value" => t.before["value"].get("value").map(crate::js::string_of).unwrap_or_default(),
            _ => t.before["displayText"].as_str().unwrap_or("").to_owned(),
        };
        let mut out = match self.visible(t) {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        out.insert("field".into(), json!(field));
        let window = text_window(&units(&text), args.get("start").and_then(Value::as_f64), args.get("length").and_then(Value::as_f64))?;
        for (k, v) in window.as_object().unwrap() {
            out.insert(k.clone(), v.clone());
        }
        Ok(Value::Object(out))
    }

    pub fn propose(&self) -> Res<Value> {
        Err(ToolError::new("UNSUPPORTED", "XLSX edits use proposeCells / office_propose_cells with {cell, input}. Read cells first; input replaces the entire cell."))
    }

    pub fn propose_cells(&mut self, input: &Value) -> Res<Value> {
        self.proto.alive()?;
        let edits = input["edits"].as_array().cloned().unwrap_or_default();
        if edits.is_empty() || edits.len() > 32 {
            return Err(ToolError::new("INVALID_ARGUMENT", "Supply 1–32 cell edits."));
        }
        let version = self.book.current_version();
        let mut changes = Vec::new();
        let mut steps = Vec::new();
        for edit in &edits {
            let Some(t) = self.cells.get(edit["cell"].as_str().unwrap_or("")).cloned() else {
                return Err(ToolError::new("UNKNOWN_CELL", "Cell handle expired or belongs to another workbook. Read cells or grep again."));
            };
            if t.version != version {
                return Err(ToolError::new("STALE_TARGET", "Workbook changed. Read fresh cell handles."));
            }
            let text = edit["input"].as_str().unwrap_or("");
            plain_text(text)?;
            if text.encode_utf16().count() > 16000 {
                return Err(ToolError::new("EDIT_LIMIT", "Cell input must fit in 16000 UTF-16 units."));
            }
            changes.push(json!({ "ref": self.ref_of(&t.sheet, &t.a1), "sheet": t.sheet, "a1": t.a1, "before": t.before, "input": text }));
            let mut guard = Map::new();
            guard.insert("value".into(), t.before["value"].clone());
            if let Some(f) = t.before.get("formula") {
                guard.insert("formula".into(), f.clone());
            }
            guard.insert("displayText".into(), t.before["displayText"].clone());
            steps.push(json!({ "op": "setCellInputs", "target": { "sheetId": t.sheet, "range": { "kind": "a1", "a1": t.a1 } }, "inputs": [[text]], "expect": { "cells": [[guard]] } }));
        }
        self.proto.stage(&self.book, input, changes, steps, &version)
    }

    pub fn render(&self, proposal: Option<&str>) -> Res<(Value, Vec<u8>)> {
        self.proto.alive()?;
        let rendered = match proposal {
            Some(id) => {
                let steps = self.proto.pending_steps(&self.book, id)?;
                let mut draft = Book { wb: open(&self.book.wb.save().map_err(engine)?)? };
                let version = draft.current_version();
                draft.apply(&steps, &version)?;
                draft.wb.render_sheet(draft.wb.active_sheet(), &RenderOptions::default()).map_err(engine)?
            }
            None => self.book.wb.render_sheet(self.book.wb.active_sheet(), &RenderOptions::default()).map_err(engine)?,
        };
        let meta = json!({ "page": 1, "pageCount": 1, "width": rendered.width, "height": rendered.height, "warnings": ["Preview uses BetterOffice rendering; compare with the target Office application when exact fidelity matters."] });
        Ok((meta, rendered.bytes))
    }

    pub fn review(&self, id: &str) -> Res<Value> {
        self.proto.review(&self.book, id)
    }
    pub fn list_proposals(&self) -> Res<Value> {
        self.proto.list_proposals(&self.book)
    }
    pub fn accept(&mut self, id: &str, args: &Value) -> Res<Value> {
        self.proto.accept(&mut self.book, id, args)
    }
    pub fn reject(&mut self, id: &str) -> Res<Value> {
        self.proto.reject(id)
    }
    pub fn export(&self, id: Option<&str>) -> Res<Vec<u8>> {
        self.proto.export(&self.book, id)
    }
    pub fn verify(&self, id: Option<&str>) -> Res<Value> {
        self.proto.verify(&self.book, id)
    }
    pub fn close(&mut self) {
        if self.proto.closed {
            return;
        }
        self.proto.close();
        self.cells.clear();
        self.refs.clear();
    }
}

/// create.ts's `createXlsxBytes`: the rows typed into the blank seed's
/// first sheet.
pub fn create(seed: &[u8], rows: &[Vec<String>]) -> Res<Vec<u8>> {
    let mut wb = open(seed)?;
    let edits: Vec<CellInput> =
        rows.iter().enumerate().flat_map(|(r, row)| row.iter().enumerate().map(move |(c, input)| CellInput { cell: CellRef::new(r as u32, c as u32), input: input.clone() })).collect();
    wb.edit_cells(SheetId(0), &edits, CalculationOptions::default()).map_err(engine)?;
    wb.save().map_err(engine)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_xlsx_addresses_and_ranges_as_the_agent_reads_them() {
        assert_eq!(address(0, 0), "A1");
        assert_eq!(address(9, 27), "AB10");
        assert_eq!(address(0, 16383), "XFD1");
        assert_eq!(corners("a1:$C$4").unwrap(), ((0, 0), (3, 2)));
        assert_eq!(corners("B2").unwrap(), ((1, 1), (1, 1)));
        assert_eq!(corners("B2:A1").unwrap_err().code, "INVALID_RANGE");
        assert_eq!(corners("Sheet1!A1").unwrap_err().message, "Use one A1 cell or a rectangular A1:B2 range, without a sheet prefix.");
        assert_eq!(corners("A0").unwrap_err().code, "INVALID_RANGE");
    }
}
