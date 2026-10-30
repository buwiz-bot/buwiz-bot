//! The numbers in `office/docx/BENCH.md`: a generated 200-page document
//! through the view's own path. Ignored by default:
//! `cargo test -p rox --lib office::docx::bench -- --ignored --nocapture`
//! (dependencies build optimized even in dev builds, so the engine's
//! numbers are release numbers).

use std::time::Instant;

use docx_layout::display_list::Primitive;

use super::draw;
use super::fixture;
use super::fonts;
use super::layout::{Layout, ms};

/// The process's peak resident memory, MB (macOS reports bytes).
fn peak_rss() -> f64 {
    let mut ru = [0i64; 18];
    unsafe extern "C" {
        fn getrusage(who: i32, usage: *mut i64) -> i32;
    }
    // SAFETY: `struct rusage` is 18 longs on 64-bit macOS.
    unsafe { getrusage(0, ru.as_mut_ptr()) };
    ru[4] as f64 / 1e6
}

#[test]
#[ignore = "bench: run with --ignored --nocapture"]
fn pages_200() {
    let t = Instant::now();
    let bytes = fixture::document(200);
    println!("generated {} KB in {:.1} ms", bytes.len() / 1024, ms(t));
    let rss0 = peak_rss();

    let t = Instant::now();
    let mut l = Layout::open(&bytes).unwrap();
    let opened = ms(t);
    let t1 = Instant::now();
    l.first(1).unwrap();
    let first = ms(t1);
    let t2 = Instant::now();
    let p0 = l.page(0).unwrap();
    let d0 = draw::page(&p0, &l.fonts, 1.0);
    let drawn0 = ms(t2);
    println!(
        "open (seed + request + fonts) {opened:.1} ms; first page laid out and listed {first:.1} ms (provisional: {}); drawn {drawn0:.1} ms ({} ops); total to first page {:.1} ms",
        l.provisional,
        d0.drawing.ops.len(),
        ms(t)
    );

    let t = Instant::now();
    l.full(0..3).unwrap();
    let full = ms(t);
    let pages = l.page_count();
    println!("full layout + lists of 3 pages: {full:.1} ms, {pages} pages; peak RSS +{:.0} MB", peak_rss() - rss0);

    let mut builds = Vec::new();
    let mut draws = Vec::new();
    let mut ops = Vec::new();
    for i in 3..pages {
        let t = Instant::now();
        l.build(&[i]).unwrap();
        builds.push(ms(t));
        let page = l.page(i).unwrap();
        let t = Instant::now();
        let d = draw::page(&page, &l.fonts, 1.0);
        draws.push(ms(t));
        ops.push(d.drawing.ops.len());
    }
    let stat = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        (v[v.len() / 2], v[v.len() * 95 / 100], v[v.len() - 1])
    };
    let (b50, b95, bmax) = stat(&mut builds);
    let (d50, d95, dmax) = stat(&mut draws);
    ops.sort();
    println!("per page display list: median {b50:.2} / p95 {b95:.2} / max {bmax:.2} ms");
    println!("per page drawing: median {d50:.2} / p95 {d95:.2} / max {dmax:.2} ms; ops median {} max {}", ops[ops.len() / 2], ops[ops.len() - 1]);
    println!("peak RSS +{:.0} MB with every page listed", peak_rss() - rss0);

    // what the pages hold
    let mut kinds = std::collections::BTreeMap::<&str, usize>::new();
    let mut images = Vec::new();
    l.with_pages(|pages| {
        for page in pages {
            let all = page.primitives.iter().chain(page.note_areas.iter().flat_map(|a| a.primitives.iter()));
            for p in all {
                let k = match p {
                    Primitive::Text(_) => "text",
                    Primitive::GlyphRun(_) => "glyphRun",
                    Primitive::Rect(_) => "rect",
                    Primitive::Line(_) => "line",
                    Primitive::Image(i) => {
                        if images.len() < 2 {
                            images.push(format!("{} at {}x{} ({} chars)", &i.rel_id[..i.rel_id.len().min(40)], i.w, i.h, i.rel_id.len()));
                        }
                        "image"
                    }
                    Primitive::Shape(_) => "shape",
                    Primitive::Decoration(_) => "decoration",
                };
                *kinds.entry(k).or_default() += 1;
            }
        }
    });
    println!("primitives: {kinds:?}");
    println!("images: {images:?}");

    // the shaper's widths against CoreText's, word by word (as drawn)
    let mut worst: (f64, String) = (0.0, String::new());
    let mut sum = 0.0;
    let mut n = 0usize;
    for i in 0..3 {
        let page = l.page(i).unwrap();
        for p in &page.primitives {
            let Primitive::GlyphRun(g) = p else { continue };
            let Some(face) = l.fonts.face(g.font_id) else { continue };
            let mut start = 0;
            for k in 0..g.glyphs.len() {
                let at = g.glyphs[k].cluster as usize;
                let end = if k + 1 < g.glyphs.len() { g.glyphs[k + 1].cluster as usize } else { g.text.len() };
                if !(g.text.get(at..end).is_some_and(|s| s.trim().is_empty()) || k + 1 == g.glyphs.len()) {
                    continue;
                }
                let a = g.glyphs[start].cluster as usize;
                let word = g.text[a..end].trim_end();
                let last = k - usize::from(word.len() < g.text[a..end].len() && k > start);
                let shaped = g.glyphs[last].x + g.glyphs[last].advance - g.glyphs[start].x;
                let ct = fonts::coretext_width(face, g.size, word);
                let d = (shaped - ct).abs();
                if !word.is_empty() {
                    sum += d;
                    n += 1;
                    if d > worst.0 {
                        worst = (d, format!("{word:?} in {} {}px: shaped {shaped:.2}, CoreText {ct:.2}", face.family, g.size));
                    }
                }
                start = k + 1;
            }
        }
    }
    println!("word widths, shaper vs CoreText: {n} words, mean |diff| {:.3} px, worst {:.3} px ({})", sum / n.max(1) as f64, worst.0, worst.1);
}

#[test]
#[ignore = "probe: run with --ignored --nocapture"]
fn probe() {
    let bytes = fixture::document(2);
    let l = Layout::open(&bytes).unwrap();
    let doc = l.engine.doc();
    for (i, s) in doc.story_segments("body").unwrap().iter().take(40).enumerate() {
        println!("{i}: {:?} {:?}", s.content, s.attributes);
    }
    println!("stories: {:?}", doc.list_stories().stories.iter().map(|s| s.story.clone()).collect::<Vec<_>>());
    println!("outline: {:?}", doc.geometry_position_outline("body"));
    let rev = fixture::reviewed();
    let l2 = Layout::open(&rev).unwrap();
    for s in l2.engine.doc().story_segments("body").unwrap() {
        println!("rev: {:?} {:?}", s.content, s.attributes);
    }
    println!("changes: {:?}", l2.engine.doc().list_changes("body"));
    println!("comments: {:?}", l2.engine.doc().list_comments());
}

#[test]
#[ignore = "bench: run with --ignored --nocapture"]
fn parts_200() {
    let bytes = fixture::document(200);
    let t = Instant::now();
    let _ = docx_parse::parse_docx_s9_wire(&bytes, Default::default()).unwrap();
    println!("parse (request) {:.1} ms", ms(t));
    let fonts = super::fonts::Fonts::new();
    let engine = docx_edit::EngineSession::new(7);
    let t = Instant::now();
    {
        let _s = fonts.enter();
        docx_edit::seed_from_docx(engine.doc(), &bytes).unwrap();
    }
    println!("seed {:.1} ms", ms(t));
    let mut f2 = super::fonts::Fonts::new();
    let t = Instant::now();
    f2.chain("arial|0|0");
    println!("fonts arial + fallback {:.1} ms", ms(t));
    let engine2 = docx_edit::EngineSession::new(8);
    let t = Instant::now();
    {
        let _s = fonts.enter();
        let ok = docx_edit::seed_docx_preview(engine2.doc(), &bytes, 40).unwrap();
        println!("preview seed (40 blocks) {:.1} ms ok={ok}", ms(t));
    }
    let mut l = Layout::open(&bytes).unwrap();
    l.full(0..1).unwrap();
    let t = Instant::now();
    l.engine.set_display_window(None);
    let _s = l.fonts.enter();
    l.engine.build_display_list_frame(&serde_json::json!({ "fontChains": l.fonts.chains_json() }).to_string(), 0).unwrap();
    drop(_s);
    println!("all 200 pages' lists in one build {:.1} ms; peak RSS {:.0} MB", ms(t), peak_rss());
    let t = Instant::now();
    let _s = l.fonts.enter();
    l.engine.set_display_window(Some(100..101));
    l.engine.build_display_list_frame(&serde_json::json!({ "fontChains": l.fonts.chains_json() }).to_string(), 0).unwrap();
    println!("a windowed rebuild {:.1} ms", ms(t));
}
