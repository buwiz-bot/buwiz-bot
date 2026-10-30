//! Word's fonts: the layout shapes with the very face the canvas draws.
//!
//! BetterOffice measures text with its own shaper (rustybuzz over the font
//! bytes it is given) while the page canvas draws with CoreText. Text lines
//! up only when both read the same face, so a family the document asks for
//! is resolved here as nsh's canvas resolves a CSS font (the family by name
//! and weight, italic by symbolic trait), and that face's tables are handed
//! to the shaper. A family not installed (Calibri, Cambria, Aptos on a Mac
//! without Office) takes an installed stand-in, for both sides: the drawing
//! names the stand-in, never the family CoreText would quietly swap.

use std::collections::{BTreeMap, HashMap};

use objc2_core_foundation::{CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_text::{
    CTFont, CTFontDescriptor, CTFontSymbolicTraits, CTFontTableOptions, kCTFontFamilyNameAttribute, kCTFontTraitsAttribute,
    kCTFontWeightTrait,
};

/// A face the shaper holds, as the canvas names it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Face {
    /// The installed family (a stand-in's for a missing one).
    pub family: String,
    pub bold: bool,
    pub italic: bool,
}

impl Face {
    /// The CSS font the canvas draws this face with at `px`.
    pub(crate) fn css(&self, px: f64) -> String {
        format!("{}{}{px}px \"{}\"", if self.italic { "italic " } else { "" }, if self.bold { "bold " } else { "" }, self.family)
    }
}

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

/// `family` as nsh's canvas makes it (`nsh-platform-macos` `family_font`
/// and `make_font`): `None` when it isn't installed.
fn installed(family: &str, bold: bool, italic: bool) -> Option<CFRetained<CTFont>> {
    let name = CFString::from_str(family);
    // NSFontWeightBold and NSFontWeightRegular
    let weight = CFNumber::new_f64(if bold { 0.4 } else { 0.0 });
    // SAFETY: immutable CoreText constants; the values have the types
    // CoreText documents for their keys.
    let font = unsafe {
        let traits = CFDictionary::<CFString, CFType>::from_slices(&[kCTFontWeightTrait], &[weight.as_ref()]);
        let attributes = CFDictionary::<CFString, CFType>::from_slices(&[kCTFontFamilyNameAttribute, kCTFontTraitsAttribute], &[name.as_ref(), traits.as_ref()]);
        let descriptor = CTFontDescriptor::with_attributes(attributes.as_opaque());
        CTFont::with_font_descriptor(&descriptor, 12.0, std::ptr::null())
    };
    // SAFETY: a valid font.
    if !unsafe { font.family_name() }.to_string().eq_ignore_ascii_case(family) {
        return None;
    }
    if !italic {
        return Some(font);
    }
    let it = CTFontSymbolicTraits::TraitItalic;
    // SAFETY: a valid font; size 0 and a null matrix keep the font's.
    Some(unsafe { font.copy_with_symbolic_traits(0.0, std::ptr::null(), it, it) }.unwrap_or(font))
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

/// The faces one document's layout measures with, in its own store.
pub(crate) struct Fonts {
    store: docx_layout::MeasureFonts,
    /// By font id.
    faces: Vec<Face>,
    by_face: HashMap<(String, bool, bool), u32>,
    /// The chains handed out, by the layout's `family|bold|italic` key.
    chains: BTreeMap<String, Vec<u32>>,
}

impl Fonts {
    pub(crate) fn new() -> Fonts {
        Fonts { store: docx_layout::MeasureFonts::default(), faces: Vec::new(), by_face: HashMap::new(), chains: BTreeMap::new() }
    }

    /// Makes this document's store the one the layout reads until the
    /// scope drops (every layout and display-list call runs in one).
    pub(crate) fn enter(&self) -> docx_layout::MeasureFontsScope {
        self.store.enter()
    }

    /// The face behind font id `id`.
    pub(crate) fn face(&self, id: u32) -> Option<&Face> {
        self.faces.get(id as usize)
    }

    /// The face the canvas draws `family` with (a stand-in for a missing one).
    pub(crate) fn resolve(family: &str, bold: bool, italic: bool) -> Face {
        let wanted = family.trim().trim_matches(['"', '\'']);
        let family = if !wanted.is_empty() && installed(wanted, bold, italic).is_some() { wanted.to_owned() } else { stand_in(&wanted.to_lowercase()).to_owned() };
        Face { family, bold, italic }
    }

    /// Registers `face` (once): its font id.
    fn register(&mut self, face: Face) -> Option<u32> {
        let key = (face.family.to_lowercase(), face.bold, face.italic);
        if let Some(id) = self.by_face.get(&key) {
            return Some(*id);
        }
        let font = installed(&face.family, face.bold, face.italic)?;
        let bytes = sfnt(&font)?;
        let _scope = self.store.enter();
        let id = match docx_layout::register_measure_font_bytes(&bytes) {
            Ok(id) => id,
            Err(e) => {
                log::warn!("word: font {}: {e}", face.family);
                return None;
            }
        };
        debug_assert_eq!(id as usize, self.faces.len());
        self.faces.push(face);
        self.by_face.insert(key, id);
        Some(id)
    }

    /// The chain for the layout's `family|bold|italic` key: the face alone.
    /// (A wide fallback such as Arial Unicode MS costs most of a second to
    /// hand over, so text in a script the face lacks is measured with its
    /// missing glyphs while CoreText draws it from a fallback.)
    pub(crate) fn chain(&mut self, key: &str) -> Vec<u32> {
        if let Some(c) = self.chains.get(key) {
            return c.clone();
        }
        let mut parts = key.rsplitn(3, '|');
        let italic = parts.next() == Some("1");
        let bold = parts.next() == Some("1");
        let family = parts.next().unwrap_or("");
        let mut chain = Vec::new();
        chain.extend(self.register(Fonts::resolve(family, bold, italic)));
        self.chains.insert(key.to_owned(), chain.clone());
        chain
    }

    /// Every chain handed out, as the layout's `fontChains` takes them.
    pub(crate) fn chains_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.chains).unwrap_or_default()
    }
}

/// The width CoreText sets `text` in, in `face` at `px` (the canvas's
/// `measureText`), for checking the shaper against it.
#[cfg(test)]
pub(crate) fn coretext_width(face: &Face, px: f64, text: &str) -> f64 {
    use objc2_core_foundation::{CFAttributedString, CFMutableAttributedString, CFRange};
    use objc2_core_text::{CTLine, kCTFontAttributeName};
    let Some(font) = installed(&face.family, face.bold, face.italic) else { return 0.0 };
    // SAFETY: a valid font; size, null matrix and descriptor as documented.
    let font = unsafe { font.copy_with_attributes(px, std::ptr::null(), None) };
    let string = CFMutableAttributedString::new(None, 0).expect("attributed string");
    // SAFETY: a valid mutable string; the font attribute takes a CTFont.
    unsafe {
        CFMutableAttributedString::replace_string(Some(&string), CFRange { location: 0, length: 0 }, Some(&CFString::from_str(text)));
        let range = CFRange { location: 0, length: CFAttributedString::length(&string) };
        CFMutableAttributedString::set_attribute(Some(&string), range, Some(kCTFontAttributeName), Some(font.as_ref()));
        let line = CTLine::with_attributed_string(&string);
        line.typographic_bounds(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
    }
}
