//! The avatar colors: the picker's swatches, the stored-value cleanup and
//! the ink a theme paints a swatch in.

use nsh_core::prelude::Hsla;
use std::str::FromStr;

/// The picker's swatches, in picker order. These are the strings saved on
/// a bot (`color`), and the same list the avatar generator on the server
/// picks from, so a generated bot's swatch shows as picked.
pub(crate) const BOT_COLORS: [&str; 11] = [
    "#FFFFFF", "#936439", "#FF263C", "#FF6700", "#FF9800", "#00C972", "#00BCA6", "#1084FE",
    "#9159FE", "#FF309B", "#777777",
];

/// The adaptive swatch: dark ink on light themes, white on dark ones.
pub(crate) const ADAPTIVE: &str = "#FFFFFF";

/// Each saved swatch and the tone the app draws it in (also the key of the
/// chat bubble fills in `theme.rs`). Bots saved with either spelling look
/// the same.
const TONES: [(&str, &str); 9] = [
    ("#936439", "#A16B39"),
    ("#FF263C", "#FC2C3E"),
    ("#FF6700", "#F9670E"),
    ("#FF9800", "#F69412"),
    ("#00C972", "#14C370"),
    ("#00BCA6", "#13B8A3"),
    ("#1084FE", "#1884FC"),
    ("#9159FE", "#905AFB"),
    ("#FF309B", "#FB3499"),
];

/// A stored color in one canonical spelling: trimmed, upper case,
/// `#RRGGBB` (short forms expanded, an opaque alpha dropped, any other
/// alpha kept as `#RRGGBBAA`), and a swatch replaced by its drawing tone.
/// Anything that isn't a hex color comes back trimmed and upper-cased.
pub(crate) fn norm_color(color: &str) -> String {
    let raw = color.trim();
    let digits = raw.strip_prefix('#').unwrap_or(raw);
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return raw.to_uppercase();
    }
    let expanded: String = match digits.len() {
        3 | 4 => digits.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => digits.to_owned(),
        _ => return raw.to_uppercase(),
    };
    let mut hex = format!("#{}", expanded.to_uppercase());
    if hex.len() == 9 && hex.ends_with("FF") {
        hex.truncate(7);
    }
    match TONES.iter().find(|(saved, _)| *saved == hex) {
        Some((_, tone)) => (*tone).to_owned(),
        None => hex,
    }
}

/// The ink a bot's `color` paints in: the swatch's tone, or for the
/// adaptive white swatch near-black on a light theme and white on a dark
/// one. An unreadable color falls back to the adaptive ink.
pub(crate) fn ink_of(color: &str, light_theme: bool) -> Hsla {
    let canonical = norm_color(color);
    let adaptive = || {
        if light_theme {
            Hsla { h: 0.0, s: 0.0, l: 0.11, a: 1.0 }
        } else {
            Hsla { h: 0.0, s: 0.0, l: 1.0, a: 1.0 }
        }
    };
    if canonical == ADAPTIVE {
        return adaptive();
    }
    Hsla::from_str(&canonical).unwrap_or_else(|_| adaptive())
}

/// `ink` pulled toward black by `amount` (0 keeps it, 1 is black): the
/// pane avatar's hover-to-edit shade.
pub(crate) fn darken(ink: Hsla, amount: f64) -> Hsla {
    let keep = (1.0 - amount.clamp(0.0, 1.0)) as f32;
    Hsla { l: ink.l * keep, ..ink }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swatches_normalize_to_their_tones() {
        assert_eq!(norm_color("#FF263C"), "#FC2C3E");
        assert_eq!(norm_color(" #ff263c "), "#FC2C3E");
        assert_eq!(norm_color("#FC2C3E"), "#FC2C3E");
        assert_eq!(norm_color("#1084FEFF"), "#1884FC");
        assert_eq!(norm_color("#fff"), "#FFFFFF");
        assert_eq!(norm_color("#777777"), "#777777");
        assert_eq!(norm_color("#f5f5f58a"), "#F5F5F58A");
        assert_eq!(norm_color("teal"), "TEAL");
        assert_eq!(norm_color(""), "");
    }

    #[test]
    fn every_swatch_is_distinct_and_has_an_ink() {
        let mut seen = std::collections::HashSet::new();
        for c in BOT_COLORS {
            assert!(seen.insert(norm_color(c)), "{c} collides");
            let light = ink_of(c, true);
            let dark = ink_of(c, false);
            assert!(light.a > 0.99 && dark.a > 0.99, "{c}");
        }
    }

    #[test]
    fn adaptive_swatch_flips_with_the_theme() {
        assert!(ink_of("#FFFFFF", true).l < 0.2);
        assert!(ink_of("#ffffff", false).l > 0.95);
        // an unreadable value draws like the adaptive swatch
        assert!(ink_of("not a color", true).l < 0.2);
        // a colored swatch is the same in both themes
        let (a, b) = (ink_of("#FF263C", true), ink_of("#FF263C", false));
        assert!((a.h - b.h).abs() < 1e-6 && (a.l - b.l).abs() < 1e-6);
    }

    #[test]
    fn saved_spellings_match_in_the_picker() {
        // the pane compares `norm_color` of the bot's value and the swatch
        for (saved, tone) in TONES {
            assert_eq!(norm_color(saved), norm_color(tone));
            assert!(BOT_COLORS.contains(&saved));
        }
    }

    #[test]
    fn darken_moves_toward_black() {
        let white = ink_of("#FFFFFF", false);
        assert!((darken(white, 0.0).l - 1.0).abs() < 1e-6);
        assert!((darken(white, 0.7).l - 0.3).abs() < 1e-4);
        assert!(darken(white, 2.0).l.abs() < 1e-6);
    }
}
