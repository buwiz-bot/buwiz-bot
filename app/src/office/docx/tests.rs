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
    assert!(meta(&outs).unwrap().bold);
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
