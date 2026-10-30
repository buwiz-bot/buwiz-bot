// Port of src/text/icons.ts. The host gives no icon resolver (Rox has none, as the Node helper
// passed none), so no icon has markup and a label leaves no room for one.

pub fn icon_markup(_name: &str) -> &'static str {
    ""
}
