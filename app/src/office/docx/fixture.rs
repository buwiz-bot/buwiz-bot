//! Word documents built in code for the tests and the bench: headings,
//! lists, tables, pictures, footnotes, a comment and tracked changes.

use std::fmt::Write;

const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture""#;

/// A 4×2 PNG: red, green, blue and white columns over black.
pub(crate) fn png() -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, 4, 2);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    let mut w = e.write_header().expect("png header");
    let row = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
    w.write_image_data(&[&row[..], &[0; 12]].concat()).expect("png data");
    drop(w);
    out
}

const TEXT: [&str; 3] = [
    "The operations team records each inspection and reviews the results at the end of the month. This section describes the equipment, the observations collected during the visit, and the actions assigned to the next shift.",
    "A regional coordinator checks the inventory against the delivery schedule before approving a transfer. The report includes a description of the materials, their destination, and the supporting records kept for the annual review.",
    "The maintenance plan brings together routine checks and scheduled repairs. Each entry names the responsible team and the expected completion date. Supervisors compare these records with the previous period to track progress.",
];

fn p(id: u32, ppr: &str, runs: &str) -> String {
    format!(r#"<w:p w14:paraId="{id:08X}"><w:pPr>{ppr}</w:pPr>{runs}</w:p>"#)
}

fn r(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

fn picture(n: u32) -> String {
    format!(
        r#"<w:r><w:drawing><wp:inline><wp:extent cx="1828800" cy="914400"/><wp:docPr id="{n}" name="Picture {n}"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="{n}" name="Picture {n}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rIdImage"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#
    )
}

/// The body of a document about `pages` pages long.
fn body(pages: usize, with_notes: bool) -> (String, usize) {
    let mut b = String::new();
    let mut id = 0x100u32;
    let mut notes = 0usize;
    let mut next = || {
        id += 1;
        id
    };
    for page in 0..pages {
        let brk = if page == 0 { "" } else { "<w:pageBreakBefore/>" };
        b += &p(next(), &format!(r#"<w:pStyle w:val="Heading1"/>{brk}"#), &r(&format!("Section {}", page + 1)));
        for k in 0..3 {
            let mut runs = r(&format!("Record {}.{}: ", page + 1, k + 1));
            runs += &format!(r#"<w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">{}</w:t></w:r>"#, "Bold words ");
            runs += &r(TEXT[(page + k) % 3]);
            if k == 1 && with_notes {
                notes += 1;
                runs += &format!(r#"<w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:footnoteReference w:id="{notes}"/></w:r>"#);
            }
            b += &p(next(), "", &runs);
        }
        for k in 0..3 {
            let num = if page % 2 == 0 { 1 } else { 2 };
            b += &p(next(), &format!(r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="{num}"/></w:numPr>"#), &r(&format!("List item {} of section {}", k + 1, page + 1)));
        }
        if page % 2 == 0 {
            b += r#"<w:tbl><w:tblPr><w:tblW w:w="9360" w:type="dxa"/><w:tblBorders><w:top w:val="single" w:sz="4" w:color="000000"/><w:left w:val="single" w:sz="4" w:color="000000"/><w:bottom w:val="single" w:sz="4" w:color="000000"/><w:right w:val="single" w:sz="4" w:color="000000"/><w:insideH w:val="single" w:sz="4" w:color="000000"/><w:insideV w:val="single" w:sz="4" w:color="000000"/></w:tblBorders><w:tblLayout w:type="fixed"/></w:tblPr><w:tblGrid><w:gridCol w:w="3120"/><w:gridCol w:w="3120"/><w:gridCol w:w="3120"/></w:tblGrid>"#;
            for row in 0..3 {
                b += "<w:tr>";
                for col in 0..3 {
                    let cell = p(next(), "", &r(&format!("Item {}.{}.{}", page + 1, row + 1, col + 1)));
                    write!(b, r#"<w:tc><w:tcPr><w:tcW w:w="3120" w:type="dxa"/></w:tcPr>{cell}</w:tc>"#).unwrap();
                }
                b += "</w:tr>";
            }
            b += "</w:tbl>";
        } else {
            b += &p(next(), "", &picture(page as u32 + 1));
        }
        b += &p(next(), "", &r(TEXT[page % 3]));
    }
    (b, notes)
}

/// A document of about `pages` pages: each a heading, three paragraphs (one
/// with a footnote), a three-item list, and a table or a picture.
pub(crate) fn document(pages: usize) -> Vec<u8> {
    let (body, notes) = body(pages, true);
    package(&body, notes, "", "")
}

/// [`document`] without footnotes.
pub(crate) fn document_without_notes(pages: usize) -> Vec<u8> {
    let (body, notes) = body(pages, false);
    package(&body, notes, "", "")
}

/// A short document with a comment, a tracked insertion and a tracked
/// deletion by two authors.
pub(crate) fn reviewed() -> Vec<u8> {
    let body = [
        p(0x11, "", &format!(r#"{}<w:commentRangeStart w:id="0"/>{}<w:commentRangeEnd w:id="0"/><w:r><w:commentReference w:id="0"/></w:r>"#, r("Plain text, "), r("commented words"))),
        p(0x12, "", &format!(r#"{}<w:ins w:id="1" w:author="Ada" w:date="2026-01-01T00:00:00Z">{}</w:ins>{}"#, r("Before "), r("inserted"), r(" after."))),
        p(0x13, "", &format!(r#"{}<w:del w:id="2" w:author="Grace" w:date="2026-01-02T00:00:00Z"><w:r><w:delText>deleted</w:delText></w:r></w:del>{}"#, r("Keep "), r(" this."))),
    ]
    .concat();
    let comments = format!(
        r#"<w:comments {W}><w:comment w:id="0" w:author="Ada" w:date="2026-01-01T00:00:00Z" w:initials="A">{}</w:comment></w:comments>"#,
        p(0x99, "", &r("Check these words."))
    );
    package(&body, 0, &comments, "")
}

/// A one-paragraph-per-line document.
pub(crate) fn paragraphs(lines: &[&str]) -> Vec<u8> {
    let body: String = lines.iter().enumerate().map(|(i, l)| p(0x200 + i as u32, "", &r(l))).collect();
    package(&body, 0, "", "")
}

/// A title (Title: Calibri Light 28) over one paragraph per line.
pub(crate) fn titled(title: &str, lines: &[&str]) -> Vec<u8> {
    let mut body = p(0x1ff, r#"<w:pStyle w:val="Title"/>"#, &r(title));
    body.extend(lines.iter().enumerate().map(|(i, l)| p(0x200 + i as u32, "", &r(l))));
    let title = r#"<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:rPr><w:rFonts w:ascii="Calibri Light" w:hAnsi="Calibri Light" w:cs="Calibri Light"/><w:sz w:val="56"/></w:rPr></w:style>"#;
    package(&body, 0, "", title)
}

fn package(body: &str, notes: usize, comments: &str, styles: &str) -> Vec<u8> {
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W}><w:body>{body}<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    let styles = format!(
        r#"<w:styles {W}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:cs="Arial"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="120" w:line="264" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:color w:val="1F3864"/><w:sz w:val="32"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="FootnoteText"><w:name w:val="footnote text"/><w:basedOn w:val="Normal"/><w:rPr><w:sz w:val="18"/></w:rPr></w:style>{styles}</w:styles>"#
    );
    let numbering = format!(
        r#"<w:numbering {W}><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="•"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum><w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num></w:numbering>"#
    );
    let mut footnotes = format!(
        r#"<w:footnotes {W}><w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>"#
    );
    for n in 1..=notes {
        write!(footnotes, r#"<w:footnote w:id="{n}"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr><w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:footnoteRef/></w:r>{}</w:p></w:footnote>"#, r(&format!(" Footnote {n}: where the record came from.")))
            .unwrap();
    }
    footnotes += "</w:footnotes>";
    let ct = |part: &str, kind: &str| format!(r#"<Override PartName="/word/{part}" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.{kind}+xml"/>"#);
    let rel = |id: &str, kind: &str, target: &str| format!(r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/>"#);
    let mut types = format!(
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/>{}{}{}{}"#,
        ct("document.xml", "document.main"),
        ct("styles.xml", "styles"),
        ct("numbering.xml", "numbering"),
        ct("footnotes.xml", "footnotes")
    );
    let mut rels = [rel("rIdStyles", "styles", "styles.xml"), rel("rIdNumbering", "numbering", "numbering.xml"), rel("rIdFootnotes", "footnotes", "footnotes.xml"), rel("rIdImage", "image", "media/image1.png")].concat();
    if !comments.is_empty() {
        types += &ct("comments.xml", "comments");
        rels += &rel("rIdComments", "comments", "comments.xml");
    }
    types += "</Types>";
    let mut parts = vec![
        ("[Content_Types].xml".to_owned(), types.into_bytes()),
        (
            "_rels/.rels".to_owned(),
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_vec(),
        ),
        ("word/_rels/document.xml.rels".to_owned(), format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#).into_bytes()),
        ("word/document.xml".to_owned(), document.into_bytes()),
        ("word/styles.xml".to_owned(), styles.into_bytes()),
        ("word/numbering.xml".to_owned(), numbering.into_bytes()),
        ("word/footnotes.xml".to_owned(), footnotes.into_bytes()),
        ("word/media/image1.png".to_owned(), png()),
    ];
    if !comments.is_empty() {
        parts.push(("word/comments.xml".to_owned(), comments.as_bytes().to_vec()));
    }
    ooxml_opc::rezip_parts(&parts).expect("zip the document")
}
