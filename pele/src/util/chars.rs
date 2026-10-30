// Port of src/util/chars.ts. Tests on UTF-16 code units, as the lexers read them; past the end
// of the text the lexers pass u32::MAX, which is none of these (upstream's NaN).

pub fn is_space(c: u32) -> bool {
    crate::js::is_space_u(c)
}

pub fn is_word(c: u32) -> bool {
    (48..=57).contains(&c) || (65..=90).contains(&c) || (97..=122).contains(&c) || c == 95
}
