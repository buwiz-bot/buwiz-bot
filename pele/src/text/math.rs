// Port of src/text/math.ts. The host gives no math resolver, so `$$…$$` stays text.

pub fn math_active() -> bool {
    false
}
