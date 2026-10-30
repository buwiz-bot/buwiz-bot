// Port of src/diagrams/flowchart/shapes.ts: Mermaid's shape names and their canonical forms.

// Mermaid's documented shape names: the short name first, then its aliases.
const SHAPES: &[&[&str]] = &[
    &["rect", "proc", "process", "rectangle", "squareRect"],
    &["rounded", "event", "roundedRect"],
    &["stadium", "terminal", "pill"],
    &["fr-rect", "subprocess", "subproc", "framed-rectangle", "subroutine"],
    &["cyl", "db", "database", "cylinder"],
    &["datastore", "data-store"],
    &["folder", "directory"],
    &["bucket"],
    &["console"],
    &["browser"],
    &["person"],
    &["circle", "circ"],
    &["bang"],
    &["cloud"],
    &["diam", "decision", "diamond", "question"],
    &["hex", "hexagon", "prepare"],
    &["lean-r", "lean-right", "in-out", "lean_right"],
    &["lean-l", "lean-left", "out-in", "lean_left"],
    &["trap-b", "priority", "trapezoid-bottom", "trapezoid"],
    &["trap-t", "manual", "trapezoid-top", "inv-trapezoid", "inv_trapezoid"],
    &["dbl-circ", "double-circle", "doublecircle"],
    &["text"],
    &["notch-rect", "card", "notched-rectangle"],
    &["lin-rect", "lined-rectangle", "lined-process", "lin-proc", "shaded-process"],
    &["sm-circ", "start", "small-circle", "stateStart"],
    &["fr-circ", "stop", "framed-circle", "stateEnd"],
    &["fork", "join", "forkJoin"],
    &["hourglass", "collate"],
    &["brace", "comment", "brace-l"],
    &["brace-r"],
    &["braces"],
    &["bolt", "com-link", "lightning-bolt"],
    &["doc", "document"],
    &["delay", "half-rounded-rectangle"],
    &["h-cyl", "das", "horizontal-cylinder"],
    &["lin-cyl", "disk", "lined-cylinder"],
    &["curv-trap", "curved-trapezoid", "display"],
    &["div-rect", "div-proc", "divided-rectangle", "divided-process"],
    &["tri", "extract", "triangle"],
    &["win-pane", "internal-storage", "window-pane"],
    &["f-circ", "junction", "filled-circle"],
    &["notch-pent", "loop-limit", "notched-pentagon"],
    &["flip-tri", "manual-file", "flipped-triangle"],
    &["sl-rect", "manual-input", "sloped-rectangle"],
    &["docs", "documents", "st-doc", "stacked-document"],
    &["st-rect", "procs", "processes", "stacked-rectangle"],
    &["bow-rect", "stored-data", "bow-tie-rectangle"],
    &["cross-circ", "summary", "crossed-circle"],
    &["tag-doc", "tagged-document"],
    &["tag-rect", "tagged-rectangle", "tag-proc", "tagged-process"],
    &["flag", "paper-tape"],
    &["odd", "rect_left_inv_arrow"],
    &["lin-doc", "lined-document"],
];

// Shapes Mermaid accepts by name without documenting them.
const INTERNAL: &[&str] = &[
    "state",
    "choice",
    "note",
    "composite",
    "rectWithTitle",
    "labelRect",
    "block_arrow",
    "collapsedGroup",
    "iconSquare",
    "iconCircle",
    "icon",
    "iconRounded",
    "imageSquare",
    "anchor",
    "kanbanItem",
    "mindmapCircle",
    "defaultMindmapNode",
    "classBox",
    "erBox",
    "requirementBox",
    "usecaseActor",
    "usecaseActorHollow",
    "usecaseActorAwesome",
    "usecaseActorIcon",
    "usecaseBusiness",
    "usecaseEllipse",
    "usecaseJsonTable",
];

pub fn is_valid_shape(name: &str) -> bool {
    SHAPES.iter().any(|names| names.contains(&name)) || INTERNAL.contains(&name)
}

pub fn canonical_shape(name: Option<&str>) -> String {
    let Some(name) = name else { return "rect".into() };
    // Shapes produced by the bracket syntax, mapped to their documented names.
    match name {
        "square" => return "rect".into(),
        "round" => return "rounded".into(),
        "ellipse" => return "ellipse".into(),
        "hexagon" => return "hex".into(),
        "diamond" => return "diam".into(),
        _ => {}
    }
    // A later group's alias overrides an earlier one, as upstream's Map does.
    let mut found: Option<&str> = None;
    for names in SHAPES {
        if names.contains(&name) {
            found = Some(names[0]);
        }
    }
    if INTERNAL.contains(&name) {
        found = Some(name);
    }
    found.unwrap_or("rect").to_string()
}
