//! The numbers in `office/BENCH.md`: generated workbooks through the
//! sheet's own path (open, first paint, the full engine, scrolled
//! windows, edits on 10k formulas). Ignored by default:
//! `cargo test -p rox --lib office::xlsx::bench -- --ignored --nocapture`
//! (dependencies build optimized even in dev builds, so the engine's
//! numbers are release numbers).

use std::path::Path;
use std::time::Instant;

use betteroffice_xlsx::{Cell, CellRef, CellValue, Sheet, Viewport, Workbook, WorkbookModel};

use super::doc::{Book, Cmd, Out};
use crate::office::draw::{self, Palette};
use crate::office::session::Document;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// The process's peak resident memory, MB (macOS reports bytes).
fn peak_rss() -> f64 {
    let mut ru: Rusage = unsafe { std::mem::zeroed() };
    unsafe { getrusage(0, &mut ru) };
    ru.ru_maxrss as f64 / 1e6
}

#[repr(C)]
struct Rusage {
    ru_utime: [i64; 2],
    ru_stime: [i64; 2],
    ru_maxrss: i64,
    rest: [i64; 13],
}

unsafe extern "C" {
    fn getrusage(who: i32, usage: *mut Rusage) -> i32;
}

/// `rows`×`cols` cells, a quarter of them text.
fn workbook(rows: u32, cols: u32) -> Vec<u8> {
    let mut sheet = Sheet::new("Data");
    for r in 0..rows {
        for c in 0..cols {
            let value = if c % 4 == 0 { CellValue::Text { value: format!("r{r}c{c}") } } else { CellValue::Number { value: (r * cols + c) as f64 * 1.25 } };
            sheet.set_cell(CellRef::new(r, c), Cell { value, ..Cell::default() });
        }
    }
    Workbook::from_model(WorkbookModel { sheets: vec![sheet], ..WorkbookModel::default() }).unwrap().save().unwrap()
}

/// A 60-row × 40-column window from row `row` (default sizes).
fn window(row: u32) -> Viewport {
    Viewport { x: 0.0, y: row as f32 * 20.0, width: 40.0 * 64.0, height: 60.0 * 20.0 }
}

fn list(book: &mut Book, vp: Viewport) -> (f64, usize, std::sync::Arc<betteroffice_xlsx::DisplayList>) {
    let t = Instant::now();
    book.run(Cmd::View { sheet: 0, vp, scale: 1.0 });
    let (outs, _) = book.tick();
    let took = ms(t);
    let l = outs.into_iter().find_map(|o| if let Out::List { list, .. } = o { Some(list) } else { None }).expect("a list");
    (took, l.commands.len(), l)
}

fn measure(_: &str, s: &str) -> f32 {
    s.len() as f32 * 6.5
}

fn wait_full(book: &mut Book) -> f64 {
    let t = Instant::now();
    loop {
        let (outs, _) = book.tick();
        if outs.iter().any(|o| matches!(o, Out::Meta(m) if m.editable)) {
            return ms(t);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn open_bench(rows: u32, cols: u32) {
    let bytes = workbook(rows, cols);
    let size = bytes.len();
    let t = Instant::now();
    let mut book = Book::open(bytes, Path::new("bench.xlsx")).unwrap();
    let open_ms = ms(t);
    let (first_ms, cmds, l) = list(&mut book, window(0));
    let t = Instant::now();
    let d = draw::drawing(&l, &Palette::light(), &measure);
    let draw_ms = ms(t);
    let first_paint = open_ms + first_ms + draw_ms;
    let full_ms = wait_full(&mut book) + first_paint;
    // windows down the sheet, scrolled once the whole sheet is in
    let mut full = Vec::new();
    for i in 0..20 {
        full.push(list(&mut book, window((rows - 60) * i / 20)).0);
    }
    full.sort_by(f64::total_cmp);
    let peak = peak_rss();
    println!(
        "{}k cells ({size} B): parse {open_ms:.0} ms + 60x40 list {first_ms:.1} ms ({cmds} cmds) + drawing {draw_ms:.1} ms ({} ops) = first paint {first_paint:.0} ms; editable after {full_ms:.0} ms; scrolled 60x40 lists median {:.1} ms (max {:.1}); peak RSS {peak:.0} MB",
        rows * cols / 1000,
        d.ops.len(),
        full[10],
        full[19],
    );
}

#[test]
#[ignore]
fn open_100k() {
    open_bench(1000, 100);
}

#[test]
#[ignore]
fn open_1m() {
    open_bench(10_000, 100);
}

#[test]
#[ignore]
fn edit_10k_formulas() {
    let n = 10_000u32;
    let mut sheet = Sheet::new("Calc");
    for r in 0..n {
        sheet.set_cell(CellRef::new(r, 0), Cell { value: CellValue::Number { value: r as f64 }, ..Cell::default() });
        sheet.set_cell(CellRef::new(r, 2), Cell { formula: Some(format!("A{}*2", r + 1)), ..Cell::default() });
    }
    sheet.set_cell(CellRef::new(0, 4), Cell { formula: Some(format!("SUM(C1:C{n})")), ..Cell::default() });
    let mut wb = Workbook::from_model(WorkbookModel { sheets: vec![sheet], ..WorkbookModel::default() }).unwrap();
    wb.recalculate_all(Default::default());
    let bytes = wb.save().unwrap();
    let mut book = Book::open(bytes, Path::new("calc.xlsx")).unwrap();
    let ready = wait_full(&mut book);
    let vp = window(4000);
    let mut times = Vec::new();
    for i in 0..50u32 {
        let t = Instant::now();
        let (_, changed) = book.run(Cmd::Edit { sheet: 0, cell: (4000 + i, 0), input: format!("{}", i * 3) });
        assert!(changed);
        book.run(Cmd::View { sheet: 0, vp, scale: 1.0 });
        let (outs, _) = book.tick();
        let l = outs.into_iter().find_map(|o| if let Out::List { list, .. } = o { Some(list) } else { None }).unwrap();
        let d = draw::drawing(&l, &Palette::light(), &measure);
        times.push(ms(t));
        std::hint::black_box(d);
    }
    times.sort_by(f64::total_cmp);
    let t = Instant::now();
    book.run(Cmd::Undo);
    let undo = ms(t);
    let t = Instant::now();
    let saved = book.save(Path::new("calc.xlsx")).unwrap().0.len();
    let save = ms(t);
    println!(
        "10k formulas (C=A*2, E1=SUM(C)): editable after {ready:.0} ms; edit + recalc + 60x40 list + drawing median {:.2} ms (max {:.2}); undo {undo:.1} ms; save {save:.0} ms ({saved} B)",
        times[25], times[49]
    );
}
