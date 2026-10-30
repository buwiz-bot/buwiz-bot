// Port of src/diagrams/flowchart/index.ts.

pub mod db;
pub mod graph;
pub mod lexer;
pub mod parser;
pub mod render;
pub mod shapes;
pub mod tokens;
pub mod types;

use crate::errors::PeleError;
use crate::preprocess::Config;
use crate::types::Rendered;
use crate::util::yaml::Value;
use db::{FlowDb, FlowDbOptions};

pub fn parse(source: &[u16], config: &Config, title: Option<&str>) -> Result<FlowDb, PeleError> {
    let inherit = matches!(config.get("flowchart").and_then(|f| f.as_map()).and_then(|f| f.get("inheritDir")), Some(Value::Bool(true)));
    let mut db = FlowDb::new(FlowDbOptions { inherit_dir: inherit, max_edges: Some(crate::core::DEFAULT_MAX_EDGES) });
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        db.title = Some(t.to_string());
    }
    parser::parse_flowchart(source, &mut db)?;
    Ok(db)
}

pub fn render(db: &FlowDb, config: &Config) -> Rendered {
    render::render_flowchart(db, config)
}
