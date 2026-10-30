// Port of src/types.ts (the parts the port uses).

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LinkInfo {
    pub id: String,
    pub href: String,
    pub internal: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rendered {
    pub svg: String,
    pub width: f64,
    pub height: f64,
    pub links: Vec<LinkInfo>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderResult {
    pub type_: &'static str,
    pub mermaid_type: &'static str,
    pub svg: String,
    pub width: f64,
    pub height: f64,
    pub links: Vec<LinkInfo>,
}
