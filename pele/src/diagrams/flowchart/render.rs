// Port of src/diagrams/flowchart/render.ts, for the default render options the host uses: no
// width limit (so neither tightening nor turning applies), 16px text, no icon resolver, and no
// FlowVariant (the diagram types that reuse this renderer come in a later phase).

use super::db::FlowDb;
use super::graph::{build_flow_graph, GraphEdge, GraphNode};
use super::shapes::canonical_shape;
use super::types::LabelType;
use crate::js;
use crate::layout::compound::{cnode, compound_layout, direction, inherit_crossed_directions, shift_layout, CEdge, CNode, Dir};
use crate::layout::layered::LayeredOptions;
use crate::preprocess::Config;
use crate::svg::builder::{esc, esc_text, label_svg, num};
use crate::svg::cluster::{cluster_title_x, mark_crossings, struck_title, title_crossed, Crossings};
use crate::svg::edges::{casings_svg, edge_casing, edge_label_svg, loop_path, marker, marker_trim, over_fills, route_path, EdgePath, Fill, LoopLabel, SideReach};
use crate::svg::ink::{ink_fill, paints};
use crate::svg::root::{svg_document, Accessible};
use crate::svg::shapes::{draw_shape, end_side, inset_route, shape_filled, shape_has_label, shape_size, shape_span};
use crate::svg::theme::{class_names, resolve_style, rounded, sets_fill, ResolvedStyle};
use crate::svg::title::title_label;
use crate::text::entities::decode_entities;
use crate::text::label::{layout_label, Label};
use crate::text::measurer::style;
use crate::types::{LinkInfo, Rendered};
use crate::util::url::{image_url, internal_link, linked, rel_attr, safe_url};
use crate::util::yaml::Value;
use std::collections::HashMap;

const NODE_SEP: f64 = 40.0;
const EDGE_SEP: f64 = 16.0;
const RANK_SEP: f64 = 48.0;
const GROUP_PAD: f64 = 20.0;
const LOOP: f64 = 26.0;
const SHAPE_ATTRS: &str = " fill=\"var(--_s)\" stroke=\"var(--_b)\"";
const LINE_ATTRS: &str = " stroke=\"var(--_b)\"";

fn number_option(section: Option<&crate::util::yaml::Map>, key: &str, fallback: f64) -> f64 {
    match section.and_then(|s| s.get(key)) {
        Some(Value::Num(v)) if *v > 0.0 => v.min(2000.0),
        _ => fallback,
    }
}

fn asset_size(value: Option<f64>, fallback: f64) -> f64 {
    match value {
        Some(v) if v > 0.0 && v <= 4000.0 => v,
        _ => fallback,
    }
}

struct NodeView {
    style: ResolvedStyle,
    label: Label,
    shape: String,
    w: f64,
    h: f64,
    dy: f64,
}

pub fn render_flowchart(db: &FlowDb, config: &Config) -> Rendered {
    draw(db, config, false, false)
}

fn sideways(cnodes: &[CNode], root_dir: Dir, i: usize) -> bool {
    let mut dir: Option<Dir> = None;
    let mut p = cnodes[i].parent;
    let mut hops = 0;
    while p >= 0 && dir.is_none() && hops < 10000 {
        dir = cnodes[p as usize].dir;
        p = cnodes[p as usize].parent;
        hops += 1;
    }
    let dir = dir.unwrap_or(root_dir);
    dir == Dir::LR || dir == Dir::RL
}

// `turned` draws a flowchart that runs across as one that runs down. `tight` sets nodes closer together.
fn draw(db: &FlowDb, config: &Config, turned: bool, tight: bool) -> Rendered {
    let group_pad = if tight { 12.0 } else { GROUP_PAD };
    let flow = config.get("flowchart").and_then(|v| v.as_map());
    let curve_option = flow.and_then(|f| f.get("curve")).and_then(|v| v.as_str());
    let graph = build_flow_graph(db, curve_option);
    let size = 16.0;
    let edge_size = js::round(size * 0.875);
    let wrap_width = number_option(flow, "wrappingWidth", 200.0);
    let pad = 8.0;

    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut views: Vec<NodeView> = Vec::new();
    let mut cnodes: Vec<CNode> = Vec::new();
    for (i, node) in graph.nodes.iter().enumerate() {
        index.insert(&node.id, i);
    }

    for node in &graph.nodes {
        let decls: Vec<String> = if !node.css_styles.is_empty() { node.css_compiled_styles.iter().chain(node.css_styles.iter()).cloned().collect() } else { node.css_compiled_styles.clone() };
        let st = resolve_style(&decls);
        let text_style = (if st.bold { style::BOLD } else { 0 }) | (if st.italic { style::ITALIC } else { 0 });
        let markdown = node.label_type == LabelType::Markdown;
        let font_size = st.font_size.unwrap_or(size);
        let mut c;
        let view;
        if node.is_group {
            let label = layout_label(node.label.as_deref(), markdown, font_size, 4000.0, text_style);
            c = cnode(0.0, 0.0);
            c.is_group = true;
            c.dir = node.dir.as_deref().map(|d| direction(Some(d)));
            c.pad_x = group_pad;
            c.pad_top = if label.height > 0.0 { label.height + 16.0 } else { GROUP_PAD };
            c.pad_bottom = group_pad;
            c.min_w = label.width + 2.0 * group_pad - 12.0;
            view = NodeView { style: st, label, shape: "group".into(), w: 0.0, h: 0.0, dy: 0.0 };
        } else {
            let shape = canonical_shape(Some(&node.shape));
            let has_asset = node.img.is_some() || node.icon.is_some();
            let text = if has_asset || shape_has_label(&shape) { node.label.as_deref() } else { None };
            let label = layout_label(text, markdown, font_size, wrap_width, text_style);
            let (w, h, mut dy);
            if has_asset {
                let bx = if node.img.is_some() { 80.0 } else { 48.0 };
                let aw = asset_size(node.asset_width, bx);
                let ah = asset_size(node.asset_height, bx);
                w = aw.max(label.width + 8.0);
                h = ah + if label.height > 0.0 { label.height + 6.0 } else { 0.0 };
                dy = if node.pos.as_deref() == Some("t") { -ah / 2.0 } else { ah / 2.0 };
                if label.height == 0.0 {
                    dy = 0.0;
                }
            } else {
                let s = shape_size(&shape, label.width, label.height);
                w = s.w;
                h = s.h;
                dy = s.dy;
            }
            c = cnode(w, h);
            view = NodeView { style: st, label, shape, w, h, dy };
        }
        c.seq = views.len() as f64;
        views.push(view);
        cnodes.push(c);
    }
    for (i, node) in graph.nodes.iter().enumerate() {
        cnodes[i].parent = match &node.parent_id {
            Some(p) => index.get(p.as_str()).map_or(-1, |&x| x as i64),
            None => -1,
        };
    }
    let ends: Vec<(i64, i64)> = graph.edges.iter().map(|e| (index.get(e.start.as_str()).map_or(-1, |&x| x as i64), index.get(e.end.as_str()).map_or(-1, |&x| x as i64))).collect();
    inherit_crossed_directions(&mut cnodes, &ends);

    let root_dir = if turned { Dir::TB } else { direction(db.direction.as_deref()) };
    // The edges on one side of a node spread along it; a picture or an icon keeps them at its middle.
    for (i, (view, node)) in views.iter().zip(&graph.nodes).enumerate() {
        if node.is_group {
            cnodes[i].span = f64::INFINITY;
        } else if node.img.is_none() && node.icon.is_none() {
            let side = sideways(&cnodes, root_dir, i);
            cnodes[i].span = shape_span(&view.shape, view.w, view.h, side);
        }
    }

    let mut cedges: Vec<CEdge> = Vec::new();
    let mut edge_labels: Vec<Label> = Vec::new();
    let mut drawn: Vec<&GraphEdge> = Vec::new();
    for edge in &graph.edges {
        let (Some(&src), Some(&dst)) = (index.get(edge.start.as_str()), index.get(edge.end.as_str())) else { continue };
        let label = layout_label(Some(&edge.label), edge.label_type == LabelType::Markdown, edge_size, wrap_width, 0);
        if src == dst {
            // A self-loop sits beside the node, across the flow, and the node's box grows to hold it.
            if sideways(&cnodes, root_dir, src) {
                cnodes[src].h += 2.0 * (LOOP + if label.height > 0.0 { label.height + 8.0 } else { 0.0 });
            } else {
                cnodes[src].w += 2.0 * (LOOP + if label.width > 0.0 { label.width + 12.0 } else { 0.0 });
            }
        }
        cedges.push(CEdge {
            src,
            dst,
            minlen: edge.minlen.unwrap_or(1.0),
            label_w: if label.width > 0.0 { label.width + 12.0 } else { 0.0 },
            label_h: if label.height > 0.0 { label.height + 4.0 } else { 0.0 },
            route: Vec::new(),
            label_x: 0.0,
            label_y: 0.0,
        });
        drawn.push(edge);
        edge_labels.push(label);
    }

    let curve_str = flow.and_then(|f| f.get("curve"));
    let layout_options = LayeredOptions {
        node_sep: number_option(flow, "nodeSpacing", NODE_SEP) * if tight { 0.5 } else { 1.0 },
        edge_sep: EDGE_SEP,
        rank_sep: number_option(flow, "rankSpacing", RANK_SEP) * if tight { 0.75 } else { 1.0 },
        port_sep: 20.0,
        tracks: matches!(curve_str, Some(Value::Str(s)) if s.starts_with("step")),
        straight: matches!(curve_str, Some(Value::Str(s)) if s == "linear"),
        apart: false,
        head_room: marker_trim("arrow_point"),
        groups: None,
        order_only: false,
    };
    let layout = compound_layout(&mut cnodes, &mut cedges, root_dir, &layout_options);

    let title = title_label(db.title.as_deref(), size, 4000.0);
    let title_height = if title.height > 0.0 { title.height + 12.0 } else { 0.0 };
    let inner = layout.width.max(title.width);
    let ox = pad + (inner - layout.width) / 2.0;
    let oy = pad + title_height;
    shift_layout(&mut cnodes, &mut cedges, ox, oy);
    let width = (inner + 2.0 * pad).ceil();
    let height = (layout.height + title_height + 2.0 * pad).ceil();
    let mut links: Vec<LinkInfo> = Vec::new();

    let mut crossings: Crossings = HashMap::new();
    if cnodes.iter().any(|c| c.is_group) {
        for e in &cedges {
            mark_crossings(&mut crossings, &e.route);
        }
    }

    let mut clusters = String::new();
    let mut struck_titles = String::new();
    let mut nodes_out = String::new();
    let mut fills: Vec<Fill> = Vec::new();
    for i in 0..views.len() {
        let view = &views[i];
        let c = &cnodes[i];
        let node: &GraphNode = &graph.nodes[i];
        let (x, y) = (c.x, c.y);
        let classes = class_names(node.css_classes.strip_prefix("default ").or_else(|| node.css_classes.strip_prefix("default")).unwrap_or(&node.css_classes));
        let id = esc_text(&node.id);
        if c.is_group {
            if paints(&view.style.shape) {
                fills.push(Fill { bx: [x - c.w / 2.0, y - c.h / 2.0, x + c.w / 2.0, y + c.h / 2.0], shape: view.style.shape.clone() });
            }
            let title_x = cluster_title_x(c, view.label.width, &crossings, group_pad);
            let title_y = y - c.h / 2.0 + 8.0 + view.label.height / 2.0;
            let struck = title_crossed(c, view.label.width, title_x, &crossings);
            // A title drawn over the edges sits on a halo in the theme's colors.
            let title_attrs = format!(
                " class=\"pele-cluster-label\"{}{}",
                if struck { " fill=\"var(--_m)\"".to_string() } else { ink_fill(&view.style.shape, &view.style.text, Some("var(--_m)")) },
                view.style.text
            );
            if struck {
                struck_titles.push_str(&struck_title(&view.label, title_x, title_y, &title_attrs, &id, &cnodes, i));
            }
            clusters.push_str(&format!(
                "<g class=\"pele-cluster{}\" data-id=\"{}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"var(--_a)\"{} stroke=\"var(--_b)\"{}/>{}</g>",
                classes,
                id,
                num(x - c.w / 2.0),
                num(y - c.h / 2.0),
                num(c.w),
                num(c.h),
                if sets_fill(&view.style.shape) { "" } else { " fill-opacity=\"0.5\"" },
                rounded(&view.style.shape),
                if struck { String::new() } else { label_svg(&view.label, title_x, title_y, &title_attrs) }
            ));
            continue;
        }

        let (w, h) = (view.w, view.h);
        let inner_svg;
        let mut ink = String::new();
        if let Some(img) = &node.img {
            let aw = asset_size(node.asset_width, 80.0);
            let ah = asset_size(node.asset_height, 80.0);
            let top = if view.label.height > 0.0 {
                if node.pos.as_deref() == Some("t") {
                    h / 2.0 - ah
                } else {
                    -h / 2.0
                }
            } else {
                -ah / 2.0
            };
            inner_svg = match image_url(&safe_url(img)) {
                None => String::new(),
                Some(src) => format!(
                    "<image href=\"{}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"{}\"/>",
                    esc(&src),
                    num(-aw / 2.0),
                    num(top),
                    num(aw),
                    num(ah),
                    if node.constraint.as_deref() == Some("on") { "xMidYMid meet" } else { "none" }
                ),
            };
        } else if let Some(icon) = &node.icon {
            let bx = asset_size(node.asset_height, 48.0);
            let top = if view.label.height > 0.0 {
                if node.pos.as_deref() == Some("t") {
                    h / 2.0 - bx
                } else {
                    -h / 2.0
                }
            } else {
                -bx / 2.0
            };
            let frame = match node.shape.as_str() {
                "icon" => String::new(),
                "iconCircle" => format!("<circle cy=\"{}\" r=\"{}\"{}{}/>", num(top + bx / 2.0), num(bx / 2.0), SHAPE_ATTRS, view.style.shape),
                _ => format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\"{}{}/>",
                    num(-bx / 2.0),
                    num(top),
                    num(bx),
                    num(bx),
                    if node.shape == "iconRounded" { 8 } else { 0 },
                    SHAPE_ATTRS,
                    view.style.shape
                ),
            };
            let glyph = bx * 0.6;
            inner_svg = frame
                + &format!(
                    "<svg class=\"pele-icon\" data-icon=\"{}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"></svg>",
                    esc_text(icon),
                    num(-glyph / 2.0),
                    num(top + (bx - glyph) / 2.0),
                    num(glyph),
                    num(glyph)
                );
        } else {
            inner_svg = draw_shape(&view.shape, w, h, &format!("{}{}", SHAPE_ATTRS, view.style.shape), &format!("{}{}", LINE_ATTRS, view.style.line));
            if shape_filled(&view.shape) {
                ink = ink_fill(&view.style.shape, &view.style.text, None);
            }
        }

        let mut text = label_svg(&view.label, 0.0, view.dy, &format!(" class=\"pele-label\"{}{}", ink, view.style.text));
        let internal = format!("{} ", classes).contains(" internal-link ");
        if internal && node.label.as_deref().is_some_and(|l| !l.is_empty()) {
            // The label is a note name, but it still ends up in an href, so it gets the same check as a URL.
            let name = decode_entities(node.label.as_deref().unwrap());
            let safe = internal_link(&name);
            let target = esc(&name);
            text = format!("<a class=\"internal-link\"{} data-href=\"{}\">{}</a>", if safe { format!(" href=\"{}\"{}", target, rel_attr()) } else { String::new() }, target, text);
            links.push(LinkInfo { id: node.id.clone(), href: name, internal: true });
        }
        let mut body = inner_svg + &text;
        if let Some(tip) = node.tooltip.as_deref().filter(|t| !t.is_empty()) {
            body = format!("<title>{}</title>{}", esc_text(tip), body);
        }
        if let Some(link) = node.link.as_deref().filter(|l| !l.is_empty()) {
            let attrs = match node.link_target.as_deref().filter(|t| !t.is_empty()) {
                Some(t) => format!(" target=\"{}\"", esc(t)),
                None => String::new(),
            };
            body = linked(&body, link, &node.id, &mut links, &attrs);
        }
        nodes_out.push_str(&format!("<g class=\"pele-node pele-shape-{}{}\" data-id=\"{}\" transform=\"translate({},{})\">{}</g>", view.shape, classes, id, num(x), num(y), body));
    }

    let mut reach = SideReach::default();
    for (i, ce) in cedges.iter().enumerate() {
        if ce.src == ce.dst || drawn[i].thickness.as_deref() == Some("invisible") || ce.route.len() < 6 {
            continue;
        }
        reach.add(ce.src, end_side(&ce.route, 0, &cnodes[ce.src]), &drawn[i].arrow_type_start);
        reach.add(ce.dst, end_side(&ce.route, ce.route.len() - 3, &cnodes[ce.dst]), &drawn[i].arrow_type_end);
    }

    let mut casings = String::new();
    let mut edges_out = String::new();
    let mut labels_out = String::new();
    let mut loop_seen: HashMap<usize, usize> = HashMap::new();
    for i in 0..cedges.len() {
        let edge = drawn[i];
        let label = &edge_labels[i];
        let st = if !edge.style.is_empty() || !edge.css_compiled_styles.is_empty() {
            let decls: Vec<String> = edge.css_compiled_styles.iter().chain(edge.style.iter()).cloned().collect();
            Some(resolve_style(&decls))
        } else {
            None
        };
        let line = st.as_ref().map_or("", |s| s.line.as_str());
        if edge.thickness.as_deref() != Some("invisible") {
            let start_type = &edge.arrow_type_start;
            let end_type = &edge.arrow_type_end;
            let ce = &cedges[i];
            let path: EdgePath = if ce.src == ce.dst {
                let view = &views[ce.src];
                let k = *loop_seen.get(&ce.src).unwrap_or(&0);
                loop_seen.insert(ce.src, k + 1);
                let side = sideways(&cnodes, root_dir, ce.src);
                let mut ll = LoopLabel { label_w: ce.label_w, label_h: ce.label_h, label_x: ce.label_x, label_y: ce.label_y };
                let c = &cnodes[ce.src];
                let p = loop_path(c.x, c.y, view.w, view.h, k, side, LOOP, marker_trim(end_type), &mut ll);
                cedges[i].label_x = ll.label_x;
                cedges[i].label_y = ll.label_y;
                p
            } else {
                let mut route = ce.route.clone();
                let (src, dst) = (ce.src, ce.dst);
                let leaves = reach.of(src, end_side(&route, 0, &cnodes[src]));
                let arrives = reach.of(dst, end_side(&route, route.len() - 3, &cnodes[dst]));
                inset_route(&mut route, 0, &views[src].shape, views[src].w, views[src].h, &cnodes[src]);
                let last = route.len() - 3;
                inset_route(&mut route, last, &views[dst].shape, views[dst].w, views[dst].h, &cnodes[dst]);
                route_path(&route, edge.curve.as_deref(), marker_trim(start_type), marker_trim(end_type), Some(leaves), Some(arrives))
            };
            let over = over_fills(&path.d, &[path.sx, path.sy, path.ex, path.ey], &fills, line, path.tokens.as_deref());
            let color = match over.as_deref() {
                Some(o) if !o.is_empty() => o.to_string(),
                _ => match st.as_ref().and_then(|s| s.stroke.as_deref()) {
                    Some(s) if !s.is_empty() => esc(s),
                    _ => "var(--_l)".to_string(),
                },
            };
            let mut attrs = String::new();
            if edge.thickness.as_deref() == Some("thick") {
                attrs.push_str(" stroke-width=\"2.5\"");
            } else if edge.thickness.as_deref() == Some("dotted") {
                attrs.push_str(" stroke-dasharray=\"3 4\"");
            }
            let classes = class_names(&edge.classes.replace("edge-thickness-normal", "").replace("edge-pattern-solid", "").replace("flowchart-link", ""));
            if over.as_deref() == Some("") {
                casings.push_str(&edge_casing(&path, start_type, end_type, line, edge.thickness.as_deref() == Some("thick")));
            }
            let over_attr = match over.as_deref() {
                Some(o) if !o.is_empty() => format!(" stroke=\"{}\"", o),
                _ => String::new(),
            };
            edges_out.push_str(&format!(
                "<g class=\"pele-edge{}\" data-id=\"{}\"{}{}><path d=\"{}\"{}/>{}{}</g>",
                classes,
                esc_text(&edge.id),
                over_attr,
                line,
                path.d,
                attrs,
                marker(start_type, path.sx, path.sy, path.sdx, path.sdy, &color, "none"),
                marker(end_type, path.ex, path.ey, path.edx, path.edy, &color, "none")
            ));
        }
        if label.width > 0.0 {
            let ce = &cedges[i];
            labels_out.push_str(&edge_label_svg(&edge.id, label, ce.label_x, ce.label_y, st.as_ref().map_or("", |s| s.text.as_str())));
        }
    }

    let fixed = matches!(flow.and_then(|f| f.get("useMaxWidth")), Some(Value::Bool(false)));
    let mut content = label_svg(&title, width / 2.0, pad + title.height / 2.0, " class=\"pele-title\" font-weight=\"var(--_tw)\"");
    if !clusters.is_empty() {
        content.push_str(&format!("<g class=\"pele-clusters\">{}</g>", clusters));
    }
    content.push_str(&casings_svg(&casings));
    if !edges_out.is_empty() {
        content.push_str(&format!("<g class=\"pele-edges\" fill=\"none\" stroke=\"var(--_l)\" stroke-linecap=\"round\">{}</g>", edges_out));
    }
    if !struck_titles.is_empty() {
        content.push_str(&format!("<g class=\"pele-cluster-titles\">{}</g>", struck_titles));
    }
    if !labels_out.is_empty() {
        content.push_str(&format!("<g class=\"pele-edge-labels\" font-size=\"{}\">{}</g>", js::num_str(edge_size), labels_out));
    }
    content.push_str(&format!("<g class=\"pele-nodes\">{}</g>", nodes_out));
    let svg = svg_document("flowchart", width, height, size, !fixed, &Accessible { acc_title: db.acc_title.as_deref(), acc_descr: db.acc_descr.as_deref() }, &content);
    Rendered { svg, width, height, links }
}
