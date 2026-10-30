//! A Rust port of Pele (<https://github.com/obsidianmd/pele>, MIT, © kepano), a Mermaid
//! renderer. Modules mirror upstream's `src/` file for file; each names the file it ports.
//! `render` returns upstream's SVG byte for byte (see tests/golden) and the same drawing as a
//! `Scene` for a native host. Types not ported yet fail with `PeleErrorCode::NotYetPorted`.

pub mod core;
pub mod detect;
pub mod diagrams;
pub mod errors;
pub mod js;
pub mod layout;
pub mod preprocess;
pub mod scene;
pub mod svg;
pub mod text;
pub mod types;
pub mod util;

pub use crate::core::{detect_type, render, PORTED};
pub use errors::{PeleError, PeleErrorCode};
pub use scene::{Element, Node, Paint, Scene};
pub use types::{LinkInfo, RenderResult};
