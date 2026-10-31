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

use super::fonts::{Fonts, Need};

/// How many body blocks a preview seeds: a first page's worth and more.
const PREVIEW_BLOCKS: usize = 80;

/// Every engine edits as this client (one local person).
const CLIENT: u64 = 0x524f_5857;

pub(crate) struct Layout {
    pub fonts: Fonts,
    pub engine: EngineSession,
    /// The region request, its fonts filled in, and as parsed (without them,
    /// and as JSON: what the font requirements are asked with).
    request: String,
    base: Value,
    base_json: String,
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
        Layout { fonts: Fonts::new(), engine: EngineSession::new(CLIENT), request: String::new(), base: Value::Null, base_json: String::new(), preview: false, provisional: false, comments: Vec::new(), epoch: 0 }
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
        let base_json = base.to_string();
        let mut layout = Layout { fonts, engine, request: String::new(), base, base_json, preview: preview.is_some(), provisional: false, comments, epoch: 0 };
        let needs = layout.font_needs()?;
        layout.fill_fonts(&needs);
        Ok(Some(layout))
    }

    /// The font every run of the text needs now: its `family|bold|italic`
    /// key and the scripts it wants fallbacks for.
    fn font_needs(&self) -> Result<Vec<Need>, String> {
        let _scope = self.fonts.enter();
        let needs: Vec<Value> = serde_json::from_str(&self.engine.layout_font_requirements_json(&self.base_json)?).map_err(|e| e.to_string())?;
        Ok(needs.iter().filter_map(Need::of_json).collect())
    }

    /// The request with a chain for each of `needs` (and every one before).
    fn fill_fonts(&mut self, needs: &[Need]) {
        for need in needs {
            self.fonts.need(need);
        }
        // the defaults' own chain, for text that names no font
        self.fonts.chain("calibri|0|0");
        let mut request = self.base.clone();
        request["measurement"] = json!({
            "fontChains": self.fonts.chains_json(),
            "defaults": {"fontSize": 11, "fontFamily": "Calibri"},
            "compat": {"noLeading": false, "doNotExpandShiftReturn": false},
            "authoritativeShaping": true,
        });
        self.request = request.to_string();
    }

    /// After an edit, before it is laid out: chains for the fonts it brought
    /// in (bold on a face the text had only upright, a new family), as the
    /// web editor asks before each relayout. Whether the request grew; the
    /// retained one would measure the new key's text one em a character.
    pub(crate) fn fonts_grew(&mut self) -> Result<bool, String> {
        let needs = self.font_needs()?;
        if needs.iter().all(|n| self.fonts.has(&n.key, &n.scripts)) {
            return Ok(false);
        }
        self.fill_fonts(&needs);
        Ok(true)
    }

    /// Lays out again after an edit in `story` with the request as it is
    /// now (one [`Self::fonts_grew`] grew); display lists for `window`.
    pub(crate) fn after_edit(&mut self, story: &str, window: std::ops::Range<usize>) -> Result<(), String> {
        let _scope = self.fonts.enter();
        self.engine.layout_document_with_regions_after_edit(&self.request, Some(story))?;
        self.engine.set_display_window(Some(window));
        let f = self.engine.build_display_list_frame(&self.extras(), self.epoch)?;
        self.took(&f);
        Ok(())
    }

    /// The display builder's extras: the same chains, and the watermark
    /// the layout placed (its `headersFooters`), which the builder draws
    /// only from here.
    fn extras(&self) -> String {
        let mut extras = json!({ "fontChains": self.fonts.chains_json() });
        if self.base.pointer("/regions/watermark").is_some()
            && let Ok(Some(hf)) = self.engine.retained_headers_footers_json()
            && let Ok(hf) = serde_json::from_str::<Value>(&hf)
        {
            extras["headersFooters"] = hf;
        }
        extras.to_string()
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

    /// The region request as built from the package (its sections, the
    /// watermark), without the fonts.
    pub(crate) fn request_base(&self) -> &Value {
        &self.base
    }

    /// The region request changed by `edit` (Page Setup's section, the
    /// watermark), its fonts kept: the next [`Self::full`] lays out with it.
    pub(crate) fn edit_request(&mut self, edit: impl FnOnce(&mut Value)) {
        edit(&mut self.base);
        self.base_json = self.base.to_string();
        self.fill_fonts(&[]);
    }
}

/// Milliseconds since `t`.
#[cfg(test)]
pub(crate) fn ms(t: std::time::Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}
