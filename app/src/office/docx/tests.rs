//! The Word document through its worker side: open, lay out, edit, undo,
//! find, save and open again. Every document is built in code
//! ([`super::fixture`]).

use std::path::Path;

use super::doc::{Cmd, Fmt, Move, Out, Word};
use super::fixture;
use crate::office::session::Document;

fn open(bytes: Vec<u8>) -> Word {
    let mut w = Word::open(bytes, Path::new("a.docx")).unwrap();
    // the first page, then the rest
    w.tick();
    w.tick();
    w
}

fn meta(outs: &[Out]) -> Option<&super::doc::Meta> {
    outs.iter().rev().find_map(|o| if let Out::Meta(m) = o { Some(m) } else { None })
}

#[test]
fn opens_and_lays_out_every_page() {
    let mut w = Word::open(fixture::document(6), Path::new("a.docx")).unwrap();
    let (first, _) = w.tick();
    assert!(first.iter().any(|o| matches!(o, Out::Page { index: 0, .. })), "the first page is drawn at once");
    assert!(meta(&first).unwrap().partial);
    let (rest, _) = w.tick();
    let m = meta(&rest).unwrap();
    assert!(!m.partial);
    assert!(m.pages.len() >= 6, "{} pages", m.pages.len());
    assert!(!w.busy());
}

/// The body's paragraph texts of saved bytes, read back by the facade.
fn texts(bytes: &[u8]) -> Vec<String> {
    let d = betteroffice_docx::Document::open(bytes).unwrap();
    d.paragraphs().iter().map(|p| betteroffice_docx::get_paragraph_text(p)).collect()
}

fn caret_of(outs: &[Out]) -> Option<&super::doc::Caret> {
    outs.iter().rev().find_map(|o| if let Out::Caret(c) = o { Some(c) } else { None })
}

#[test]
fn typing_relays_out_and_undo_takes_it_back() {
    let mut w = open(fixture::paragraphs(&["Hello world", "Second line"]));
    let (outs, changed) = w.run(Cmd::Move { to: Move::LineEnd, extend: false });
    assert!(!changed);
    assert!(caret_of(&outs).and_then(|c| c.caret).is_some());
    let (outs, changed) = w.run(Cmd::Type("!".into()));
    assert!(changed);
    assert!(outs.iter().any(|o| matches!(o, Out::Page { index: 0, .. })), "the edited page is drawn again");
    assert!(meta(&outs).unwrap().can_undo);
    let (bytes, _) = w.save(Path::new("a.docx")).unwrap();
    assert_eq!(texts(&bytes), vec!["Hello world!", "Second line"]);
    let (_, changed) = w.run(Cmd::Undo);
    assert!(changed);
    w.edited = true;
    let (bytes, _) = w.save(Path::new("a.docx")).unwrap();
    assert_eq!(texts(&bytes), vec!["Hello world", "Second line"]);
}

#[test]
fn enter_backspace_and_paste_split_and_join_paragraphs() {
    let mut w = open(fixture::paragraphs(&["Alpha beta", "Gamma"]));
    w.run(Cmd::Move { to: Move::WordRight, extend: false });
    w.run(Cmd::Enter);
    let (bytes, _) = w.save(Path::new("a.docx")).unwrap();
    assert_eq!(texts(&bytes), vec!["Alpha", " beta", "Gamma"]);
    w.run(Cmd::Backspace);
    w.run(Cmd::Move { to: Move::DocEnd, extend: false });
    w.run(Cmd::Paste("one\ntwo".into()));
    let (bytes, _) = w.save(Path::new("a.docx")).unwrap();
    assert_eq!(texts(&bytes), vec!["Alpha beta", "Gammaone", "two"]);
}

#[test]
fn bold_on_a_selection_saves_as_bold() {
    let mut w = open(fixture::paragraphs(&["make this bold"]));
    w.run(Cmd::Move { to: Move::WordRight, extend: false });
    w.run(Cmd::Move { to: Move::WordRight, extend: true });
    let (outs, _) = w.run(Cmd::Format(Fmt::Bold));
    // the toolbar's B lights for the bold selection
    assert!(meta(&outs).unwrap().fmt.bold);
    let (bytes, _) = w.save(Path::new("a.docx")).unwrap();
    let parts = ooxml_opc::unzip_parts(&bytes).unwrap();
    let xml = String::from_utf8_lossy(&parts.iter().find(|(n, _)| n == "word/document.xml").unwrap().1).into_owned();
    assert!(xml.contains("<w:b/>"), "{xml}");
    assert_eq!(texts(&bytes), vec!["make this bold"]);
}

#[test]
fn a_list_item_is_taken_out_of_its_list() {
    let mut w = open(fixture::document(1));
    // the heading, three paragraphs, then the first bulleted item
    for _ in 0..4 {
        w.run(Cmd::Move { to: Move::Down, extend: false });
    }
    let (outs, _) = w.run(Cmd::Format(Fmt::Bullets));
    assert!(!outs.iter().any(|o| matches!(o, Out::Note(_))), "no error");
}

#[test]
fn find_selects_each_match_in_turn() {
    let mut w = open(fixture::paragraphs(&["one fish", "two fish", "red fish"]));
    let (outs, _) = w.run(Cmd::Find { query: "FISH".into(), back: false });
    assert_eq!(meta(&outs).unwrap().found, (3, 1));
    assert_eq!(caret_of(&outs).unwrap().found.len(), 3);
    let (outs, _) = w.run(Cmd::Find { query: "FISH".into(), back: false });
    assert_eq!(meta(&outs).unwrap().found, (3, 2));
    let (outs, _) = w.run(Cmd::Copy { cut: false });
    assert!(outs.iter().any(|o| matches!(o, Out::Copied(t) if t == "fish")));
}

#[test]
fn tracked_changes_and_comments_are_listed_and_colored() {
    let mut w = open(fixture::reviewed());
    let (outs, _) = w.run(Cmd::View { pages: vec![0], zoom: 1.0 });
    let _ = outs;
    let m = w.run(Cmd::Move { to: Move::Right, extend: false }).0;
    let review = &meta(&m).unwrap().review;
    assert!(review.iter().any(|(a, k, t)| a == "Ada" && k == "Inserted" && t == "inserted"), "{review:?}");
    assert!(review.iter().any(|(a, k, _)| a == "Grace" && k == "Deleted"), "{review:?}");
    assert!(review.iter().any(|(a, k, t)| a == "Ada" && k == "Comment" && t.contains("Check these words")), "{review:?}");
}

#[test]
fn saving_unedited_keeps_the_bytes() {
    let bytes = fixture::document(2);
    let mut w = open(bytes.clone());
    let (saved, to) = w.save(Path::new("x.docx")).unwrap();
    assert_eq!(saved, bytes);
    assert_eq!(to, Path::new("x.docx"));
}

#[test]
fn a_file_that_isnt_a_doc_fails_to_import() {
    assert!(Word::open(b"not a word file".to_vec(), Path::new("a.doc")).is_err());
}

#[test]
fn print_html_has_the_text_and_tables() {
    let mut w = open(fixture::document(1));
    let (outs, _) = w.run(Cmd::Print);
    let html = outs.iter().find_map(|o| if let Out::Print(h) = o { Some(h.clone()) } else { None }).unwrap();
    assert!(html.contains("<h1><b>Section 1</b></h1>"), "{}", &html[..html.len().min(400)]);
    assert!(html.contains("<table") && html.contains("Item 1.1.1"));
    assert!(html.contains("<b>Bold words </b>"));
}

#[test]
fn typing_patches_the_index_as_a_rebuild_would() {
    let mut w = open(fixture::document(2));
    w.run(Cmd::Move { to: Move::Down, extend: false });
    w.run(Cmd::Move { to: Move::WordRight, extend: false });
    for t in ["a", "bc", "é", "😀"] {
        w.run(Cmd::Type(t.into()));
        assert!(w.index_is_fresh(), "after typing {t:?}");
    }
    w.run(Cmd::Backspace);
    w.run(Cmd::Backspace);
    assert!(w.index_is_fresh(), "after Backspace");
}

/// The glyph advances of `word` on page 0 (its first place there, in
/// runs one after another), and whether any text on the page fell back to
/// a synthetic width (a run whose font has no chain is listed as text, one
/// em per character).
fn glyph_width_and_synthetic(w: &Word, word: &str) -> (f64, bool) {
    use docx_layout::display_list::Primitive;
    let page = w.page(0).unwrap();
    let runs: Vec<_> = page.primitives.iter().filter_map(|p| if let Primitive::GlyphRun(g) = p { Some(g) } else { None }).collect();
    let width = (0..runs.len())
        .find_map(|i| {
            let (mut text, mut width) = (String::new(), 0.0);
            for g in &runs[i..] {
                text += &g.text;
                width += g.glyphs.iter().map(|x| x.advance).sum::<f64>();
                if !word.starts_with(text.as_str()) {
                    return None;
                }
                if text == word {
                    return Some(width);
                }
            }
            None
        })
        .unwrap_or(0.0);
    (width, page.primitives.iter().any(|p| matches!(p, Primitive::Text(_))))
}

fn selection_width(outs: &[Out]) -> f64 {
    caret_of(outs).unwrap().sel.iter().map(|r| r.3 as f64).sum()
}

#[test]
fn a_title_word_made_bold_and_italic_keeps_its_selection_to_its_glyphs() {
    let mut w = open(fixture::titled("Welcome to Rox", &["A first paragraph."]));
    w.run(Cmd::Move { to: Move::DocStart, extend: false });
    w.run(Cmd::Move { to: Move::WordRight, extend: true });
    for f in [Fmt::Bold, Fmt::Italic] {
        let (outs, _) = w.run(Cmd::Format(f));
        let (glyphs, synthetic) = glyph_width_and_synthetic(&w, "Welcome");
        assert!(!synthetic, "after {f:?}: no run is measured without its font");
        assert!(glyphs > 0.0, "after {f:?}: the word is a glyph run");
        let sel = selection_width(&outs);
        assert!((sel - glyphs).abs() < 0.5, "after {f:?}: selection {sel} px, glyphs {glyphs} px");
    }
    assert!(w.has_chain("calibri light|1|1"));
    // undo and redo lay out with the chains too
    for cmd in [Cmd::Undo, Cmd::Redo] {
        w.run(cmd);
        assert!(!glyph_width_and_synthetic(&w, "Welcome").1);
    }
}

#[test]
fn typing_in_a_bold_italic_word_keeps_to_its_glyphs() {
    let mut w = open(fixture::titled("Welcome to Rox", &["A first paragraph."]));
    w.run(Cmd::Move { to: Move::DocStart, extend: false });
    w.run(Cmd::Move { to: Move::WordRight, extend: true });
    w.run(Cmd::Format(Fmt::Bold));
    w.run(Cmd::Format(Fmt::Italic));
    // inside the word: typing takes its face, whose chain came with the format
    w.run(Cmd::Move { to: Move::Left, extend: false });
    w.run(Cmd::Move { to: Move::Right, extend: false });
    w.run(Cmd::Type("xy".into()));
    w.run(Cmd::Move { to: Move::DocStart, extend: false });
    let (outs, _) = w.run(Cmd::Move { to: Move::WordRight, extend: true });
    let (glyphs, synthetic) = glyph_width_and_synthetic(&w, "Wxyelcome");
    assert!(!synthetic);
    assert!(glyphs > 0.0);
    assert!((selection_width(&outs) - glyphs).abs() < 0.5);
}

#[test]
fn a_font_a_style_change_brings_in_gets_a_chain() {
    let mut w = open(fixture::paragraphs(&["Plain words here", "Second line"]));
    assert!(!w.has_chain("courier new|0|0"));
    w.run(Cmd::Move { to: Move::WordRight, extend: true });
    let outs = w.set_family("Courier New");
    assert!(w.has_chain("courier new|0|0"));
    let (glyphs, synthetic) = glyph_width_and_synthetic(&w, "Plain");
    assert!(!synthetic);
    assert!((selection_width(&outs) - glyphs).abs() < 0.5);
    // pasted text takes the font where it lands, measured with its chain
    w.run(Cmd::Move { to: Move::DocStart, extend: false });
    w.run(Cmd::Move { to: Move::Right, extend: false });
    w.run(Cmd::Paste("ab\ncd".into()));
    assert!(!glyph_width_and_synthetic(&w, "cdlain").1);
}

/// The lines of page 0 as laid out: the glyph runs' text on each baseline.
fn lines(w: &Word) -> Vec<String> {
    use docx_layout::display_list::Primitive;
    let page = w.page(0).unwrap();
    let mut lines: Vec<(f64, String)> = Vec::new();
    for p in &page.primitives {
        let Primitive::GlyphRun(g) = p else { continue };
        let Some(y) = g.glyphs.first().map(|gl| gl.y) else { continue };
        match lines.last_mut() {
            Some((at, text)) if (*at - y).abs() < 0.5 => *text += &g.text,
            _ => lines.push((y, g.text.clone())),
        }
    }
    lines.into_iter().map(|(_, t)| t.trim().to_owned()).collect()
}

#[test]
fn the_demo_breaks_its_lines_where_the_web_editor_does() {
    let w = open(include_bytes!("../../../../server/src/office/seeds/betteroffice-demo.docx").to_vec());
    let lines = lines(&w);
    // the subtitle in Calibri Light (Carlito's metrics), as the web lays it
    let i = lines.iter().position(|l| l.starts_with("A faithful, open .docx editor")).unwrap_or_else(|| panic!("{lines:?}"));
    assert!(lines[i].ends_with("engine in your"), "{lines:?}");
    assert!(lines[i + 1].starts_with("browser"), "{lines:?}");
}

fn last_meta(w: &mut Word, cmd: Cmd) -> super::doc::Meta {
    let (outs, _) = w.run(cmd);
    meta(&outs).cloned().unwrap_or_else(|| panic!("no meta after the command: {} outs", outs.len()))
}

#[test]
fn insert_puts_a_table_breaks_and_a_picture_at_the_caret() {
    use super::doc::{Image, Insert};
    let mut w = open(fixture::paragraphs(&["Hello world", "Second line"]));
    let pages = meta(&w.run(Cmd::Move { to: Move::WordRight, extend: false }).0).unwrap().pages.len();
    // mid-paragraph: the paragraph splits and the table goes between
    let m = last_meta(&mut w, Cmd::Insert(Insert::Table { rows: 2, cols: 3 }));
    assert_eq!(m.table, Some((2, 3)), "the caret is in the new table's first cell");
    w.run(Cmd::Move { to: Move::DocEnd, extend: false });
    let m = last_meta(&mut w, Cmd::Insert(Insert::PageBreak));
    assert_eq!(m.table, None);
    assert!(m.pages.len() > pages, "a page break adds a page: {} after {pages}", m.pages.len());
    use base64::Engine as _;
    let src = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(fixture::png()));
    let (outs, changed) = w.run(Cmd::Insert(Insert::Image(Image { src: src.clone(), alt: "dot.png".into(), width: 40, height: 20 })));
    assert!(changed);
    let last = meta(&outs).unwrap().pages.len() - 1;
    w.run(Cmd::View { pages: vec![last], zoom: 1.0 });
    let page = w.page(last).unwrap();
    let images: Vec<_> = page.primitives.iter().filter_map(|p| if let docx_layout::display_list::Primitive::Image(i) = p { Some(i) } else { None }).collect();
    assert_eq!(images.len(), 1, "the picture is laid out");
    assert_eq!((images[0].w.as_f64(), images[0].h.as_f64()), (Some(40.0), Some(20.0)));
    assert!(images[0].rel_id.starts_with("data:") || !images[0].rel_id.is_empty(), "{}", &images[0].rel_id[..20.min(images[0].rel_id.len())]);
}

#[test]
fn text_direction_sets_the_paragraphs_bidi() {
    let mut w = open(fixture::paragraphs(&["Hello", "Second"]));
    assert!(!last_meta(&mut w, Cmd::Move { to: Move::Right, extend: false }).bidi);
    assert!(last_meta(&mut w, Cmd::Bidi(true)).bidi);
    assert!(!last_meta(&mut w, Cmd::Bidi(false)).bidi);
}

#[test]
fn page_setup_lays_the_pages_out_again_at_the_new_size_and_margins() {
    use super::doc::PageSetup;
    let mut w = open(fixture::document(2));
    let m = last_meta(&mut w, Cmd::Move { to: Move::Right, extend: false });
    assert_eq!(m.pages[0], (816.0, 1056.0));
    let text_left = |w: &Word| w.page(0).unwrap().primitives.iter().filter_map(|p| if let docx_layout::display_list::Primitive::Text(t) = p { t.x.as_f64() } else { None }).fold(f64::MAX, f64::min);
    let was = text_left(&w);
    let before = m.setup.clone();
    assert_eq!(before.orientation, "portrait");
    // A4 landscape, half-inch margins
    let s = PageSetup { width: 16838.0, height: 11906.0, orientation: "landscape".into(), top: 720.0, bottom: 720.0, left: 720.0, right: 720.0 };
    let m = last_meta(&mut w, Cmd::PageSetup(s.clone()));
    assert_eq!(m.setup, s);
    let (pw, ph) = m.pages[0];
    assert!((pw - 16838.0 / 15.0).abs() < 1.0 && (ph - 11906.0 / 15.0).abs() < 1.0, "{pw}x{ph}");
    let left = text_left(&w);
    assert!((was - left - 48.0).abs() < 2.0, "the text starts half an inch further left: {was} then {left}");
}

#[test]
fn a_text_watermark_is_drawn_on_every_page() {
    let mut w = open(fixture::document(2));
    w.run(Cmd::Move { to: Move::Right, extend: false });
    let wm = serde_json::json!({"kind": "text", "text": "DRAFT", "font": "Calibri", "color": "#C0C0C0", "semitransparent": true, "layout": "diagonal"});
    let m = last_meta(&mut w, Cmd::Watermark(Some(wm.clone())));
    assert_eq!(m.watermark, Some(wm));
    assert!(w.page(0).unwrap().watermark_primitive_count.unwrap_or(0) > 0, "the first page has the watermark: {:?}", w.headers_footers_json());
    last_meta(&mut w, Cmd::Watermark(None));
    assert_eq!(w.page(0).unwrap().watermark_primitive_count.unwrap_or(0), 0);
}

#[test]
fn a_next_page_section_break_starts_a_page_and_a_continuous_one_none() {
    use super::doc::Insert;
    let mut w = open(fixture::paragraphs(&["Hello world", "Second line"]));
    let m = last_meta(&mut w, Cmd::Move { to: Move::WordRight, extend: false });
    assert_eq!(m.pages.len(), 1);
    let m = last_meta(&mut w, Cmd::Insert(Insert::SectionBreak("nextPage")));
    assert_eq!(m.pages.len(), 2, "{}", w.request_sections());
    // a continuous one starts none
    let m = last_meta(&mut w, Cmd::Move { to: Move::DocEnd, extend: false });
    assert_eq!(m.pages.len(), 2);
    w.run(Cmd::Move { to: Move::WordLeft, extend: false });
    let m = last_meta(&mut w, Cmd::Insert(Insert::SectionBreak("continuous")));
    assert_eq!(m.pages.len(), 2, "{}", w.request_sections());
}
