//! The workbook on the office worker ([`buwiz_office_kit::session`]): opens
//! fast for the first paint, edits through BetterOffice's `Workbook`.
//!
//! BetterOffice's `Workbook::open` builds a collaboration document of the
//! whole model before it returns (~0.7 s for 100k cells, ~10 s for 1M,
//! `BENCH.md`), while parsing alone is a fraction of that. So a workbook
//! opens in two steps: the parsed model draws at once (the renderer runs
//! straight on it), and the editable `Workbook` is built on a helper
//! thread; edits made before it lands wait for it and then apply in order.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use betteroffice_xlsx::{
    CalculationOptions, CellInput, CellRef, CellValue, DisplayList, GridGeometry, HistoryState, Op, SheetId, Viewport, Workbook,
    WorkbookModel,
};

use super::convert;
use buwiz_office_kit::session::Document;

/// A rectangle of cells, inclusive: `(row, col)` corners.
pub type Range = ((u32, u32), (u32, u32));

pub enum Cmd {
    /// Draw `vp` of sheet `sheet` at `scale` (the latest request wins).
    View { sheet: usize, vp: Viewport, scale: f32 },
    /// Show sheet `i` (it becomes the active sheet, saved with the file).
    Sheet(usize),
    /// What the formula bar shows for a cell.
    Read { sheet: usize, cell: (u32, u32) },
    /// Typed into a cell, as a person types it (`=` starts a formula).
    Edit { sheet: usize, cell: (u32, u32), input: String },
    /// Rows of tab-separated text pasted at `at`.
    Paste { sheet: usize, at: (u32, u32), rows: Vec<Vec<String>> },
    Clear { sheet: usize, range: Range },
    /// Copy `range`: its shown text as TSV and an HTML table.
    Copy { sheet: usize, range: Range },
    Undo,
    Redo,
    /// Insert (`count` > 0) or delete rows or columns at `at`.
    Rows { sheet: usize, at: u32, count: i64 },
    Cols { sheet: usize, at: u32, count: i64 },
    /// Slide chart frame `frame` by `dx`/`dy` sheet pixels (one undo step).
    MoveChart { sheet: usize, frame: String, dx: f32, dy: f32 },
    /// Sizes or hides columns (`cols`) or rows, one undo step for them all.
    Dims { sheet: usize, cols: bool, dims: Vec<(u32, Dim)> },
    /// What an autofit of these columns measures: [`Out::Fit`].
    Fit { sheet: usize, cols: Vec<u32> },
}

/// A column's or row's change: a width in characters or a height in
/// points (`None`: the default, or for a row its automatic height), or
/// hidden / shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dim {
    Size(Option<f64>),
    Hidden(bool),
}

/// A cell's shown text as the grid draws it: its CSS font, the text (a
/// line of it) and its indent level.
#[derive(Clone, Debug, PartialEq)]
pub struct FitText {
    pub font: String,
    pub text: String,
    pub indent: u32,
}

/// The workbook's shape the UI lays out with.
#[derive(Clone)]
pub struct Meta {
    /// Bumped by every change: lists drawn before it are stale.
    pub rev: u64,
    pub sheets: Vec<String>,
    pub active: usize,
    /// Frozen rows and columns of the active sheet.
    pub frozen: (u32, u32),
    /// Its row and column sizes: hit testing, scrolling to a cell.
    pub geometry: Arc<GridGeometry>,
    /// One past the last used row and column.
    pub used: (u32, u32),
    /// The editing engine is ready (until then edits wait).
    pub editable: bool,
    pub history: HistoryState,
}

pub enum Out {
    Meta(Meta),
    /// A drawn window, and the merged ranges in it (a selection grows
    /// over them).
    List { rev: u64, sheet: usize, vp: Viewport, scale: f32, list: Arc<DisplayList>, merges: Arc<Vec<Range>> },
    Cell { sheet: usize, cell: (u32, u32), input: String },
    Copied { tsv: String, html: String },
    /// A note for the person: an edit the engine refused, a save format.
    Note(String),
    /// The texts of the columns asked to autofit, by column.
    Fit { sheet: usize, cols: Vec<(u32, Vec<FitText>)> },
}

/// What the open's helper thread hands over, in order.
enum Stage {
    /// The whole model, when the first paint parsed only the top rows.
    Model(Box<WorkbookModel>),
    /// The editable workbook.
    Full(Result<Workbook, String>),
}

/// The open's remaining work, run on a helper thread after the first paint.
type Rest = Box<dyn FnOnce(&std::sync::mpsc::Sender<Stage>) + Send>;

enum Engine {
    /// Parsed, drawing; the full workbook is on its way.
    Quick { model: Box<WorkbookModel>, ready: Receiver<Stage> },
    Full(Box<Workbook>),
    /// The full workbook couldn't be built: view only.
    ReadOnly(Box<WorkbookModel>),
}

pub struct Book {
    engine: Engine,
    active: usize,
    rev: u64,
    /// The geometry of the active sheet at `rev` (rebuilt after a change).
    geometry: Option<(u64, usize, Arc<GridGeometry>)>,
    view: Option<(usize, Viewport, f32)>,
    /// The open's remaining work, until it starts.
    rest: Option<(Rest, std::sync::mpsc::Sender<Stage>)>,
    opened: std::time::Instant,
    /// Edits made before the full workbook was ready.
    waiting: Vec<Cmd>,
    /// The file's format: `xlsx` (and `xlsm`), `csv`/`tsv` or `xls`.
    format: String,
    meta_sent: bool,
}

fn calc() -> CalculationOptions {
    CalculationOptions::default()
}

fn no_chart(chart: &betteroffice_xlsx::SheetChart) -> Result<Arc<ooxml_drawingml::chart::ChartSpace>, xlsx_render::RenderError> {
    Err(xlsx_render::RenderError::ChartSourceUnavailable { part: chart.part.clone() })
}

impl Book {
    /// The model and the active sheet of `bytes` in `format`, quickly, and
    /// how to build the full workbook.
    fn quick(bytes: Vec<u8>, format: &str, name: &str) -> Result<(WorkbookModel, usize, Rest), String> {
        let full = |bytes: Vec<u8>| -> Rest { Box::new(move |tx| drop(tx.send(Stage::Full(Workbook::open(&bytes).map_err(|e| e.to_string()))))) };
        match format {
            "csv" | "tsv" => {
                let text = String::from_utf8_lossy(&bytes);
                let sep = if format == "tsv" { '\t' } else { ',' };
                let model = convert::delimited_model(&convert::parse_delimited(&text, sep), name);
                let copy = model.clone();
                Ok((model, 0, Box::new(move |tx| drop(tx.send(Stage::Full(Workbook::from_model(copy).map_err(|e| e.to_string())))))))
            }
            "xls" => {
                let xlsx = convert::xls_to_xlsx(&bytes)?;
                let (model, active, _) = parse(&xlsx, None)?;
                Ok((model, active, full(xlsx)))
            }
            _ => {
                // the top of each big sheet first; the rest after the paint
                let (model, active, cut) = parse(&bytes, Some(FIRST_PAINT_BYTES))?;
                if !cut {
                    return Ok((model, active, full(bytes)));
                }
                let rest: Rest = Box::new(move |tx| {
                    if let Ok((model, _, _)) = parse(&bytes, None) {
                        let _ = tx.send(Stage::Model(Box::new(model)));
                    }
                    full(bytes)(tx)
                });
                Ok((model, active, rest))
            }
        }
    }

    fn model(&self) -> &WorkbookModel {
        match &self.engine {
            Engine::Quick { model, .. } | Engine::ReadOnly(model) => model,
            Engine::Full(wb) => wb.model(),
        }
    }

    fn sheet_count(&self) -> usize {
        self.model().sheets.len()
    }

    fn geometry(&mut self) -> Arc<GridGeometry> {
        if let Some((rev, sheet, g)) = &self.geometry
            && *rev == self.rev
            && *sheet == self.active
        {
            return g.clone();
        }
        let model = self.model();
        let sheet = &model.sheets[self.active];
        let g = Arc::new(GridGeometry::new(sheet, &model.styles));
        self.geometry = Some((self.rev, self.active, g.clone()));
        g
    }

    fn meta(&mut self) -> Meta {
        let geometry = self.geometry();
        let model = self.model();
        let sheet = &model.sheets[self.active];
        let used = sheet.used_range().map_or((0, 0), |r| (r.end.row + 1, r.end.col + 1));
        Meta {
            rev: self.rev,
            sheets: model.sheets.iter().map(|s| s.name.clone()).collect(),
            active: self.active,
            frozen: sheet.freeze_pane.map_or((0, 0), |p| (p.rows, p.cols)),
            geometry,
            used,
            editable: matches!(self.engine, Engine::Full(_)),
            history: match &self.engine {
                Engine::Full(wb) => wb.history_state(),
                _ => HistoryState::default(),
            },
        }
    }

    fn draw(&mut self, sheet: usize, vp: Viewport, scale: f32) -> Option<Out> {
        if sheet >= self.sheet_count() || vp.width <= 0.0 || vp.height <= 0.0 {
            return None;
        }
        let list = match &self.engine {
            Engine::Full(wb) => wb.display_list_for(SheetId(sheet as u32), &vp).map_err(|e| e.to_string()),
            Engine::Quick { model, .. } | Engine::ReadOnly(model) => {
                let g = if sheet == self.active { self.geometry() } else { Arc::new(GridGeometry::new(&model.sheets[sheet], &model.styles)) };
                let model = self.model();
                xlsx_render::build_display_list_with_geometry(model, SheetId(sheet as u32), &vp, &[], &g, Some(betteroffice_xlsx::MAX_DISPLAY_CELLS), no_chart)
                    .map_err(|e| e.to_string())
            }
        };
        match list {
            Ok(list) => {
                let list = if scale == 1.0 { list } else { xlsx_render::scaled(list, scale) };
                let merges = Arc::new(self.merges(sheet, &vp));
                Some(Out::List { rev: self.rev, sheet, vp, scale, list: Arc::new(list), merges })
            }
            Err(e) => {
                log::warn!("office: drawing {vp:?}: {e}");
                None
            }
        }
    }

    /// The merged ranges `vp` shows, as the web editor takes them (at most
    /// [`MAX_MERGES`]).
    fn merges(&mut self, sheet: usize, vp: &Viewport) -> Vec<Range> {
        let g = if sheet == self.active { self.geometry() } else { Arc::new(GridGeometry::new(&self.model().sheets[sheet], &self.model().styles)) };
        let s = &self.model().sheets[sheet];
        let mut all = xlsx_render::visible_merged_ranges_with_geometry(s, vp, &g, Some(betteroffice_xlsx::MAX_DISPLAY_CELLS));
        all.truncate(MAX_MERGES);
        all.into_iter().map(|m| ((m.start.row, m.start.col), (m.end.row, m.end.col))).collect()
    }

    /// The shown texts of columns `cols` (an autofit measures them): their
    /// used cells, leaving out those merged across columns, as Excel does.
    fn fit(&self, sheet: usize, cols: Vec<u32>) -> Out {
        let model = self.model();
        let Some(s) = model.sheets.get(sheet) else { return Out::Fit { sheet, cols: Vec::new() } };
        let last = s.used_range().map_or(0, |r| r.end.row);
        let merged = |at: CellRef| s.merges.iter().any(|m| m.start.col != m.end.col && m.contains(at));
        let cols = cols
            .into_iter()
            .map(|c| {
                let range = betteroffice_xlsx::CellRange::new(CellRef::new(0, c), CellRef::new(last, c));
                let mut texts = Vec::new();
                for (at, cell) in s.cells_in_range(range) {
                    let text = xlsx_render::display_text(&model.styles, model.date_system, cell);
                    if text.is_empty() || merged(at) {
                        continue;
                    }
                    let style = cell.style.unwrap_or(0);
                    let font = model.styles.font_for(style);
                    let size = font.and_then(|f| f.size_pt).unwrap_or(11.0) as f32;
                    let css = buwiz_office_kit::draw::css_font(size, font.is_some_and(|f| f.bold), font.is_some_and(|f| f.italic), font.and_then(|f| f.name.as_deref()));
                    // the renderer indents left- and right-aligned text only
                    let a = model.styles.alignment_for(style);
                    let indent = a.filter(|a| a.h.is_some_and(|h| matches!(h.as_sml(), "left" | "right"))).and_then(|a| a.indent).unwrap_or(0);
                    for line in text.split('\n') {
                        texts.push(FitText { font: css.clone(), text: line.to_owned(), indent });
                    }
                }
                (c, texts)
            })
            .collect();
        Out::Fit { sheet, cols }
    }

    /// A command that changes the workbook, on the full engine.
    fn change(wb: &mut Workbook, cmd: Cmd) -> Result<bool, String> {
        let at = |(r, c): (u32, u32)| CellRef::new(r, c);
        let applied = match cmd {
            Cmd::Edit { sheet, cell, input } => wb.edit_cell(SheetId(sheet as u32), at(cell), &input, calc()).map_err(|e| e.to_string())?.applied,
            Cmd::Paste { sheet, at: (r0, c0), rows } => {
                let edits: Vec<CellInput> = rows
                    .iter()
                    .enumerate()
                    .flat_map(|(r, row)| row.iter().enumerate().map(move |(c, input)| CellInput { cell: CellRef::new(r0 + r as u32, c0 + c as u32), input: input.clone() }))
                    .collect();
                wb.edit_cells(SheetId(sheet as u32), &edits, calc()).map_err(|e| e.to_string())?.applied
            }
            Cmd::Clear { sheet, range: ((r0, c0), (r1, c1)) } => {
                let s = wb.sheet(SheetId(sheet as u32)).map_err(|e| e.to_string())?;
                // only the cells that hold something: a whole column is
                // a million empty ones
                let mut edits = Vec::new();
                if let Some(used) = s.used_range() {
                    for r in r0..=r1.min(used.end.row) {
                        for c in c0..=c1.min(used.end.col) {
                            if s.cell(CellRef::new(r, c)).is_some_and(|cell| cell.value != CellValue::Empty || cell.formula.is_some()) {
                                edits.push(CellInput { cell: CellRef::new(r, c), input: String::new() });
                            }
                        }
                    }
                }
                wb.edit_cells(SheetId(sheet as u32), &edits, calc()).map_err(|e| e.to_string())?.applied
            }
            Cmd::Undo => wb.undo(calc()).map_err(|e| e.to_string())?.applied,
            Cmd::Redo => wb.redo(calc()).map_err(|e| e.to_string())?.applied,
            Cmd::Rows { sheet, at, count } => {
                let sheet = SheetId(sheet as u32);
                let op = if count > 0 { Op::InsertRows { sheet, at, count: count as u32 } } else { Op::DeleteRows { sheet, at, count: (-count) as u32 } };
                wb.apply_ops(vec![op], calc()).map_err(|e| e.to_string())?.applied
            }
            Cmd::Cols { sheet, at, count } => {
                let sheet = SheetId(sheet as u32);
                let op = if count > 0 { Op::InsertCols { sheet, at, count: count as u32 } } else { Op::DeleteCols { sheet, at, count: (-count) as u32 } };
                wb.apply_ops(vec![op], calc()).map_err(|e| e.to_string())?.applied
            }
            Cmd::MoveChart { sheet, frame, dx, dy } => wb.move_chart(SheetId(sheet as u32), &frame, dx, dy, calc()).map_err(|e| e.to_string())?.applied,
            Cmd::Dims { sheet, cols, dims } => {
                let sheet = SheetId(sheet as u32);
                let ops = dims
                    .into_iter()
                    .map(|(i, d)| match (cols, d) {
                        (true, Dim::Size(width)) => Op::SetColWidth { sheet, col: i, width },
                        (true, Dim::Hidden(hidden)) => Op::SetColVisibility { sheet, col: i, hidden },
                        (false, Dim::Size(height)) => Op::SetRowHeight { sheet, row: i, height },
                        (false, Dim::Hidden(hidden)) => Op::SetRowVisibility { sheet, row: i, hidden },
                    })
                    .collect::<Vec<_>>();
                if ops.is_empty() {
                    return Ok(false);
                }
                wb.apply_ops(ops, calc()).map_err(|e| e.to_string())?.applied
            }
            _ => false,
        };
        Ok(applied)
    }

    fn read(&self, sheet: usize, cell: (u32, u32)) -> String {
        let at = CellRef::new(cell.0, cell.1);
        match &self.engine {
            Engine::Full(wb) => wb.cell(SheetId(sheet as u32), at).map(|c| c.input).unwrap_or_default(),
            Engine::Quick { model, .. } | Engine::ReadOnly(model) => {
                let Some(c) = model.sheets.get(sheet).and_then(|s| s.cell(at)) else { return String::new() };
                match &c.formula {
                    Some(f) => format!("={f}"),
                    None => xlsx_render::display_text(&model.styles, model.date_system, c),
                }
            }
        }
    }

    fn copy(&self, sheet: usize, ((r0, c0), (r1, c1)): Range) -> Out {
        let model = self.model();
        let Some(s) = model.sheets.get(sheet) else { return Out::Copied { tsv: String::new(), html: String::new() } };
        // a whole row or column copies only as far as the used range
        let (r1, c1) = s.used_range().map_or((r0, c0), |u| (r1.min(u.end.row.max(r0)), c1.min(u.end.col.max(c0))));
        let text = |r: u32, c: u32| s.cell(CellRef::new(r, c)).map(|cell| xlsx_render::display_text(&model.styles, model.date_system, cell)).unwrap_or_default();
        let mut tsv = String::new();
        let mut html = String::from("<table>");
        for r in r0..=r1 {
            html.push_str("<tr>");
            let row: Vec<String> = (c0..=c1).map(|c| text(r, c)).collect();
            for t in &row {
                html.push_str("<td>");
                html.push_str(&escape_html(t));
                html.push_str("</td>");
            }
            html.push_str("</tr>");
            // a cell holding a tab or line break is quoted, as Excel does
            let fields: Vec<String> = row.iter().map(|t| if t.contains(['\t', '\n', '"']) { format!("\"{}\"", t.replace('"', "\"\"")) } else { t.clone() }).collect();
            tsv.push_str(&fields.join("\t"));
            if r < r1 {
                tsv.push('\n');
            }
        }
        html.push_str("</table>");
        Out::Copied { tsv, html }
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\n', "<br>")
}

/// How much of a worksheet's XML the first paint parses: a few
/// screens of rows (the rest comes right after).
const FIRST_PAINT_BYTES: usize = 256 * 1024;

/// The merged ranges a drawn window carries, as the web editor caps them.
const MAX_MERGES: usize = 1024;

/// Parses an `.xlsx` package into its model and active sheet: the model
/// alone (the package BetterOffice keeps for saving costs a third more,
/// and the full workbook parses its own). With `cut`, worksheets longer
/// than that keep only their first rows; the flag says whether any was cut.
fn parse(bytes: &[u8], cut: Option<usize>) -> Result<(WorkbookModel, usize, bool), String> {
    let mut parts = ooxml_opc::unzip_parts(bytes).map_err(|e| format!("Couldn't open the workbook: {e}"))?;
    let mut was_cut = false;
    if let Some(budget) = cut {
        for (name, xml) in parts.iter_mut() {
            if name.trim_start_matches('/').starts_with("xl/worksheets/")
                && let Some(short) = top_rows(xml, budget)
            {
                *xml = short;
                was_cut = true;
            }
        }
    }
    let model = xlsx_parse::parse_workbook(&parts).map_err(|e| format!("Couldn't read the workbook: {e}"))?;
    if model.sheets.is_empty() {
        return Err("The workbook has no sheets".to_owned());
    }
    let workbook = parts.iter().find(|(name, _)| name.trim_start_matches('/') == "xl/workbook.xml");
    let active = workbook.and_then(|(_, xml)| active_tab(&String::from_utf8_lossy(xml))).unwrap_or(0);
    let active = active.min(model.sheets.len() - 1);
    Ok((model, active, was_cut))
}

/// A worksheet's XML with its `<sheetData>` cut after the first row that
/// ends past `budget` bytes, what follows the rows (merges, links, …)
/// kept; `None` when it is short enough (or not laid out as expected).
fn top_rows(xml: &[u8], budget: usize) -> Option<Vec<u8>> {
    let find = |hay: &[u8], needle: &[u8], from: usize| hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from);
    let start = find(xml, b"<sheetData", 0)?;
    let end = find(xml, b"</sheetData>", start)?;
    if end - start <= budget {
        return None;
    }
    let row_end = find(xml, b"</row>", start + budget)? + b"</row>".len();
    if row_end >= end {
        return None;
    }
    let mut out = Vec::with_capacity(row_end + xml.len() - end);
    out.extend_from_slice(&xml[..row_end]);
    out.extend_from_slice(&xml[end..]);
    Some(out)
}

/// `<workbookView … activeTab="N">`: the sheet the file opens on.
fn active_tab(xml: &str) -> Option<usize> {
    let view = &xml[xml.find("<workbookView")?..];
    let view = &view[..view.find('>')?];
    let at = view.find("activeTab=")? + "activeTab=".len();
    let quote = view[at..].chars().next()?;
    let rest = &view[at + 1..];
    rest[..rest.find(quote)?].parse().ok()
}

impl Document for Book {
    type Cmd = Cmd;
    type Out = Out;

    fn open(bytes: Vec<u8>, path: &Path) -> Result<Self, String> {
        let format = super::ext(path);
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Sheet1".to_owned());
        let opened = std::time::Instant::now();
        let (model, active, rest) = Self::quick(bytes, &format, &name)?;
        let (tx, ready) = std::sync::mpsc::channel();
        Ok(Book {
            engine: Engine::Quick { model: Box::new(model), ready },
            active,
            rev: 0,
            geometry: None,
            view: None,
            rest: Some((rest, tx)),
            opened,
            waiting: Vec::new(),
            format,
            meta_sent: false,
        })
    }

    fn run(&mut self, cmd: Cmd) -> (Vec<Out>, bool) {
        match cmd {
            Cmd::View { sheet, vp, scale } => {
                self.view = Some((sheet, vp, scale));
                (Vec::new(), false)
            }
            Cmd::Sheet(i) if i < self.sheet_count() && i != self.active => {
                self.active = i;
                if let Engine::Full(wb) = &mut self.engine {
                    let _ = wb.set_active_sheet(SheetId(i as u32));
                }
                (vec![Out::Meta(self.meta())], false)
            }
            Cmd::Sheet(_) => (Vec::new(), false),
            Cmd::Read { sheet, cell } => (vec![Out::Cell { sheet, cell, input: self.read(sheet, cell) }], false),
            Cmd::Copy { sheet, range } => (vec![self.copy(sheet, range)], false),
            Cmd::Fit { sheet, cols } => (vec![self.fit(sheet, cols)], false),
            cmd => match &mut self.engine {
                Engine::Quick { .. } => {
                    self.waiting.push(cmd);
                    (Vec::new(), false)
                }
                Engine::ReadOnly(_) => (vec![Out::Note("This workbook can be viewed but not edited here".to_owned())], false),
                Engine::Full(wb) => match Self::change(wb, cmd) {
                    Ok(false) => (Vec::new(), false),
                    Ok(true) => {
                        self.rev += 1;
                        (vec![Out::Meta(self.meta())], true)
                    }
                    Err(e) => (vec![Out::Note(e)], false),
                },
            },
        }
    }

    fn tick(&mut self) -> (Vec<Out>, bool) {
        let mut outs = Vec::new();
        let mut changed = false;
        if let Engine::Quick { model, ready } = &mut self.engine {
            match ready.try_recv() {
                Ok(Stage::Model(whole)) => {
                    *model = whole;
                    self.active = self.active.min(model.sheets.len().saturating_sub(1));
                    self.rev += 1;
                    outs.push(Out::Meta(self.meta()));
                }
                Ok(Stage::Full(Ok(mut wb))) => {
                    let _ = wb.set_active_sheet(SheetId(self.active as u32));
                    self.engine = Engine::Full(Box::new(wb));
                    self.rev += 1;
                    let waiting = std::mem::take(&mut self.waiting);
                    for cmd in waiting {
                        let (o, c) = self.run(cmd);
                        outs.extend(o);
                        changed |= c;
                    }
                    outs.push(Out::Meta(self.meta()));
                }
                Ok(Stage::Full(Err(e))) => {
                    log::warn!("office: the workbook opened view-only: {e}");
                    let model = std::mem::take(model);
                    self.engine = Engine::ReadOnly(model);
                    self.waiting.clear();
                    outs.push(Out::Note(format!("This workbook opened view-only: {e}")));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    let model = std::mem::take(model);
                    self.engine = Engine::ReadOnly(model);
                }
            }
        }
        if !self.meta_sent {
            self.meta_sent = true;
            outs.push(Out::Meta(self.meta()));
        }
        if let Some((sheet, vp, scale)) = self.view.take() {
            outs.extend(self.draw(sheet, vp, scale));
        }
        // the rest of the open waits for the first paint (it would compete
        // with it for the cores and the allocator), or a moment
        if self.rest.is_some() && (outs.iter().any(|o| matches!(o, Out::List { .. })) || !self.waiting.is_empty() || self.opened.elapsed().as_millis() > 300) {
            let (rest, tx) = self.rest.take().expect("checked");
            let spawned = std::thread::Builder::new().name("rox-office-open".to_owned()).spawn(move || rest(&tx));
            if let Err(e) = spawned {
                log::warn!("office: no thread for the workbook: {e}");
            }
        }
        (outs, changed)
    }

    fn busy(&self) -> bool {
        matches!(self.engine, Engine::Quick { .. }) || self.view.is_some()
    }

    fn save(&mut self, path: &Path) -> Result<(Vec<u8>, PathBuf), String> {
        let wb = match &self.engine {
            Engine::Full(wb) => wb,
            _ => return Err("The workbook is still opening".to_owned()),
        };
        match self.format.as_str() {
            "csv" | "tsv" => {
                if wb.sheet_count() > 1 {
                    log::info!("office: a {} saves its first sheet only", self.format);
                }
                let sep = if self.format == "tsv" { '\t' } else { ',' };
                Ok((convert::write_delimited(wb.model(), sep).into_bytes(), path.to_owned()))
            }
            "xls" => {
                let bytes = wb.save().map_err(|e| e.to_string())?;
                self.format = "xlsx".to_owned();
                Ok((bytes, path.with_extension("xlsx")))
            }
            _ => Ok((wb.save().map_err(|e| e.to_string())?, path.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs commands and ticks until the full engine is in, as the
    /// session's loop does.
    fn ready(book: &mut Book) -> Vec<Out> {
        let t = std::time::Instant::now();
        let mut outs = Vec::new();
        while !matches!(book.engine, Engine::Full(_)) {
            outs.extend(book.tick().0);
            assert!(t.elapsed().as_secs() < 10);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        outs
    }

    pub fn sample() -> Vec<u8> {
        let mut sheet = betteroffice_xlsx::Sheet::new("Data");
        for r in 0..5u32 {
            sheet.set_cell(CellRef::new(r, 0), betteroffice_xlsx::Cell { value: CellValue::Number { value: (r + 1) as f64 }, ..Default::default() });
            sheet.set_cell(CellRef::new(r, 2), betteroffice_xlsx::Cell { formula: Some(format!("A{}*2", r + 1)), ..Default::default() });
        }
        let model = WorkbookModel { sheets: vec![sheet], ..WorkbookModel::default() };
        let mut wb = Workbook::from_model(model).unwrap();
        wb.recalculate_all(calc());
        wb.save().unwrap()
    }

    fn value(model: &WorkbookModel, r: u32, c: u32) -> CellValue {
        model.sheets[0].cell(CellRef::new(r, c)).map(|c| c.value.clone()).unwrap_or_default()
    }

    #[test]
    fn draws_before_the_engine_is_ready_and_edits_wait_for_it() {
        let mut book = Book::open(sample(), Path::new("a.xlsx")).unwrap();
        book.run(Cmd::View { sheet: 0, vp: Viewport { x: 0.0, y: 0.0, width: 400.0, height: 200.0 }, scale: 1.0 });
        // an edit before the engine: queued, applied once it lands
        book.run(Cmd::Edit { sheet: 0, cell: (1, 0), input: "10".into() });
        let (outs, _) = book.tick();
        assert!(outs.iter().any(|o| matches!(o, Out::List { .. })));
        assert!(outs.iter().any(|o| matches!(o, Out::Meta(m) if m.used == (5, 3))));
        let outs = ready(&mut book);
        assert!(outs.iter().any(|o| matches!(o, Out::Meta(m) if m.editable)));
        assert_eq!(value(book.model(), 1, 2), CellValue::Number { value: 20.0 });
    }

    #[test]
    fn edit_recalc_undo_and_structure() {
        let mut book = Book::open(sample(), Path::new("a.xlsx")).unwrap();
        ready(&mut book);
        let (_, changed) = book.run(Cmd::Edit { sheet: 0, cell: (0, 0), input: "7".into() });
        assert!(changed);
        assert_eq!(value(book.model(), 0, 2), CellValue::Number { value: 14.0 });
        book.run(Cmd::Undo);
        assert_eq!(value(book.model(), 0, 2), CellValue::Number { value: 2.0 });
        book.run(Cmd::Redo);
        assert_eq!(value(book.model(), 0, 2), CellValue::Number { value: 14.0 });
        // a row inserted above moves the formula with its input
        book.run(Cmd::Rows { sheet: 0, at: 0, count: 1 });
        assert_eq!(book.read(0, (1, 2)), "=A2*2");
        book.run(Cmd::Rows { sheet: 0, at: 0, count: -1 });
        book.run(Cmd::Cols { sheet: 0, at: 1, count: -1 });
        assert_eq!(book.read(0, (0, 1)), "=A1*2");
        book.run(Cmd::Clear { sheet: 0, range: ((0, 0), (1_048_575, 0)) });
        assert_eq!(value(book.model(), 0, 0), CellValue::Empty);
        assert_eq!(value(book.model(), 0, 1), CellValue::Number { value: 0.0 });
    }

    #[test]
    fn paste_and_copy() {
        let mut book = Book::open(sample(), Path::new("a.xlsx")).unwrap();
        ready(&mut book);
        book.run(Cmd::Paste { sheet: 0, at: (0, 4), rows: vec![vec!["a".into(), "=A1+1".into()], vec!["b<c".into(), "3".into()]] });
        let (outs, _) = book.run(Cmd::Copy { sheet: 0, range: ((0, 4), (1, 5)) });
        let Some(Out::Copied { tsv, html }) = outs.into_iter().next() else { panic!("copied") };
        assert_eq!(tsv, "a\t2\nb<c\t3");
        assert_eq!(html, "<table><tr><td>a</td><td>2</td></tr><tr><td>b&lt;c</td><td>3</td></tr></table>");
    }

    #[test]
    fn edit_save_reopen_round_trip_checked_by_office_oxide() {
        let mut book = Book::open(sample(), Path::new("a.xlsx")).unwrap();
        ready(&mut book);
        book.run(Cmd::Edit { sheet: 0, cell: (1, 0), input: "21".into() });
        let (bytes, to) = book.save(Path::new("a.xlsx")).unwrap();
        assert_eq!(to, Path::new("a.xlsx"));
        // an independent reader sees the recalculated C2 = A2*2
        let doc = office_oxide::xlsx::XlsxDocument::from_reader(std::io::Cursor::new(bytes.clone())).unwrap();
        let c2 = doc.worksheets[0].rows.iter().find(|r| r.index == 2).and_then(|r| r.cells.iter().find(|c| c.reference.col == 2)).map(|c| c.value.clone());
        assert!(matches!(c2, Some(office_oxide::xlsx::CellValue::Number(v)) if v == 42.0), "{c2:?}");
        // and reopening keeps it
        let (model, _, _) = parse(&bytes, None).unwrap();
        assert_eq!(value(&model, 1, 2), CellValue::Number { value: 42.0 });
        assert_eq!(model.sheets[0].cell(CellRef::new(1, 2)).unwrap().formula.as_deref(), Some("A2*2"));
    }

    #[test]
    fn the_first_paint_parses_the_top_rows_then_the_rest() {
        let mut sheet = betteroffice_xlsx::Sheet::new("Big");
        for r in 0..20_000u32 {
            sheet.set_cell(CellRef::new(r, 0), betteroffice_xlsx::Cell { value: CellValue::Number { value: r as f64 }, ..Default::default() });
        }
        sheet.merges.push(betteroffice_xlsx::CellRange::parse_a1("C1:D2").unwrap());
        let bytes = Workbook::from_model(WorkbookModel { sheets: vec![sheet], ..Default::default() }).unwrap().save().unwrap();
        let (top, _, cut) = parse(&bytes, Some(64 * 1024)).unwrap();
        assert!(cut);
        let rows = top.sheets[0].used_range().unwrap().end.row;
        assert!(rows > 100 && rows < 19_999, "{rows}");
        // what follows the rows survives the cut
        assert_eq!(top.sheets[0].merges.len(), 1);
        let mut book = Book::open(bytes, Path::new("big.xlsx")).unwrap();
        let outs = ready(&mut book);
        assert!(outs.iter().any(|o| matches!(o, Out::Meta(m) if m.used.0 == 20_000)));
        assert_eq!(value(book.model(), 19_999, 0), CellValue::Number { value: 19_999.0 });
    }

    /// The anchor's top left cell of the chart on sheet 0.
    fn chart_from(model: &WorkbookModel) -> (u32, u32) {
        let from = model.sheets[0].charts[0].anchor.from_cell().unwrap();
        (from.row, from.col)
    }

    #[test]
    fn charts_move_and_columns_resize_each_in_one_step_and_save() {
        let mut book = Book::open(super::super::fixture::showcase(), Path::new("a.xlsx")).unwrap();
        ready(&mut book);
        let frame = book.model().sheets[0].charts[0].frame_id();
        assert_eq!(chart_from(book.model()), (1, 7));
        // past a column (64 pixels) right, a row (20) down
        let (_, changed) = book.run(Cmd::MoveChart { sheet: 0, frame: frame.clone(), dx: 70.0, dy: 20.0 });
        assert!(changed);
        assert_eq!(chart_from(book.model()), (2, 8));
        // three columns sized at once, one undo puts all back
        book.run(Cmd::Dims { sheet: 0, cols: true, dims: (1..=3).map(|c| (c, Dim::Size(Some(14.0)))).collect() });
        assert!((1..=3).all(|c| book.model().sheets[0].col_widths.get(&c) == Some(&14.0)));
        book.run(Cmd::Undo);
        assert!((1..=3).all(|c| !book.model().sheets[0].col_widths.contains_key(&c)));
        book.run(Cmd::Redo);
        // hidden and shown again
        book.run(Cmd::Dims { sheet: 0, cols: true, dims: vec![(4, Dim::Hidden(true))] });
        assert!(book.model().sheets[0].col_hidden(4));
        book.run(Cmd::Dims { sheet: 0, cols: true, dims: vec![(4, Dim::Hidden(false))] });
        assert!(!book.model().sheets[0].col_hidden(4));
        // a row's automatic height back
        book.run(Cmd::Dims { sheet: 0, cols: false, dims: vec![(2, Dim::Size(Some(30.0)))] });
        book.run(Cmd::Dims { sheet: 0, cols: false, dims: vec![(2, Dim::Size(None))] });
        assert!(!book.model().sheets[0].row_heights.contains_key(&2));
        // saved and read again: the width and the anchor kept
        let (bytes, _) = book.save(Path::new("a.xlsx")).unwrap();
        let (model, _, _) = parse(&bytes, None).unwrap();
        assert_eq!(model.sheets[0].col_widths.get(&2), Some(&14.0));
        assert_eq!(chart_from(&model), (2, 8));
        // an independent reader opens it
        assert!(office_oxide::xlsx::XlsxDocument::from_reader(std::io::Cursor::new(bytes)).is_ok());
    }

    #[test]
    fn an_autofit_measures_the_shown_text_in_its_font() {
        let mut book = Book::open(super::super::fixture::showcase(), Path::new("a.xlsx")).unwrap();
        ready(&mut book);
        let Out::Fit { cols, .. } = book.fit(0, vec![0, 2]) else { panic!("fit") };
        // the merged title is left out of A
        assert!(cols[0].1.iter().all(|t| t.text != "Q3 Sales Dashboard"), "{:?}", cols[0]);
        let total = cols[1].1.iter().find(|t| t.text.contains("1,504,750")).unwrap();
        assert_eq!(total.text, "$1,504,750.00");
        assert!(total.font.starts_with("bold "), "{}", total.font);
    }

    /// showcase.xlsx's total, C9 (bold Calibri 11, `$1,504,750.00`), in
    /// its column as the file sets it (14 characters, 103 pixels), measured
    /// as the canvas measures it: in Carlito, Calibri's metrics, it fits
    /// and isn't `#####`.
    #[test]
    fn the_showcase_total_fits_its_column_in_calibris_metrics() {
        use nsh_core::canvas::CanvasOp;
        let seed = include_bytes!("../../../server/src/office/seeds/showcase.xlsx");
        let mut book = Book::open(seed.to_vec(), Path::new("showcase.xlsx")).unwrap();
        ready(&mut book);
        let Some(Out::List { list, .. }) = book.draw(0, Viewport { x: 0.0, y: 0.0, width: 800.0, height: 400.0 }, 1.0) else { panic!("a list") };
        let measure = |font: &str, text: &str| buwiz_office_kit::fonts::canvas_width(font, text);
        let d = buwiz_office_kit::draw::drawing(&list, &buwiz_office_kit::draw::Palette::light(), &measure);
        let texts: Vec<String> = d.ops.iter().filter_map(|op| if let CanvasOp::FillText { text, .. } = op { Some(text.to_string()) } else { None }).collect();
        assert!(texts.iter().any(|t| t == "$1,504,750.00"), "{texts:?}");
        assert!(!texts.iter().any(|t| !t.is_empty() && t.chars().all(|c| c == '#')), "{texts:?}");
        // about 87 pixels, against the 99 the column leaves its text
        let w = measure(&buwiz_office_kit::draw::css_font(11.0, true, false, Some("Calibri")), "$1,504,750.00");
        assert!(w > 80.0 && w < 95.0, "{w}");
    }

    #[test]
    fn reads_the_active_tab() {
        assert_eq!(active_tab(r#"<bookViews><workbookView xWindow="0" activeTab="2"/></bookViews>"#), Some(2));
        assert_eq!(active_tab("<bookViews><workbookView xWindow='0' activeTab='1'></workbookView>"), Some(1));
        assert_eq!(active_tab("<bookViews><workbookView/></bookViews>"), None);
    }

    #[test]
    fn xls_opens_and_saves_as_xlsx() {
        let bytes = convert::tests::xls("Old", &[(0, 0, Ok(2.0)), (1, 0, Err("two"))]);
        let mut book = Book::open(bytes, Path::new("/x/old.xls")).unwrap();
        // the first paint comes from the imported workbook
        assert_eq!(book.model().sheets[0].name, "Old");
        ready(&mut book);
        book.run(Cmd::Edit { sheet: 0, cell: (0, 1), input: "=A1*2".into() });
        let (bytes, to) = book.save(Path::new("/x/old.xls")).unwrap();
        assert_eq!(to, Path::new("/x/old.xlsx"));
        let (model, _, _) = parse(&bytes, None).unwrap();
        assert_eq!(value(&model, 0, 1), CellValue::Number { value: 4.0 });
        assert_eq!(value(&model, 1, 0), CellValue::Text { value: "two".into() });
        // saved once as .xlsx, it stays .xlsx
        assert_eq!(book.save(Path::new("/x/old.xlsx")).unwrap().1, Path::new("/x/old.xlsx"));
    }

    #[test]
    fn csv_opens_as_one_sheet_and_saves_as_csv() {
        let src = "name,qty\nwidget,3\n";
        let mut book = Book::open(src.as_bytes().to_vec(), Path::new("/x/list.csv")).unwrap();
        ready(&mut book);
        assert_eq!(book.model().sheets[0].name, "list");
        book.run(Cmd::Edit { sheet: 0, cell: (1, 1), input: "4".into() });
        book.run(Cmd::Edit { sheet: 0, cell: (2, 0), input: "gadget, large".into() });
        let (bytes, to) = book.save(Path::new("/x/list.csv")).unwrap();
        assert_eq!(to, Path::new("/x/list.csv"));
        assert_eq!(String::from_utf8(bytes).unwrap(), "name,qty\nwidget,4\n\"gadget, large\"\n");
    }
}
