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

/// Every engine edits as this client (one local person).
const CLIENT: u64 = 0x524f_5857;

pub(crate) struct Layout {
    pub fonts: Fonts,
    pub engine: EngineSession,
    /// The region request, its fonts filled in.
    request: String,
    /// Only the first pages are laid out (a quick first paint).
    pub provisional: bool,
    /// The package's comments as parsed (author, words).
    pub comments: Vec<Value>,
}

/// The region request for `bytes`: each section with its properties, the
/// final one again (as hosts send it), the settings and every normal note;
/// and the comments (the engine keeps their ranges, not their words).
fn region_request(bytes: &[u8]) -> Result<(Value, Vec<Value>), String> {
    let package = docx_parse::parse_docx_s9_wire(bytes, Default::default()).map_err(|e| e.to_string())?.document.package;
    let comments = serde_json::to_value(&package.document.comments).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let mut sections: Vec<Value> = package.document.sections.clone().unwrap_or_default().into_iter().map(|s| json!({"sectionId": s.id, "properties": s.properties})).collect();
    let last = sections.last().map(|s| s["properties"].clone()).unwrap_or_else(|| json!(package.document.final_section_properties));
    sections.push(json!({ "properties": last }));
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
        let fonts = Fonts::new();
        let engine = EngineSession::new(CLIENT);
        {
            let _scope = fonts.enter();
            docx_edit::seed_from_docx(engine.doc(), bytes)?;
        }
        let (request, comments) = region_request(bytes)?;
        let mut layout = Layout { fonts, engine, request: String::new(), provisional: false, comments };
        layout.request = layout.fill_fonts(request)?;
        Ok(layout)
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
        self.engine.build_display_list_frame(&self.extras(), 0)?;
        Ok(())
    }

    /// Lays out the whole document; display lists are built for `window`.
    pub(crate) fn full(&mut self, window: std::ops::Range<usize>) -> Result<(), String> {
        let _scope = self.fonts.enter();
        self.engine.layout_document_with_regions_retained(&self.request)?;
        self.provisional = false;
        self.engine.set_display_window(Some(window));
        self.engine.build_display_list_frame(&self.extras(), 0)?;
        Ok(())
    }

    /// Builds the display lists of `pages` not built yet.
    pub(crate) fn build(&mut self, pages: &[usize]) -> Result<(), String> {
        let _scope = self.fonts.enter();
        let epoch = self.frame_epoch();
        self.engine.build_display_pages_frame(pages, epoch)?;
        Ok(())
    }

    fn frame_epoch(&self) -> u64 {
        // the engine wants the last frame applied; a mismatch only widens
        // the next frame, which this host doesn't read
        0
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
