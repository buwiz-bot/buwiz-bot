//! Hidden characters (spec 4.2): the one definition a plugin's text is judged by
//! wherever the guide says "hidden character" (a badge's text, the card the AI's
//! press shows). A character that changes how a text reads, or whether it is one
//! line, without showing: explicit ranges, so no Unicode table decides it and the
//! Rust and JavaScript sides cannot differ.

/// Whether `c` is hidden: the C0 and C1 controls and U+007F, U+00AD, U+061C, U+180E,
/// U+200B to U+200F, U+2028 to U+202E (the line and paragraph separators and the bidi
/// overrides), U+2060 to U+206F, U+FE00 to U+FE0F, U+FEFF, U+FFF9 to U+FFFB and
/// U+E0000 to U+E0FFF.
pub fn hidden(c: char) -> bool {
    matches!(c as u32,
        0x00..=0x1F | 0x7F..=0x9F | 0xAD | 0x61C | 0x180E | 0x200B..=0x200F | 0x2028..=0x202E
        | 0x2060..=0x206F | 0xFE00..=0xFE0F | 0xFEFF | 0xFFF9..=0xFFFB | 0xE0000..=0xE0FFF)
}

#[cfg(test)]
mod tests {
    use super::hidden;

    #[test]
    fn the_set_is_the_guides() {
        for c in ['\u{0}', '\n', '\t', '\u{7f}', '\u{85}', '\u{9f}', '\u{ad}', '\u{61c}', '\u{180e}', '\u{200b}', '\u{200f}', '\u{2028}', '\u{2029}', '\u{202e}', '\u{2060}', '\u{2066}', '\u{206f}', '\u{fe0f}', '\u{feff}', '\u{fff9}', '\u{fffb}', '\u{e0041}', '\u{e0fff}'] {
            assert!(hidden(c), "{c:?}");
        }
        for c in [' ', 'a', '9', '\u{a0}', '\u{a1}', '\u{2010}', '\u{2027}', '\u{202f}', '\u{205f}', '\u{fdff}', '\u{fffc}', '\u{e1000}', 'Å'] {
            assert!(!hidden(c), "{c:?}");
        }
    }
}
