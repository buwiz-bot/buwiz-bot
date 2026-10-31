//! The menu bar's commands on the worker: Insert's table, breaks and
//! picture, Format's text direction, Page Setup and the watermark, each
//! shaped as the web editor sends it (`docx-react`'s yrs commands; the
//! arguments as `docx-edit`'s wasm session shapes them).
//!
//! Page Setup and the watermark change what the engine lays out with, the
//! region request's last section and its `regions.watermark` (the web
//! builds that request from its document model, which those dialogs
//! change), and are kept in the document's section margins and part
//! metadata for saving.

use docx_edit::{EditHistory, EditOperation, EditRequest, EditSource, EditStep, ParaAttrDelta, ParaSelector, ParagraphTarget, Patch, Position, StoryRange, TableLocator};
use serde_json::{Value, json};
use yrs::Any;

use super::super::index::cell_of;
use super::{Out, Word, spot};

/// What Insert puts at the caret.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Insert {
    Table { rows: u32, cols: u32 },
    PageBreak,
    /// `"nextPage"` or `"continuous"`.
    SectionBreak(&'static str),
    Image(Image),
}

/// A picture as the web inserts it (`handleImageFileChange`): its `data:`
/// URL, the file's name as its alt text, and its size in pixels.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Image {
    pub src: String,
    pub alt: String,
    pub width: u32,
    pub height: u32,
}

/// A section's page in twips, as Page Setup edits it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PageSetup {
    pub width: f64,
    pub height: f64,
    /// `"portrait"` or `"landscape"`.
    pub orientation: String,
    pub top: f64,
    pub bottom: f64,
    pub left: f64,
    pub right: f64,
}

/// Letter and inch margins: what Page Setup shows for properties that
/// name none (its `_`, `B`, `r`).
impl Default for PageSetup {
    fn default() -> Self {
        PageSetup { width: 12240.0, height: 15840.0, orientation: "portrait".into(), top: 1440.0, bottom: 1440.0, left: 1440.0, right: 1440.0 }
    }
}

impl PageSetup {
    /// Read from section properties as the dialog reads them: a missing or
    /// zero size is Letter's, the orientation follows the size when unset.
    pub(crate) fn of(props: &Value) -> PageSetup {
        let d = PageSetup::default();
        let n = |k: &str| props.get(k).and_then(Value::as_f64);
        let width = n("pageWidth").filter(|v| *v != 0.0).unwrap_or(d.width);
        let height = n("pageHeight").filter(|v| *v != 0.0).unwrap_or(d.height);
        let orientation = props
            .get("orientation")
            .and_then(Value::as_str)
            .filter(|o| !o.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| if width > height { "landscape" } else { "portrait" }.to_owned());
        PageSetup {
            width,
            height,
            orientation,
            top: n("marginTop").unwrap_or(d.top),
            bottom: n("marginBottom").unwrap_or(d.bottom),
            left: n("marginLeft").unwrap_or(d.left),
            right: n("marginRight").unwrap_or(d.right),
        }
    }

    fn margins(&self) -> Value {
        json!({"marginTop": self.top, "marginBottom": self.bottom, "marginLeft": self.left, "marginRight": self.right})
    }

    /// What `updateFinalSectionProperties` merges into the section.
    fn patch(&self) -> Value {
        let mut v = self.margins();
        v["pageWidth"] = json!(self.width);
        v["pageHeight"] = json!(self.height);
        v["orientation"] = json!(self.orientation);
        v
    }
}

/// `b`'s keys over `a`'s.
fn merge(a: &mut Value, b: &Value) {
    if !a.is_object() {
        *a = json!({});
    }
    if let (Some(a), Some(b)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in b {
            a.insert(k.clone(), v.clone());
        }
    }
}

impl Word {
    /// The rest of the document laid out before the request changes: the
    /// whole document's layout starts again from the package's request.
    pub(super) fn complete_first(&mut self, outs: &mut Vec<Out>) {
        if !self.complete {
            self.finish(outs);
        }
    }

    /// Laid out again from the top after the request changed.
    pub(super) fn relaid(&mut self, outs: &mut Vec<Out>, r: Result<(), String>) -> bool {
        if let Err(e) = r {
            outs.push(Out::Note(e));
            return false;
        }
        let window = self.window();
        // every page's list built again: a watermark changes no page's
        // layout, so a build against the last frame would keep them
        self.layout.epoch = 0;
        if let Err(e) = self.layout.full(window) {
            outs.push(Out::Note(format!("Couldn't lay the document out: {e}")));
        }
        self.rev += 1;
        self.edited = true;
        self.index = None;
        self.review = None;
        outs.push(Out::Meta(Box::new(self.meta())));
        self.redraw(outs);
        outs.push(self.caret(false));
        true
    }

    /// Where a block goes (the web's `insertPageBreak`/`insertTable`): at
    /// a caret inside a paragraph the paragraph splits there and the block
    /// goes at the start of its second half.
    fn block_at(&mut self) -> Result<(String, u32), String> {
        let at = self.head.offset;
        let Some((_, p)) = self.head_para() else { return Ok((self.head.story.clone(), at)) };
        if at > p.start {
            self.layout.engine.doc().split_paragraph(&self.ctx(), Position::new(&p.story, at), None).map_err(|e| e.to_string())?;
            return Ok((p.story, at + 1));
        }
        Ok((p.story, at))
    }

    /// Insert's command; the story it changed.
    pub(super) fn insert_block(&mut self, what: Insert) -> Result<String, String> {
        let doc_err = |e: docx_edit::OpError| e.to_string();
        match what {
            Insert::Table { rows, cols } => {
                let (story, at) = self.block_at()?;
                let receipt = self.layout.engine.doc().insert_table(&self.ctx(), Position::new(&story, at), rows, cols).map_err(doc_err)?;
                // the caret into the first cell
                if let Some(first) = receipt.created_story_ids.first() {
                    self.set_caret(spot(first, 0));
                }
                Ok(story)
            }
            Insert::PageBreak => {
                let (story, at) = self.block_at()?;
                self.layout.engine.doc().insert_embed(&self.ctx(), Position::new(&story, at), "pageBreak", Vec::new()).map_err(doc_err)?;
                self.set_caret(spot(&story, at + 1));
                Ok(story)
            }
            Insert::SectionBreak(kind) => {
                // The web inserts a `sectionBreak` embed at the caret, which
                // the engine's layout doesn't lower to a section: the
                // paragraph splits at the caret and its first half closes
                // the section, as Word's own break does.
                let Some((_, p)) = self.head_para() else { return Err("Put the caret in a paragraph first.".to_owned()) };
                if p.story != "body" {
                    return Err("A section break goes in the document's body.".to_owned());
                }
                let at = self.head.offset;
                let doc = self.layout.engine.doc();
                doc.split_paragraph(&self.ctx(), Position::new(&p.story, at), None).map_err(doc_err)?;
                let step = EditStep::new(EditOperation::InsertSectionBreakAfter { target: ParagraphTarget { story: p.story.clone(), para_id: p.para_id.clone() }, kind: kind.to_owned() });
                let request = EditRequest { expect_version: doc.version(), source: EditSource::Host, history: EditHistory::Separate, steps: vec![step] };
                match doc.apply_edits(&request, &self.undo) {
                    Ok(Ok(_)) => {}
                    Ok(Err(refused)) => return Err(format!("{refused:?}")),
                    Err(e) => return Err(format!("{e:?}")),
                }
                self.set_caret(spot(&p.story, at + 1));
                Ok(p.story)
            }
            Insert::Image(img) => {
                let at = self.head.clone();
                // the web's image object, in its order, and the aspect lock
                // its `insert_image` adds
                let n = |v: u32| Any::Number(f64::from(v));
                let payload: Vec<(String, Any)> = [
                    ("src", Any::from(img.src.as_str())),
                    ("alt", Any::from(img.alt.as_str())),
                    ("width", n(img.width)),
                    ("height", n(img.height)),
                    ("wrapType", Any::from("inline")),
                    ("displayMode", Any::from("inline")),
                    ("distTop", n(0)),
                    ("distBottom", n(0)),
                    ("distLeft", n(0)),
                    ("distRight", n(0)),
                    ("noChangeAspect", Any::Bool(true)),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect();
                self.layout.engine.doc().insert_embed(&self.ctx(), Position::new(&at.story, at.offset), "image", payload).map_err(doc_err)?;
                self.set_caret(spot(&at.story, at.offset + 1));
                Ok(at.story)
            }
        }
    }

    /// The request's sections again from the document's (a section
    /// break made one): each closing paragraph's properties and the final
    /// one's, in order.
    pub(super) fn sections_from_doc(&mut self) -> Result<(), String> {
        let sections: Vec<Value> = self.layout.engine.doc().list_sections()?.into_iter().map(|s| json!({ "properties": s.authored })).collect();
        self.layout.edit_request(|base| base["regions"]["sections"] = Value::Array(sections));
        self.setup = None;
        Ok(())
    }

    /// Format's text direction on the selected paragraphs (`bidi`).
    pub(super) fn set_bidi(&mut self, rtl: bool) -> Result<(), String> {
        let (story, a, b) = self.range();
        let selector = ParaSelector::Range(StoryRange::new(&story, a, b.max(a)));
        let delta = ParaAttrDelta { bidi: Patch::Set(rtl), ..Default::default() };
        self.layout.engine.doc().set_paragraph_attrs(&self.ctx(), &selector, &delta).map(drop).map_err(|e| e.to_string())
    }

    /// Whether the caret's paragraph runs right to left.
    pub(super) fn head_bidi(&mut self) -> bool {
        let Some((_, p)) = self.head_para() else { return false };
        let Ok(segments) = self.layout.engine.doc().story_segments(&p.story) else { return false };
        segments.iter().any(|s| match &s.content {
            docx_edit::SegmentContent::Pilcrow(props) if props.para_id == p.para_id => matches!(props.values.get("bidi"), Some(Any::Bool(true))),
            _ => false,
        })
    }

    /// The caret's table, rows by columns.
    pub(super) fn head_table(&self) -> Option<(u32, u32)> {
        let (parent, table, _, _) = cell_of(&self.head.story)?;
        let payload = self.layout.engine.doc().table_payload(&TableLocator::new(parent, table)).ok()?;
        let len = |k: &str| match payload.get(k) {
            Some(Any::Array(a)) => a.len() as u32,
            _ => 0,
        };
        Some((len("rows"), len("grid")))
    }

    /// The last section's page, from the request the layout runs on.
    pub(super) fn setup(&mut self) -> PageSetup {
        if let Some(s) = &self.setup {
            return s.clone();
        }
        let props = self.layout.request_base().pointer("/regions/sections").and_then(Value::as_array).and_then(|s| s.last()).map(|s| s["properties"].clone()).unwrap_or(Value::Null);
        let s = PageSetup::of(&props);
        self.setup = Some(s.clone());
        s
    }

    /// Page Setup applied, as the web's `updateFinalSectionProperties`:
    /// merged into the last section's properties.
    pub(super) fn page_setup(&mut self, setup: &PageSetup) -> Result<(), String> {
        let doc = self.layout.engine.doc();
        doc.set_section_margins(&self.ctx(), "final", &setup.margins().to_string())?;
        // the size and orientation in the final section's properties
        let mut meta: Value = serde_json::from_str(&doc.part_metadata_json()).unwrap_or_else(|_| json!({}));
        if !meta.is_object() {
            meta = json!({});
        }
        if !meta["body"].is_object() {
            meta["body"] = json!({});
        }
        let mut last = doc.list_sections()?.last().map(|s| s.authored.clone()).unwrap_or_else(|| json!({}));
        merge(&mut last, &setup.patch());
        meta["body"]["finalSectionProperties"] = last.clone();
        if let Some(s) = meta["body"]["sections"].as_array_mut().and_then(|s| s.last_mut()) {
            merge(&mut s["properties"], &setup.patch());
        }
        doc.set_part_metadata(&self.ctx(), &meta.to_string(), &[])?;
        let patch = setup.patch();
        self.layout.edit_request(|base| {
            if let Some(s) = base.pointer_mut("/regions/sections").and_then(Value::as_array_mut).and_then(|s| s.last_mut()) {
                merge(&mut s["properties"], &patch);
            }
        });
        self.setup = Some(setup.clone());
        Ok(())
    }

    /// The watermark the layout draws (`regions.watermark`), and on every
    /// header part as the web's `setDocumentWatermark` puts it.
    pub(super) fn set_watermark(&mut self, w: Option<Value>) {
        let wm = w.clone();
        self.layout.edit_request(|base| match wm {
            Some(v) => base["regions"]["watermark"] = v,
            None => {
                if let Some(r) = base["regions"].as_object_mut() {
                    r.remove("watermark");
                }
            }
        });
        let doc = self.layout.engine.doc();
        if let Ok(mut meta) = serde_json::from_str::<Value>(&doc.part_metadata_json())
            && meta["body"].is_object()
            && let Some(headers) = meta["headers"].as_array_mut()
        {
            for h in headers.iter_mut() {
                if let Some(o) = h.get_mut(1).and_then(Value::as_object_mut) {
                    match &w {
                        Some(v) => o.insert("watermark".to_owned(), v.clone()),
                        None => o.remove("watermark"),
                    };
                }
            }
            if let Err(e) = doc.set_part_metadata(&self.ctx(), &meta.to_string(), &[]) {
                log::warn!("word: watermark in the headers: {e}");
            }
        }
        self.watermark = w;
    }
}

#[cfg(test)]
impl Word {
    /// The request's sections.
    pub(crate) fn request_sections(&self) -> String {
        self.layout.request_base()["regions"]["sections"].to_string()
    }

    /// The retained layout's headers and footers (and watermark), as JSON.
    pub(crate) fn headers_footers_json(&self) -> Option<String> {
        self.layout.engine.retained_headers_footers_json().ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_setup_reads_as_the_dialog_does() {
        let s = PageSetup::of(&json!({"pageWidth": 15840, "pageHeight": 12240, "marginTop": 720}));
        assert_eq!((s.width, s.height, s.orientation.as_str(), s.top, s.left), (15840.0, 12240.0, "landscape", 720.0, 1440.0));
        assert_eq!(PageSetup::of(&Value::Null), PageSetup::default());
    }
}
