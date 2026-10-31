//! Word's fonts: the layout shapes with the very face the canvas draws.
//!
//! BetterOffice measures text with its own shaper (rustybuzz over the font
//! bytes it is given) while the page canvas draws with CoreText. Text lines
//! up only when both read the same face, so a family the document asks for
//! takes the chain the web editor's registry gives it: BetterOffice's
//! bundled faces ([`buwiz_office_kit::fonts`]: Carlito for Calibri, Liberation
//! Sans for Arial and for any family with no face of its own), their files
//! handed to the shaper and registered with CoreText, which draws them by
//! name. A face standing in for another family is measured through a view
//! with Word's line metrics and pitch for that family
//! (`register_substitute_measure_font`), as the web does.
//!
//! Without the bundled files (a build run outside the app bundle with no
//! node_modules), the family is resolved as nsh's canvas resolves a CSS
//! font (by name and weight, italic by symbolic trait), a missing one
//! taking an installed stand-in for both sides.

use std::collections::{BTreeMap, HashMap};

use objc2_core_foundation::{CFData, CFRetained};
use objc2_core_text::{CTFont, CTFontTableOptions};

use buwiz_office_kit as office;
pub use buwiz_office_kit::fonts::Face;
use buwiz_office_kit::fonts::installed;
#[cfg(test)]
pub use buwiz_office_kit::fonts::coretext_width;

/// The installed family that stands in for `family` (lowercase) when it is
/// missing: metric-near where one ships with macOS.
fn stand_in(family: &str) -> &'static str {
    match family {
        "cambria" | "cambria math" | "constantia" | "book antiqua" | "palatino linotype" | "garamond" | "sitka text" | "times" => "Times New Roman",
        "consolas" | "cascadia code" | "cascadia mono" | "lucida console" | "courier" => "Courier New",
        f if f.contains("mono") || f.contains("code") => "Courier New",
        f if f.contains("serif") && !f.contains("sans") => "Times New Roman",
        _ => "Arial",
    }
}

/// The face's tables as one sfnt (a face of a collection too: the shaper
/// reads single fonts only).
fn sfnt(font: &CTFont) -> Option<Vec<u8>> {
    // SAFETY: a valid font; the array holds unboxed table tags.
    let tags = unsafe { font.available_tables(CTFontTableOptions::NoOptions) }?;
    let mut tables: Vec<(u32, CFRetained<CFData>)> = Vec::new();
    for i in 0..tags.count() {
        // SAFETY: in bounds; the values are tags, not objects.
        let tag = unsafe { tags.value_at_index(i) } as usize as u32;
        // SAFETY: a valid font.
        if let Some(data) = unsafe { font.table(tag, CTFontTableOptions::NoOptions) } {
            tables.push((tag, data));
        }
    }
    if tables.is_empty() {
        return None;
    }
    tables.sort_by_key(|t| t.0);
    let n = tables.len() as u16;
    let entry_selector = 15 - n.leading_zeros() as u16;
    let search_range = (1u16 << entry_selector) * 16;
    let cff = tables.iter().any(|t| &t.0.to_be_bytes() == b"CFF ");
    let mut out = Vec::new();
    out.extend_from_slice(if cff { b"OTTO" } else { &[0, 1, 0, 0] });
    for v in [n, search_range, entry_selector, n * 16 - search_range] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, data) in &tables {
        let bytes = data.to_vec();
        let sum = bytes.chunks(4).fold(0u32, |s, c| {
            let mut w = [0u8; 4];
            w[..c.len()].copy_from_slice(c);
            s.wrapping_add(u32::from_be_bytes(w))
        });
        out.extend_from_slice(&tag.to_be_bytes());
        out.extend_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        body.extend_from_slice(&bytes);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() + body.len();
    }
    out.extend_from_slice(&body);
    Some(out)
}

/// What the layout says a run of text needs (`layout_font_requirements_json`).
#[derive(Debug)]
pub struct Need {
    pub key: String,
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    /// Scripts the text has that want a fallback face (`hebrew`, `cjk-jp`).
    pub scripts: Vec<String>,
}

impl Need {
    /// A requirement as the layout lists it (`None` without a key).
    pub fn of_json(v: &serde_json::Value) -> Option<Need> {
        let key = v["key"].as_str()?;
        let mut need = Need::of_key(key);
        if let Some(family) = v["family"].as_str() {
            need.family = family.to_owned();
        }
        need.scripts = v["scripts"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        Some(need)
    }

    /// The need of a `family|bold|italic` key alone.
    pub fn of_key(key: &str) -> Need {
        let mut parts = key.rsplitn(3, '|');
        let italic = parts.next() == Some("1");
        let bold = parts.next() == Some("1");
        let family = parts.next().unwrap_or("").to_owned();
        Need { key: key.to_owned(), family, bold, italic, scripts: Vec::new() }
    }
}

/// The faces one document's layout measures with, in its own store.
pub struct Fonts {
    store: docx_layout::MeasureFonts,
    /// By font id (a substitute's view draws as its face).
    faces: HashMap<u32, Face>,
    by_face: HashMap<(String, bool, bool), u32>,
    /// Bundled files registered, by file; their substitute views, by file
    /// and the family (lowercase) they stand in for.
    by_file: HashMap<&'static str, u32>,
    views: HashMap<(&'static str, String), u32>,
    /// The chains handed out, by the layout's `family|bold|italic` key,
    /// and the scripts each one's fallbacks were added for.
    chains: BTreeMap<String, Vec<u32>>,
    scripts: HashMap<String, Vec<String>>,
}

impl Fonts {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Fonts {
        Fonts {
            store: docx_layout::MeasureFonts::default(),
            faces: HashMap::new(),
            by_face: HashMap::new(),
            by_file: HashMap::new(),
            views: HashMap::new(),
            chains: BTreeMap::new(),
            scripts: HashMap::new(),
        }
    }

    /// Makes this document's store the one the layout reads until the
    /// scope drops (every layout and display-list call runs in one).
    pub fn enter(&self) -> docx_layout::MeasureFontsScope {
        self.store.enter()
    }

    /// The face behind font id `id`.
    pub fn face(&self, id: u32) -> Option<&Face> {
        self.faces.get(&id)
    }

    /// The face the canvas draws `family` with: the first of its bundled
    /// chain, else the installed family or a stand-in for a missing one.
    pub fn resolve(family: &str, bold: bool, italic: bool) -> Face {
        let wanted = family.trim().trim_matches(['"', '\'']);
        if office::fonts::dir().is_some()
            && let Some(face) = office::fonts::word_chain(wanted, bold, italic).first()
        {
            return Face { family: face.family.to_owned(), bold: face.bold, italic: face.italic };
        }
        let family = if !wanted.is_empty() && installed(wanted, bold, italic).is_some() { wanted.to_owned() } else { stand_in(&wanted.to_lowercase()).to_owned() };
        Face { family, bold, italic }
    }

    /// Hands `bytes` to the shaper as `face`: its font id.
    fn add(&mut self, bytes: &[u8], face: Face) -> Option<u32> {
        let _scope = self.store.enter();
        match docx_layout::register_measure_font_bytes(bytes) {
            Ok(id) => {
                self.faces.insert(id, face);
                Some(id)
            }
            Err(e) => {
                log::warn!("word: font {}: {e}", face.family);
                None
            }
        }
    }

    /// Registers installed `face` (once): its font id.
    fn register(&mut self, face: Face) -> Option<u32> {
        let key = (face.family.to_lowercase(), face.bold, face.italic);
        if let Some(id) = self.by_face.get(&key) {
            return Some(*id);
        }
        let font = installed(&face.family, face.bold, face.italic)?;
        let bytes = sfnt(&font)?;
        let id = self.add(&bytes, face)?;
        self.by_face.insert(key, id);
        Some(id)
    }

    /// Registers bundled `face` (once) and, standing in for `requested`,
    /// its measurement view with Word's metrics for that family: the id
    /// the chain holds.
    fn bundled(&mut self, face: &'static office::fonts::BundledFace, requested: Option<&str>) -> Option<u32> {
        let base = match self.by_file.get(face.file) {
            Some(id) => *id,
            None => {
                let bytes = office::fonts::bytes(face)?;
                let id = self.add(&bytes, Face { family: face.family.to_owned(), bold: face.bold, italic: face.italic })?;
                self.by_file.insert(face.file, id);
                id
            }
        };
        let Some(requested) = requested.map(|r| r.trim().to_lowercase()) else { return Some(base) };
        if let Some(id) = self.views.get(&(face.file, requested.clone())) {
            return Some(*id);
        }
        // The wasm export, called off wasm: its errors are `JsValue`s, which
        // panic here, but it errs only outside an entered store (this one is
        // entered) or for an id the store lacks (`base` was just registered).
        let id = {
            let _scope = self.store.enter();
            docx_layout::register_substitute_measure_font(base, &requested).unwrap_or(base)
        };
        if let Some(f) = self.faces.get(&base).cloned() {
            self.faces.insert(id, f);
        }
        self.views.insert((face.file, requested), id);
        Some(id)
    }

    /// The chain for `need`, as the web registry builds it: the bundled
    /// metric-compatible face and the last resort, each as a view for the
    /// family asked for, then the fallbacks for its scripts. (The web puts
    /// a face the document embeds first; Rox doesn't read those yet.)
    pub fn need(&mut self, need: &Need) -> Vec<u32> {
        if self.has(&need.key, &need.scripts) {
            return self.chains[&need.key].clone();
        }
        let mut chain = match self.chains.get(&need.key) {
            Some(c) => c.clone(),
            None if office::fonts::dir().is_some() => {
                let mut chain = Vec::new();
                for face in office::fonts::word_chain(&need.family, need.bold, need.italic) {
                    if let Some(id) = self.bundled(face, Some(&need.family))
                        && !chain.contains(&id)
                    {
                        chain.push(id);
                    }
                }
                chain
            }
            // the face alone. (A wide fallback such as Arial Unicode MS costs
            // most of a second to hand over, so text in a script the face
            // lacks is measured with its missing glyphs while CoreText draws
            // it from a fallback.)
            None => self.register(Fonts::resolve(&need.family, need.bold, need.italic)).into_iter().collect(),
        };
        let seen = self.scripts.entry(need.key.clone()).or_default();
        let new: Vec<String> = need.scripts.iter().filter(|s| !seen.contains(s)).cloned().collect();
        seen.extend(new.iter().cloned());
        for script in &new {
            if let Some(face) = office::fonts::script_face(script)
                && let Some(id) = self.bundled(face, None)
                && !chain.contains(&id)
            {
                chain.push(id);
            }
        }
        self.chains.insert(need.key.clone(), chain.clone());
        chain
    }

    /// The chain for the layout's `family|bold|italic` key, scripts aside.
    pub fn chain(&mut self, key: &str) -> Vec<u32> {
        self.need(&Need::of_key(key))
    }

    /// Whether a chain was handed out for `key`, with fallbacks for `scripts`.
    pub fn has(&self, key: &str, scripts: &[String]) -> bool {
        self.chains.contains_key(key) && scripts.iter().all(|s| self.scripts.get(key).is_some_and(|seen| seen.contains(s)))
    }

    /// Every chain handed out, as the layout's `fontChains` takes them.
    pub fn chains_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.chains).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_substituted_face_measures_as_the_family_asked_for() {
        let mut fonts = Fonts::new();
        // Calibri Light: Carlito's regular face, bold dropped
        let light = fonts.chain("calibri light|1|0");
        assert_eq!(light.len(), 1);
        assert_eq!(fonts.face(light[0]), Some(&Face { family: "Carlito".into(), bold: false, italic: false }));
        let calibri = fonts.chain("calibri|0|0");
        assert_eq!(calibri.iter().map(|id| fonts.face(*id).unwrap().family.as_str()).collect::<Vec<_>>(), ["Carlito", "Liberation Sans"]);
        // one file, two views: Word's line metrics for each family
        assert_ne!(light[0], calibri[0]);
        let _scope = fonts.enter();
        let metrics = |id| docx_layout::with_measure_face(id, |_, m| m.to_owned()).unwrap();
        assert_ne!(metrics(light[0]), metrics(calibri[0]));
        // Hebrew text adds its fallback to the chain once
        drop(_scope);
        let need = Need { scripts: vec!["hebrew".into()], ..Need::of_key("calibri|0|0") };
        assert!(!fonts.has(&need.key, &need.scripts));
        let chain = fonts.need(&need);
        assert_eq!(fonts.face(chain[2]).unwrap().family, "Noto Sans Hebrew");
        assert!(fonts.has(&need.key, &need.scripts));
        assert_eq!(fonts.need(&need), chain);
    }
}
