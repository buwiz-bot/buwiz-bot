//! The formatting bar's commands through the worker: each one's effect in
//! the engine, what the bar then shows of the selection, and what the save
//! writes for it (read back by docx-parse and office_oxide).

use std::io::Cursor;
use std::path::Path;

use super::doc::{Cmd, EditMode, Fmt, Insert, Link, Meta, Move, Out, Picture, TableOp, TextColor, Word};
use super::fixture;
use crate::office::session::Document;

fn open(bytes: Vec<u8>) -> Word {
    let mut w = Word::open(bytes, Path::new("a.docx")).unwrap();
    w.tick();
    w.tick();
    w
}

/// The last meta a command sent, with no note (an error) among its answers.
fn ok(w: &mut Word, cmd: Cmd) -> Meta {
    let (outs, _) = w.run(cmd);
    if let Some(n) = outs.iter().find_map(|o| if let Out::Note(n) = o { Some(n) } else { None }) {
        panic!("a note: {n}");
    }
    outs.into_iter().rev().find_map(|o| if let Out::Meta(m) = o { Some(*m) } else { None }).unwrap_or_else(|| meta(w))
}

/// The meta as it is now (a caret move reads it again).
fn meta(w: &mut Word) -> Meta {
    let (outs, _) = w.run(Cmd::Find { query: String::new(), back: false });
    outs.into_iter().rev().find_map(|o| if let Out::Meta(m) = o { Some(*m) } else { None }).unwrap()
}

/// The saved document part.
fn saved(w: &mut Word) -> String {
    w.edited = true;
    let (bytes, _) = w.save(Path::new("a.docx")).unwrap();
    docx_parse::parse_docx_s9_wire(&bytes, Default::default()).expect("docx-parse reads the save");
    office_oxide::Document::from_reader(Cursor::new(bytes.clone()), office_oxide::DocumentFormat::Docx).expect("office_oxide reads the save");
    part(&bytes, "word/document.xml")
}

fn part(bytes: &[u8], name: &str) -> String {
    let parts = ooxml_opc::unzip_parts(bytes).unwrap();
    parts.iter().find(|(n, _)| n == name).map(|(_, b)| String::from_utf8_lossy(b).into_owned()).unwrap_or_default()
}

/// "make this bold": the second word selected.
fn word_selected(lines: &[&str]) -> Word {
    let mut w = open(fixture::paragraphs(lines));
    w.run(Cmd::Move { to: Move::WordRight, extend: false });
    w.run(Cmd::Move { to: Move::Right, extend: false });
    w.run(Cmd::Move { to: Move::WordRight, extend: true });
    w
}

#[test]
fn each_toggle_sets_its_mark_and_lights() {
    let mut w = word_selected(&["make this bold"]);
    type Lit = fn(&Meta) -> bool;
    let marks: [(Fmt, Lit); 5] = [(Fmt::Bold, |m| m.fmt.bold), (Fmt::Italic, |m| m.fmt.italic), (Fmt::Underline, |m| m.fmt.underline), (Fmt::Strike, |m| m.fmt.strike), (Fmt::Superscript, |m| m.fmt.superscript)];
    for (f, on) in marks {
        assert!(!on(&meta(&mut w)), "{f:?} off first");
        assert!(on(&ok(&mut w, Cmd::Format(f))), "{f:?} lit");
    }
    // subscript takes superscript's place
    let m = ok(&mut w, Cmd::Format(Fmt::Subscript));
    assert!(m.fmt.subscript && !m.fmt.superscript);
    let xml = saved(&mut w);
    for want in ["<w:b/>", "<w:i/>", r#"<w:u w:val="single"/>"#, "<w:strike/>", r#"<w:vertAlign w:val="subscript"/>"#] {
        assert!(xml.contains(want), "{want} in {xml}");
    }
    // Clear formatting takes every mark off
    let m = ok(&mut w, Cmd::Format(Fmt::Clear));
    assert!(!m.fmt.bold && !m.fmt.italic && !m.fmt.underline && !m.fmt.strike && !m.fmt.subscript);
}

#[test]
fn a_style_applies_and_shows() {
    let mut w = open(fixture::paragraphs(&["A heading", "Body"]));
    let m = meta(&mut w);
    assert_eq!(m.fmt.style, "Normal");
    assert!(m.styles.iter().any(|s| s.id == "Heading1"), "{:?}", m.styles);
    let m = ok(&mut w, Cmd::Style("Heading1".into()));
    assert_eq!(m.fmt.style, "Heading1");
    assert!(saved(&mut w).contains(r#"<w:pStyle w:val="Heading1"/>"#));
}

#[test]
fn a_font_family_and_size_apply_and_show() {
    let mut w = word_selected(&["make this big"]);
    let m = meta(&mut w);
    // the run defaults: the package's
    assert_eq!((m.fmt.family.as_str(), m.fmt.size), ("Arial", 11.0));
    let m = ok(&mut w, Cmd::FontFamily("Georgia".into()));
    assert_eq!(m.fmt.family, "Georgia");
    let m = ok(&mut w, Cmd::FontSize(18.0));
    assert_eq!(m.fmt.size, 18.0);
    assert_eq!(ok(&mut w, Cmd::Format(Fmt::Size(true))).fmt.size, 20.0);
    let xml = saved(&mut w);
    assert!(xml.contains(r#"w:ascii="Georgia""#) && xml.contains(r#"<w:sz w:val="40"/>"#), "{xml}");
}

#[test]
fn colours_and_highlight_apply_and_show() {
    let mut w = word_selected(&["make this red"]);
    let m = ok(&mut w, Cmd::Color(Some(TextColor::Rgb("FF0000".into()))));
    assert_eq!(m.fmt.color.as_deref(), Some("FF0000"));
    let m = ok(&mut w, Cmd::Highlight(Some("FFFF00".into())));
    assert_eq!(m.fmt.highlight.as_deref(), Some("FFFF00"));
    let xml = saved(&mut w);
    assert!(xml.contains(r#"<w:color w:val="FF0000"/>"#) && xml.contains(r#"<w:highlight w:val="yellow"/>"#), "{xml}");
    // a theme colour keeps its slot; Automatic and No Color take them off
    let m = ok(&mut w, Cmd::Color(Some(TextColor::Theme("accent1".into()))));
    assert_eq!(m.fmt.color.as_deref(), Some("4472C4"));
    assert!(saved(&mut w).contains(r#"w:themeColor="accent1""#));
    let m = ok(&mut w, Cmd::Color(None));
    let m2 = ok(&mut w, Cmd::Highlight(None));
    assert!(m.fmt.color.is_none() && m2.fmt.highlight.is_none());
}

#[test]
fn a_link_inserts_edits_and_comes_off() {
    let mut w = word_selected(&["Visit site today"]);
    let m = ok(&mut w, Cmd::Link(Link { href: "https://example.com".into(), text: Some("site".into()), tooltip: None, edit: false }));
    // the caret after the link, still in it
    assert_eq!(m.fmt.link.as_ref().map(|l| l.0.as_str()), Some("https://example.com"));
    assert_eq!(m.fmt.link_text, "site");
    let xml = saved(&mut w);
    assert!(xml.contains("<w:hyperlink"), "{xml}");
    // edited: a new address and text over the whole link
    w.run(Cmd::Move { to: Move::Left, extend: false });
    let m = ok(&mut w, Cmd::Link(Link { href: "https://example.org".into(), text: Some("the site".into()), tooltip: Some("Go".into()), edit: true }));
    assert_eq!(m.fmt.link, Some(("https://example.org".into(), Some("Go".into()))));
    w.edited = true;
    let (bytes, _) = w.save(Path::new("a.docx")).unwrap();
    let rels = part(&bytes, "word/_rels/document.xml.rels");
    assert!(rels.contains("https://example.org") && !rels.contains("https://example.com"), "{rels}");
    let d = betteroffice_docx::Document::open(&bytes).unwrap();
    assert_eq!(betteroffice_docx::get_paragraph_text(d.paragraphs()[0]), "Visit the site today");
    w.run(Cmd::Move { to: Move::Left, extend: false });
    let m = ok(&mut w, Cmd::Unlink);
    assert!(m.fmt.link.is_none());
    assert!(!saved(&mut w).contains("<w:hyperlink"));
}

#[test]
fn each_alignment_sets_the_paragraph() {
    let mut w = open(fixture::paragraphs(&["Line"]));
    assert_eq!(meta(&mut w).fmt.align, "left");
    for (a, jc) in [("center", "center"), ("right", "right"), ("both", "both"), ("left", "left")] {
        assert_eq!(ok(&mut w, Cmd::Format(Fmt::Align(a))).fmt.align, a);
        assert!(saved(&mut w).contains(&format!(r#"<w:jc w:val="{jc}"/>"#)), "{a}");
    }
}

#[test]
fn lists_toggle_and_indent_by_level() {
    let mut w = open(fixture::paragraphs(&["One", "Two"]));
    let m = ok(&mut w, Cmd::Format(Fmt::Bullets));
    assert_eq!(m.fmt.list, Some((1, 0)));
    assert_eq!(ok(&mut w, Cmd::Format(Fmt::Indent(true))).fmt.list, Some((1, 1)));
    let xml = saved(&mut w);
    assert!(xml.contains(r#"<w:ilvl w:val="1"/>"#) && xml.contains(r#"<w:numId w:val="1"/>"#), "{xml}");
    assert_eq!(ok(&mut w, Cmd::Format(Fmt::Indent(false))).fmt.list, Some((1, 0)));
    // out from the first level: out of the list
    assert_eq!(ok(&mut w, Cmd::Format(Fmt::Indent(false))).fmt.list, None);
    // numbers, then numbers again: off
    assert_eq!(ok(&mut w, Cmd::Format(Fmt::Numbers)).fmt.list, Some((2, 0)));
    assert_eq!(ok(&mut w, Cmd::Format(Fmt::Numbers)).fmt.list, None);
    // no list: the left indent by 720, down to none
    assert_eq!(ok(&mut w, Cmd::Format(Fmt::Indent(true))).fmt.indent_left, 720.0);
    assert!(saved(&mut w).contains(r#"w:left="720""#));
    assert_eq!(ok(&mut w, Cmd::Format(Fmt::Indent(false))).fmt.indent_left, 0.0);
}

#[test]
fn line_spacing_sets_the_line() {
    let mut w = open(fixture::paragraphs(&["Line"]));
    let m = ok(&mut w, Cmd::Format(Fmt::LineSpacing(360)));
    assert_eq!(m.fmt.line_spacing, Some(360.0));
    let xml = saved(&mut w);
    // `auto` is Word's default rule: the writer leaves it out
    assert!(xml.contains(r#"<w:spacing w:line="360"/>"#), "{xml}");
}

#[test]
fn the_table_group_reads_the_cell_and_its_commands_work() {
    let mut w = open(fixture::paragraphs(&["Before", "After"]));
    assert!(meta(&mut w).cell.is_none());
    let m = ok(&mut w, Cmd::Insert(Insert::Table { rows: 2, cols: 2 }));
    let c = m.cell.expect("in the table");
    assert_eq!((c.rows, c.cols, c.multi), (2, 2, false));
    assert_eq!(ok(&mut w, Cmd::Table(TableOp::RowBelow)).cell.unwrap().rows, 3);
    assert_eq!(ok(&mut w, Cmd::Table(TableOp::ColumnRight)).cell.unwrap().cols, 3);
    // a row above keeps the caret with its cell
    let m = ok(&mut w, Cmd::Table(TableOp::RowAbove));
    assert_eq!(m.cell.unwrap().rows, 4);
    let m = ok(&mut w, Cmd::Table(TableOp::Fill(Some("FFFF00".into()))));
    assert_eq!(m.cell.unwrap().fill.as_deref(), Some("FFFF00"));
    ok(&mut w, Cmd::Table(TableOp::BorderWidth(12)));
    let m = ok(&mut w, Cmd::Table(TableOp::BorderColor("FF0000".into())));
    assert_eq!(m.cell.unwrap().border.as_deref(), Some("FF0000"));
    ok(&mut w, Cmd::Table(TableOp::Justify("center")));
    let xml = saved(&mut w);
    for want in [r#"w:fill="FFFF00""#, r#"w:color="FF0000""#, r#"w:sz="12""#, r#"<w:jc w:val="center"/>"#] {
        assert!(xml.contains(want), "{want} in {xml}");
    }
    // merge wants more than one cell: the whole table selected
    let (outs, _) = w.run(Cmd::Table(TableOp::Merge));
    assert!(outs.iter().any(|o| matches!(o, Out::Note(n) if n == "Select more than one cell.")));
    let m = ok(&mut w, Cmd::Table(TableOp::SelectTable));
    assert!(m.cell.unwrap().multi);
    let m = ok(&mut w, Cmd::Table(TableOp::Merge));
    assert!(m.cell.unwrap().can_split, "one merged cell");
    let m = ok(&mut w, Cmd::Table(TableOp::DeleteTable));
    assert!(m.cell.is_none());
    assert!(!saved(&mut w).contains("<w:tbl>"));
}

#[test]
fn rows_and_columns_delete_down_to_one() {
    let mut w = open(fixture::paragraphs(&["Before"]));
    ok(&mut w, Cmd::Insert(Insert::Table { rows: 2, cols: 2 }));
    assert_eq!(ok(&mut w, Cmd::Table(TableOp::DeleteRow)).cell.unwrap().rows, 1);
    let (outs, _) = w.run(Cmd::Table(TableOp::DeleteRow));
    assert!(outs.iter().any(|o| matches!(o, Out::Note(n) if n == "A table needs at least one row.")));
    assert_eq!(ok(&mut w, Cmd::Table(TableOp::DeleteColumn)).cell.unwrap().cols, 1);
}

#[test]
fn the_borders_menu_sets_its_sides() {
    let mut w = open(fixture::paragraphs(&["Before"]));
    ok(&mut w, Cmd::Insert(Insert::Table { rows: 1, cols: 1 }));
    ok(&mut w, Cmd::Table(TableOp::Borders("none")));
    ok(&mut w, Cmd::Table(TableOp::BorderWidth(24)));
    ok(&mut w, Cmd::Table(TableOp::Borders("none")));
    ok(&mut w, Cmd::Table(TableOp::Borders("top")));
    let xml = saved(&mut w);
    let tc = &xml[xml.find("<w:tcBorders>").expect("borders")..];
    let tc = &tc[..tc.find("</w:tcBorders>").unwrap()];
    // the top at the width last chosen, the rest none
    assert!(tc.contains(r#"<w:top w:val="single" w:sz="24""#) && tc.contains(r#"<w:bottom w:val="none""#), "{tc}");
}

/// A document with a picture inserted after "Picture", the picture selected.
fn with_picture() -> Word {
    let mut w = open(fixture::paragraphs(&["Picture here"]));
    w.run(Cmd::Move { to: Move::WordRight, extend: false });
    let img = super::chrome::picture(&fixture::png(), "dot.png").unwrap();
    ok(&mut w, Cmd::Insert(Insert::Image(img)));
    // the caret after it: Shift+Left takes it
    w.run(Cmd::Move { to: Move::Left, extend: true });
    w
}

#[test]
fn the_image_group_shows_for_a_selected_picture_and_wraps_and_turns_it() {
    let mut w = with_picture();
    let m = meta(&mut w);
    assert_eq!(m.fmt.image.as_deref(), Some("inline"));
    let m = ok(&mut w, Cmd::Picture(Picture::Wrap("wrapRight")));
    assert_eq!(m.fmt.image.as_deref(), Some("wrapRight"));
    ok(&mut w, Cmd::Picture(Picture::Transform("rotateCW")));
    let xml = saved(&mut w);
    assert!(xml.contains("<wp:anchor") && xml.contains("<wp:wrapSquare"), "{xml}");
    assert!(xml.contains(r#"rot="5400000""#), "a quarter turn: {xml}");
    // a caret elsewhere: no picture
    w.run(Cmd::Move { to: Move::DocEnd, extend: false });
    assert!(meta(&mut w).fmt.image.is_none());
}

#[test]
fn suggesting_tracks_typing_and_deleting() {
    let mut w = open(fixture::paragraphs(&["Plain words"]));
    assert_eq!(ok(&mut w, Cmd::Mode(EditMode::Suggesting)).mode, EditMode::Suggesting);
    w.run(Cmd::Move { to: Move::LineEnd, extend: false });
    ok(&mut w, Cmd::Type(" added".into()));
    w.run(Cmd::Move { to: Move::DocStart, extend: false });
    w.run(Cmd::Move { to: Move::WordRight, extend: true });
    ok(&mut w, Cmd::Backspace);
    let xml = saved(&mut w);
    assert!(xml.contains("<w:ins ") && xml.contains(r#"w:author="Rox""#), "{xml}");
    assert!(xml.contains("<w:del ") && xml.contains("<w:delText"), "{xml}");
    // what Suggesting can't track is refused
    let (outs, _) = w.run(Cmd::Insert(Insert::PageBreak));
    assert!(outs.iter().any(|o| matches!(o, Out::Note(n) if n == "This change can't be recorded as a suggestion.")));
}

#[test]
fn viewing_takes_no_edit() {
    let mut w = open(fixture::paragraphs(&["Read only"]));
    ok(&mut w, Cmd::Mode(EditMode::Viewing));
    let rev = meta(&mut w).rev;
    for cmd in [Cmd::Type("x".into()), Cmd::Format(Fmt::Bold), Cmd::Enter, Cmd::Style("Heading1".into())] {
        let (outs, changed) = w.run(cmd);
        assert!(!changed && outs.iter().any(|o| matches!(o, Out::Note(n) if n == "Switch to Editing or Suggesting to make changes.")));
    }
    // moving and selecting still do
    let (_, changed) = w.run(Cmd::Move { to: Move::Right, extend: true });
    assert!(!changed);
    assert_eq!(meta(&mut w).rev, rev);
    ok(&mut w, Cmd::Mode(EditMode::Editing));
    let (_, changed) = w.run(Cmd::Type("x".into()));
    assert!(changed);
}
