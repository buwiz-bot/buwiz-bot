// Port of src/diagrams/flowchart/parser.ts.

use super::db::FlowDb;
use super::lexer::tokenize;
use super::tokens as t;
use super::types::{DocItem, FlowText, LabelType, LinkInfo};
use crate::errors::{input_offset, syntax_error, Edit, PeleError};
use crate::js;
use crate::util::chars::is_space;

const ID: &[u8] = &[t::NUM, t::NODE_STRING, t::DOWN, t::MINUS, t::DEFAULT, t::COMMA, t::COLON, t::AMP, t::BRKT, t::MULT, t::UNICODE_TEXT];
const ALPHANUM: &[u8] = &[t::NUM, t::UNICODE_TEXT, t::NODE_STRING, t::DIR, t::DOWN, t::MINUS, t::COMMA, t::COLON, t::AMP, t::BRKT, t::MULT];
const NO_TAGS: &[u8] = &[
    t::NUM, t::NODE_STRING, t::SPACE, t::MINUS, t::AMP, t::UNICODE_TEXT, t::COLON, t::MULT, t::BRKT, t::STYLE, t::LINKSTYLE, t::CLASSDEF, t::CLASS, t::CLICK, t::GRAPH, t::DIR, t::subgraph, t::end, t::DOWN, t::UP, t::START_LINK,
];
const STYLE_PART: &[u8] = &[t::NUM, t::NODE_STRING, t::COLON, t::SPACE, t::BRKT, t::STYLE];

const TEXT_TOKENS: &str = "'UNICODE_TEXT', 'TEXT', 'TAGSTART'";
const DIRECTIONS: [&str; 5] = ["TB", "BT", "RL", "LR", "TD"];
const STATEMENT_START: &str =
    "'SEMI', 'NEWLINE', 'SPACE', 'EOF', 'subgraph', 'acc_title', 'acc_descr', 'acc_descr_multiline_value', 'STYLE', 'LINKSTYLE', 'CLASSDEF', 'CLASS', 'CLICK', 'NODE_STRING'";

struct Frame {
    id: Option<FlowText>,
    title: Option<FlowText>,
    same: bool,
    list: Vec<DocItem>,
    collect: bool,
}

type R<T> = Result<T, PeleError>;

/// `source.replace(/}\s*\n/g, '}\n')` and the edits it makes.
fn brace_trail(source: &[u16]) -> (Vec<u16>, Vec<Edit>) {
    let mut out = Vec::with_capacity(source.len());
    let mut edits = Vec::new();
    let mut i = 0;
    while i < source.len() {
        if source[i] == b'}' as u16 {
            let mut e = i + 1;
            while e < source.len() && is_space(source[e] as u32) {
                e += 1;
            }
            if let Some(last) = (i + 1..e).rev().find(|&k| source[k] == 10) {
                edits.push((i, last + 1, 2));
                out.push(b'}' as u16);
                out.push(10);
                i = last + 1;
                continue;
            }
        }
        out.push(source[i]);
        i += 1;
    }
    (out, edits)
}

struct P<'a> {
    source: &'a [u16],
    changed: bool,
    edits: Vec<Edit>,
    types: Vec<u8>,
    texts: Vec<String>,
    starts: Vec<usize>,
    i: usize,
    pending_data: Option<String>,
}

impl P<'_> {
    fn ty(&self) -> u8 {
        self.types[self.i]
    }

    fn fail<T>(&self, expected: &str) -> R<T> {
        let ty = self.ty();
        let at = if !self.changed { self.starts[self.i] } else { input_offset(&[self.edits.clone()], self.starts[self.i]) };
        if ty == t::ERROR {
            return Err(syntax_error("flowchart", self.source, at, "", true));
        }
        Err(syntax_error("flowchart", self.source, at, &format!("Expecting {}, got '{}'", expected, t::TOKEN_NAMES[ty as usize]), false))
    }

    fn expect(&mut self, ty: u8, also: &str) -> R<String> {
        if self.ty() != ty {
            let name = t::TOKEN_NAMES[ty as usize];
            return self.fail(&if also.is_empty() { format!("'{}'", name) } else { format!("'{}', {}", name, also) });
        }
        self.i += 1;
        Ok(self.texts[self.i - 1].clone())
    }

    fn separator(&mut self) -> R<()> {
        let ty = self.ty();
        if ty != t::NEWLINE && ty != t::SEMI && ty != t::EOF {
            return self.fail("'SEMI', 'NEWLINE', 'EOF'");
        }
        self.i += 1;
        Ok(())
    }

    fn run(&mut self, set: &[u8], what: &str) -> R<String> {
        if !set.contains(&self.ty()) {
            return self.fail(what);
        }
        let mut s = self.texts[self.i].clone();
        self.i += 1;
        while set.contains(&self.ty()) {
            s.push_str(&self.texts[self.i]);
            self.i += 1;
        }
        Ok(s)
    }

    fn id_string(&mut self) -> R<String> {
        self.run(ID, "'NODE_STRING'")
    }

    fn alpha_num(&mut self) -> R<String> {
        self.run(ALPHANUM, "'NODE_STRING'")
    }

    fn text(&mut self) -> R<FlowText> {
        let ty = self.ty();
        let type_ = if ty == t::STR {
            LabelType::String
        } else if ty == t::MD_STR {
            LabelType::Markdown
        } else if ty == t::TEXT || ty == t::TAGSTART || ty == t::TAGEND || ty == t::UNICODE_TEXT {
            LabelType::Text
        } else {
            return self.fail(&format!("{}, 'TAGEND', 'STR', 'MD_STR'", TEXT_TOKENS));
        };
        let mut s = self.texts[self.i].clone();
        self.i += 1;
        loop {
            let n = self.ty();
            if n != t::TEXT && n != t::TAGSTART && n != t::TAGEND && n != t::UNICODE_TEXT {
                break;
            }
            s.push_str(&self.texts[self.i]);
            self.i += 1;
        }
        Ok(FlowText { text: s, type_ })
    }

    fn closed(&mut self, ty: u8) -> R<()> {
        self.expect(ty, TEXT_TOKENS).map(|_| ())
    }

    fn edge_text(&mut self) -> R<FlowText> {
        let ty = self.ty();
        let type_ = if ty == t::STR {
            LabelType::String
        } else if ty == t::MD_STR {
            LabelType::Markdown
        } else if ty == t::EDGE_TEXT || ty == t::UNICODE_TEXT {
            LabelType::Text
        } else {
            return self.fail("'EDGE_TEXT', 'STR', 'MD_STR'");
        };
        let mut s = self.texts[self.i].clone();
        self.i += 1;
        while self.ty() == t::EDGE_TEXT || self.ty() == t::UNICODE_TEXT {
            s.push_str(&self.texts[self.i]);
            self.i += 1;
        }
        Ok(FlowText { text: s, type_ })
    }

    fn text_no_tags(&mut self) -> R<FlowText> {
        let ty = self.ty();
        let type_ = if ty == t::STR {
            LabelType::Text
        } else if ty == t::MD_STR {
            LabelType::Markdown
        } else if NO_TAGS.contains(&ty) {
            LabelType::Text
        } else {
            return self.fail("'NODE_STRING', 'STR', 'MD_STR'");
        };
        let mut s = self.texts[self.i].clone();
        self.i += 1;
        while NO_TAGS.contains(&self.ty()) {
            s.push_str(&self.texts[self.i]);
            self.i += 1;
        }
        Ok(FlowText { text: s, type_ })
    }

    fn shape_data(&mut self) -> String {
        let mut s = self.texts[self.i].clone();
        self.i += 1;
        while self.ty() == t::SHAPE_DATA {
            s.push_str(&self.texts[self.i]);
            self.i += 1;
        }
        s
    }

    fn vertex(&mut self, db: &mut FlowDb) -> R<String> {
        let id = self.id_string()?;
        let shaped = |p: &mut Self, db: &mut FlowDb, close: u8, shape: &str| -> R<()> {
            p.i += 1;
            let label = p.text()?;
            p.closed(close)?;
            db.add_vertex(&id, Some(&label), Some(shape), None, None, None)
        };
        match self.ty() {
            t::SQS => shaped(self, db, t::SQE, "square")?,
            t::DOUBLECIRCLESTART => shaped(self, db, t::DOUBLECIRCLEEND, "doublecircle")?,
            t::PS => {
                self.i += 1;
                if self.ty() == t::PS {
                    self.i += 1;
                    let label = self.text()?;
                    self.closed(t::PE)?;
                    self.expect(t::PE, "")?;
                    db.add_vertex(&id, Some(&label), Some("circle"), None, None, None)?;
                } else {
                    let label = self.text()?;
                    self.closed(t::PE)?;
                    db.add_vertex(&id, Some(&label), Some("round"), None, None, None)?;
                }
            }
            t::ELLIPSE_START => shaped(self, db, t::ELLIPSE_END, "ellipse")?,
            t::STADIUMSTART => shaped(self, db, t::STADIUMEND, "stadium")?,
            t::SUBROUTINESTART => shaped(self, db, t::SUBROUTINEEND, "subroutine")?,
            t::VERTEX_WITH_PROPS_START => {
                self.i += 1;
                let field = self.expect(t::NODE_STRING, "")?;
                self.expect(t::COLON, "")?;
                let value = self.expect(t::NODE_STRING, "")?;
                self.expect(t::PIPE, "")?;
                let label = self.text()?;
                self.closed(t::SQE)?;
                db.add_vertex(&id, Some(&label), Some("rect"), None, Some(vec![(field, value)]), None)?;
            }
            t::CYLINDERSTART => shaped(self, db, t::CYLINDEREND, "cylinder")?,
            t::DIAMOND_START => {
                self.i += 1;
                if self.ty() == t::DIAMOND_START {
                    self.i += 1;
                    let label = self.text()?;
                    self.closed(t::DIAMOND_STOP)?;
                    self.expect(t::DIAMOND_STOP, "")?;
                    db.add_vertex(&id, Some(&label), Some("hexagon"), None, None, None)?;
                } else {
                    let label = self.text()?;
                    self.closed(t::DIAMOND_STOP)?;
                    db.add_vertex(&id, Some(&label), Some("diamond"), None, None, None)?;
                }
            }
            t::TAGEND => shaped(self, db, t::SQE, "odd")?,
            t::TRAPSTART => {
                self.i += 1;
                let label = self.text()?;
                if self.ty() == t::INVTRAPEND {
                    self.i += 1;
                    db.add_vertex(&id, Some(&label), Some("lean_right"), None, None, None)?;
                } else {
                    self.closed(t::TRAPEND)?;
                    db.add_vertex(&id, Some(&label), Some("trapezoid"), None, None, None)?;
                }
            }
            t::INVTRAPSTART => {
                self.i += 1;
                let label = self.text()?;
                if self.ty() == t::TRAPEND {
                    self.i += 1;
                    db.add_vertex(&id, Some(&label), Some("lean_left"), None, None, None)?;
                } else {
                    self.closed(t::INVTRAPEND)?;
                    db.add_vertex(&id, Some(&label), Some("inv_trapezoid"), None, None, None)?;
                }
            }
            _ => db.add_vertex(&id, None, None, None, None, None)?,
        }
        Ok(id)
    }

    fn styled_vertex(&mut self, db: &mut FlowDb) -> R<String> {
        let id = self.vertex(db)?;
        if self.ty() == t::STYLE_SEPARATOR {
            self.i += 1;
            let class = self.id_string()?;
            db.set_class(&id, &class);
        }
        Ok(id)
    }

    fn node(&mut self, db: &mut FlowDb) -> R<Vec<String>> {
        let mut ids = vec![self.styled_vertex(db)?];
        self.pending_data = None;
        loop {
            let data = if self.ty() == t::SHAPE_DATA { Some(self.shape_data()) } else { None };
            if self.ty() != t::SPACE {
                self.pending_data = data;
                return Ok(ids);
            }
            while self.ty() == t::SPACE {
                self.i += 1;
            }
            if self.ty() != t::AMP {
                if data.is_some() {
                    return self.fail("'AMP'");
                }
                return Ok(ids);
            }
            self.i += 1;
            if self.ty() != t::SPACE {
                return self.fail("'SPACE'");
            }
            while self.ty() == t::SPACE {
                self.i += 1;
            }
            let next = self.styled_vertex(db)?;
            if let Some(d) = &data {
                db.add_vertex(ids.last().unwrap(), None, None, None, None, Some(d))?;
            }
            ids.push(next);
        }
    }

    fn link(&mut self, db: &mut FlowDb) -> R<LinkInfo> {
        let mut id: Option<String> = None;
        if self.ty() == t::LINK_ID {
            id = Some(self.texts[self.i].clone());
            self.i += 1;
        }
        if self.ty() == t::LINK {
            let k = db.destruct_link(&self.texts[self.i], None);
            self.i += 1;
            let mut info = LinkInfo { type_: k.type_, stroke: k.stroke, length: k.length, text: None, id };
            if self.ty() == t::PIPE {
                self.i += 1;
                info.text = Some(self.text()?);
                self.closed(t::PIPE)?;
                if self.ty() == t::SPACE {
                    self.i += 1;
                }
            }
            return Ok(info);
        }
        if self.ty() == t::START_LINK {
            let start = self.texts[self.i].clone();
            self.i += 1;
            let label = self.edge_text()?;
            let end = self.expect(t::LINK, "'EDGE_TEXT'")?;
            let k = db.destruct_link(&end, Some(&start));
            return Ok(LinkInfo { type_: k.type_, stroke: k.stroke, length: k.length, text: Some(label), id });
        }
        self.fail("'LINK', 'START_LINK'")
    }

    fn vertex_statement(&mut self, db: &mut FlowDb, collect: bool) -> R<Vec<String>> {
        let mut stmt = self.node(db)?;
        if let Some(d) = self.pending_data.clone() {
            db.add_vertex(stmt.last().unwrap(), None, None, None, None, Some(&d))?;
        }
        let mut nodes = stmt.clone();
        loop {
            let ty = self.ty();
            if ty != t::LINK && ty != t::START_LINK && ty != t::LINK_ID {
                return Ok(nodes);
            }
            let info = self.link(db)?;
            let next = self.node(db)?;
            if let Some(d) = self.pending_data.clone() {
                db.add_vertex(next.last().unwrap(), None, None, None, None, Some(&d))?;
            }
            db.add_link(&stmt, &next, Some(&info))?;
            if collect {
                let mut joined = next.clone();
                joined.extend(nodes);
                nodes = joined;
            }
            stmt = next;
        }
    }

    fn style(&mut self) -> R<String> {
        self.run(STYLE_PART, "'NODE_STRING', 'NUM', 'COLON', 'BRKT'")
    }

    fn styles_opt(&mut self) -> R<Vec<String>> {
        let mut out = vec![self.style()?];
        while self.ty() == t::COMMA {
            self.i += 1;
            out.push(self.style()?);
        }
        Ok(out)
    }

    fn link_style_statement(&mut self, db: &mut FlowDb) -> R<()> {
        self.i += 1;
        self.expect(t::SPACE, "")?;
        let mut positions: Vec<String>;
        if self.ty() == t::DEFAULT {
            positions = vec![self.texts[self.i].clone()];
            self.i += 1;
        } else {
            positions = vec![self.expect(t::NUM, "'DEFAULT'")?];
            while self.ty() == t::COMMA {
                self.i += 1;
                positions.push(self.expect(t::NUM, "")?);
            }
        }
        self.expect(t::SPACE, "")?;
        if self.ty() != t::INTERPOLATE {
            let styles = self.styles_opt()?;
            return db.update_link(&positions, styles);
        }
        self.i += 1;
        self.expect(t::SPACE, "")?;
        let interpolate = self.alpha_num()?;
        if self.ty() == t::SPACE {
            self.i += 1;
            let styles = self.styles_opt()?;
            db.update_link_interpolate(&positions, &interpolate)?;
            db.update_link(&positions, styles)
        } else {
            db.update_link_interpolate(&positions, &interpolate)
        }
    }

    fn link_tail(&mut self, db: &mut FlowDb, id: &str, url: &str) -> R<()> {
        if self.ty() != t::SPACE {
            db.set_link(id, url, None);
            return Ok(());
        }
        self.i += 1;
        if self.ty() == t::LINK_TARGET {
            let target = self.texts[self.i].clone();
            self.i += 1;
            db.set_link(id, url, Some(&target));
            return Ok(());
        }
        let tooltip = self.expect(t::STR, "'LINK_TARGET'")?;
        if self.ty() == t::SPACE {
            self.i += 1;
            let target = self.expect(t::LINK_TARGET, "")?;
            db.set_link(id, url, Some(&target));
        } else {
            db.set_link(id, url, None);
        }
        db.set_tooltip(id, &tooltip);
        Ok(())
    }

    fn click_statement(&mut self, db: &mut FlowDb) -> R<()> {
        let id = self.texts[self.i].clone();
        self.i += 1;
        match self.ty() {
            t::CALLBACKNAME => {
                self.i += 1;
                if self.ty() == t::CALLBACKARGS {
                    self.i += 1;
                }
                let mut tooltip = None;
                if self.ty() == t::SPACE {
                    self.i += 1;
                    tooltip = Some(self.expect(t::STR, "")?);
                }
                db.set_click_event(&id);
                if let Some(tip) = tooltip {
                    db.set_tooltip(&id, &tip);
                }
                Ok(())
            }
            t::HREF => {
                self.i += 1;
                let url = self.expect(t::STR, "")?;
                self.link_tail(db, &id, &url)
            }
            t::STR => {
                let url = self.texts[self.i].clone();
                self.i += 1;
                self.link_tail(db, &id, &url)
            }
            _ => {
                self.alpha_num()?;
                if self.ty() == t::SPACE {
                    self.i += 1;
                    let tooltip = self.expect(t::STR, "")?;
                    db.set_click_event(&id);
                    db.set_tooltip(&id, &tooltip);
                } else {
                    db.set_click_event(&id);
                }
                Ok(())
            }
        }
    }

    fn subgraph_header(&mut self) -> R<Frame> {
        self.i += 1;
        let ty = self.ty();
        if ty == t::NEWLINE || ty == t::SEMI || ty == t::EOF {
            self.i += 1;
            return Ok(Frame { id: None, title: None, same: false, list: Vec::new(), collect: false });
        }
        self.expect(t::SPACE, "'SEMI', 'NEWLINE', 'EOF'")?;
        let id = self.text_no_tags()?;
        let mut title = id.clone();
        let mut same = true;
        if self.ty() == t::SQS {
            self.i += 1;
            title = self.text()?;
            self.closed(t::SQE)?;
            same = false;
        }
        let ty = self.ty();
        if ty != t::NEWLINE && ty != t::SEMI && ty != t::EOF {
            return self.fail("'SEMI', 'NEWLINE', 'EOF', 'SQS'");
        }
        self.i += 1;
        Ok(Frame { id: Some(id), title: Some(title), same, list: Vec::new(), collect: false })
    }

    fn statement(&mut self, db: &mut FlowDb, collect: bool) -> R<DocItem> {
        let ty = self.ty();
        match ty {
            t::STYLE => {
                self.i += 1;
                self.expect(t::SPACE, "")?;
                let id = self.id_string()?;
                self.expect(t::SPACE, "")?;
                let styles = self.styles_opt()?;
                db.add_vertex(&id, None, None, Some(styles), None, None)?;
                self.separator()?;
                Ok(DocItem::List(Vec::new()))
            }
            t::LINKSTYLE => {
                self.link_style_statement(db)?;
                self.separator()?;
                Ok(DocItem::List(Vec::new()))
            }
            t::CLASSDEF => {
                self.i += 1;
                self.expect(t::SPACE, "")?;
                let id = self.id_string()?;
                self.expect(t::SPACE, "")?;
                let styles = self.styles_opt()?;
                db.add_class(&id, &styles);
                self.separator()?;
                Ok(DocItem::List(Vec::new()))
            }
            t::CLASS => {
                self.i += 1;
                self.expect(t::SPACE, "")?;
                let ids = self.id_string()?;
                self.expect(t::SPACE, "")?;
                let class = self.id_string()?;
                db.set_class(&ids, &class);
                self.separator()?;
                Ok(DocItem::List(Vec::new()))
            }
            t::CLICK => {
                self.click_statement(db)?;
                self.separator()?;
                Ok(DocItem::List(Vec::new()))
            }
            t::direction_tb | t::direction_bt | t::direction_rl | t::direction_lr | t::direction_td => {
                self.i += 1;
                Ok(DocItem::Dir(DIRECTIONS[(ty - t::direction_tb) as usize].into()))
            }
            t::acc_title => {
                self.i += 1;
                let value = js::trim(&self.expect(t::acc_title_value, "")?).to_string();
                db.set_acc_title(&value);
                Ok(DocItem::Str(value))
            }
            t::acc_descr => {
                self.i += 1;
                let value = js::trim(&self.expect(t::acc_descr_value, "")?).to_string();
                db.set_acc_description(&value);
                Ok(DocItem::Str(value))
            }
            t::acc_descr_multiline_value => {
                let value = js::trim(&self.texts[self.i]).to_string();
                self.i += 1;
                db.set_acc_description(&value);
                Ok(DocItem::Str(value))
            }
            _ => {
                if !ID.contains(&ty) {
                    return self.fail(STATEMENT_START);
                }
                let nodes = self.vertex_statement(db, collect)?;
                self.separator()?;
                Ok(DocItem::List(nodes))
            }
        }
    }

    // Subgraphs nest without recursion: each open one keeps the list and collect flag of its parent.
    fn document(&mut self, db: &mut FlowDb) -> R<()> {
        let mut open: Vec<Frame> = Vec::new();
        let mut list: Vec<DocItem> = Vec::new();
        let mut collect = false;
        loop {
            let ty = self.ty();
            if ty == t::END {
                break;
            }
            if ty == t::end {
                let Some(frame) = open.pop() else { break };
                self.i += 1;
                let id = db.add_sub_graph(frame.id.as_ref(), &list, frame.title.as_ref(), frame.same);
                list = frame.list;
                collect = frame.collect;
                if collect {
                    list.push(DocItem::Str(id));
                }
            } else if ty == t::SEMI || ty == t::NEWLINE || ty == t::SPACE || ty == t::EOF {
                if collect {
                    list.push(DocItem::Str(self.texts[self.i].clone()));
                }
                self.i += 1;
            } else if ty == t::subgraph {
                let mut frame = self.subgraph_header()?;
                frame.list = std::mem::take(&mut list);
                frame.collect = collect;
                open.push(frame);
                collect = true;
            } else {
                let item = self.statement(db, collect)?;
                if collect && !matches!(&item, DocItem::List(l) if l.is_empty()) {
                    list.push(item);
                }
            }
        }
        if !open.is_empty() {
            return self.fail("'end'");
        }
        Ok(())
    }
}

pub fn parse_flowchart(source: &[u16], db: &mut FlowDb) -> R<()> {
    let (src, edits) = if source.contains(&(b'}' as u16)) { brace_trail(source) } else { (source.to_vec(), Vec::new()) };
    let changed = !edits.is_empty();
    let tk = tokenize(&src);
    let mut p = P { source, changed, edits, types: tk.types, texts: tk.texts.iter().map(|t| js::from16(t)).collect(), starts: tk.starts, i: 0, pending_data: None };

    while p.ty() == t::SPACE || p.ty() == t::NEWLINE {
        p.i += 1;
    }
    p.expect(t::GRAPH, "'SPACE', 'NEWLINE'")?;
    if p.ty() == t::NODIR {
        p.i += 1;
        db.set_direction("TB");
    } else {
        let dir = p.expect(t::DIR, "'NODIR'")?;
        let mut ty = p.ty();
        if ty == t::SPACE {
            while p.ty() == t::SPACE {
                p.i += 1;
            }
            ty = p.ty();
            if ty != t::NEWLINE {
                return p.fail("'NEWLINE', 'SPACE'");
            }
        } else if ty != t::SEMI && ty != t::NEWLINE {
            return p.fail("'SEMI', 'NEWLINE', 'SPACE'");
        }
        p.i += 1;
        db.set_direction(&dir);
    }
    p.document(db)?;
    if p.ty() != t::END {
        return p.fail(STATEMENT_START);
    }
    Ok(())
}
