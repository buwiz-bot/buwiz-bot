// Port of src/text/measurer.ts: the metrics measurer, which upstream uses wherever there is no
// canvas, and so in the Node helper this port replaces. Widths come from a table of advances; a
// host never supplies its own measurer, as one would change the geometry.

use super::metrics::{BOLD, REGULAR};
use std::sync::OnceLock;

pub mod style {
    pub const BOLD: u32 = 1;
    pub const ITALIC: u32 = 2;
    pub const MONO: u32 = 4;
    pub const SUB: u32 = 8;
    pub const SUP: u32 = 16;
    pub const UNDERLINE: u32 = 32;
}

// Approximate divisor for CSS font-size="smaller".
const SMALLER: f64 = 1.2;

fn unpack(packed: &str) -> [u16; 95] {
    let mut out = [0u16; 95];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u16::from_str_radix(&packed[i * 2..i * 2 + 2], 36).unwrap();
    }
    out
}

fn regular() -> &'static [u16; 95] {
    static T: OnceLock<[u16; 95]> = OnceLock::new();
    T.get_or_init(|| unpack(REGULAR))
}

fn bold() -> &'static [u16; 95] {
    static T: OnceLock<[u16; 95]> = OnceLock::new();
    T.get_or_init(|| unpack(BOLD))
}

fn is_wide(c: u32) -> bool {
    (0x1100..=0x115f).contains(&c)
        || (0x2e80..=0xa4cf).contains(&c)
        || (0xac00..=0xd7a3).contains(&c)
        || (0xf900..=0xfaff).contains(&c)
        || (0xfe30..=0xfe4f).contains(&c)
        || (0xff00..=0xff60).contains(&c)
        || (0xffe0..=0xffe6).contains(&c)
        || (0xd800..=0xdbff).contains(&c)
}

fn is_zero_width(c: u32) -> bool {
    c == 0xad || (0x300..=0x36f).contains(&c) || (0x200b..=0x200f).contains(&c) || (0xdc00..=0xdfff).contains(&c) || (0xfe00..=0xfe0f).contains(&c) || c == 0xfeff
}

/// The width of `text` at `size` pixels in `style` (a set of `style` bits).
pub fn width(text: &str, size: f64, style: u32) -> f64 {
    let mono = style & style::MONO != 0;
    let eff = if style & (style::SUB | style::SUP) != 0 { size / SMALLER } else { size };
    let table = if style & style::BOLD != 0 { bold() } else { regular() };
    let mut units: u32 = 0;
    for c in text.encode_utf16() {
        let c = c as u32;
        if c < 32 {
            continue;
        }
        if c < 127 {
            units += if mono { 600 } else { table[(c - 32) as usize] as u32 };
        } else if is_zero_width(c) {
            continue;
        } else if is_wide(c) {
            units += if mono { 1200 } else { 1000 };
        } else {
            units += if mono { 600 } else { 580 };
        }
    }
    (units as f64 * eff) / 1000.0
}
