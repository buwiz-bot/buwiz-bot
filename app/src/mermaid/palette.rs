//! The colors (and fonts, radius, weights) Pele's `--pele-*` tokens take in
//! Rox: its own look by default — the ten the old Node helper set (its
//! `PALETTES`), Pele's own fallbacks for the rest — and
//! the user's overrides on top, per mode (`mermaid.colors`: the `colors`
//! of Rox's own `/state/rox-mermaid`, as its zoom and drafts are kept).

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

/// Every token Pele reads, with Pele's own default.
pub(crate) const PELE_DEFAULTS: &[(&str, &str)] = &[
    ("bg", "#fff"),
    ("fg", "#1f1f1f"),
    ("muted", "#6e6e6e"),
    ("line", "#1f1f1f"),
    ("surface", "#f3f3f3"),
    ("surface-alt", "#e6e6e6"),
    ("border", "#8a8a8a"),
    ("accent", "#5b7bd5"),
    ("font", "sans-serif"),
    ("font-mono", "monospace"),
    ("critical", "#e45756"),
    ("error", "#e45756"),
    ("radius", "4px"),
    ("zoom-top", "8px"),
    ("zoom-right", "8px"),
    ("zoom-bottom", "auto"),
    ("zoom-left", "auto"),
    ("zoom-cursor", "pointer"),
    ("fg-on-light", "#1f1f1f"),
    ("fg-on-dark", "#fff"),
    ("title-weight", "bold"),
    ("heading-weight", "bold"),
    ("bold-weight", "bold"),
    ("series-1", "#4c78a8"),
    ("series-2", "#f58518"),
    ("series-3", "#54a24b"),
    ("series-4", "#e45756"),
    ("series-5", "#72b7b2"),
    ("series-6", "#eeca3b"),
    ("series-7", "#b279a2"),
    ("series-8", "#9d755d"),
];

// Rox's agent-bubble surface and inks (theme.rs: AGENT_BUBBLE, TEXT, DIM…),
// as the Node helper set them.
const DARK: &[(&str, &str)] = &[
    ("bg", "#1f1f1f"),
    ("fg", "#f5f5f5"),
    ("muted", "#a8a8a8"),
    ("line", "#c8c8c8"),
    ("surface", "#2a2a2a"),
    ("surface-alt", "#333333"),
    ("border", "#5a5a5a"),
    ("accent", "#4d9bff"),
    ("font", "Helvetica Neue, sans-serif"),
    ("font-mono", "Menlo, monospace"),
];
const LIGHT: &[(&str, &str)] = &[
    ("bg", "#f0f0f0"),
    ("fg", "#1c1c1c"),
    ("muted", "#6e6e6e"),
    ("line", "#3a3a3a"),
    ("surface", "#ffffff"),
    ("surface-alt", "#e6e6e6"),
    ("border", "#a0a0a0"),
    ("accent", "#1884fc"),
    ("font", "Helvetica Neue, sans-serif"),
    ("font-mono", "Menlo, monospace"),
];

/// The value of each `--pele-*` token (name without `--pele-`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PelePalette {
    pub vars: BTreeMap<String, String>,
}

impl Hash for PelePalette {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for (k, v) in &self.vars {
            k.hash(state);
            v.hash(state);
        }
    }
}

/// The user's overrides: token → value, for dark and for light.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    pub dark: BTreeMap<String, String>,
    pub light: BTreeMap<String, String>,
}

impl Overrides {
    /// `{"dark": {"accent": "#f80"}, "light": {…}}`, keeping only known
    /// tokens with a value.
    pub fn from_json(v: &serde_json::Value) -> Self {
        let side = |k: &str| -> BTreeMap<String, String> {
            v.get(k)
                .and_then(|m| m.as_object())
                .map(|m| {
                    m.iter()
                        .filter(|(name, _)| PELE_DEFAULTS.iter().any(|(d, _)| d == name))
                        .filter_map(|(name, val)| Some((name.clone(), val.as_str()?.trim().to_owned())))
                        .filter(|(_, val)| !val.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        Overrides { dark: side("dark"), light: side("light") }
    }

    /// From `GET /state/rox-mermaid`'s body: `{"state": {"colors": {…}}}`.
    pub fn from_state(v: &serde_json::Value) -> Self {
        v.get("state").and_then(|st| st.get("colors")).map(Self::from_json).unwrap_or_default()
    }
}

impl PelePalette {
    /// Rox's palette for the mode, with `overrides` applied.
    pub fn new(dark: bool, overrides: &Overrides) -> Self {
        let mut vars: BTreeMap<String, String> = PELE_DEFAULTS.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        for (k, v) in if dark { DARK } else { LIGHT } {
            vars.insert((*k).to_owned(), (*v).to_owned());
        }
        for (k, v) in if dark { &overrides.dark } else { &overrides.light } {
            vars.insert(k.clone(), v.clone());
        }
        PelePalette { vars }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }

    pub fn hash64(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_token_has_a_value_and_overrides_win() {
        let mut o = Overrides::default();
        o.dark.insert("accent".into(), "#ff8800".into());
        let p = PelePalette::new(true, &o);
        for (k, _) in PELE_DEFAULTS {
            assert!(p.get(k).is_some(), "{k}");
        }
        assert_eq!(p.get("accent"), Some("#ff8800"));
        assert_eq!(p.get("bg"), Some("#1f1f1f"));
        assert_eq!(PelePalette::new(false, &o).get("accent"), Some("#1884fc"));
        assert_ne!(p.hash64(), PelePalette::new(true, &Overrides::default()).hash64());
        let parsed = Overrides::from_json(&serde_json::json!({"light": {"fg": "#000", "nope": "#fff"}}));
        assert_eq!(parsed.light.len(), 1);
    }

    #[test]
    fn overrides_come_from_the_mermaid_state() {
        let body = serde_json::json!({"state": {"colors": {"dark": {"accent": "#f80"}, "light": {"bg": "#fafafa"}}}});
        let o = Overrides::from_state(&body);
        assert_eq!(o.dark.get("accent").map(String::as_str), Some("#f80"));
        assert_eq!(o.light.get("bg").map(String::as_str), Some("#fafafa"));
        assert_eq!(Overrides::from_state(&serde_json::json!({"state": null})), Overrides::default());
        assert_eq!(Overrides::from_state(&serde_json::json!({})), Overrides::default());
    }
}
