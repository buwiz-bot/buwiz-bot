// Port of src/diagrams/flowchart/db.ts.

use super::shapes::is_valid_shape;
use super::types::*;
use crate::errors::{PeleError, PeleErrorCode};
use crate::js;
use crate::util::url::safe_url;
use crate::util::yaml::{parse_yaml, Map, Value};
use std::collections::{HashMap, HashSet};

fn label_type(type_: Option<&str>) -> LabelType {
    match type_ {
        Some("text") => LabelType::Text,
        Some("string") => LabelType::String,
        _ => LabelType::Markdown,
    }
}

fn label_type_of(t: LabelType) -> LabelType {
    t
}

fn unquote(text: &str) -> String {
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        text[1..text.len() - 1].to_string()
    } else {
        text.to_string()
    }
}

pub struct LinkKind {
    pub type_: String,
    pub stroke: String,
    pub length: Option<f64>,
}

fn destruct_start_link(raw: &str) -> LinkKind {
    let mut s = js::trim(raw);
    let mut type_ = "arrow_open";
    match s.chars().next() {
        Some('<') => {
            type_ = "arrow_point";
            s = &s[1..];
        }
        Some('x') => {
            type_ = "arrow_cross";
            s = &s[1..];
        }
        Some('o') => {
            type_ = "arrow_circle";
            s = &s[1..];
        }
        _ => {}
    }
    let mut stroke = "normal";
    if s.contains('=') {
        stroke = "thick";
    }
    if s.contains('.') {
        stroke = "dotted";
    }
    LinkKind { type_: type_.into(), stroke: stroke.into(), length: None }
}

fn destruct_end_link(raw: &str) -> LinkKind {
    let s = js::trim(raw);
    let mut line = if s.is_empty() { "" } else { &s[..s.len() - 1] };
    let mut type_ = "arrow_open".to_string();
    match s.chars().last() {
        Some('x') => {
            type_ = "arrow_cross".into();
            if s.starts_with('x') {
                type_ = format!("double_{}", type_);
                line = &line[1.min(line.len())..];
            }
        }
        Some('>') => {
            type_ = "arrow_point".into();
            if s.starts_with('<') {
                type_ = format!("double_{}", type_);
                line = &line[1.min(line.len())..];
            }
        }
        Some('o') => {
            type_ = "arrow_circle".into();
            if s.starts_with('o') {
                type_ = format!("double_{}", type_);
                line = &line[1.min(line.len())..];
            }
        }
        _ => {}
    }
    let mut stroke = "normal";
    let mut length = js::len16(line) as f64 - 1.0;
    if line.starts_with('=') {
        stroke = "thick";
    }
    if line.starts_with('~') {
        stroke = "invisible";
    }
    let dots = line.chars().filter(|&c| c == '.').count();
    if dots > 0 {
        stroke = "dotted";
        length = dots as f64;
    }
    LinkKind { type_, stroke: stroke.into(), length: Some(length) }
}

#[derive(Default)]
pub struct FlowDbOptions {
    pub max_edges: Option<usize>,
    pub inherit_dir: bool,
}

#[derive(Default)]
pub struct FlowDb {
    pub direction: Option<String>,
    pub nodes: Vec<FlowNode>,
    pub node_index: HashMap<String, usize>,
    pub edges: Vec<FlowEdge>,
    pub subgraphs: Vec<FlowSubgraph>,
    pub classes: HashMap<String, FlowClassDef>,
    pub tooltips: HashMap<String, String>,
    pub default_edge_style: Option<Vec<String>>,
    pub default_interpolate: Option<String>,
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,

    subgraph_lookup: HashMap<String, usize>,
    members: HashSet<String>,
    edge_by_id: HashMap<String, usize>,
    pair_count: HashMap<(String, String), usize>,
    sub_count: usize,
    options: FlowDbOptions,
}

/// A value that upstream keeps as whatever YAML gave it, here kept as a string when truthy.
/// A truthy non-string never matches a curve name.
fn truthy_string(v: &Value) -> Option<String> {
    match v {
        Value::Str(s) if !s.is_empty() => Some(s.clone()),
        v if v.truthy() => Some(format!("\u{0}{}", v.js_string())),
        _ => None,
    }
}

type R<T> = Result<T, PeleError>;

impl FlowDb {
    pub fn new(options: FlowDbOptions) -> Self {
        FlowDb { options, ..Default::default() }
    }

    pub fn node(&self, id: &str) -> Option<&FlowNode> {
        self.node_index.get(id).map(|&i| &self.nodes[i])
    }

    fn node_mut(&mut self, id: &str) -> Option<&mut FlowNode> {
        match self.node_index.get(id) {
            Some(&i) => Some(&mut self.nodes[i]),
            None => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_vertex(&mut self, id: &str, text_obj: Option<&FlowText>, type_: Option<&str>, style: Option<Vec<String>>, props: Option<Vec<(String, String)>>, metadata: Option<&str>) -> R<()> {
        if id.is_empty() || js::trim(id).is_empty() {
            return Ok(());
        }

        let mut doc: Option<Map> = None;
        if let Some(metadata) = metadata {
            let yaml = if metadata.contains('\n') { format!("{}\n", metadata) } else { format!("{{\n{}\n}}", metadata) };
            match parse_yaml(&yaml) {
                Ok(Value::Map(m)) => doc = Some(m),
                Ok(_) => {}
                Err(message) => return Err(PeleError::typed(format!("Metadata of \"{}\" is not valid YAML. {}", id, message), PeleErrorCode::Syntax, "flowchart")),
            }
        }

        if let (Some(&si), Some(doc)) = (self.subgraph_lookup.get(id), doc.as_ref()) {
            let sg = &mut self.subgraphs[si];
            let mut merged = sg.metadata.clone().unwrap_or_default();
            for (k, v) in &doc.0 {
                merged.set(k, v.clone());
            }
            sg.metadata = Some(merged);
            return Ok(());
        }

        if let Some(&ei) = self.edge_by_id.get(id) {
            if let Some(doc) = &doc {
                let edge = &mut self.edges[ei];
                if let Some(v) = doc.get("animate") {
                    edge.animate = v.truthy();
                }
                if let Some(v) = doc.get("animation") {
                    edge.animation = Some(v.js_string());
                }
                if let Some(v) = doc.get("curve") {
                    edge.interpolate = truthy_string(v);
                }
            }
            return Ok(());
        }

        if !self.node_index.contains_key(id) {
            self.node_index.insert(id.to_string(), self.nodes.len());
            self.nodes.push(FlowNode { id: id.to_string(), ..Default::default() });
        }
        let vertex = self.node_mut(id).unwrap();

        if let Some(t) = text_obj {
            vertex.label_type = t.type_;
            vertex.text = Some(unquote(js::trim(&t.text)));
        } else if vertex.text.is_none() {
            vertex.text = Some(id.to_string());
        }
        if let Some(t) = type_ {
            vertex.type_ = Some(t.to_string());
        }
        if let Some(style) = style {
            vertex.styles.extend(style);
        }
        match (&mut vertex.props, props) {
            (None, p) => vertex.props = Some(p.unwrap_or_default()),
            (Some(existing), Some(p)) => {
                for (k, v) in p {
                    match existing.iter_mut().find(|(ek, _)| *ek == k) {
                        Some(slot) => slot.1 = v,
                        None => existing.push((k, v)),
                    }
                }
            }
            _ => {}
        }

        let Some(doc) = doc else { return Ok(()) };

        if let Some(shape) = doc.get("shape").filter(|v| v.truthy()) {
            let shape = shape.js_string();
            if shape != shape.to_lowercase() || shape.contains('_') {
                return Err(PeleError::typed(format!("No such shape: {}. Shape names should be lowercase.", shape), PeleErrorCode::Semantic, "flowchart"));
            }
            if !is_valid_shape(&shape) {
                return Err(PeleError::typed(format!("No such shape: {}.", shape), PeleErrorCode::Semantic, "flowchart"));
            }
            vertex.type_ = Some(shape);
        }
        let label = doc.get("label").and_then(|v| v.as_str()).map(str::to_string);
        let label_blank = label.as_deref().is_none_or(|l| js::trim(l).is_empty());
        if let Some(l) = doc.get("label").filter(|v| v.truthy()) {
            vertex.text = Some(l.js_string());
            vertex.label_type = label_type(doc.get("labelType").and_then(|v| v.as_str()));
        }
        if let Some(icon) = doc.get("icon").filter(|v| v.truthy()) {
            vertex.icon = Some(icon.js_string());
            if label_blank && vertex.text.as_deref() == Some(id) {
                vertex.text = Some(String::new());
            }
        }
        if let Some(v) = doc.get("form").filter(|v| v.truthy()) {
            vertex.form = Some(v.js_string());
        }
        if let Some(v) = doc.get("pos").filter(|v| v.truthy()) {
            vertex.pos = Some(v.js_string());
        }
        if let Some(img) = doc.get("img").filter(|v| v.truthy()) {
            vertex.img = Some(img.js_string());
            if label_blank && vertex.text.as_deref() == Some(id) {
                vertex.text = Some(String::new());
            }
        }
        if let Some(v) = doc.get("constraint").filter(|v| v.truthy()) {
            vertex.constraint = Some(v.js_string());
        }
        if let Some(v) = doc.get("w").filter(|v| v.truthy()) {
            vertex.asset_width = Some(v.js_number());
        }
        if let Some(v) = doc.get("h").filter(|v| v.truthy()) {
            vertex.asset_height = Some(v.js_number());
        }
        Ok(())
    }

    pub fn add_single_link(&mut self, start: &str, end: &str, link: Option<&LinkInfo>, id: Option<&str>) -> R<()> {
        let mut edge = FlowEdge {
            start: start.to_string(),
            end: end.to_string(),
            type_: None,
            text: String::new(),
            label_type: LabelType::Text,
            stroke: None,
            length: None,
            style: None,
            classes: Vec::new(),
            id: None,
            is_user_defined_id: false,
            interpolate: self.default_interpolate.clone(),
            animate: false,
            animation: None,
        };
        if let Some(t) = link.and_then(|l| l.text.as_ref()) {
            edge.text = unquote(js::trim(&t.text));
            edge.label_type = label_type_of(t.type_);
        }
        if let Some(l) = link {
            edge.type_ = Some(l.type_.clone());
            edge.stroke = Some(l.stroke.clone());
            edge.length = l.length.map(|v| if v > 10.0 { 10.0 } else { v });
        }
        let pair = (start.to_string(), end.to_string());
        let count = self.pair_count.get(&pair).copied().unwrap_or(0);
        match id {
            Some(id) if !id.is_empty() && !self.edge_by_id.contains_key(id) => {
                edge.id = Some(id.to_string());
                edge.is_user_defined_id = true;
            }
            _ => edge.id = Some(format!("L_{}_{}_{}", start, end, if count == 0 { 0 } else { count + 1 })),
        }

        if let Some(max) = self.options.max_edges {
            if self.edges.len() >= max {
                return Err(PeleError::typed(format!("Edge limit exceeded. The limit is {}.", max), PeleErrorCode::Limit, "flowchart"));
            }
        }
        let eid = edge.id.clone().unwrap();
        self.edges.push(edge);
        self.pair_count.insert(pair, count + 1);
        self.edge_by_id.entry(eid).or_insert(self.edges.len() - 1);
        Ok(())
    }

    pub fn add_link(&mut self, starts: &[String], ends: &[String], link: Option<&LinkInfo>) -> R<()> {
        let id = link.and_then(|l| l.id.as_ref()).map(|id| id.replacen('@', "", 1));
        let last_start = starts.last().unwrap();
        let first_end = &ends[0];
        for start in starts {
            for end in ends {
                let own = if start == last_start && end == first_end { id.as_deref() } else { None };
                self.add_single_link(start, end, link, own)?;
            }
        }
        Ok(())
    }

    pub fn update_link_interpolate(&mut self, positions: &[String], interpolate: &str) -> R<()> {
        for pos in positions {
            if pos == "default" {
                self.default_interpolate = Some(interpolate.to_string()).filter(|s| !s.is_empty());
            } else {
                let ei = self.edge_at(pos)?;
                self.edges[ei].interpolate = Some(interpolate.to_string()).filter(|s| !s.is_empty());
            }
        }
        Ok(())
    }

    pub fn update_link(&mut self, positions: &[String], mut style: Vec<String>) -> R<()> {
        // Upstream assigns one array to every position and adds `fill:none` to it once: each edge
        // ends up with the array as it is after the first.
        let mut targets = Vec::new();
        for pos in positions {
            if pos == "default" {
                self.default_edge_style = Some(style.clone());
                continue;
            }
            let ei = self.edge_at(pos)?;
            targets.push(ei);
            if !style.is_empty() && !style.iter().any(|s| s.starts_with("fill")) {
                style.push("fill:none".into());
            }
        }
        for ei in targets {
            self.edges[ei].style = Some(style.clone());
        }
        Ok(())
    }

    fn edge_at(&self, pos: &str) -> R<usize> {
        let n = js::to_number(pos);
        if n >= 0.0 && n.fract() == 0.0 && (n as usize) < self.edges.len() {
            return Ok(n as usize);
        }
        Err(PeleError::typed(
            format!(
                "The index {} for linkStyle is out of bounds. Valid indices for linkStyle are between 0 and {}. (Help: Ensure that the index is within the range of existing edges.)",
                pos,
                self.edges.len() as i64 - 1
            ),
            PeleErrorCode::Semantic,
            "flowchart",
        ))
    }

    pub fn add_class(&mut self, ids: &str, styles: &[String]) {
        let joined = styles.join(",").replace("\\,", "§§§").replace(',', ";").replace("§§§", ",");
        let style: Vec<String> = joined.split(';').map(str::to_string).collect();
        for id in ids.split(',') {
            let class = self.classes.entry(id.to_string()).or_insert_with(|| FlowClassDef { id: id.to_string(), styles: Vec::new(), text_styles: Vec::new() });
            for s in &style {
                if s.contains("color") {
                    class.text_styles.push(s.replacen("fill", "bgFill", 1));
                }
                class.styles.push(s.clone());
            }
        }
    }

    pub fn set_direction(&mut self, dir: &str) {
        let mut d = js::trim(dir).to_string();
        if d.contains('<') {
            d = "RL".into();
        }
        if d.contains('^') {
            d = "BT".into();
        }
        if d.contains('>') {
            d = "LR".into();
        }
        if d.contains('v') {
            d = "TB".into();
        }
        if d == "TD" {
            d = "TB".into();
        }
        self.direction = Some(d);
    }

    pub fn set_class(&mut self, ids: &str, class_name: &str) {
        for id in ids.split(',') {
            if let Some(n) = self.node_mut(id) {
                n.classes.push(class_name.to_string());
            }
            if let Some(&ei) = self.edge_by_id.get(id) {
                self.edges[ei].classes.push(class_name.to_string());
            }
            if let Some(&si) = self.subgraph_lookup.get(id) {
                self.subgraphs[si].classes.push(class_name.to_string());
            }
        }
    }

    pub fn set_tooltip(&mut self, ids: &str, tooltip: &str) {
        for id in ids.split(',') {
            self.tooltips.insert(id.to_string(), tooltip.to_string());
        }
    }

    pub fn set_link(&mut self, ids: &str, link: &str, target: Option<&str>) {
        let url = js::trim(link);
        for id in ids.split(',') {
            if let Some(v) = self.node_mut(id) {
                v.link = if url.is_empty() { None } else { Some(safe_url(url)) };
                v.link_target = target.map(str::to_string);
            }
        }
        self.set_class(ids, "clickable");
    }

    // Callbacks are never executed; the node is only marked clickable, as in Mermaid's strict mode.
    pub fn set_click_event(&mut self, ids: &str) {
        self.set_class(ids, "clickable");
    }

    pub fn set_acc_title(&mut self, text: &str) {
        self.acc_title = Some(js::trim_start(text).to_string());
    }

    pub fn set_acc_description(&mut self, text: &str) {
        // text.replace(/\n\s+/g, '\n')
        let c: Vec<char> = text.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < c.len() {
            if c[i] == '\n' && i + 1 < c.len() && js::is_space(c[i + 1]) {
                out.push('\n');
                i += 1;
                while i < c.len() && js::is_space(c[i]) {
                    i += 1;
                }
                continue;
            }
            out.push(c[i]);
            i += 1;
        }
        self.acc_descr = Some(out);
    }

    /// `same` is whether the id and the title are one token (no `[title]` was given).
    pub fn add_sub_graph(&mut self, id_text: Option<&FlowText>, list: &[DocItem], title_text: Option<&FlowText>, same: bool) -> String {
        let mut id: Option<String> = id_text.map(|t| js::trim(&t.text).to_string());
        let mut title = title_text.map_or(String::new(), |t| t.text.clone());
        if id_text.is_some() && same && title.chars().any(js::is_space) {
            id = None;
        }

        let mut seen: HashSet<String> = HashSet::new();
        let mut node_list: Vec<String> = Vec::new();
        let mut dir: Option<String> = None;
        for entry in list {
            match entry {
                DocItem::Str(s) => {
                    if !js::trim(s).is_empty() && !seen.contains(s) {
                        seen.insert(s.clone());
                        node_list.push(s.clone());
                    }
                }
                DocItem::List(items) => {
                    for item in items {
                        if !js::trim(item).is_empty() && !seen.contains(item) {
                            seen.insert(item.clone());
                            node_list.push(item.clone());
                        }
                    }
                }
                DocItem::Dir(v) => dir = Some(v.clone()),
            }
        }
        if dir.is_none() && self.options.inherit_dir {
            dir = self.direction.clone();
        }

        let id = id.unwrap_or_else(|| format!("subGraph{}", self.sub_count));
        if title.is_empty() {
            title = String::new();
        }
        self.sub_count += 1;

        // A node belongs to the first subgraph that lists it, and a subgraph never contains itself.
        let nodes: Vec<String> = node_list.into_iter().filter(|n| !self.members.contains(n) && *n != id).collect();
        for n in &nodes {
            self.members.insert(n.clone());
        }
        let subgraph = FlowSubgraph {
            id: id.clone(),
            nodes: nodes.clone(),
            title: js::trim(&title).to_string(),
            classes: Vec::new(),
            dir,
            label_type: label_type(title_text.map(|t| match t.type_ {
                LabelType::Text => "text",
                LabelType::String => "string",
                LabelType::Markdown => "markdown",
            })),
            metadata: None,
        };

        match self.subgraph_lookup.get(&id) {
            Some(&si) => self.subgraphs[si].nodes.extend(nodes),
            None => {
                self.subgraphs.push(subgraph);
                self.subgraph_lookup.insert(id.clone(), self.subgraphs.len() - 1);
            }
        }
        id
    }

    pub fn destruct_link(&self, end: &str, start: Option<&str>) -> LinkKind {
        let info = destruct_end_link(end);
        let Some(start) = start.filter(|s| !s.is_empty()) else { return info };
        let mut start_info = destruct_start_link(start);
        if start_info.stroke != info.stroke {
            return LinkKind { type_: "INVALID".into(), stroke: "INVALID".into(), length: None };
        }
        if start_info.type_ == "arrow_open" {
            start_info.type_ = info.type_;
        } else {
            if start_info.type_ != info.type_ {
                return LinkKind { type_: "INVALID".into(), stroke: "INVALID".into(), length: None };
            }
            start_info.type_ = format!("double_{}", start_info.type_);
        }
        if start_info.type_ == "double_arrow" {
            start_info.type_ = "double_arrow_point".into();
        }
        start_info.length = info.length;
        start_info
    }

    pub fn subgraph(&self, id: &str) -> Option<&FlowSubgraph> {
        self.subgraph_lookup.get(id).map(|&i| &self.subgraphs[i])
    }
}
