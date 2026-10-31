//! The save, one command family at a time: an edit through the engine,
//! the save, and the saved package read again by docx-parse and
//! office_oxide, with the XML the web editor writes for it.

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;

use docx_edit::{CellLoc, EditCtx, EngineSession, FormatPolicy, InlineFormatDelta, ParaAttrDelta, ParaSelector, Patch, Position, SimpleFormat, StoryRange, TableLocator, TableRange};
use serde_json::json;
use yrs::Any;

use super::fixture;
use super::save::{self, State};

const NOW: &str = "2026-01-02T03:04:05.006Z";

fn ctx() -> EditCtx {
    EditCtx::local("Rox", "2026-01-01T00:00:00Z")
}

fn seeded(bytes: &[u8]) -> EngineSession {
    let engine = EngineSession::new(7);
    docx_edit::seed_from_docx(engine.doc(), bytes).unwrap();
    engine
}

/// Saved with `state`, then read again by both readers.
fn saved_with(source: &[u8], engine: &EngineSession, state: &mut State) -> Vec<u8> {
    let bytes = save::docx(source, engine.doc(), state, NOW).unwrap();
    docx_parse::parse_docx_s9_wire(&bytes, Default::default()).expect("docx-parse reads the save");
    office_oxide::Document::from_reader(Cursor::new(bytes.clone()), office_oxide::DocumentFormat::Docx).expect("office_oxide reads the save");
    bytes
}

fn saved(source: &[u8], engine: &EngineSession) -> Vec<u8> {
    saved_with(source, engine, &mut State::default())
}

fn parts(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    ooxml_opc::unzip_parts(bytes).unwrap()
}

fn part(bytes: &[u8], name: &str) -> String {
    parts(bytes).into_iter().find(|(n, _)| n == name).map(|(_, b)| String::from_utf8(b).unwrap()).unwrap_or_default()
}

fn body(bytes: &[u8]) -> String {
    part(bytes, "word/document.xml")
}

/// The `w:p` holding `text`.
fn paragraph_with(xml: &str, text: &str) -> String {
    let at = xml.find(text).unwrap_or_else(|| panic!("{text:?} in {xml}"));
    let start = xml[..at].rfind("<w:p ").or_else(|| xml[..at].rfind("<w:p>")).unwrap();
    let end = xml[at..].find("</w:p>").unwrap() + at + 6;
    xml[start..end].to_owned()
}

/// The story-global start and end (its pilcrow) of each of a story's paragraphs.
fn spans(engine: &EngineSession, story: &str) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let (mut at, mut start) = (0u32, 0u32);
    for s in engine.doc().story_segments(story).unwrap() {
        match s.content {
            docx_edit::SegmentContent::Text(t) => at += t.encode_utf16().count() as u32,
            docx_edit::SegmentContent::Pilcrow(_) => {
                out.push((start, at));
                at += 1;
                start = at;
            }
            docx_edit::SegmentContent::OtherEmbed { .. } => at += 1,
        }
    }
    out
}

fn count(xml: &str, what: &str) -> usize {
    xml.matches(what).count()
}

#[test]
fn an_unchanged_document_saves_as_its_source() {
    let source = fixture::document(2);
    let engine = seeded(&source);
    let bytes = saved(&source, &engine);
    let (before, after) = (parts(&source), parts(&bytes));
    for (name, data) in &before {
        let got = after.iter().find(|(n, _)| n == name).map(|(_, d)| d);
        assert_eq!(got, Some(data), "{name} kept byte for byte");
    }
    // only the modified date is the save's own
    let extra: Vec<&String> = after.iter().map(|(n, _)| n).filter(|n| !before.iter().any(|(b, _)| b == *n)).collect();
    assert!(extra.iter().all(|n| n.starts_with("docProps/") || n.ends_with(".rels") || n.as_str() == "[Content_Types].xml"), "{extra:?}");
}

#[test]
fn bold_saves_as_bold_and_leaves_the_other_paragraphs_as_they_were() {
    let source = fixture::paragraphs(&["Hello world", "Second line"]);
    let engine = seeded(&source);
    let (a, _) = spans(&engine, "body")[0];
    engine.doc().toggle_format(&ctx(), StoryRange::new("body", a, a + 5), SimpleFormat::Bold).unwrap();
    let xml = body(&saved(&source, &engine));
    let p = paragraph_with(&xml, "Hello");
    assert!(p.contains("<w:b/>"), "{p}");
    assert!(!paragraph_with(&xml, " world").contains("<w:b/>") || p.contains(" world") && p.matches("<w:b/>").count() == 1);
    // the paragraph the edit didn't touch is its source XML
    let src = body(&source);
    assert!(xml.contains(&paragraph_with(&src, "Second line")));
}

#[test]
fn run_formats_save_as_run_properties() {
    let source = fixture::paragraphs(&["Alpha beta gamma delta"]);
    let engine = seeded(&source);
    let (a, _) = spans(&engine, "body")[0];
    let doc = engine.doc();
    let delta = InlineFormatDelta {
        italic: Patch::Set(true),
        underline: Patch::Set(docx_edit::UnderlinePatch { style: Some("single".into()), color: None }),
        strike: Patch::Set(docx_edit::StrikePatch { double: false }),
        color: Patch::Set(docx_edit::ColorPatch::Rgb("FF0000".into())),
        highlight: Patch::Set("yellow".into()),
        font_size: Patch::Set(14.0),
        font_family: Patch::Set(docx_edit::FontFamilyPatch { ascii: "Courier New".into(), h_ansi: None }),
        ..Default::default()
    };
    doc.format_range(&ctx(), StoryRange::new("body", a, a + 5), &delta).unwrap();
    doc.toggle_format(&ctx(), StoryRange::new("body", a + 6, a + 10), SimpleFormat::Superscript).unwrap();
    doc.toggle_format(&ctx(), StoryRange::new("body", a + 11, a + 16), SimpleFormat::Subscript).unwrap();
    let xml = body(&saved(&source, &engine));
    for want in ["<w:i/>", r#"<w:u w:val="single"/>"#, "<w:strike/>", r#"<w:color w:val="FF0000"/>"#, r#"<w:highlight w:val="yellow"/>"#, r#"<w:sz w:val="28"/>"#, r#"w:ascii="Courier New""#, r#"<w:vertAlign w:val="superscript"/>"#, r#"<w:vertAlign w:val="subscript"/>"#] {
        assert!(xml.contains(want), "{want} in {xml}");
    }
}

#[test]
fn paragraph_properties_and_lists_save() {
    let source = fixture::document_without_notes(1);
    let engine = seeded(&source);
    let doc = engine.doc();
    let p = spans(&engine, "body");
    let one = |n: usize| ParaSelector::Range(StoryRange::new("body", p[n].0, p[n].0));
    let delta = ParaAttrDelta { alignment: Patch::Set("center".into()), space_before: Patch::Set(240.0), indent_left: Patch::Set(720.0), bidi: Patch::Set(true), ..Default::default() };
    doc.set_paragraph_attrs(&ctx(), &one(1), &delta).unwrap();
    let mut style = ParaAttrDelta::default();
    style.other.insert("pStyle".into(), Some(Any::from("Heading1")));
    doc.set_paragraph_attrs(&ctx(), &one(2), &style).unwrap();
    let mut list = ParaAttrDelta::default();
    list.other.insert("numPr".into(), Some(save::num_pr(2, 0)));
    doc.set_paragraph_attrs(&ctx(), &one(3), &list).unwrap();
    let xml = body(&saved(&source, &engine));
    let p1 = paragraph_with(&xml, "Record 1.1");
    for want in [r#"<w:jc w:val="center"/>"#, r#"w:before="240""#, r#"w:left="720""#, "<w:bidi/>"] {
        assert!(p1.contains(want), "{want} in {p1}");
    }
    assert!(paragraph_with(&xml, "Record 1.2").contains(r#"<w:pStyle w:val="Heading1"/>"#));
    assert!(paragraph_with(&xml, "Record 1.3").contains(r#"<w:numId w:val="2"/>"#));
}

#[test]
fn a_hyperlink_saves_with_its_relationship() {
    let source = fixture::paragraphs(&["Visit the site today"]);
    let engine = seeded(&source);
    let (a, _) = spans(&engine, "body")[0];
    let link = Any::from_json(r#"{"href":"https://example.com/"}"#).unwrap();
    engine.doc().set_hyperlink(&ctx(), StoryRange::new("body", a + 10, a + 14), Some(link)).unwrap();
    let bytes = saved(&source, &engine);
    let xml = body(&bytes);
    let at = xml.find("<w:hyperlink").expect("a hyperlink");
    let id = xml[at..].split("r:id=\"").nth(1).unwrap().split('"').next().unwrap().to_owned();
    assert!(xml[at..].split("</w:hyperlink>").next().unwrap().contains("site"));
    let rels = part(&bytes, "word/_rels/document.xml.rels");
    assert!(rels.contains(&format!(r#"Id="{id}""#)) && rels.contains("https://example.com/") && rels.contains(r#"TargetMode="External""#), "{rels}");
}

fn image_payload(png: &[u8]) -> Vec<(String, Any)> {
    use base64::Engine as _;
    let src = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png));
    let n = |v: f64| Any::Number(v);
    [("src", Any::from(src.as_str())), ("alt", Any::from("A chart")), ("width", n(96.0)), ("height", n(48.0)), ("wrapType", Any::from("inline")), ("displayMode", Any::from("inline")), ("distTop", n(0.0)), ("distBottom", n(0.0)), ("distLeft", n(0.0)), ("distRight", n(0.0)), ("noChangeAspect", Any::Bool(true))]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect()
}

#[test]
fn an_inserted_picture_saves_inline_with_its_media() {
    let source = fixture::paragraphs(&["Picture here"]);
    let engine = seeded(&source);
    let (a, _) = spans(&engine, "body")[0];
    engine.doc().insert_embed(&ctx(), Position::new("body", a + 8), "image", image_payload(&fixture::png())).unwrap();
    let bytes = saved(&source, &engine);
    let xml = body(&bytes);
    assert!(xml.contains("<wp:inline"), "{xml}");
    assert!(xml.contains(r#"cx="914400" cy="457200""#), "96×48 px in EMU: {xml}");
    let id = xml.split("r:embed=\"").nth(1).unwrap().split('"').next().unwrap();
    let rels = part(&bytes, "word/_rels/document.xml.rels");
    let target = rels.split(&format!(r#"Id="{id}""#)).nth(1).and_then(|r| r.split("Target=\"").nth(1)).unwrap().split('"').next().unwrap();
    assert!(parts(&bytes).iter().any(|(n, d)| n == &format!("word/{target}") && *d == fixture::png()), "{target}");
}

#[test]
fn a_floating_picture_saves_its_wrap_and_position() {
    let source = fixture::paragraphs(&["Picture here"]);
    let engine = seeded(&source);
    let (a, _) = spans(&engine, "body")[0];
    let doc = engine.doc();
    doc.insert_embed(&ctx(), Position::new("body", a), "image", image_payload(&fixture::png())).unwrap();
    let position = Any::from_json(r#"{"horizontal":{"relativeTo":"page","posOffset":914400},"vertical":{"relativeTo":"page","posOffset":457200}}"#).unwrap();
    doc.set_embed_attrs(&ctx(), Position::new("body", a), vec![("wrapType".into(), Any::from("square")), ("displayMode".into(), Any::from("float")), ("cssFloat".into(), Any::from("left")), ("wrapText".into(), Any::from("right")), ("position".into(), position)]).unwrap();
    let xml = body(&saved(&source, &engine));
    for want in ["<wp:anchor", "<wp:wrapSquare", r#"relativeFrom="page""#, "<wp:posOffset>914400</wp:posOffset>"] {
        assert!(xml.contains(want), "{want} in {xml}");
    }
}

#[test]
fn deleting_one_of_two_pictures_keeps_the_other() {
    let source = fixture::pictures_and_break();
    let engine = seeded(&source);
    let (a, _) = spans(&engine, "body")[0];
    // "Before " then the two pictures
    engine.doc().delete_range(&ctx(), StoryRange::new("body", a + 7, a + 8)).unwrap();
    let xml = body(&saved(&source, &engine));
    let p = paragraph_with(&xml, "Before");
    assert_eq!(count(&p, "<w:drawing>"), 1, "{p}");
    // the one left is the second, wider and described
    assert!(p.contains(r#"cx="2743200""#) && p.contains(r#"descr="Second""#), "{p}");
    assert!(p.contains(" after"));
}

#[test]
fn page_breaks_save_once_and_an_inserted_one_saves() {
    let source = fixture::pictures_and_break();
    let engine = seeded(&source);
    let doc = engine.doc();
    // an edit elsewhere: the parsed break isn't doubled
    let p = spans(&engine, "body");
    doc.insert_text(&ctx(), Position::new("body", p[2].1), "!", FormatPolicy::Inherit).unwrap();
    let xml = body(&saved(&source, &engine));
    assert_eq!(count(&xml, r#"w:type="page""#), 1, "{xml}");
    // a break inserted before the last line
    let p = spans(&engine, "body");
    doc.insert_embed(&ctx(), Position::new("body", p[2].0), "pageBreak", Vec::new()).unwrap();
    let xml = body(&saved(&source, &engine));
    assert_eq!(count(&xml, r#"w:type="page""#), 2, "{xml}");
    let last = xml.find("Last line").unwrap();
    assert!(xml[..last].rfind(r#"w:type="page""#).unwrap() > xml.find(r#"w14:paraId="00000302""#).unwrap(), "the new break comes before the last line");
}

fn section_break(engine: &EngineSession, n: usize, kind: &str) {
    use docx_edit::{EditHistory, EditOperation, EditRequest, EditSource, EditStep, ParagraphTarget};
    let doc = engine.doc();
    let id = doc
        .story_segments("body")
        .unwrap()
        .into_iter()
        .filter_map(|s| match s.content {
            docx_edit::SegmentContent::Pilcrow(p) => Some(p.para_id),
            _ => None,
        })
        .nth(n)
        .unwrap();
    let undo = docx_edit::UndoSession::new();
    undo.track(doc);
    let step = EditStep::new(EditOperation::InsertSectionBreakAfter { target: ParagraphTarget { story: "body".into(), para_id: id }, kind: kind.to_owned() });
    let request = EditRequest { expect_version: doc.version(), source: EditSource::Host, history: EditHistory::Separate, steps: vec![step] };
    doc.apply_edits(&request, &undo).unwrap().unwrap();
}

#[test]
fn section_breaks_save_next_page_and_continuous() {
    let source = fixture::paragraphs(&["One", "Two", "Three"]);
    let engine = seeded(&source);
    section_break(&engine, 0, "nextPage");
    section_break(&engine, 1, "continuous");
    let xml = body(&saved(&source, &engine));
    let one = paragraph_with(&xml, "One");
    let two = paragraph_with(&xml, "Two");
    assert!(one.contains("<w:sectPr") && two.contains("<w:sectPr"), "{xml}");
    assert!(xml.contains(r#"<w:type w:val="continuous"/>"#) || xml.contains(r#"<w:type w:val="nextPage"/>"#), "{xml}");
    // each closing paragraph keeps the page it had
    assert!(one.contains(r#"<w:pgSz w:w="12240" w:h="15840"/>"#), "{one}");
}

#[test]
fn page_setup_saves_size_orientation_and_margins() {
    let source = fixture::paragraphs(&["One"]);
    let engine = seeded(&source);
    let doc = engine.doc();
    doc.set_section_margins(&ctx(), "final", r#"{"marginLeft":720,"marginTop":1080}"#).unwrap();
    let meta = json!({ "body": { "finalSectionProperties": { "pageWidth": 15840, "pageHeight": 12240, "orientation": "landscape" } } });
    doc.set_part_metadata(&ctx(), &meta.to_string(), &[]).unwrap();
    let xml = body(&saved(&source, &engine));
    let sect = &xml[xml.rfind("<w:sectPr").unwrap()..];
    assert!(sect.contains(r#"<w:pgSz w:w="15840" w:h="12240" w:orient="landscape"/>"#), "{sect}");
    assert!(sect.contains(r#"w:left="720""#) && sect.contains(r#"w:top="1080""#), "{sect}");
}

/// The header parts the body's final section names.
fn headers(bytes: &[u8]) -> Vec<String> {
    let xml = body(bytes);
    let rels = part(bytes, "word/_rels/document.xml.rels");
    xml.split("<w:headerReference").skip(1).filter_map(|r| r.split("r:id=\"").nth(1)?.split('"').next()).map(|id| {
        let target = rels.split(&format!(r#"Id="{id}""#)).nth(1).and_then(|r| r.split("Target=\"").nth(1)).unwrap().split('"').next().unwrap();
        part(bytes, &format!("word/{target}"))
    })
    .collect()
}

#[test]
fn a_text_watermark_saves_in_a_new_header() {
    let source = fixture::paragraphs(&["One"]);
    let engine = seeded(&source);
    let mut state = State { watermark: Some(Some(json!({ "kind": "text", "text": "DRAFT", "font": "Calibri", "color": "#C0C0C0", "semitransparent": true, "layout": "diagonal" }))), ..Default::default() };
    let bytes = saved_with(&source, &engine, &mut state);
    let headers = headers(&bytes);
    assert_eq!(headers.len(), 1, "a default header made");
    assert!(headers[0].contains("PowerPlusWaterMarkObject") && headers[0].contains("DRAFT"), "{}", headers[0]);
    assert!(state.whole, "a new part: the web writes every part whole from now on");
}

#[test]
fn a_picture_watermark_saves_with_its_image() {
    use base64::Engine as _;
    let source = fixture::paragraphs(&["One"]);
    let engine = seeded(&source);
    let url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(fixture::png()));
    let mut state = State { watermark: Some(Some(json!({ "kind": "picture", "dataUrl": url, "contentType": "image/png", "scale": 1.0, "washout": true }))), ..Default::default() };
    let bytes = saved_with(&source, &engine, &mut state);
    let headers = headers(&bytes);
    assert!(headers.first().is_some_and(|h| h.contains("<v:imagedata")), "{headers:?}");
}

#[test]
fn a_watermark_set_on_the_headers_metadata_saves() {
    // as Word's chrome sets it on a document with a header
    let source = fixture::paragraphs(&["One"]);
    let engine = seeded(&source);
    let mut state = State { watermark: Some(Some(json!({ "kind": "text", "text": "SECRET", "font": "Calibri", "color": "#C0C0C0", "semitransparent": true, "layout": "horizontal" }))), ..Default::default() };
    let first = saved_with(&source, &engine, &mut state);
    // saved again, opened from the save: the watermark is kept, and taken off
    let engine = seeded(&first);
    let again = saved(&first, &engine);
    assert!(headers(&again)[0].contains("SECRET"));
    let mut off = State { watermark: Some(None), ..Default::default() };
    let gone = saved_with(&first, &engine, &mut off);
    assert!(!headers(&gone).iter().any(|h| h.contains("SECRET")));
}

fn ranges(engine: &EngineSession, n: usize, from: u32, to: u32) -> Vec<StoryRange> {
    let (a, _) = spans(engine, "body")[n];
    vec![StoryRange::new("body", a + from, a + to)]
}

#[test]
fn comments_save_added_answered_resolved_and_deleted() {
    let source = fixture::reviewed();
    let engine = seeded(&source);
    let doc = engine.doc();
    let added = doc.add_comment(&ranges(&engine, 2, 0, 4), "Grace", "2026-01-03T00:00:00Z", Any::from("Keep this?")).unwrap();
    let reply = doc.add_comment(&ranges(&engine, 2, 0, 4), "Ada", "2026-01-04T00:00:00Z", Any::from("Yes.")).unwrap();
    doc.update_comment_metadata(&reply, BTreeMap::from([("parentId".to_owned(), Any::from(added.as_str()))])).unwrap();
    doc.update_comment_metadata(&added, BTreeMap::from([("done".to_owned(), Any::Bool(true))])).unwrap();
    let bytes = saved(&source, &engine);
    let comments = part(&bytes, "word/comments.xml");
    for want in ["Check these words.", "Keep this?", "Yes."] {
        assert!(comments.contains(want), "{want} in {comments}");
    }
    let xml = body(&bytes);
    assert_eq!(count(&xml, "<w:commentRangeStart"), 3, "{xml}");
    let extended = part(&bytes, "word/commentsExtended.xml");
    assert!(extended.contains(r#"w15:done="1""#) && extended.contains("w15:paraIdParent"), "{extended}");
    // the parsed comment deleted
    doc.apply_raw_ops("body", vec![docx_edit::RawOp::RemoveComment { id: "0".into() }], &ctx()).unwrap();
    let bytes = saved(&source, &engine);
    let comments = part(&bytes, "word/comments.xml");
    assert!(!comments.contains("Check these words.") && comments.contains("Keep this?"), "{comments}");
}

#[test]
fn tracked_changes_save_and_accept_and_reject() {
    let source = fixture::paragraphs(&["Plain words here"]);
    let engine = seeded(&source);
    let doc = engine.doc();
    let suggest = EditCtx::suggesting(EditCtx::local("Ada", "2026-01-05T00:00:00Z"));
    let (a, _) = spans(&engine, "body")[0];
    doc.insert_text(&suggest, Position::new("body", a + 6), "new ", FormatPolicy::Inherit).unwrap();
    doc.delete_range(&suggest, StoryRange::new("body", a + 10, a + 16)).unwrap();
    let xml = body(&saved(&source, &engine));
    assert!(xml.contains("<w:ins ") && xml.contains(r#"w:author="Ada""#), "{xml}");
    assert!(xml.contains("<w:del ") && xml.contains("<w:delText"), "{xml}");
    // accepting the insertion, rejecting the deletion
    let changes = doc.list_changes("body").unwrap();
    for c in &changes {
        let target = docx_edit::ChangeTarget::Revision(c.revision_id.clone());
        match c.kind {
            docx_edit::ChangeKind::Insertion => doc.accept_change(&ctx(), &target).map(drop).unwrap(),
            _ => doc.reject_change(&ctx(), &target).map(drop).unwrap(),
        }
    }
    let xml = body(&saved(&source, &engine));
    assert!(!xml.contains("<w:ins ") && !xml.contains("<w:del "), "{xml}");
    assert!(paragraph_with(&xml, "Plain").replace("</w:t></w:r><w:r><w:t xml:space=\"preserve\">", "").contains("new"));
}

#[test]
fn the_parsed_revisions_and_comment_survive_an_edit() {
    let source = fixture::reviewed();
    let engine = seeded(&source);
    let (a, _) = spans(&engine, "body")[1];
    engine.doc().insert_text(&ctx(), Position::new("body", a), "Now ", FormatPolicy::Inherit).unwrap();
    let bytes = saved(&source, &engine);
    let xml = body(&bytes);
    assert!(xml.contains(r#"w:author="Ada""#) && xml.contains(r#"w:author="Grace""#) && xml.contains("Now "), "{xml}");
    assert!(xml.contains(r#"<w:commentRangeStart w:id="0"/>"#));
    assert!(part(&bytes, "word/comments.xml").contains("Check these words."));
}

fn table_xml(xml: &str, n: usize) -> String {
    let at = xml.match_indices("<w:tbl>").nth(n).unwrap().0;
    let end = xml[at..].find("</w:tbl>").unwrap() + at + 8;
    xml[at..end].to_owned()
}

#[test]
fn a_new_table_saves_with_its_grid() {
    let source = fixture::paragraphs(&["Before", "After"]);
    let engine = seeded(&source);
    let p = spans(&engine, "body");
    engine.doc().insert_table(&ctx(), Position::new("body", p[1].0), 2, 3).unwrap();
    let xml = body(&saved(&source, &engine));
    let t = table_xml(&xml, 0);
    assert_eq!(count(&t, "<w:gridCol "), 3, "{t}");
    assert_eq!(count(&t, "<w:tr"), 2);
    assert!(xml.find("Before").unwrap() < xml.find("<w:tbl>").unwrap() && xml.find("<w:tbl>").unwrap() < xml.find("After").unwrap());
}

#[test]
fn a_table_inserted_mid_paragraph_splits_it() {
    let source = fixture::paragraphs(&["Left half right half"]);
    let engine = seeded(&source);
    let doc = engine.doc();
    let (a, _) = spans(&engine, "body")[0];
    doc.split_paragraph(&ctx(), Position::new("body", a + 10), None).unwrap();
    let p = spans(&engine, "body");
    doc.insert_table(&ctx(), Position::new("body", p[1].0), 1, 2).unwrap();
    let xml = body(&saved(&source, &engine));
    let (l, t, r) = (xml.find("Left half").unwrap(), xml.find("<w:tbl>").unwrap(), xml.find("right half").unwrap());
    assert!(l < t && t < r, "{xml}");
}

fn cell(n: u32, row: u32, col: u32) -> CellLoc {
    CellLoc::new("body", n, row, col)
}

#[test]
fn an_inserted_row_saves() {
    let source = fixture::document_without_notes(1);
    let engine = seeded(&source);
    engine.doc().insert_row(&ctx(), &cell(0, 2, 0), true).unwrap();
    let xml = body(&saved(&source, &engine));
    assert_eq!(count(&table_xml(&xml, 0), "<w:tr>") + count(&table_xml(&xml, 0), "<w:tr "), 4, "{}", table_xml(&xml, 0));
}

#[test]
fn table_operations_save() {
    let source = fixture::document_without_notes(1);
    let engine = seeded(&source);
    let doc = engine.doc();
    doc.insert_column(&ctx(), &cell(0, 0, 2), true).unwrap();
    doc.merge_cells(&ctx(), &TableRange::new(cell(0, 0, 0), cell(0, 0, 1))).unwrap();
    doc.set_cell_shading(&ctx(), &TableRange::cell(cell(0, 1, 0)), Some("FFFF00")).unwrap();
    let border = Any::from_json(r#"{"style":"double","size":12,"color":{"rgb":"FF0000"}}"#).unwrap();
    doc.set_cell_borders(&ctx(), &TableRange::cell(cell(0, 2, 2)), &HashMap::from([("top".to_owned(), border)])).unwrap();
    doc.set_column_width(&ctx(), &cell(0, 0, 3), 1440.0).unwrap();
    doc.set_table_properties(&ctx(), &TableLocator::new("body", 0), &HashMap::from([("justification".to_owned(), Any::from("center"))])).unwrap();
    let xml = body(&saved(&source, &engine));
    let t = table_xml(&xml, 0);
    assert_eq!(count(&t, "<w:gridCol "), 4, "{t}");
    for want in [r#"<w:gridSpan w:val="2"/>"#, r#"w:fill="FFFF00""#, r#"w:val="double""#, r#"<w:gridCol w:w="1440"/>"#, r#"<w:jc w:val="center"/>"#] {
        assert!(t.contains(want), "{want} in {t}");
    }
}

#[test]
fn a_split_cell_saves() {
    let source = fixture::document_without_notes(1);
    let engine = seeded(&source);
    let doc = engine.doc();
    doc.merge_cells(&ctx(), &TableRange::new(cell(0, 0, 0), cell(0, 0, 1))).unwrap();
    doc.split_cell_grid(&ctx(), &cell(0, 0, 0), None, Some(2)).unwrap();
    let t = table_xml(&body(&saved(&source, &engine)), 0);
    assert!(!t.contains("<w:gridSpan"), "{t}");
    assert_eq!(count(t.split("</w:tr>").next().unwrap(), "<w:tc>"), 3, "{t}");
}

#[test]
fn headers_footers_and_notes_save_their_edits() {
    let source = fixture::document(1);
    let engine = seeded(&source);
    let (a, _) = spans(&engine, "fn:1")[0];
    engine.doc().insert_text(&ctx(), Position::new("fn:1", a + 1), "Edited ", FormatPolicy::Inherit).unwrap();
    let bytes = saved(&source, &engine);
    let notes = part(&bytes, "word/footnotes.xml");
    assert!(notes.contains("Edited"), "{notes}");
    assert!(notes.contains("<w:footnoteRef/>"), "the note's own mark is kept: {notes}");
}

// ---- through Word's own commands ----

use super::doc::{Cmd, Insert, Move, PageSetup, Word};
use crate::office::session::Document;

fn word(bytes: Vec<u8>) -> Word {
    let mut w = Word::open(bytes, std::path::Path::new("a.docx")).unwrap();
    w.tick();
    w.tick();
    w
}

fn save_word(w: &mut Word) -> Vec<u8> {
    w.edited = true;
    let (bytes, _) = w.save(std::path::Path::new("a.docx")).unwrap();
    docx_parse::parse_docx_s9_wire(&bytes, Default::default()).expect("docx-parse reads the save");
    office_oxide::Document::from_reader(Cursor::new(bytes.clone()), office_oxide::DocumentFormat::Docx).expect("office_oxide reads the save");
    bytes
}

#[test]
fn words_section_breaks_save() {
    let mut w = word(fixture::paragraphs(&["Hello world", "Second line"]));
    w.run(Cmd::Move { to: Move::WordRight, extend: false });
    w.run(Cmd::Insert(Insert::SectionBreak("nextPage")));
    w.run(Cmd::Move { to: Move::DocEnd, extend: false });
    w.run(Cmd::Move { to: Move::WordLeft, extend: false });
    w.run(Cmd::Insert(Insert::SectionBreak("continuous")));
    let xml = body(&save_word(&mut w));
    // two sections closed by paragraphs, and the body's own
    assert_eq!(count(&xml, "<w:pPr><w:sectPr>"), 2, "{xml}");
    // a section's type is how it starts: the second on a new page, the last continuing
    let second = &xml[xml.find("<w:pPr><w:sectPr>").unwrap() + 1..];
    let second = &second[second.find("<w:pPr><w:sectPr>").unwrap()..];
    assert!(second.split("</w:sectPr>").next().unwrap().contains(r#"<w:type w:val="nextPage"/>"#), "{xml}");
    let last = &xml[xml.rfind("<w:sectPr").unwrap()..];
    assert!(last.contains(r#"<w:type w:val="continuous"/>"#), "{last}");
    assert!(paragraph_with(&xml, "Hello").contains("<w:sectPr>"));
}

#[test]
fn words_page_setup_saves() {
    let mut w = word(fixture::document(1));
    w.run(Cmd::Move { to: Move::Right, extend: false });
    let s = PageSetup { width: 16838.0, height: 11906.0, orientation: "landscape".into(), top: 720.0, bottom: 720.0, left: 720.0, right: 720.0 };
    w.run(Cmd::PageSetup(s));
    let xml = body(&save_word(&mut w));
    let sect = &xml[xml.rfind("<w:sectPr").unwrap()..];
    assert!(sect.contains(r#"<w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/>"#), "{sect}");
    assert!(sect.contains(r#"w:top="720""#) && sect.contains(r#"w:left="720""#) && sect.contains(r#"w:right="720""#) && sect.contains(r#"w:bottom="720""#), "{sect}");
}

#[test]
fn words_watermarks_save_text_and_picture() {
    use base64::Engine as _;
    let mut w = word(fixture::document(1));
    w.run(Cmd::Move { to: Move::Right, extend: false });
    let text = json!({ "kind": "text", "text": "DRAFT", "font": "Calibri", "color": "#C0C0C0", "semitransparent": true, "layout": "diagonal" });
    w.run(Cmd::Watermark(Some(text)));
    let bytes = save_word(&mut w);
    assert!(headers(&bytes).iter().any(|h| h.contains("DRAFT") && h.contains("PowerPlusWaterMarkObject")), "{:?}", headers(&bytes));
    let url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(fixture::png()));
    w.run(Cmd::Watermark(Some(json!({ "kind": "picture", "dataUrl": url, "contentType": "image/png", "scale": 1.0, "washout": true }))));
    let bytes = save_word(&mut w);
    let hs = headers(&bytes);
    assert!(hs.iter().any(|h| h.contains("<v:imagedata")) && !hs.iter().any(|h| h.contains("DRAFT")), "{hs:?}");
    w.run(Cmd::Watermark(None));
    let hs = headers(&save_word(&mut w));
    assert!(!hs.iter().any(|h| h.contains("<v:imagedata") || h.contains("DRAFT")), "{hs:?}");
}

#[test]
fn words_table_mid_paragraph_saves_between_the_halves() {
    let mut w = word(fixture::paragraphs(&["Hello world", "Second line"]));
    w.run(Cmd::Move { to: Move::WordRight, extend: false });
    w.run(Cmd::Insert(Insert::Table { rows: 2, cols: 3 }));
    let xml = body(&save_word(&mut w));
    let (l, t, r) = (xml.find("Hello").unwrap(), xml.find("<w:tbl>").unwrap(), xml.find("world").unwrap());
    assert!(l < t && t < r, "{xml}");
    assert_eq!(count(&table_xml(&xml, 0), "<w:gridCol "), 3);
}
