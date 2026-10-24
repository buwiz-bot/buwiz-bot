//! The chat palette: Grok Shell's `tc()` table carried to adaptive colors.
//!
//! Each constant is the (light, dark) pair the TS theme produced. Colors
//! are plain `Hsla` literals — `to_hsla`/`clamp` in const eval are not in
//! ra-jit's evaluator yet, and a function whose consts cannot be evaluated
//! goes to cargo, not a patch.

use std::str::FromStr;

use nsh_core::ui::prelude::*;

pub const fn tone(light: Hsla, dark: Hsla) -> AdaptiveColor {
    AdaptiveColor { light, dark }
}
pub const fn hsla(h: f32, s: f32, l: f32, a: f32) -> Hsla {
    Hsla { h, s, l, a }
}

// Panel backgrounds (TS: SIDEBAR, PANE, MAIN, BUBBLE).
pub const SIDEBAR: AdaptiveColor = tone(hsla(0.0, 0.0, 0.96, 1.0), hsla(0.0, 0.0, 0.055, 1.0));
pub const PANE: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 0.07, 1.0));
pub const MAIN: AdaptiveColor = tone(hsla(0.0, 0.0, 0.99, 1.0), hsla(0.0, 0.0, 0.086, 1.0));
pub const BUBBLE: AdaptiveColor = tone(hsla(0.0, 0.0, 0.0, 0.06), hsla(0.0, 0.0, 0.12, 1.0));
pub const CARD: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 0.122, 1.0));
pub const CARD2: AdaptiveColor = tone(hsla(0.0, 0.0, 0.11, 0.06), hsla(0.0, 0.0, 0.141, 1.0));
/// The avatar editor's floating card — `tc("#1f1f1f")` (App.tsx:3180):
/// solid #fff on light, #1f1f1f on dark (CARD2's 6% light side reads as
/// transparent over the pane).
pub const EDITOR: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 0.122, 1.0));
pub const MENU_BG: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 0.165, 1.0));
pub const DOCK: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 0.8), hsla(0.0, 0.0, 0.086, 0.8));

// Foregrounds (TS: TEXT, DIM, MUTED, FAINT, GHOST).
pub const TEXT: AdaptiveColor = tone(hsla(0.0, 0.0, 0.11, 1.0), hsla(0.0, 0.0, 0.96, 1.0));
pub const DIM: AdaptiveColor = tone(hsla(0.0, 0.0, 0.11, 0.66), hsla(0.0, 0.0, 0.96, 0.66));
pub const MUTED: AdaptiveColor = tone(hsla(0.0, 0.0, 0.11, 0.54), hsla(0.0, 0.0, 0.96, 0.54));
pub const FAINT: AdaptiveColor = tone(hsla(0.0, 0.0, 0.11, 0.40), hsla(0.0, 0.0, 0.96, 0.40));
pub const GHOST: AdaptiveColor = tone(hsla(0.0, 0.0, 0.11, 0.30), hsla(0.0, 0.0, 0.96, 0.30));

// Hairlines and fills.
pub const BORDER_F: AdaptiveColor = tone(hsla(0.0, 0.0, 0.11, 0.06), hsla(0.0, 0.0, 0.96, 0.06));
pub const BORDER_S: AdaptiveColor = tone(hsla(0.0, 0.0, 0.11, 0.12), hsla(0.0, 0.0, 0.96, 0.12));
pub const HOVER: AdaptiveColor = tone(hsla(0.0, 0.0, 0.11, 0.08), hsla(0.0, 0.0, 0.96, 0.08));
pub const ACTIVE: AdaptiveColor = tone(hsla(0.0, 0.0, 0.11, 0.10), hsla(0.0, 0.0, 0.96, 0.10));
pub const SUBTLE: AdaptiveColor = tone(hsla(0.0, 0.0, 0.11, 0.04), hsla(0.0, 0.0, 0.96, 0.04));
pub const PILL_BG: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 0.94), hsla(0.0, 0.0, 0.14, 0.94));

// Accents (TS: UP, DOWN, ACCENT, PURPLE).
pub const UP: AdaptiveColor = tone(hsla(0.37, 0.90, 0.38, 1.0), hsla(0.37, 0.90, 0.55, 1.0));
pub const UP_TINT: AdaptiveColor = tone(hsla(0.37, 0.90, 0.38, 0.12), hsla(0.37, 0.90, 0.55, 0.12));
pub const DOWN: AdaptiveColor = tone(hsla(0.02, 0.85, 0.50, 1.0), hsla(0.02, 0.85, 0.62, 1.0));
pub const DOWN_TINT: AdaptiveColor = tone(hsla(0.02, 0.85, 0.50, 0.14), hsla(0.02, 0.85, 0.62, 0.14));
pub const ACCENT: AdaptiveColor = tone(hsla(0.60, 0.99, 0.55, 1.0), hsla(0.60, 0.99, 0.63, 1.0));
pub const ACCENT_TINT: AdaptiveColor = tone(hsla(0.60, 0.99, 0.55, 0.15), hsla(0.60, 0.99, 0.63, 0.15));
pub const PURPLE: AdaptiveColor = tone(hsla(0.75, 0.85, 0.55, 1.0), hsla(0.75, 0.85, 0.68, 1.0));

// The page color the avatar/status discs knock out to.
pub const PAGE: AdaptiveColor = MAIN;

/// The status dot and running accents of an agent's card.
pub const RUNNING: AdaptiveColor = tone(hsla(0.58, 0.90, 0.50, 1.0), hsla(0.58, 0.90, 0.65, 1.0));

/// theme.ts `tc`: `#f5f5f5<alpha>` dark neutrals map to `#1c1c1c` in light
/// (with Sand's alpha remap); the SOLID table covers the opaque neutrals;
/// everything else is one color in both modes.
pub(crate) fn tc(hex: &str) -> AdaptiveColor {
    let d = Hsla::from_str(hex).unwrap_or_default();
    let k = hex.to_lowercase();
    let l = match k.as_str() {
        "#0e0e0e" => "#f5f5f5",
        "#121212" | "#1f1f1f" => "#ffffff",
        "#161616" => "#fcfcfc",
        "#1a1a1a" => "#fafafa",
        "#222222" | "#242424" => "#1c1c1c0f",
        "#262626" | "#2a2a2a" | "#2c2c2c" => "#1c1c1c14",
        _ => {
            if let Some(a) = k.strip_prefix("#f5f5f5") {
                let a = match a {
                    "a8" => "ad",
                    "8a" => "8f",
                    "e6" => "eb",
                    "a3" => "a8",
                    a => a,
                };
                return AdaptiveColor {
                    light: Hsla::from_str(&format!("#1c1c1c{a}")).unwrap_or(d),
                    dark: d,
                };
            }
            return AdaptiveColor { light: d, dark: d };
        }
    };
    AdaptiveColor {
        light: Hsla::from_str(l).unwrap_or(d),
        dark: d,
    }
}
