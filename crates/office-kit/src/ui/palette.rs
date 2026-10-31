//! The web Word editor's colors (`@betteroffice/docx-react` 0.4.3,
//! `dist/styles.css`): its `--doc-*` tokens and the Tailwind theme tokens
//! its utility classes read (`bg-muted`, `text-muted-foreground`, …), each
//! as the (light, dark) pair of `.oox-root` and `.oox-root.dark`, so the
//! Word view has the web's pixels in each appearance and follows Rox's.
//!
//! Plain `Hsla` literals (hue as a fraction of a turn), converted from the
//! stylesheet's hex: `to_hsla` in const eval is not in ra-jit's evaluator
//! (see `theme.rs`). Tokens the light block leaves out take the value the
//! web falls back to (`--doc-page-bg` white, `--doc-caret` black).

// `--border`'s 31.8% saturation reads to clippy as 1/π.
#![allow(clippy::approx_constant)]

use nsh_core::prelude::*;

/// As the app's `theme.rs` makes its tokens: plain literals, for ra-jit's
/// const evaluator.
pub const fn tone(light: Hsla, dark: Hsla) -> AdaptiveColor {
    AdaptiveColor { light, dark }
}
pub const fn hsla(h: f32, s: f32, l: f32, a: f32) -> Hsla {
    Hsla { h, s, l, a }
}

/// `--doc-surface`: #fff / #1f1f1f.
pub const SURFACE: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 0.1216, 1.0));
/// `--doc-card`: #f8fbff / #2d2d2d.
pub const CARD: AdaptiveColor = tone(hsla(0.5952, 1.0, 0.9863, 1.0), hsla(0.0, 0.0, 0.1765, 1.0));
/// `--doc-bg`: #f8f9fa / #262626.
pub const BG: AdaptiveColor = tone(hsla(0.5833, 0.1667, 0.9765, 1.0), hsla(0.0, 0.0, 0.149, 1.0));
/// `--doc-bg-subtle`: #f5f5f5 / #2d2d2d.
pub const BG_SUBTLE: AdaptiveColor = tone(hsla(0.0, 0.0, 0.9608, 1.0), hsla(0.0, 0.0, 0.1765, 1.0));
/// `--doc-bg-hover`: #f1f3f4 / #3a3a3a.
pub const BG_HOVER: AdaptiveColor = tone(hsla(0.5556, 0.12, 0.951, 1.0), hsla(0.0, 0.0, 0.2275, 1.0));
/// `--doc-bg-input`: #f8f9fa / #333.
pub const BG_INPUT: AdaptiveColor = tone(hsla(0.5833, 0.1667, 0.9765, 1.0), hsla(0.0, 0.0, 0.2, 1.0));
/// `--doc-primary`: #1a73e8 / #4e9dff.
pub const PRIMARY: AdaptiveColor = tone(hsla(0.5947, 0.8175, 0.5059, 1.0), hsla(0.5923, 1.0, 0.6529, 1.0));
/// `--doc-primary-hover`: #1557b0 / #6cb0ff.
pub const PRIMARY_HOVER: AdaptiveColor = tone(hsla(0.5957, 0.7868, 0.3863, 1.0), hsla(0.5896, 1.0, 0.7118, 1.0));
/// `--doc-primary-light`: #e8f0fe / rgba(78,157,255,.18).
pub const PRIMARY_LIGHT: AdaptiveColor = tone(hsla(0.6061, 0.9167, 0.9529, 1.0), hsla(0.5923, 1.0, 0.6529, 0.18));
/// `--doc-accent`: #6366f1 / #4e9dff.
pub const ACCENT: AdaptiveColor = tone(hsla(0.6631, 0.8353, 0.6667, 1.0), hsla(0.5923, 1.0, 0.6529, 1.0));
/// `--doc-accent-bg`: #eef2ff / #1f3047.
pub const ACCENT_BG: AdaptiveColor = tone(hsla(0.6275, 1.0, 0.9667, 1.0), hsla(0.5958, 0.3922, 0.2, 1.0));
/// `--doc-on-primary`: #fff / #1f1f1f.
pub const ON_PRIMARY: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 0.1216, 1.0));
/// `--doc-toggle-active-bg`: hsl(var(--foreground)) / var(--doc-primary-light).
pub const TOGGLE_ACTIVE_BG: AdaptiveColor = tone(hsla(0.6172, 0.84, 0.049, 1.0), hsla(0.5923, 1.0, 0.6529, 0.18));
/// `--doc-toggle-active-fg`: #fff / var(--doc-primary).
pub const TOGGLE_ACTIVE_FG: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 1.0), hsla(0.5923, 1.0, 0.6529, 1.0));
/// `--doc-text`: #202124 / #e8e8e8.
pub const TEXT: AdaptiveColor = tone(hsla(0.625, 0.0588, 0.1333, 1.0), hsla(0.0, 0.0, 0.9098, 1.0));
/// `--doc-text-muted`: #5f6368 / #a0a0a0.
pub const TEXT_MUTED: AdaptiveColor = tone(hsla(0.5926, 0.0452, 0.3902, 1.0), hsla(0.0, 0.0, 0.6275, 1.0));
/// `--doc-text-subtle`: #9aa0a6 / #6e6e6e.
pub const TEXT_SUBTLE: AdaptiveColor = tone(hsla(0.5833, 0.0632, 0.6275, 1.0), hsla(0.0, 0.0, 0.4314, 1.0));
/// `--doc-text-placeholder`: #999 / #6e6e6e.
pub const TEXT_PLACEHOLDER: AdaptiveColor = tone(hsla(0.0, 0.0, 0.6, 1.0), hsla(0.0, 0.0, 0.4314, 1.0));
/// `--doc-border`: #e0e0e0 / #3c3c3c.
pub const BORDER: AdaptiveColor = tone(hsla(0.0, 0.0, 0.8784, 1.0), hsla(0.0, 0.0, 0.2353, 1.0));
/// `--doc-border-light`: #dadce0 / #353535.
pub const BORDER_LIGHT: AdaptiveColor = tone(hsla(0.6111, 0.0882, 0.8667, 1.0), hsla(0.0, 0.0, 0.2078, 1.0));
/// `--doc-border-dark`: #d0d0d0 / #555.
pub const BORDER_DARK: AdaptiveColor = tone(hsla(0.0, 0.0, 0.8157, 1.0), hsla(0.0, 0.0, 0.3333, 1.0));
/// `--doc-border-input`: #ccc / #4a4a4a.
pub const BORDER_INPUT: AdaptiveColor = tone(hsla(0.0, 0.0, 0.8, 1.0), hsla(0.0, 0.0, 0.2902, 1.0));
/// `--doc-error`: #c5221f / #f28b82.
pub const ERROR: AdaptiveColor = tone(hsla(0.003, 0.7281, 0.4471, 1.0), hsla(0.0134, 0.8116, 0.7294, 1.0));
/// `--doc-error-bg`: #fce8e6 / #3c1f1d.
pub const ERROR_BG: AdaptiveColor = tone(hsla(0.0152, 0.7857, 0.9451, 1.0), hsla(0.0108, 0.3483, 0.1745, 1.0));
/// `--doc-success`: #34a853 / #81c995.
pub const SUCCESS: AdaptiveColor = tone(hsla(0.3779, 0.5273, 0.4314, 1.0), hsla(0.3796, 0.4, 0.6471, 1.0));
/// `--doc-success-bg`: #e8f5e9 / #1f2e22.
pub const SUCCESS_BG: AdaptiveColor = tone(hsla(0.3462, 0.3939, 0.9353, 1.0), hsla(0.3667, 0.1948, 0.151, 1.0));
/// `--doc-warning`: #f9a825 / #fdd663.
pub const WARNING: AdaptiveColor = tone(hsla(0.103, 0.9464, 0.5608, 1.0), hsla(0.1245, 0.9747, 0.6902, 1.0));
/// `--doc-warning-bg`: #fff8e1 / #332a1a.
pub const WARNING_BG: AdaptiveColor = tone(hsla(0.1278, 1.0, 0.9412, 1.0), hsla(0.1067, 0.3247, 0.151, 1.0));
/// `--doc-warning-text`: #856404 / #fdd663.
pub const WARNING_TEXT: AdaptiveColor = tone(hsla(0.124, 0.9416, 0.2686, 1.0), hsla(0.1245, 0.9747, 0.6902, 1.0));
/// `--doc-link`: #0563c1 / #6eb9ff.
pub const LINK: AdaptiveColor = tone(hsla(0.5833, 0.9495, 0.3882, 1.0), hsla(0.5805, 1.0, 0.7157, 1.0));
/// `--doc-shadow`: rgba(0,0,0,.15) / rgba(0,0,0,.4).
pub const SHADOW: AdaptiveColor = tone(hsla(0.0, 0.0, 0.0, 0.15), hsla(0.0, 0.0, 0.0, 0.4));
/// `--doc-shadow-strong`: rgba(0,0,0,.3) / rgba(0,0,0,.6).
pub const SHADOW_STRONG: AdaptiveColor = tone(hsla(0.0, 0.0, 0.0, 0.3), hsla(0.0, 0.0, 0.0, 0.6));
/// `--doc-shadow-subtle`: rgba(0,0,0,.06) / rgba(0,0,0,.2).
pub const SHADOW_SUBTLE: AdaptiveColor = tone(hsla(0.0, 0.0, 0.0, 0.06), hsla(0.0, 0.0, 0.0, 0.2));
/// `--doc-overlay`: rgba(0,0,0,.5) / rgba(0,0,0,.7).
pub const OVERLAY: AdaptiveColor = tone(hsla(0.0, 0.0, 0.0, 0.5), hsla(0.0, 0.0, 0.0, 0.7));
/// `--doc-focus-ring`: rgba(26,115,232,.4) / rgba(78,157,255,.45).
pub const FOCUS_RING: AdaptiveColor = tone(hsla(0.5947, 0.8175, 0.5059, 0.4), hsla(0.5923, 1.0, 0.6529, 0.45));
/// `--doc-selection`: rgba(26,115,232,.3) / rgba(78,157,255,.32).
pub const SELECTION: AdaptiveColor = tone(hsla(0.5947, 0.8175, 0.5059, 0.3), hsla(0.5923, 1.0, 0.6529, 0.32));
/// `--doc-page-bg`: #fff / #ccc.
pub const PAGE_BG: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 0.8, 1.0));
/// `--doc-caret`: #000 / #e8e8e8.
pub const CARET: AdaptiveColor = tone(hsla(0.0, 0.0, 0.0, 1.0), hsla(0.0, 0.0, 0.9098, 1.0));

// The Tailwind theme (`--background`, `--muted`, …): HSL triples the
// utility classes wrap in `hsl(var(..))`.
/// `--background` (Tailwind `hsl(var(..))`): `0 0% 100%` / `0 0% 12%`.
pub const TW_BACKGROUND: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 0.12, 1.0));
/// `--foreground` (Tailwind `hsl(var(..))`): `222.2 84% 4.9%` / `0 0% 91%`.
pub const TW_FOREGROUND: AdaptiveColor = tone(hsla(0.6172, 0.84, 0.049, 1.0), hsla(0.0, 0.0, 0.91, 1.0));
/// `--popover` (Tailwind `hsl(var(..))`): `0 0% 100%` / `0 0% 18%`.
pub const TW_POPOVER: AdaptiveColor = tone(hsla(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 0.18, 1.0));
/// `--popover-foreground` (Tailwind `hsl(var(..))`): `222.2 84% 4.9%` / `0 0% 91%`.
pub const TW_POPOVER_FOREGROUND: AdaptiveColor = tone(hsla(0.6172, 0.84, 0.049, 1.0), hsla(0.0, 0.0, 0.91, 1.0));
/// `--muted` (Tailwind `hsl(var(..))`): `210 40% 96.1%` / `0 0% 20%`.
pub const TW_MUTED: AdaptiveColor = tone(hsla(0.5833, 0.4, 0.961, 1.0), hsla(0.0, 0.0, 0.2, 1.0));
/// `--muted-foreground` (Tailwind `hsl(var(..))`): `215.4 16.3% 46.9%` / `0 0% 63%`.
pub const TW_MUTED_FOREGROUND: AdaptiveColor = tone(hsla(0.5983, 0.163, 0.469, 1.0), hsla(0.0, 0.0, 0.63, 1.0));
/// `--accent` (Tailwind `hsl(var(..))`): `210 40% 96.1%` / `0 0% 23%`.
pub const TW_ACCENT: AdaptiveColor = tone(hsla(0.5833, 0.4, 0.961, 1.0), hsla(0.0, 0.0, 0.23, 1.0));
/// `--accent-foreground` (Tailwind `hsl(var(..))`): `222.2 47.4% 11.2%` / `212 100% 65%`.
pub const TW_ACCENT_FOREGROUND: AdaptiveColor = tone(hsla(0.6172, 0.474, 0.112, 1.0), hsla(0.5889, 1.0, 0.65, 1.0));
/// `--border` (Tailwind `hsl(var(..))`): `214.3 31.8% 91.4%` / `0 0% 24%`.
pub const TW_BORDER: AdaptiveColor = tone(hsla(0.5953, 0.318, 0.914, 1.0), hsla(0.0, 0.0, 0.24, 1.0));
/// `--input` (Tailwind `hsl(var(..))`): `214.3 31.8% 91.4%` / `0 0% 24%`.
pub const TW_INPUT: AdaptiveColor = tone(hsla(0.5953, 0.318, 0.914, 1.0), hsla(0.0, 0.0, 0.24, 1.0));
/// `--ring` (Tailwind `hsl(var(..))`): `221.2 83.2% 53.3%` / `212 100% 60%`.
pub const TW_RING: AdaptiveColor = tone(hsla(0.6144, 0.832, 0.533, 1.0), hsla(0.5889, 1.0, 0.6, 1.0));
/// `--primary` (Tailwind `hsl(var(..))`): `221.2 83.2% 53.3%` / `212 100% 65%`.
pub const TW_PRIMARY: AdaptiveColor = tone(hsla(0.6144, 0.832, 0.533, 1.0), hsla(0.5889, 1.0, 0.65, 1.0));
/// `--primary-foreground` (Tailwind `hsl(var(..))`): `210 40% 98%` / `0 0% 12%`.
pub const TW_PRIMARY_FOREGROUND: AdaptiveColor = tone(hsla(0.5833, 0.4, 0.98, 1.0), hsla(0.0, 0.0, 0.12, 1.0));
/// `--secondary` (Tailwind `hsl(var(..))`): `210 40% 96.1%` / `0 0% 20%`.
pub const TW_SECONDARY: AdaptiveColor = tone(hsla(0.5833, 0.4, 0.961, 1.0), hsla(0.0, 0.0, 0.2, 1.0));
/// `--secondary-foreground` (Tailwind `hsl(var(..))`): `222.2 47.4% 11.2%` / `0 0% 91%`.
pub const TW_SECONDARY_FOREGROUND: AdaptiveColor = tone(hsla(0.6172, 0.474, 0.112, 1.0), hsla(0.0, 0.0, 0.91, 1.0));
/// `--destructive` (Tailwind `hsl(var(..))`): `0 84.2% 60.2%` / `0 72% 60%`.
pub const TW_DESTRUCTIVE: AdaptiveColor = tone(hsla(0.0, 0.842, 0.602, 1.0), hsla(0.0, 0.72, 0.6, 1.0));
/// `--destructive-foreground` (Tailwind `hsl(var(..))`): `210 40% 98%` / `0 0% 98%`.
pub const TW_DESTRUCTIVE_FOREGROUND: AdaptiveColor = tone(hsla(0.5833, 0.4, 0.98, 1.0), hsla(0.0, 0.0, 0.98, 1.0));

/// `hover:bg-muted/80`, `focus:bg-muted/80` (the select trigger's and the
/// font size steppers' hover).
pub const TW_MUTED_80: AdaptiveColor = tone(hsla(0.5833, 0.4, 0.961, 0.8), hsla(0.0, 0.0, 0.2, 0.8));
/// `border-border/50`: a toolbar group's right edge.
pub const TW_BORDER_50: AdaptiveColor = tone(hsla(0.5953, 0.318, 0.914, 0.5), hsla(0.0, 0.0, 0.24, 0.5));
/// The outline a near-white swatch bar gets (`1px solid #bbb`, the colour
/// button's `ly`).
pub const SWATCH_RING: AdaptiveColor = tone(hsla(0.0, 0.0, 0.7333, 1.0), hsla(0.0, 0.0, 0.7333, 1.0));

/// The colors a canvas paints with (rulers, the table grid, page chrome),
/// resolved for one appearance: a canvas takes `Hsla`, not
/// `AdaptiveColor`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub surface: Hsla,
    pub bg: Hsla,
    pub hover: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_subtle: Hsla,
    pub border: Hsla,
    pub border_light: Hsla,
    pub primary: Hsla,
    pub primary_light: Hsla,
    pub page_bg: Hsla,
    pub selection: Hsla,
    pub caret: Hsla,
    pub shadow: Hsla,
}

impl Palette {
    pub fn of(dark: bool) -> Self {
        let pick = |c: AdaptiveColor| if dark { c.dark } else { c.light };
        Palette {
            dark,
            surface: pick(SURFACE),
            bg: pick(BG),
            hover: pick(BG_HOVER),
            text: pick(TEXT),
            text_muted: pick(TEXT_MUTED),
            text_subtle: pick(TEXT_SUBTLE),
            border: pick(BORDER),
            border_light: pick(BORDER_LIGHT),
            primary: pick(PRIMARY),
            primary_light: pick(PRIMARY_LIGHT),
            page_bg: pick(PAGE_BG),
            selection: pick(SELECTION),
            caret: pick(CARET),
            shadow: pick(SHADOW),
        }
    }

    /// The colors of the appearance shown now (reading it subscribes).
    pub fn current() -> Self {
        Palette::of(dark())
    }
}
