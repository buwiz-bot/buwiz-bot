// Port of src/core.ts: render() with the default options the host uses.

use crate::detect::detect_diagram;
use crate::diagrams::flowchart;
use crate::errors::{input_offset, locating_in, Edit, Origin, PeleError, PeleErrorCode};
use crate::js;
use crate::preprocess::{cleanup_comments, encode_entities, normalize_line_breaks, preprocess};
use crate::text::entities::{decode_entities, written_codes};
use crate::types::RenderResult;

// Mermaid's default maxTextSize.
pub const DEFAULT_LIMIT: usize = 50000;
// A source within the limit can still describe a drawing of many megabytes.
pub const DEFAULT_OUTPUT_LIMIT: usize = 4_000_000;
// `A & B --> C & D` multiplies: without a limit, a few kilobytes of source ask for millions of edges.
pub const DEFAULT_MAX_EDGES: usize = 5000;

/// The diagram types this port renders. Any other type that upstream renders is a
/// `NotYetPorted` error, for the host to draw some other way.
pub const PORTED: [&str; 1] = ["flowchart"];

fn tag(mut error: PeleError, type_: &str, mermaid_type: &str) -> PeleError {
    if error.type_.is_none() {
        error.type_ = Some(type_.to_string());
    }
    if error.mermaid_type.is_none() {
        error.mermaid_type = Some(mermaid_type.to_string());
    }
    error.message = written_codes(&error.message);
    error.snippet = written_codes(&error.snippet);
    error
}

// Record preprocessing edits only when a source position is needed.
fn in_source(text: Vec<u16>) -> impl Fn(usize) -> (Vec<u16>, usize) {
    move |at| {
        let mut steps: Vec<Vec<Edit>> = Vec::new();
        if let Ok(pre) = preprocess(&text, Some(&mut steps)) {
            let cleaned = cleanup_comments(&pre.with_comments, Some(&mut steps));
            encode_entities(&cleaned, Some(&mut steps));
        }
        // Match the line break appended for parsing.
        let mut source = normalize_line_breaks(&text);
        source.push(10);
        (source, input_offset(&steps, at))
    }
}

/// The type a text declares, or None.
pub fn detect_type(text: &str) -> Option<&'static str> {
    let pre = preprocess(&js::u16s(text), None).ok()?;
    detect_diagram(&js::from16(&pre.text)).map(|d| d.type_)
}

pub fn render(text: &str) -> Result<RenderResult, PeleError> {
    let source = js::u16s(text);
    if source.len() > DEFAULT_LIMIT {
        return Err(PeleError::new(format!("Diagram source is longer than the limit of {} characters.", DEFAULT_LIMIT), PeleErrorCode::Limit));
    }
    let pre = preprocess(&source, None)?;
    let Some(found) = detect_diagram(&js::from16(&pre.text)) else {
        return Err(PeleError::new("No diagram type detected for the given text.", PeleErrorCode::UnsupportedDiagram));
    };
    let (type_, mermaid_type) = (found.type_, found.mermaid_type);
    if !PORTED.contains(&type_) {
        let mut e = PeleError::new(format!("Diagram type \"{}\" is not yet ported.", type_), PeleErrorCode::NotYetPorted);
        e.type_ = Some(type_.into());
        e.mermaid_type = Some(mermaid_type.into());
        return Err(e);
    }
    let config = pre.config;
    let mut parsed = encode_entities(&pre.text, None);
    parsed.push(10);
    let origin = Origin { parsed: parsed.clone(), find: Box::new(in_source(source.clone())) };
    let model = locating_in(origin, || flowchart::parse(&parsed, &config, pre.title.as_deref())).map_err(|e| tag(e, type_, mermaid_type))?;
    let mut rendered = flowchart::render(&model, &config);
    if rendered.svg.len() > DEFAULT_OUTPUT_LIMIT && js::len16(&rendered.svg) > DEFAULT_OUTPUT_LIMIT {
        let mut e = PeleError::new(format!("Diagram output is longer than the limit of {} characters.", DEFAULT_OUTPUT_LIMIT), PeleErrorCode::Limit);
        e.type_ = Some(type_.into());
        e.mermaid_type = Some(mermaid_type.into());
        return Err(e);
    }
    for link in rendered.links.iter_mut() {
        link.id = decode_entities(&link.id);
    }
    Ok(RenderResult { type_, mermaid_type, svg: rendered.svg, width: rendered.width, height: rendered.height, links: rendered.links })
}
