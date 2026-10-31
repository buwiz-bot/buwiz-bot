//! A Word document in BetterOffice's editing engine (`docx-edit`'s
//! `EngineSession`, the one its web editor runs in wasm): the stories it
//! edits, their pagination and the pages' display lists, all kept between
//! edits so an edit lays out again from its paragraph and rebuilds only
//! the pages it touched.
//!
//! The engine talks JSON (its host is JavaScript): a region request names
//! the sections, the notes and the fonts each `family|bold|italic` key
//! measures with ([`super::fonts`]).

use docx_edit::EngineSession;
use docx_layout::display_list::DisplayPage;
use serde_json::{Value, json};

use super::fonts::Fonts;

/// How many body blocks a preview seeds: a first page's worth and more.
const PREVIEW_BLOCKS: usize = 80;

/// Every engine edits as this client (one local person).
const CLIENT: u64 = 0x524f_5857;

pub(crate) struct Layout {
    pub fonts: Fonts,
    pub engine: EngineSession,
    /// The region request, its fonts filled in, and as parsed (without them).
    request: String,
    base: Value,
    /// Seeded with the first blocks only (a quick first page, display only).
    pub preview: bool,
    /// Only the first pages are laid out (a quick first paint).
    pub provisional: bool,
    /// The package's comments as parsed (author, words).
    pub comments: Vec<Value>,
    /// The last display frame the engine built: a build that names it
    /// changes only what changed (any other is a full rebuild of every page).
    pub epoch: u64,
}

/// The frame epoch a FrameDelta carries (its header's u64 at byte 32).
fn frame_epoch(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(32..40)?.try_into().ok()?))
}

/// The region request for `bytes`: each section with its properties, the
/// settings and every normal note;
/// and the comments (the engine keeps their ranges, not their words).
fn region_request(bytes: &[u8]) -> Result<(Value, Vec<Value>), String> {
    let package = docx_parse::parse_docx_s9_wire(bytes, Default::default()).map_err(|e| e.to_string())?.document.package;
    let comments = serde_json::to_value(&package.document.comments).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let mut sections: Vec<Value> = package.document.sections.clone().unwrap_or_default().into_iter().map(|s| json!({"sectionId": s.id, "properties": s.properties})).collect();
    // the final section (the body's own properties) when no break names it;
    // not repeated, as some hosts send it: a repeated one takes the
    // engine's resident relayout away from every edit
    if sections.is_empty() {
        sections.push(json!({ "properties": package.document.final_section_properties }));
    }
    let mut notes = Vec::new();
    for (list, kind) in [(&package.footnotes, "footnote"), (&package.endnotes, "endnote")] {
        for note in list.iter().flatten() {
            if note.note_type.is_empty() || note.note_type == "normal" {
                notes.push(json!({"id": note.id as i64, "noteKind": kind, "height": 0}));
            }
        }
    }
    Ok((
        json!({
            "bodyStory": "body",
            "renderEnv": {},
            "options": {"pageGap": 0},
            "regions": {"sections": sections, "settings": package.settings},
            "notes": {"contents": notes},
        }),
        comments,
    ))
}

impl Layout {
    /// Seeds `bytes` (the whole document) and prepares its request; nothing
    /// is laid out yet.
    pub(crate) fn open(bytes: &[u8]) -> Result<Layout, String> {
        let (base, comments) = region_request(bytes)?;
        Layout::seeded(bytes, Fonts::new(), base, comments, None).map(|l| l.expect("a full seed"))
    }

    /// Seeds only the first blocks of `bytes`, enough for the first page:
    /// `None` for a document whose start doesn't lay out as its whole does.
    pub(crate) fn preview(bytes: &[u8]) -> Result<Option<Layout>, String> {
        let (base, comments) = region_request(bytes)?;
        Layout::seeded(bytes, Fonts::new(), base, comments, Some(PREVIEW_BLOCKS))
    }

    /// A stand-in while a preview gives way to the whole document.
    pub(crate) fn empty() -> Layout {
        Layout { fonts: Fonts::new(), engine: EngineSession::new(CLIENT), request: String::new(), base: Value::Null, preview: false, provisional: false, comments: Vec::new(), epoch: 0 }
    }

    /// The whole document after a preview: the fonts and the request kept.
    pub(crate) fn whole(self, bytes: &[u8]) -> Result<Layout, String> {
        Layout::seeded(bytes, self.fonts, self.base, self.comments, None).map(|l| l.expect("a full seed"))
    }

    fn seeded(bytes: &[u8], fonts: Fonts, base: Value, comments: Vec<Value>, preview: Option<usize>) -> Result<Option<Layout>, String> {
        let engine = EngineSession::new(CLIENT);
        {
            let _scope = fonts.enter();
            match preview {
                Some(blocks) => {
                    if !docx_edit::seed_docx_preview(engine.doc(), bytes, blocks)? {
                        return Ok(None);
                    }
                }
                None => docx_edit::seed_from_docx(engine.doc(), bytes)?,
            }
        }
        let mut layout = Layout { fonts, engine, request: String::new(), base: base.clone(), preview: preview.is_some(), provisional: false, comments, epoch: 0 };
        layout.request = layout.fill_fonts(base)?;
        Ok(Some(layout))
    }

    /// `request` with a chain for every font its text needs.
    fn fill_fonts(&mut self, mut request: Value) -> Result<String, String> {
        let needs: Vec<Value> = {
            let _scope = self.fonts.enter();
            serde_json::from_str(&self.engine.layout_font_requirements_json(&request.to_string())?).map_err(|e| e.to_string())?
        };
        for n in &needs {
            if let Some(key) = n["key"].as_str() {
                self.fonts.chain(key);
            }
        }
        // the defaults' own chain, for text that names no font
        self.fonts.chain("calibri|0|0");
        request["measurement"] = json!({
            "fontChains": self.fonts.chains_json(),
            "defaults": {"fontSize": 11, "fontFamily": "Calibri"},
            "compat": {"noLeading": false, "doNotExpandShiftReturn": false},
            "authoritativeShaping": true,
        });
        Ok(request.to_string())
    }

    /// The display builder's extras: the same chains.
    fn extras(&self) -> String {
        json!({ "fontChains": self.fonts.chains_json() }).to_string()
    }

    /// Lays out the first `pages` pages only (`provisional` until
    /// [`Self::full`]); their display lists are built.
    pub(crate) fn first(&mut self, pages: usize) -> Result<(), String> {
        let _scope = self.fonts.enter();
        let out = self.engine.layout_document_with_regions_prefix_retained_json(&self.request, pages)?;
        self.provisional = out.contains("\"provisional\":true");
        self.engine.set_display_window(Some(0..pages));
        let f = self.engine.build_display_list_frame(&self.extras(), self.epoch)?;
        self.took(&f);
        Ok(())
    }

    /// Lays out the whole document; display lists are built for `window`.
    pub(crate) fn full(&mut self, window: std::ops::Range<usize>) -> Result<(), String> {
        let _scope = self.fonts.enter();
        self.engine.layout_document_with_regions_retained(&self.request)?;
        self.provisional = false;
        self.engine.set_display_window(Some(window));
        let f = self.engine.build_display_list_frame(&self.extras(), self.epoch)?;
        self.took(&f);
        Ok(())
    }

    /// Builds the display lists of `pages` not built yet.
    pub(crate) fn build(&mut self, pages: &[usize]) -> Result<(), String> {
        let _scope = self.fonts.enter();
        let f = self.engine.build_display_pages_frame(pages, self.epoch)?;
        self.took(&f);
        Ok(())
    }

    /// Notes the frame a build returned (an empty one was superseded).
    pub(crate) fn took(&mut self, frame: &[u8]) {
        if let Some(e) = frame_epoch(frame) {
            self.epoch = e;
        }
    }

    pub(crate) fn page_count(&self) -> usize {
        self.engine.with_display_list(|l| l.pages.len()).unwrap_or(0)
    }

    /// Page `i` as built (`unbuilt` pages carry their size only).
    pub(crate) fn page(&self, i: usize) -> Option<DisplayPage> {
        self.engine.with_display_list(|l| l.pages.get(i).cloned()).flatten()
    }

    /// Reads the display list in place.
    pub(crate) fn with_pages<T>(&self, read: impl FnOnce(&[DisplayPage]) -> T) -> Option<T> {
        self.engine.with_display_list(|l| read(&l.pages))
    }
}

/// Milliseconds since `t`.
#[cfg(test)]
pub(crate) fn ms(t: std::time::Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}
