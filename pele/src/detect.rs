// Port of src/detect.ts: Mermaid's detectors, in the order Mermaid registers them. The first match wins.

use crate::js;

pub struct Detected {
    pub type_: &'static str,
    pub mermaid_type: &'static str,
}

fn word_after(s: &str, len: usize) -> bool {
    s[len..].chars().next().is_some_and(js::is_word)
}

fn starts_ci(s: &str, word: &str) -> bool {
    s.len() >= word.len() && s.is_char_boundary(word.len()) && s[..word.len()].eq_ignore_ascii_case(word)
}

// (type, mermaid type, test on the text after its leading whitespace)
type Rule = (&'static str, &'static str, fn(&str) -> bool);

const DETECTORS: [Rule; 37] = [
    ("flowchart", "flowchart-elk", |s| s.starts_with("flowchart-elk")),
    ("mindmap", "mindmap", |s| s.starts_with("mindmap")),
    ("architecture", "architecture", |s| s.starts_with("architecture")),
    ("agentflow", "agentflow", |s| s.starts_with("agentflow-beta") && !word_after(s, 14)),
    ("c4", "c4", |s| ["C4Context", "C4Container", "C4Component", "C4Dynamic", "C4Deployment"].iter().any(|w| s.starts_with(w))),
    ("kanban", "kanban", |s| s.starts_with("kanban")),
    ("class", "classDiagram", |s| s.starts_with("classDiagram")),
    ("er", "er", |s| s.starts_with("erDiagram")),
    ("gantt", "gantt", |s| s.starts_with("gantt")),
    ("info", "info", |s| s.starts_with("info")),
    ("pie", "pie", |s| s.starts_with("pie")),
    ("requirement", "requirement", |s| s.starts_with("requirement")),
    ("sequence", "sequence", |s| s.starts_with("sequenceDiagram")),
    ("swimlane", "swimlane", |s| s.starts_with("swimlane-beta") && !word_after(s, 13)),
    ("flowchart", "flowchart-v2", |s| s.starts_with("graph") || s.starts_with("flowchart")),
    ("timeline", "timeline", |s| s.starts_with("timeline")),
    ("gitGraph", "gitGraph", |s| s.starts_with("gitGraph")),
    ("state", "stateDiagram", |s| s.starts_with("stateDiagram")),
    ("journey", "journey", |s| s.starts_with("journey")),
    ("quadrantChart", "quadrantChart", |s| s.starts_with("quadrantChart")),
    ("sankey", "sankey", |s| s.starts_with("sankey")),
    ("packet", "packet", |s| s.starts_with("packet")),
    ("xychart", "xychart", |s| s.starts_with("xychart")),
    ("block", "block", |s| s.starts_with("block")),
    ("eventmodeling", "eventmodeling", |s| s.starts_with("eventmodeling")),
    ("treeView", "treeView", |s| s.starts_with("treeView-beta")),
    ("radar", "radar", |s| s.starts_with("radar-beta")),
    ("ishikawa", "ishikawa", |s| starts_ci(s, "ishikawa") && !word_after(s, 8)),
    ("treemap", "treemap", |s| s.starts_with("treemap")),
    ("railroad", "railroad", |s| starts_ci(s, "railroad-beta")),
    ("railroad", "railroadEbnf", |s| starts_ci(s, "railroad-ebnf-beta")),
    ("railroad", "railroadAbnf", |s| starts_ci(s, "railroad-abnf-beta")),
    ("railroad", "railroadPeg", |s| starts_ci(s, "railroad-peg-beta")),
    ("venn", "venn", |s| s.starts_with("venn-beta")),
    ("wardley", "wardley", |s| starts_ci(s, "wardley-beta")),
    ("cynefin", "cynefin", |s| s.starts_with("cynefin-beta") && s[12..].chars().next().is_none_or(|c| js::is_space(c) || c == ':')),
    ("usecase", "usecase", |s| s.starts_with("usecase-beta") && s[12..].chars().next().is_none_or(js::is_space)),
];

pub fn detect_diagram(cleaned: &str) -> Option<Detected> {
    let s = js::trim_start(cleaned);
    DETECTORS.iter().find(|(_, _, test)| test(s)).map(|&(type_, mermaid_type, _)| Detected { type_, mermaid_type })
}
