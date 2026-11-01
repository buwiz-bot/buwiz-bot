//! New documents (office/create.ts, and office.mjs's copy the chat tool
//! uses): a docx written as `@betteroffice/docx`'s `createDocx` writes one
//! (the same parts, paragraphs and run properties), an xlsx typed into the
//! blank seed, a pptx built slide by slide on the blank seed.

use serde_json::Value;

use super::proto::{plain_text, Res, ToolError};

const BLANK_XLSX: &[u8] = include_bytes!("seeds/blank.xlsx");
const BLANK_PPTX: &[u8] = include_bytes!("seeds/blank.pptx");

/// Which copy's errors: the MCP tool's (create.ts, coded) or the chat
/// tool's (office.mjs, plain messages without the final period).
#[derive(Clone, Copy, PartialEq)]
pub enum Flavor {
    Tool,
    Chat,
}

fn err(flavor: Flavor, code: &str, tool: &str, chat: &str) -> ToolError {
    match flavor {
        Flavor::Tool => ToolError::new(code, tool),
        Flavor::Chat => ToolError::new("ENGINE_ERROR", chat),
    }
}

fn plain(flavor: Flavor, text: &str, label: &str) -> Res<()> {
    plain_text(text).map_err(|e| match flavor {
        Flavor::Tool => e,
        Flavor::Chat => ToolError::new("ENGINE_ERROR", format!("{label} must contain complete Unicode characters and no XML control characters")),
    })
}

fn units(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `officeCreateBytes(format, input)`.
pub fn bytes(format: &str, input: &Value, flavor: Flavor) -> Res<Vec<u8>> {
    match format {
        "docx" => docx(input.get("docx"), flavor),
        "xlsx" => xlsx(input.get("xlsx"), flavor),
        "pptx" => pptx(input.get("pptx"), flavor),
        other => Err(ToolError::new("ENGINE_ERROR", format!("unknown office format {other}"))),
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

const DOCUMENT_OPEN: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:wpc="http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:wp14="http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:w10="urn:schemas-microsoft-com:office:word" xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml" xmlns:w16se="http://schemas.microsoft.com/office/word/2015/wordml/symex" xmlns:w16cid="http://schemas.microsoft.com/office/word/2016/wordml/cid" xmlns:w16="http://schemas.microsoft.com/office/word/2018/wordml" xmlns:w16cex="http://schemas.microsoft.com/office/word/2018/wordml/cex" xmlns:w16sdtdh="http://schemas.microsoft.com/office/word/2020/wordml/sdtdatahash" xmlns:wne="http://schemas.microsoft.com/office/word/2006/wordml" xmlns:wpg="http://schemas.microsoft.com/office/word/2010/wordprocessingGroup" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" mc:Ignorable="w14 w15 w16se w16cid w16 w16cex w16sdtdh wp14"><w:body>"#;

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n  <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n  <Default Extension=\"xml\" ContentType=\"application/xml\"/>\n  <Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\n  <Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\n  <Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\n  <Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>\n</Types>";
const RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n  <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\n  <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>\n  <Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties\" Target=\"docProps/app.xml\"/>\n</Relationships>";
const DOCUMENT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n  <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\n</Relationships>";
const STYLES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\n  <w:docDefaults>\n    <w:rPrDefault>\n      <w:rPr>\n        <w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\"/>\n        <w:sz w:val=\"22\"/>\n      </w:rPr>\n    </w:rPrDefault>\n    <w:pPrDefault>\n      <w:pPr>\n        <w:spacing w:after=\"200\" w:line=\"276\" w:lineRule=\"auto\"/>\n      </w:pPr>\n    </w:pPrDefault>\n  </w:docDefaults>\n  <w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\">\n    <w:name w:val=\"Normal\"/>\n  </w:style>\n</w:styles>";
const APP: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\">\n  <Application>OpenOOXML DOCX Editor</Application>\n  <AppVersion>1.0.0</AppVersion>\n</Properties>";

fn core(now: &str) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n  <dc:creator>OpenOOXML DOCX Editor</dc:creator>\n  <dcterms:created xsi:type=\"dcterms:W3CDTF\">{now}</dcterms:created>\n  <dcterms:modified xsi:type=\"dcterms:W3CDTF\">{now}</dcterms:modified>\n</cp:coreProperties>")
}

/// (bold, sz half-points, outlineLvl, spacing before, spacing after) of a
/// create.ts style.
fn heading(style: &str) -> (bool, u32, Option<u32>, Option<u32>, Option<u32>) {
    match style {
        "title" => (true, 64, None, None, Some(240)),
        "heading1" => (true, 40, Some(0), Some(240), Some(120)),
        "heading2" => (true, 32, Some(1), Some(160), Some(80)),
        _ => (false, 24, None, None, None),
    }
}

fn docx(input: Option<&Value>, flavor: Flavor) -> Res<Vec<u8>> {
    let paragraphs = input.and_then(|i| i.get("paragraphs")).and_then(Value::as_array).filter(|p| !p.is_empty());
    let Some(paragraphs) = paragraphs else {
        return Err(err(flavor, "INVALID_ARGUMENT", "docx requires at least one paragraph.", "docx requires at least one paragraph"));
    };
    if paragraphs.len() > 5000 {
        return Err(err(flavor, "INVALID_ARGUMENT", "docx creation is limited to 5000 paragraphs.", "docx creation is limited to 5000 paragraphs"));
    }
    let mut body = String::from(DOCUMENT_OPEN);
    for p in paragraphs {
        let text = p.get("text").map(crate::js::string_of).unwrap_or_else(|| "undefined".into());
        match flavor {
            Flavor::Tool => {
                plain(flavor, &text, "paragraph text")?;
                if units(&text) > 16000 {
                    return Err(ToolError::new("EDIT_LIMIT", "Paragraph text must fit in 16000 UTF-16 units."));
                }
            }
            Flavor::Chat => {
                if units(&text) > 16000 {
                    return Err(ToolError::new("ENGINE_ERROR", "paragraph text must fit in 16000 UTF-16 units"));
                }
                plain(flavor, &text, "paragraph text")?;
            }
        }
        let (bold, sz, outline, before, after) = heading(p.get("style").and_then(Value::as_str).unwrap_or("normal"));
        body.push_str("<w:p>");
        if outline.is_some() || before.is_some() || after.is_some() {
            body.push_str("<w:pPr>");
            if before.is_some() || after.is_some() {
                body.push_str("<w:spacing");
                if let Some(b) = before {
                    body.push_str(&format!(" w:before=\"{b}\""));
                }
                if let Some(a) = after {
                    body.push_str(&format!(" w:after=\"{a}\""));
                }
                body.push_str("/>");
            }
            if let Some(o) = outline {
                body.push_str(&format!("<w:outlineLvl w:val=\"{o}\"/>"));
            }
            body.push_str("</w:pPr>");
        }
        let bold = p.get("bold").and_then(Value::as_bool).unwrap_or(bold);
        body.push_str("<w:r><w:rPr>");
        body.push_str(if bold { "<w:b/>" } else { "<w:b w:val=\"0\"/>" });
        if let Some(i) = p.get("italic").and_then(Value::as_bool) {
            body.push_str(if i { "<w:i/>" } else { "<w:i w:val=\"0\"/>" });
        }
        body.push_str(&format!("<w:sz w:val=\"{sz}\"/></w:rPr>"));
        let preserve = text != text.trim();
        body.push_str(if preserve { "<w:t xml:space=\"preserve\">" } else { "<w:t>" });
        body.push_str(&escape(&text));
        body.push_str("</w:t></w:r></w:p>");
    }
    body.push_str("</w:body></w:document>");
    let now = crate::js::iso(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0));
    let parts: Vec<(String, Vec<u8>)> = vec![
        ("[Content_Types].xml".into(), CONTENT_TYPES.as_bytes().to_vec()),
        ("_rels/.rels".into(), RELS.as_bytes().to_vec()),
        ("word/_rels/document.xml.rels".into(), DOCUMENT_RELS.as_bytes().to_vec()),
        ("word/document.xml".into(), body.into_bytes()),
        ("word/styles.xml".into(), STYLES.as_bytes().to_vec()),
        ("docProps/core.xml".into(), core(&now).into_bytes()),
        ("docProps/app.xml".into(), APP.as_bytes().to_vec()),
    ];
    ooxml_opc::rezip_parts(&parts).map_err(super::proto::engine)
}

fn xlsx(input: Option<&Value>, flavor: Flavor) -> Res<Vec<u8>> {
    let rows = input.and_then(|i| i.get("rows")).and_then(Value::as_array).filter(|r| !r.is_empty() && r.iter().any(|row| row.as_array().is_some_and(|c| !c.is_empty())));
    let Some(rows) = rows else {
        return Err(err(flavor, "INVALID_ARGUMENT", "xlsx requires rows: string[][].", "xlsx requires rows: string[][]"));
    };
    if rows.len() > 4096 || rows.iter().any(|r| r.as_array().map_or(0, Vec::len) > 256) {
        return Err(err(flavor, "INVALID_ARGUMENT", "xlsx creation is limited to 4096x256 cells.", "xlsx creation is limited to 4096x256 cells"));
    }
    let mut cells: Vec<Vec<String>> = Vec::new();
    for row in rows {
        let mut out = Vec::new();
        for cell in row.as_array().into_iter().flatten() {
            let text = crate::js::string_of(cell);
            match flavor {
                Flavor::Tool => {
                    plain(flavor, &text, "cell")?;
                    if units(&text) > 16000 {
                        return Err(ToolError::new("EDIT_LIMIT", "Cell input must fit in 16000 UTF-16 units."));
                    }
                }
                Flavor::Chat => {
                    if units(&text) > 16000 {
                        return Err(ToolError::new("ENGINE_ERROR", "cell text must fit in 16000 UTF-16 units"));
                    }
                    plain(flavor, &text, "cell")?;
                }
            }
            out.push(text);
        }
        cells.push(out);
    }
    super::xlsx::create(BLANK_XLSX, &cells)
}

fn pptx(input: Option<&Value>, flavor: Flavor) -> Res<Vec<u8>> {
    let slides = input.and_then(|i| i.get("slides")).and_then(Value::as_array).filter(|s| !s.is_empty());
    let Some(slides) = slides else {
        return Err(err(flavor, "INVALID_ARGUMENT", "pptx requires at least one slide.", "pptx requires at least one slide"));
    };
    if slides.len() > 200 {
        return Err(err(flavor, "INVALID_ARGUMENT", "pptx creation is limited to 200 slides.", "pptx creation is limited to 200 slides"));
    }
    for slide in slides {
        let mut texts = vec![slide.get("title").map(crate::js::string_of).unwrap_or_default()];
        texts.extend(slide.get("bullets").and_then(Value::as_array).into_iter().flatten().map(crate::js::string_of));
        texts.push(slide.get("notes").map(crate::js::string_of).unwrap_or_default());
        for t in texts {
            plain(flavor, &t, "slide text")?;
        }
    }
    super::pptx::create(BLANK_PPTX, slides)
}
