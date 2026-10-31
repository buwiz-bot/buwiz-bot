//! BetterOffice's bundled fonts (`@betteroffice/fonts` 0.4.0, OFL): the
//! open faces the web editor measures and draws Office's families with
//! (Carlito for Calibri, Caladea for Cambria, Liberation for Arial, Times
//! New Roman and Courier New, ...), and its resolvers, ported from the
//! package's `dist/index.js`, so a family picks the same files here as
//! there.
//!
//! The files ship in the app bundle, not in git: `app/bundle-dev.sh`
//! copies them from `server/node_modules/@betteroffice/fonts/assets` into
//! `Contents/Resources/fonts/office/`. Dev runs and tests read them from
//! there, from `ROX_OFFICE_FONTS`, or from the checkout's node_modules.
//! They are registered with CoreText before the first paint ([`register`])
//! so the canvas draws by family name ("Carlito") from the very files the
//! engines measure with.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// A face of the manifest (`BUNDLED_FONTS`).
#[derive(Debug, PartialEq)]
pub struct BundledFace {
    /// Its family name, as CoreText registers it.
    pub family: &'static str,
    /// The Office family it is metric-compatible with.
    compat: Option<&'static str>,
    pub bold: bool,
    pub italic: bool,
    pub file: &'static str,
    /// The script it covers as a fallback (`hebrew`, `arabic`, `cjk-*`).
    script: Option<&'static str>,
}

const fn f(family: &'static str, compat: Option<&'static str>, bold: bool, italic: bool, file: &'static str, script: Option<&'static str>) -> BundledFace {
    BundledFace { family, compat, bold, italic, file, script }
}

/// `familyFaces`: regular, bold, italic, bold italic.
macro_rules! four {
    ($family:literal, $compat:literal, $base:literal) => {
        [
            f($family, Some($compat), false, false, concat!($base, "-Regular.ttf"), None),
            f($family, Some($compat), true, false, concat!($base, "-Bold.ttf"), None),
            f($family, Some($compat), false, true, concat!($base, "-Italic.ttf"), None),
            f($family, Some($compat), true, true, concat!($base, "-BoldItalic.ttf"), None),
        ]
    };
}

/// The package's manifest, in its order (lookups take the first match).
/// The CJK faces are `@betteroffice/fonts-cjk`'s, which Rox doesn't ship:
/// they resolve, and a missing file leaves them out of a chain.
static BUNDLED: [BundledFace; 70] = {
    let [c0, c1, c2, c3] = four!("Carlito", "Calibri", "Carlito");
    let [d0, d1, d2, d3] = four!("Caladea", "Cambria", "Caladea");
    let [s0, s1, s2, s3] = four!("Liberation Sans", "Arial", "LiberationSans");
    let [r0, r1, r2, r3] = four!("Liberation Serif", "Times New Roman", "LiberationSerif");
    let [m0, m1, m2, m3] = four!("Liberation Mono", "Courier New", "LiberationMono");
    let [g0, g1, g2, g3] = four!("Gelasio", "Georgia", "Gelasio");
    let [i0, i1, i2, i3] = four!("Inter", "Inter", "Inter");
    let [o0, o1, o2, o3] = four!("Roboto", "Roboto", "Roboto");
    let [p0, p1, p2, p3] = four!("Source Sans 3", "Source Sans Pro", "SourceSans3");
    let [a0, a1, a2, a3] = four!("DM Sans", "DM Sans", "DMSans");
    let [n0, n1, n2, n3] = four!("Open Sans", "Open Sans", "OpenSans");
    let [t0, t1, t2, t3] = four!("Montserrat", "Montserrat", "Montserrat");
    let [q0, q1, q2, q3] = four!("Poppins", "Poppins", "Poppins");
    [
        c0, c1, c2, c3, d0, d1, d2, d3, s0, s1, s2, s3, r0, r1, r2, r3, m0, m1, m2, m3, g0, g1, g2, g3, i0, i1, i2, i3, o0, o1, o2, o3, p0, p1, p2, p3, a0, a1, a2, a3, n0, n1, n2, n3, t0, t1,
        t2, t3, q0, q1, q2, q3,
        // faces with no italic, or no bold: the missing ones fall back
        // through the chain
        f("Oswald", Some("Oswald"), false, false, "Oswald-Regular.ttf", None),
        f("Oswald", Some("Oswald"), true, false, "Oswald-Bold.ttf", None),
        f("Heebo", Some("Heebo"), false, false, "Heebo-Regular.ttf", None),
        f("Heebo", Some("Heebo"), true, false, "Heebo-Bold.ttf", None),
        f("DM Serif Display", Some("DM Serif Display"), false, false, "DMSerifDisplay-Regular.ttf", None),
        f("DM Serif Display", Some("DM Serif Display"), false, true, "DMSerifDisplay-Italic.ttf", None),
        f("Comic Relief", Some("Comic Sans MS"), false, false, "ComicRelief-Regular.ttf", None),
        f("Comic Relief", Some("Comic Sans MS"), true, false, "ComicRelief-Bold.ttf", None),
        // right-to-left script fallbacks: no Office family of their own
        f("Noto Sans Hebrew", None, false, false, "NotoSansHebrew-Regular.ttf", Some("hebrew")),
        f("Noto Sans Hebrew", None, true, false, "NotoSansHebrew-Bold.ttf", Some("hebrew")),
        f("Noto Sans Arabic", None, false, false, "NotoSansArabic-Regular.ttf", Some("arabic")),
        f("Noto Sans Arabic", None, true, false, "NotoSansArabic-Bold.ttf", Some("arabic")),
        f("Noto Naskh Arabic", None, false, false, "NotoNaskhArabic-Regular.ttf", Some("arabic")),
        f("Noto Sans SC", Some("Microsoft YaHei"), false, false, "NotoSansSC-Regular.otf", Some("cjk-sc")),
        f("Noto Serif SC", Some("SimSun"), false, false, "NotoSerifSC-Regular.otf", Some("cjk-sc")),
        f("Noto Sans TC", Some("Microsoft JhengHei"), false, false, "NotoSansTC-Regular.otf", Some("cjk-tc")),
        f("Noto Sans JP", Some("MS Gothic"), false, false, "NotoSansJP-Regular.otf", Some("cjk-jp")),
        f("Noto Sans KR", Some("Malgun Gothic"), false, false, "NotoSansKR-Regular.otf", Some("cjk-kr")),
    ]
};

/// `WORD_FAMILY_ALIASES`: a family (lowercase) laid out against another's
/// metrics.
const ALIASES: &[(&str, &str)] = &[
    ("helvetica", "arial"),
    ("times", "times new roman"),
    ("courier", "courier new"),
    ("arimo", "arial"),
    ("liberation sans", "arial"),
    ("nimbus sans", "arial"),
    ("helvetica neue", "arial"),
    ("carlito", "calibri"),
    ("tinos", "times new roman"),
    ("liberation serif", "times new roman"),
    ("nimbus roman", "times new roman"),
    ("cousine", "courier new"),
    ("liberation mono", "courier new"),
    ("nimbus mono ps", "courier new"),
    ("caladea", "cambria"),
    ("simhei", "microsoft yahei"),
    ("dengxian", "microsoft yahei"),
    ("微软雅黑", "microsoft yahei"),
    ("黑体", "microsoft yahei"),
    ("等线", "microsoft yahei"),
    ("nsimsun", "simsun"),
    ("fangsong", "simsun"),
    ("kaiti", "simsun"),
    ("宋体", "simsun"),
    ("仿宋", "simsun"),
    ("楷体", "simsun"),
    ("微軟正黑體", "microsoft jhenghei"),
    ("pmingliu", "microsoft jhenghei"),
    ("mingliu", "microsoft jhenghei"),
    ("dfkai-sb", "microsoft jhenghei"),
    ("新細明體", "microsoft jhenghei"),
    ("細明體", "microsoft jhenghei"),
    ("標楷體", "microsoft jhenghei"),
    ("ms pgothic", "ms gothic"),
    ("meiryo", "ms gothic"),
    ("yu gothic", "ms gothic"),
    ("ｍｓ ゴシック", "ms gothic"),
    ("ｍｓ ｐゴシック", "ms gothic"),
    ("メイリオ", "ms gothic"),
    ("游ゴシック", "ms gothic"),
    ("ms mincho", "ms gothic"),
    ("ms pmincho", "ms gothic"),
    ("yu mincho", "ms gothic"),
    ("ｍｓ 明朝", "ms gothic"),
    ("ｍｓ ｐ明朝", "ms gothic"),
    ("游明朝", "ms gothic"),
    ("맑은 고딕", "malgun gothic"),
    ("gulim", "malgun gothic"),
    ("dotum", "malgun gothic"),
    ("batang", "malgun gothic"),
    ("gungsuh", "malgun gothic"),
    ("굴림", "malgun gothic"),
    ("돋움", "malgun gothic"),
    ("바탕", "malgun gothic"),
    ("궁서", "malgun gothic"),
];

/// `resolveMetricCompatFamily`: the bundled family laid out with
/// `family`'s metrics.
pub fn metric_compat_family(family: &str) -> Option<&'static str> {
    let key = family.trim().to_lowercase();
    let key = ALIASES.iter().find(|(from, _)| *from == key).map_or(key.as_str(), |(_, to)| to);
    // the JS map is filled in manifest order: the last face wins
    BUNDLED.iter().rev().find(|face| face.compat.is_some_and(|c| c.to_lowercase() == key)).map(|face| face.family)
}

/// `resolveMetricCompatFace`: the face of that family in the style, else
/// its regular one.
pub fn metric_compat_face(family: &str, bold: bool, italic: bool) -> Option<&'static BundledFace> {
    let family = metric_compat_family(family)?;
    let faces = || BUNDLED.iter().filter(move |face| face.family == family);
    faces().find(|face| face.bold == bold && face.italic == italic).or_else(|| faces().find(|face| !face.bold && !face.italic))
}

/// `resolveScriptFallbackFace`.
fn script_fallback_face(script: &str, bold: bool, italic: bool) -> Option<&'static BundledFace> {
    let faces = || BUNDLED.iter().filter(move |face| face.script == Some(script));
    faces()
        .find(|face| face.bold == bold && face.italic == italic)
        .or_else(|| faces().find(|face| face.bold == bold && !face.italic))
        .or_else(|| faces().find(|face| !face.bold && !face.italic))
        .or_else(|| faces().next())
}

fn looks_serif(family: &str) -> bool {
    let lower = family.to_lowercase();
    [
        "antiqua", "bookman", "calisto", "schoolbook", "goudy", "perpetua", "rockwell", "caslon", "constantia", "didot", "baskerville", "times", "georgia", "garamond", "palatino", "bodoni", "cambria",
        "minion", "mincho", "明朝", "明體", "宋", "ming", "song", "serif",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

/// The family's words, split as `/[\s-]+/` splits it.
fn words(family: &str) -> Vec<&str> {
    family.trim().split(|c: char| c.is_whitespace() || c == '-').filter(|w| !w.is_empty()).collect()
}

fn looks_mono(family: &str) -> bool {
    let lower = family.to_lowercase();
    ["consolas", "monaco", "menlo", "console", "andale", "inconsolata", "cascadia", "typewriter"].iter().any(|w| lower.contains(w)) || words(&lower).contains(&"mono")
}

/// `lightVariantOf` (`heavy`: `heavyVariantOf`): "Calibri Light" is
/// Calibri's face at regular weight, bold dropped; "Arial Black" Arial's
/// bold.
fn variant_of(family: &str, italic: bool, heavy: bool) -> Option<&'static BundledFace> {
    const LIGHTER: [&str; 7] = ["thin", "hairline", "extralight", "ultralight", "semilight", "light", "medium"];
    const HEAVIER: [&str; 6] = ["black", "heavy", "extrabold", "ultrabold", "extrablack", "ultra"];
    let words = words(family);
    let (last, base) = words.split_last()?;
    let set: &[&str] = if heavy { &HEAVIER } else { &LIGHTER };
    if base.is_empty() || !set.contains(&last.to_lowercase().as_str()) {
        return None;
    }
    metric_compat_face(&base.join(" "), heavy, italic)
}

/// `resolveLastResortFace`: the face that stands in for a family with no
/// metric-compatible one (`word`: Arial is the sans fallback, as Word's;
/// else Calibri, as PowerPoint's).
pub fn last_resort_face(family: &str, bold: bool, italic: bool, word: bool) -> Option<&'static BundledFace> {
    if let Some(face) = variant_of(family, italic, false).or_else(|| variant_of(family, italic, true)) {
        return Some(face);
    }
    if looks_mono(family) {
        return metric_compat_face("Courier New", bold, italic);
    }
    // WIDTH_MATCHED_SUBSTITUTES
    if family.trim().eq_ignore_ascii_case("verdana")
        && let Some(face) = metric_compat_face("Montserrat", bold, italic)
    {
        return Some(face);
    }
    let base = if looks_serif(family) {
        "Times New Roman"
    } else if word {
        "Arial"
    } else {
        "Calibri"
    };
    metric_compat_face(base, bold, italic)
}

/// The bundled faces a Word family measures with, in the web registry's
/// order (`resolveChain`, after the document's embedded face): the
/// metric-compatible face, then the last resort, each once.
pub fn word_chain(family: &str, bold: bool, italic: bool) -> Vec<&'static BundledFace> {
    let mut chain: Vec<&'static BundledFace> = Vec::new();
    for face in [metric_compat_face(family, bold, italic), last_resort_face(family, bold, italic, true)].into_iter().flatten() {
        if !chain.contains(&face) {
            chain.push(face);
        }
    }
    chain
}

/// The face text in `script` falls back to (the web asks for the regular
/// one, whatever the run's style).
pub fn script_face(script: &str) -> Option<&'static BundledFace> {
    script_fallback_face(script, false, false)
}

/// The family the canvas draws `family` with: its bundled
/// metric-compatible one (Calibri is drawn and measured as Carlito), else
/// itself.
pub fn canvas_family(family: &str) -> &str {
    metric_compat_family(family).unwrap_or(family)
}

/// The folder holding the font files: `ROX_OFFICE_FONTS`, the bundle's
/// `Resources/fonts/office`, this checkout's node_modules (dev builds and
/// tests), or the dev bundle's (Buwiz Bot.app's, then Rox.app's). `None`
/// (logged once) when none has them.
pub fn dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(d) = std::env::var_os("ROX_OFFICE_FONTS") {
            dirs.push(d.into());
        }
        if let Ok(exe) = std::env::current_exe() {
            dirs.extend(crate::bundle::exe_resources(&exe, "fonts/office"));
        }
        dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../server/node_modules/@betteroffice/fonts/assets"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.extend(crate::bundle::installed_resources(Path::new(&home), "fonts/office"));
        }
        let found = dirs.iter().find(|d| d.join(BUNDLED[0].file).is_file()).cloned();
        if found.is_none() {
            log::warn!("no office fonts in {dirs:?}: Word and sheets measure with installed stand-ins (app/bundle-dev.sh copies them; or set ROX_OFFICE_FONTS)");
        }
        found
    })
    .as_deref()
}

/// The files read, by name (`None`: not there).
type Read = HashMap<&'static str, Option<Arc<[u8]>>>;

/// The bytes of `face`'s file, read once; `None` when it isn't there.
pub fn bytes(face: &'static BundledFace) -> Option<Arc<[u8]>> {
    static READ: OnceLock<Mutex<Read>> = OnceLock::new();
    let mut read = READ.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    read.entry(face.file).or_insert_with(|| std::fs::read(dir()?.join(face.file)).ok().map(Arc::from)).clone()
}

/// Registers every bundled file there is with CoreText, for this process,
/// once (a later caller waits for the first). Before anything draws them:
/// the canvas caches the font each family resolved to. About 1 ms a file
/// once CoreText is warm.
pub fn register() {
    use objc2_core_foundation::{CFRetained, CFURL};
    use objc2_core_text::{CTFontManagerRegisterFontsForURL, CTFontManagerScope};
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(dir) = dir() else { return };
        for face in &BUNDLED {
            let path = dir.join(face.file);
            if !path.is_file() {
                continue;
            }
            let Some(url) = CFURL::from_file_path(&path) else { continue };
            let mut error = std::ptr::null_mut();
            // SAFETY: a valid file URL and error out-param.
            if !unsafe { CTFontManagerRegisterFontsForURL(&url, CTFontManagerScope::Process, &mut error) } {
                // SAFETY: on failure the out-param holds a +1 CFError.
                let Some(error) = std::ptr::NonNull::new(error).map(|e| unsafe { CFRetained::from_raw(e) }) else { continue };
                // kCTFontManagerErrorAlreadyRegistered: the family is installed too
                if error.code() != 105 {
                    log::warn!("office font {}: {:?}", face.file, error.description());
                }
            }
        }
    });
}

/// A face the shaper holds, as the canvas names it.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// The family the canvas names: a bundled one, or installed (a
    /// stand-in's for a missing one).
    pub family: String,
    pub bold: bool,
    pub italic: bool,
}

impl Face {
    /// The CSS font the canvas draws this face with at `px`.
    pub fn css(&self, px: f64) -> String {
        format!("{}{}{px}px \"{}\"", if self.italic { "italic " } else { "" }, if self.bold { "bold " } else { "" }, self.family)
    }
}

/// `family` as nsh's canvas makes it (`nsh-platform-macos` `family_font`
/// and `make_font`): `None` when it isn't installed.
pub fn installed(family: &str, bold: bool, italic: bool) -> Option<objc2_core_foundation::CFRetained<objc2_core_text::CTFont>> {
    use objc2_core_foundation::{CFDictionary, CFNumber, CFString, CFType};
    use objc2_core_text::{CTFont, CTFontDescriptor, CTFontSymbolicTraits, kCTFontFamilyNameAttribute, kCTFontTraitsAttribute, kCTFontWeightTrait};
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

/// The width the canvas measures `text` in CSS font `font` with, by the
/// lookup nsh's macOS canvas makes (family and weight, italic by trait).
#[cfg(any(test, feature = "testing"))]
pub fn canvas_width(font: &str, text: &str) -> f32 {
    register();
    let font = nsh_core::canvas::CanvasFont::parse(font).unwrap_or_default();
    let face = Face { family: font.family.as_deref().unwrap_or("Helvetica").to_owned(), bold: font.weight >= 600, italic: font.italic };
    coretext_width(&face, font.size.0 as f64, text) as f32
}

/// The width CoreText sets `text` in, in `face` at `px` (the canvas's
/// `measureText`), for checking the shaper against it.
#[cfg(any(test, feature = "testing"))]
pub fn coretext_width(face: &Face, px: f64, text: &str) -> f64 {
    use objc2_core_foundation::{CFAttributedString, CFMutableAttributedString, CFRange, CFString};
    use objc2_core_text::{CTLine, kCTFontAttributeName};
    register();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The JS resolver's chains (`resolveMetricCompatFace`, then
    /// `resolveLastResortFace(…, "word")`, each file once), for regular,
    /// bold, italic and bold italic, worked from `@betteroffice/fonts`
    /// 0.4.0's `dist/index.js`.
    const WEB: &[(&str, [&[&str]; 4])] = &[
        (
            "Calibri",
            [
                &["Carlito-Regular.ttf", "LiberationSans-Regular.ttf"],
                &["Carlito-Bold.ttf", "LiberationSans-Bold.ttf"],
                &["Carlito-Italic.ttf", "LiberationSans-Italic.ttf"],
                &["Carlito-BoldItalic.ttf", "LiberationSans-BoldItalic.ttf"],
            ],
        ),
        // no alias: lightVariantOf, Calibri's regular weight
        ("Calibri Light", [&["Carlito-Regular.ttf"], &["Carlito-Regular.ttf"], &["Carlito-Italic.ttf"], &["Carlito-Italic.ttf"]]),
        (
            "Cambria",
            [
                &["Caladea-Regular.ttf", "LiberationSerif-Regular.ttf"],
                &["Caladea-Bold.ttf", "LiberationSerif-Bold.ttf"],
                &["Caladea-Italic.ttf", "LiberationSerif-Italic.ttf"],
                &["Caladea-BoldItalic.ttf", "LiberationSerif-BoldItalic.ttf"],
            ],
        ),
        ("Arial", [&["LiberationSans-Regular.ttf"], &["LiberationSans-Bold.ttf"], &["LiberationSans-Italic.ttf"], &["LiberationSans-BoldItalic.ttf"]]),
        ("Helvetica", [&["LiberationSans-Regular.ttf"], &["LiberationSans-Bold.ttf"], &["LiberationSans-Italic.ttf"], &["LiberationSans-BoldItalic.ttf"]]),
        ("Helvetica Neue", [&["LiberationSans-Regular.ttf"], &["LiberationSans-Bold.ttf"], &["LiberationSans-Italic.ttf"], &["LiberationSans-BoldItalic.ttf"]]),
        ("Times New Roman", [&["LiberationSerif-Regular.ttf"], &["LiberationSerif-Bold.ttf"], &["LiberationSerif-Italic.ttf"], &["LiberationSerif-BoldItalic.ttf"]]),
        ("Times", [&["LiberationSerif-Regular.ttf"], &["LiberationSerif-Bold.ttf"], &["LiberationSerif-Italic.ttf"], &["LiberationSerif-BoldItalic.ttf"]]),
        (
            "Courier New",
            [
                &["LiberationMono-Regular.ttf", "LiberationSans-Regular.ttf"],
                &["LiberationMono-Bold.ttf", "LiberationSans-Bold.ttf"],
                &["LiberationMono-Italic.ttf", "LiberationSans-Italic.ttf"],
                &["LiberationMono-BoldItalic.ttf", "LiberationSans-BoldItalic.ttf"],
            ],
        ),
        (
            "Courier",
            [
                &["LiberationMono-Regular.ttf", "LiberationSans-Regular.ttf"],
                &["LiberationMono-Bold.ttf", "LiberationSans-Bold.ttf"],
                &["LiberationMono-Italic.ttf", "LiberationSans-Italic.ttf"],
                &["LiberationMono-BoldItalic.ttf", "LiberationSans-BoldItalic.ttf"],
            ],
        ),
        (
            "Georgia",
            [
                &["Gelasio-Regular.ttf", "LiberationSerif-Regular.ttf"],
                &["Gelasio-Bold.ttf", "LiberationSerif-Bold.ttf"],
                &["Gelasio-Italic.ttf", "LiberationSerif-Italic.ttf"],
                &["Gelasio-BoldItalic.ttf", "LiberationSerif-BoldItalic.ttf"],
            ],
        ),
        // unknown: Word's Arial
        ("Some Unknown Font", [&["LiberationSans-Regular.ttf"], &["LiberationSans-Bold.ttf"], &["LiberationSans-Italic.ttf"], &["LiberationSans-BoldItalic.ttf"]]),
        ("Aptos", [&["LiberationSans-Regular.ttf"], &["LiberationSans-Bold.ttf"], &["LiberationSans-Italic.ttf"], &["LiberationSans-BoldItalic.ttf"]]),
        ("Consolas", [&["LiberationMono-Regular.ttf"], &["LiberationMono-Bold.ttf"], &["LiberationMono-Italic.ttf"], &["LiberationMono-BoldItalic.ttf"]]),
        ("Garamond", [&["LiberationSerif-Regular.ttf"], &["LiberationSerif-Bold.ttf"], &["LiberationSerif-Italic.ttf"], &["LiberationSerif-BoldItalic.ttf"]]),
        ("Verdana", [&["Montserrat-Regular.ttf"], &["Montserrat-Bold.ttf"], &["Montserrat-Italic.ttf"], &["Montserrat-BoldItalic.ttf"]]),
        ("Arial Black", [&["LiberationSans-Bold.ttf"], &["LiberationSans-Bold.ttf"], &["LiberationSans-BoldItalic.ttf"], &["LiberationSans-BoldItalic.ttf"]]),
        // "Segoe UI" has no bundled face: the light variant falls through
        ("Segoe UI Semilight", [&["LiberationSans-Regular.ttf"], &["LiberationSans-Bold.ttf"], &["LiberationSans-Italic.ttf"], &["LiberationSans-BoldItalic.ttf"]]),
        (
            "Comic Sans MS",
            [
                &["ComicRelief-Regular.ttf", "LiberationSans-Regular.ttf"],
                &["ComicRelief-Bold.ttf", "LiberationSans-Bold.ttf"],
                &["ComicRelief-Regular.ttf", "LiberationSans-Italic.ttf"],
                &["ComicRelief-Regular.ttf", "LiberationSans-BoldItalic.ttf"],
            ],
        ),
        (
            "Carlito",
            [
                &["Carlito-Regular.ttf", "LiberationSans-Regular.ttf"],
                &["Carlito-Bold.ttf", "LiberationSans-Bold.ttf"],
                &["Carlito-Italic.ttf", "LiberationSans-Italic.ttf"],
                &["Carlito-BoldItalic.ttf", "LiberationSans-BoldItalic.ttf"],
            ],
        ),
    ];

    #[test]
    fn word_chains_pick_the_web_resolvers_files() {
        for (family, styles) in WEB {
            for (i, want) in styles.iter().enumerate() {
                let (bold, italic) = (i & 1 == 1, i & 2 == 2);
                let got: Vec<&str> = word_chain(family, bold, italic).iter().map(|f| f.file).collect();
                assert_eq!(&got, want, "{family} bold={bold} italic={italic}");
            }
        }
    }

    #[test]
    fn scripts_and_the_canvas_take_the_web_faces() {
        assert_eq!(script_face("hebrew").map(|f| f.file), Some("NotoSansHebrew-Regular.ttf"));
        assert_eq!(script_face("arabic").map(|f| f.file), Some("NotoSansArabic-Regular.ttf"));
        assert_eq!(script_face("cjk-jp").map(|f| f.file), Some("NotoSansJP-Regular.otf"));
        assert_eq!(script_face("latin"), None);
        assert_eq!(canvas_family("Calibri"), "Carlito");
        assert_eq!(canvas_family(" cambria "), "Caladea");
        assert_eq!(canvas_family("Helvetica"), "Liberation Sans");
        assert_eq!(canvas_family("Menlo"), "Menlo");
    }

    #[test]
    fn every_ttf_of_the_manifest_is_there_and_registers() {
        let dir = dir().expect("the office fonts (npm ci in server/, or ROX_OFFICE_FONTS)");
        for face in BUNDLED.iter().filter(|f| f.file.ends_with(".ttf")) {
            assert!(dir.join(face.file).is_file(), "{}", face.file);
        }
        // CoreText finds a registered family by name, at its weight
        let w = canvas_width("bold 20px \"Carlito\"", "Revenue");
        let r = canvas_width("20px \"Carlito\"", "Revenue");
        assert!(w > r && r > 0.0, "bold {w}, regular {r}");
    }
}
