//! The chat palette: Grok Shell's `tc()` table carried to adaptive colors.
//!
//! Each constant is the (light, dark) pair the TS theme produced. Colors
//! are plain `Hsla` literals — `to_hsla`/`clamp` in const eval are not in
//! ra-jit's evaluator yet, and a function whose consts cannot be evaluated
//! goes to cargo, not a patch.

use std::str::FromStr;

use nsh_core::prelude::*;

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
/// A floating card over content (the ⌘K palette): opaque in both themes
/// — `CARD2` is a 6% wash on light, and the chat showed through it.
pub const POPOVER: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 0.141, 1.0));
pub const CARD2: AdaptiveColor = tone(hsla(0.0, 0.0, 0.11, 0.06), hsla(0.0, 0.0, 0.141, 1.0));
/// The avatar editor's floating card — `tc("#1f1f1f")` (App.tsx:3180):
/// solid #fff on light, #1f1f1f on dark (CARD2's 6% light side reads as
/// transparent over the pane).
pub const EDITOR: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 0.122, 1.0));
pub const MENU_BG: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 0.165, 1.0));

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

/// `agentFill` (shell.tsx:11): the agent bubble — `#1f1f1f` on dark, 6%
/// black on light (Grok's "light grey, not white").
pub const AGENT_BUBBLE: AdaptiveColor = tone(hsla(0.0, 0.0, 0.0, 0.06), hsla(0.0, 0.0, 0.122, 1.0));
/// The agent bubble's fill as Grok Bot draws it on dark (read off its
/// page over CDP): `--sand-fill-bubble-agent` #f5f5f50a under a
/// `linear-gradient(#ffffff08, transparent)` sheen, as one translucent
/// top-down gradient; light stays the flat 6% wash.
pub const AGENT_BUBBLE_TOP: AdaptiveColor = tone(hsla(0.0, 0.0, 0.0, 0.06), hsla(0.0, 0.0, 0.96, 0.07));
pub const AGENT_BUBBLE_BOTTOM: AdaptiveColor = tone(hsla(0.0, 0.0, 0.0, 0.06), hsla(0.0, 0.0, 0.96, 0.04));
/// Grok Bot's 0.5pt inset ring (`--sand-border-quaternary-translucent`,
/// #f5f5f50a on dark), at 2.5%: drawn as a border on the gradient's
/// edge it read brighter than Grok's inset shadow at 4%.
pub const AGENT_RIM: AdaptiveColor = tone(hsla(0.0, 0.0, 0.11, 0.06), hsla(0.0, 0.0, 0.96, 0.025));

// Inside a filled user bubble the ink is the dark ramp in both modes —
// `OnFill` makes `tc` pass dark values through (theme.ts:34). The quote
// row's two whites are literals, not `tc` (`#ffffffb3` / `#ffffffcc`).
pub const ON_FILL: AdaptiveColor = tone(hsla(0.0, 0.0, 0.96, 1.0), hsla(0.0, 0.0, 0.96, 1.0));
pub const ON_FILL_QUOTE: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 0.8), hsla(0.0, 0.0, 1.0, 0.8));
pub const ON_FILL_ICON: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 0.7), hsla(0.0, 0.0, 1.0, 0.7));

// Accents (TS: UP, DOWN, ACCENT, PURPLE).
pub const UP: AdaptiveColor = tone(hsla(0.37, 0.90, 0.38, 1.0), hsla(0.37, 0.90, 0.55, 1.0));
pub const UP_TINT: AdaptiveColor = tone(hsla(0.37, 0.90, 0.38, 0.12), hsla(0.37, 0.90, 0.55, 0.12));
pub const DOWN: AdaptiveColor = tone(hsla(0.02, 0.85, 0.50, 1.0), hsla(0.02, 0.85, 0.62, 1.0));
pub const DOWN_TINT: AdaptiveColor = tone(hsla(0.02, 0.85, 0.50, 0.14), hsla(0.02, 0.85, 0.62, 0.14));
pub const ACCENT: AdaptiveColor = tone(hsla(0.60, 0.99, 0.55, 1.0), hsla(0.60, 0.99, 0.63, 1.0));
pub const ACCENT_TINT: AdaptiveColor = tone(hsla(0.60, 0.99, 0.55, 0.15), hsla(0.60, 0.99, 0.63, 0.15));

/// The generation placeholder's dot grid (`dotgrid.rs`): a resting dot is
/// a faint speck of the ink; a filled one is Rox's blue (#3b82f6).
pub const DOT_IDLE: AdaptiveColor = tone(hsla(0.0, 0.0, 0.11, 0.16), hsla(0.0, 0.0, 0.96, 0.16));
pub const DOT_FILL: AdaptiveColor = tone(hsla(0.603, 0.91, 0.60, 1.0), hsla(0.603, 0.91, 0.60, 1.0));

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

/// `BUBBLE` (theme.ts:47) — the user bubble's fill per canonical bot
/// color: [dark Fill, light FillHover]. Adaptive/gray bots fall through
/// to the neutral bubble: 18% `#f5f5f5` on dark, `#1c1c1c` on light.
fn bubble_pair(color: &str) -> (&'static str, &'static str) {
    match color {
        "#FC2C3E" => ("#cf2b35", "#c82532"),
        "#F9670E" => ("#cd5309", "#d2550a"),
        "#F69412" => ("#c8770c", "#c2730b"),
        "#14C370" => ("#0e9f5a", "#0d9655"),
        "#13B8A3" => ("#0e9886", "#0d9381"),
        "#1884FC" => ("#116dd2", "#126fd4"),
        "#905AFB" => ("#7845d7", "#7442d1"),
        "#FB3499" => ("#cf297e", "#c82779"),
        "#A16B39" => ("#8c5d31", "#86592e"),
        _ => ("#f5f5f52e", "#1c1c1c"),
    }
}

/// `OnFill` (theme.ts:34) — inside a filled user bubble `tc` passes the
/// dark value through in both themes; `always_dark` is the same collapse
/// for a ready `AdaptiveColor`.
pub(crate) fn always_dark(c: AdaptiveColor) -> AdaptiveColor {
    AdaptiveColor {
        light: c.dark,
        dark: c.dark,
    }
}

/// `bubbleFill` for an explicit bot color (theme.ts:58).
pub(crate) fn bubble_fill_of(color: &str) -> AdaptiveColor {
    let (d, l) = bubble_pair(&crate::mascot::norm_color(color));
    AdaptiveColor {
        light: Hsla::from_str(l).unwrap_or_default(),
        dark: Hsla::from_str(d).unwrap_or_default(),
    }
}

/// The bubble fill of chat `chat`'s bot (a plain chat: the default).
/// Tracked: a bot's color change repaints.
pub(crate) fn bubble_fill_for(chat: &str) -> AdaptiveColor {
    let st = crate::live::store();
    let bot = st.threads.with(|t| t.iter().find(|t| t.id == chat).and_then(|t| t.bot.clone()));
    let color = bot
        .and_then(|tb| st.bots.with(|b| b.iter().find(|b| b.id == tb.id).map(|b| b.color.clone())))
        .unwrap_or_default();
    bubble_fill_of(&color)
}

