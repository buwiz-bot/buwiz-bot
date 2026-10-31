//! The formatting bar's commands on the worker, each as the web editor
//! runs it (`docx-react`'s `Bv`, `f2`, `g2`, the `setHyperlink` and table
//! branches of its yrs command runner, `resolveImageLayoutAttrs`, `Pg`),
//! and what the bar shows of the selection ([`Formats`], the web's
//! selection context): the run's marks at the selection's start, its
//! paragraph's properties, the picture selected, the caret's cell.
//!
//! Suggesting runs every edit with [`EditCtx::suggesting`]; Viewing takes
//! none (the web's `viewing-mode` gate).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use docx_edit::{CellLoc, EditCtx, InlineFormatDelta, ParaAttrDelta, ParaSelector, Patch, Position, SegmentContent, SimpleFormat, StoryRange, TableLocator, TableRange};
use docx_layout::display_list::Primitive;
use yrs::Any;

use super::super::index::cell_of;
use super::super::save;
use super::{Fmt, Word, local, spot};

/// The editing mode dropdown's modes (`ks`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum EditMode {
    #[default]
    Editing,
    Suggesting,
    Viewing,
}

/// The reasons the web gives for a command it won't run
/// (`commands.reasons.*`).
pub(crate) const VIEWING: &str = "Switch to Editing or Suggesting to make changes.";
pub(crate) const NOT_SUGGESTED: &str = "This change can't be recorded as a suggestion.";
pub(crate) const UNSUPPORTED: &str = "This action is not supported yet.";

/// A text colour as the colour picker gives it (`uE`): an `RRGGBB`, or a
/// theme slot (the engine keeps the slot; a tint or shade goes with it no
/// further, as the web's `formatRange` takes it).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TextColor {
    Rgb(String),
    Theme(String),
}

/// The Hyperlink dialog's submit (`setHyperlink`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Link {
    pub href: String,
    pub text: Option<String>,
    pub tooltip: Option<String>,
    /// Editing the link at the caret (its whole run).
    pub edit: bool,
}

/// The Image group's commands on the selected picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Picture {
    /// A text wrapping choice (`M1`: `inline`, `wrapRight`, `wrapLeft`,
    /// `behind`, `inFront`).
    Wrap(&'static str),
    /// `rotateCW`, `rotateCCW`, `flipH` or `flipV`.
    Transform(&'static str),
}

/// The Table group's commands (`handleTableAction`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TableOp {
    RowAbove,
    RowBelow,
    ColumnLeft,
    ColumnRight,
    Merge,
    Split,
    SelectTable,
    DeleteRow,
    DeleteColumn,
    DeleteTable,
    /// The borders menu (`_1`): `all`, `outside`, `inside`, `top`,
    /// `bottom`, `left`, `right`, `none`.
    Borders(&'static str),
    /// `RRGGBB`.
    BorderColor(String),
    /// Eighths of a point (`fE`).
    BorderWidth(u32),
    Fill(Option<String>),
    /// The table's alignment (`tableProperties` `justification`).
    Justify(&'static str),
    /// Table Properties' width (none for `auto`), its kind and alignment.
    Properties { width: Option<f64>, width_type: String, justification: String },
}

/// The selection as the formatting bar shows it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Formats {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub superscript: bool,
    pub subscript: bool,
    /// Points.
    pub size: f32,
    pub family: String,
    /// `RRGGBB`.
    pub color: Option<String>,
    pub highlight: Option<String>,
    /// The link at the selection's start: its address and tooltip.
    pub link: Option<(String, Option<String>)>,
    /// That link's text.
    pub link_text: String,
    /// The selection's text (the Hyperlink dialog's display text).
    pub text: String,
    pub style: String,
    pub align: String,
    /// 240ths of a line.
    pub line_spacing: Option<f64>,
    /// The paragraph's list: `numId` and level.
    pub list: Option<(u32, u32)>,
    pub indent_left: f64,
    /// A picture is the selection: its wrap as the wrap menu names it.
    pub image: Option<String>,
}

impl Default for Formats {
    fn default() -> Self {
        Formats {
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            superscript: false,
            subscript: false,
            size: 11.0,
            family: String::new(),
            color: None,
            highlight: None,
            link: None,
            link_text: String::new(),
            text: String::new(),
            style: "Normal".to_owned(),
            align: "left".to_owned(),
            line_spacing: None,
            list: None,
            indent_left: 0.0,
            image: None,
        }
    }
}

/// The caret's table cell, as the Table group gates its commands.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CellInfo {
    pub rows: u32,
    pub cols: u32,
    /// More than one cell selected ("Select entire table").
    pub multi: bool,
    pub can_split: bool,
    pub fill: Option<String>,
    pub border: Option<String>,
    pub justification: Option<String>,
    /// The table's width and its kind (`auto`, `dxa`, `pct`).
    pub width: Option<f64>,
    pub width_type: Option<String>,
}

/// A paragraph style the style list offers (`resolveParagraphStyleOptions`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StyleInfo {
    pub id: String,
    pub name: String,
    /// Half-points.
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub italic: bool,
    pub color: Option<String>,
}

/// What the package says once: its styles (for the list and for
/// restyling), its fonts and the run defaults.
pub(crate) struct Styles {
    pub paragraph: Arc<Vec<StyleInfo>>,
    pub fonts: Arc<Vec<String>>,
    pub family: String,
    pub size: f32,
    resolver: Option<docx_edit::StyleResolver>,
}

/// Word's highlight names (`w:highlight`) as `RRGGBB` (`resolveHighlightColor`).
pub(crate) fn highlight_hex(name: &str) -> Option<&'static str> {
    Some(match name {
        "yellow" => "FFFF00",
        "green" => "00FF00",
        "cyan" => "00FFFF",
        "magenta" => "FF00FF",
        "blue" => "0000FF",
        "red" => "FF0000",
        "darkBlue" => "000080",
        "darkCyan" => "008080",
        "darkGreen" => "008000",
        "darkMagenta" => "800080",
        "darkRed" => "800000",
        "darkYellow" => "808000",
        "darkGray" => "808080",
        "lightGray" => "C0C0C0",
        "black" => "000000",
        "white" => "FFFFFF",
        _ => return None,
    })
}

/// An attribute's string.
fn s_of(v: Option<&Any>) -> Option<String> {
    match v? {
        Any::String(s) => Some(s.to_string()),
        _ => None,
    }
}

fn n_of(v: Option<&Any>) -> Option<f64> {
    match v? {
        Any::Number(n) => Some(*n),
        Any::BigInt(n) => Some(*n as f64),
        Any::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn field<'a>(v: Option<&'a Any>, k: &str) -> Option<&'a Any> {
    match v? {
        Any::Map(m) => m.get(k),
        _ => None,
    }
}

/// `w:val`-style attribute `name` in `tag`.
fn attr(tag: &str, name: &str) -> Option<String> {
    let at = tag.find(&format!("{name}=\""))? + name.len() + 2;
    Some(tag[at..].split('"').next()?.to_owned())
}

/// The first `<w:{el}` tag in `xml`, up to its `>`.
fn tag<'a>(xml: &'a str, el: &str) -> Option<&'a str> {
    let at = xml.find(&format!("<w:{el}"))?;
    let rest = &xml[at..];
    let end = rest.find('>')?;
    // `<w:b>` is not `<w:bCs>`
    let next = rest.as_bytes().get(el.len() + 3)?;
    (matches!(next, b' ' | b'/' | b'>')).then(|| &rest[..end])
}

/// A toggle property (`<w:b/>`, `<w:b w:val="0"/>`) in `rpr`.
fn on_off(rpr: &str, el: &str) -> Option<bool> {
    let t = tag(rpr, el)?;
    Some(!matches!(attr(t, "w:val").as_deref(), Some("0" | "false" | "off")))
}

impl Styles {
    /// From the package's `styles.xml`, `fontTable.xml` and theme.
    pub(crate) fn read(source: &[u8]) -> Styles {
        let parts = ooxml_opc::unzip_parts(source).unwrap_or_default();
        let part = |name: &str| parts.iter().find(|(n, _)| n == name).map(|(_, b)| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
        let xml = part("word/styles.xml");
        let mut list = Vec::new();
        for block in xml.split("<w:style ").skip(1) {
            let head = &block[..block.find('>').unwrap_or(block.len())];
            if attr(head, "w:type").as_deref() != Some("paragraph") {
                continue;
            }
            let Some(id) = attr(head, "w:styleId") else { continue };
            let block = &block[..block.find("</w:style>").unwrap_or(block.len())];
            let has = |el: &str| tag(block, el).is_some();
            if !has("qFormat") && (has("hidden") || has("semiHidden")) {
                continue;
            }
            let rpr = block.split("<w:rPr>").nth(1).map(|r| &r[..r.find("</w:rPr>").unwrap_or(r.len())]).unwrap_or("");
            let name = tag(block, "name").and_then(|t| attr(t, "w:val")).unwrap_or_else(|| id.clone());
            let priority = tag(block, "uiPriority").and_then(|t| attr(t, "w:val")).and_then(|v| v.parse::<i64>().ok()).unwrap_or(99);
            let info = StyleInfo {
                id,
                name,
                size: tag(rpr, "sz").and_then(|t| attr(t, "w:val")).and_then(|v| v.parse().ok()),
                bold: on_off(rpr, "b"),
                italic: on_off(rpr, "i").unwrap_or(false),
                color: tag(rpr, "color").and_then(|t| attr(t, "w:val")).filter(|c| c != "auto"),
            };
            list.push((priority, info));
        }
        // a stable sort, as `Array.prototype.sort`
        list.sort_by_key(|(p, _)| *p);
        let paragraph = list.into_iter().map(|(_, s)| s).collect();
        // the run defaults: their font (a theme font through the theme) and size
        let defaults = xml.split("<w:rPrDefault>").nth(1).map(|d| &d[..d.find("</w:rPrDefault>").unwrap_or(d.len())]).unwrap_or("");
        let fonts_tag = tag(defaults, "rFonts").unwrap_or("");
        let theme = part("word/theme/theme1.xml");
        let theme_font = |which: &str| {
            let t = theme.split(&format!("<a:{which}Font>")).nth(1)?;
            let latin = &t[t.find("<a:latin ")?..];
            attr(&latin[..latin.find('>')?], "typeface")
        };
        let family = attr(fonts_tag, "w:ascii")
            .or_else(|| attr(fonts_tag, "w:asciiTheme").and_then(|t| theme_font(if t.starts_with("major") { "major" } else { "minor" })))
            .unwrap_or_else(|| "Calibri".to_owned());
        let size = tag(defaults, "sz").and_then(|t| attr(t, "w:val")).and_then(|v| v.parse::<f32>().ok()).map_or(11.0, |hp| hp / 2.0);
        let table = part("word/fontTable.xml");
        let fonts = table.split("<w:font ").skip(1).filter_map(|f| attr(&f[..f.find('>').unwrap_or(f.len())], "w:name")).collect();
        Styles { paragraph: Arc::new(paragraph), fonts: Arc::new(fonts), family, size, resolver: docx_edit::StyleResolver::from_parts(&parts).ok() }
    }
}

/// `resolveImageLayoutAttrs`: a wrap target's attributes over a picture's
/// (a `None` value removes the attribute).
pub(crate) fn wrap_attrs(target: &str, attrs: &BTreeMap<String, Any>) -> Vec<(&'static str, Option<Any>)> {
    let position = || {
        let p = attrs.get("position");
        if field(p, "horizontal").is_some() && field(p, "vertical").is_some() {
            return p.cloned();
        }
        // the inline picture's own place isn't read from the page here: it
        // floats from the column's and paragraph's corner
        let side = |rel: &str| Any::Map(Arc::new(HashMap::from([("relativeTo".to_owned(), Any::from(rel)), ("posOffset".to_owned(), Any::Number(0.0))])));
        Some(Any::Map(Arc::new(HashMap::from([("horizontal".to_owned(), side("column")), ("vertical".to_owned(), side("paragraph"))]))))
    };
    let s = |v: &str| Some(Any::from(v));
    match target {
        "inline" => vec![
            ("wrapType", s("inline")),
            ("displayMode", s("inline")),
            ("cssFloat", s("none")),
            ("wrapText", None),
            ("position", None),
            ("distTop", None),
            ("distBottom", None),
            ("distLeft", None),
            ("distRight", None),
        ],
        "squareLeft" | "squareRight" => {
            let side = if target == "squareLeft" { "left" } else { "right" };
            let kind = match s_of(attrs.get("wrapType")).as_deref() {
                Some(k @ ("tight" | "through")) => k.to_owned(),
                _ => "square".to_owned(),
            };
            vec![("wrapType", Some(Any::from(kind))), ("displayMode", s("float")), ("cssFloat", s(side)), ("wrapText", s(if side == "left" { "right" } else { "left" })), ("position", position())]
        }
        "topAndBottom" => vec![("wrapType", s("topAndBottom")), ("displayMode", s("block")), ("cssFloat", s("none")), ("wrapText", s("bothSides")), ("position", position())],
        // behind, inFront
        _ => vec![("wrapType", s(target)), ("displayMode", s("float")), ("cssFloat", s("none")), ("wrapText", s("bothSides")), ("position", position())],
    }
}

/// The wrap menu's value for a picture (`A1`).
pub(crate) fn wrap_of(attrs: &BTreeMap<String, Any>) -> String {
    let (mode, float) = (s_of(attrs.get("displayMode")), s_of(attrs.get("cssFloat")));
    match (mode.as_deref(), float.as_deref()) {
        (Some("float"), Some("left")) => "wrapRight".to_owned(),
        (Some("float"), Some("right")) => "wrapLeft".to_owned(),
        _ => s_of(attrs.get("wrapType")).unwrap_or_else(|| "inline".to_owned()),
    }
}

/// `toolbarValueToLayoutTarget`.
fn layout_target(value: &str) -> Option<&'static str> {
    Some(match value {
        "inline" => "inline",
        "square" | "tight" | "through" | "wrapRight" => "squareLeft",
        "wrapLeft" => "squareRight",
        "topAndBottom" => "topAndBottom",
        "behind" => "behind",
        "inFront" => "inFront",
        _ => return None,
    })
}

/// A picture's CSS transform after a turn or flip (`Pg`); `None` when it
/// comes back to none.
pub(crate) fn transformed(current: &str, action: &str) -> Option<String> {
    let mut deg = current
        .split("rotate(")
        .nth(1)
        .and_then(|r| r.split("deg)").next())
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(0.0);
    let (mut fx, mut fy) = (current.contains("scaleX(-1)"), current.contains("scaleY(-1)"));
    match action {
        "rotateCW" => deg = (deg + 90.0) % 360.0,
        "rotateCCW" => deg = (deg - 90.0 + 360.0) % 360.0,
        "flipH" => fx = !fx,
        _ => fy = !fy,
    }
    let mut out = Vec::new();
    if deg != 0.0 {
        out.push(format!("rotate({}deg)", crate::office::ui::field::fmt_number(deg)));
    }
    if fx {
        out.push("scaleX(-1)".to_owned());
    }
    if fy {
        out.push("scaleY(-1)".to_owned());
    }
    (!out.is_empty()).then(|| out.join(" "))
}

/// A cell border (`borderSpecRef`): style, eighths of a point, colour.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Border {
    pub size: u32,
    pub color: String,
}

impl Default for Border {
    fn default() -> Self {
        Border { size: 4, color: "000000".to_owned() }
    }
}

impl Border {
    fn any(&self, style: &str, size: u32) -> Any {
        Any::Map(Arc::new(HashMap::from([
            ("style".to_owned(), Any::from(style)),
            ("size".to_owned(), Any::Number(f64::from(size))),
            ("color".to_owned(), Any::Map(Arc::new(HashMap::from([("rgb".to_owned(), Any::from(self.color.as_str()))])))),
        ])))
    }
}

/// The sides a borders choice sets, and the border each gets (the
/// handler's `d`, all six; `borderNone` is `none` at 0 in black).
pub(crate) fn border_sides(choice: &str, current: &Border) -> HashMap<String, Any> {
    let all = ["top", "bottom", "left", "right", "insideH", "insideV"];
    let sides: Vec<&str> = match choice {
        "all" | "none" => all.to_vec(),
        "outside" => vec!["top", "bottom", "left", "right"],
        "inside" => vec!["insideH", "insideV"],
        one => vec![one],
    };
    let value = if choice == "none" { Border::default().any("none", 0) } else { current.any("single", current.size) };
    sides.into_iter().map(|s| (s.to_owned(), value.clone())).collect()
}

impl Word {
    /// The context every edit runs in: tracked while suggesting.
    pub(super) fn ctx(&self) -> EditCtx {
        let c = local();
        if self.mode == EditMode::Suggesting { c.suggesting() } else { c }
    }

    pub(super) fn styles(&mut self) -> &Styles {
        if self.styles.is_none() {
            self.styles = Some(Styles::read(&self.source));
        }
        self.styles.as_ref().expect("read")
    }

    /// The selection's formats: the marks of the text the caret follows
    /// (a selection's first character), its paragraph's properties.
    pub(super) fn formats(&mut self) -> Formats {
        let (family, size) = {
            let s = self.styles();
            (s.family.clone(), s.size)
        };
        let mut f = Formats { family, size, ..Formats::default() };
        let s = if self.anchor.offset <= self.head.offset { self.anchor.clone() } else { self.head.clone() };
        let Ok(segments) = self.layout.engine.doc().story_segments(&s.story) else { return f };
        // the text a caret follows, as typing continues it; a selection's
        // first character (the one before it is outside: ⌘B on a word
        // left the B unlit)
        let after = if self.collapsed() { s.offset.max(1) } else { s.offset + 1 };
        let mut at = 0u32;
        let mut attrs = None;
        for seg in &segments {
            let n = match &seg.content {
                SegmentContent::Text(t) => t.encode_utf16().count() as u32,
                _ => 1,
            };
            if at < after && after <= at + n {
                attrs = Some(&seg.attributes);
                // one picture selected
                if !self.collapsed()
                    && self.head.story == self.anchor.story
                    && self.anchor.offset.abs_diff(self.head.offset) == 1
                    && let SegmentContent::OtherEmbed { kind, payload } = &seg.content
                    && kind == "image"
                {
                    f.image = Some(wrap_of(payload));
                }
                break;
            }
            at += n;
        }
        if let Some(a) = attrs {
            let on = |k: &str| matches!(a.get(k), Some(Any::Bool(true)));
            let set = |k: &str| a.get(k).is_some_and(|v| !matches!(v, Any::Null | Any::Bool(false)));
            f.bold = on("bold");
            f.italic = on("italic");
            f.underline = set("underline");
            f.strike = set("strike");
            f.superscript = on("superscript");
            f.subscript = on("subscript");
            if let Some(hp) = n_of(field(a.get("fontSize"), "size")) {
                f.size = hp as f32 / 2.0;
            }
            if let Some(fam) = s_of(field(a.get("fontFamily"), "ascii")) {
                f.family = fam;
            }
            let color = a.get("textColor");
            f.color = s_of(field(color, "rgb")).or_else(|| {
                let slot = s_of(field(color, "themeColor"))?;
                let slot = match slot.as_str() {
                    "text1" => "dk1".to_owned(),
                    "background1" => "lt1".to_owned(),
                    "text2" => "dk2".to_owned(),
                    "background2" => "lt2".to_owned(),
                    s => s.to_owned(),
                };
                crate::office::ui::color::THEME.iter().find(|(k, _)| *k == slot).map(|(_, h)| (*h).to_owned())
            });
            f.highlight = s_of(field(a.get("highlight"), "color")).map(|c| highlight_hex(&c).map_or(c.trim_start_matches('#').to_uppercase(), str::to_owned));
            let link = a.get("hyperlink");
            f.link = s_of(field(link, "href")).map(|h| (h, s_of(field(link, "tooltip"))));
        }
        if let Some((_, p)) = self.head_para() {
            let props = segments.iter().find_map(|s| match &s.content {
                SegmentContent::Pilcrow(props) if props.para_id == p.para_id => Some(props.values.clone()),
                _ => None,
            });
            if let Some(v) = props {
                if let Some(st) = s_of(v.get("pStyle")) {
                    f.style = st;
                }
                if let Some(al) = s_of(v.get("alignment")).filter(|a| matches!(a.as_str(), "left" | "center" | "right" | "both")) {
                    f.align = al;
                }
                f.line_spacing = n_of(v.get("lineSpacing"));
                f.indent_left = n_of(v.get("indentLeft")).unwrap_or(0.0);
                f.list = v.get("numPr").and_then(save::num_id).map(|n| (n, n_of(field(v.get("numPr"), "ilvl")).unwrap_or(0.0) as u32));
            }
        }
        let story = self.head.story.clone();
        if !self.collapsed() {
            let (_, a, b) = self.range();
            f.text = self.index().text_between(&spot(&story, a), &spot(&story, b));
        }
        if f.link.is_some()
            && let Some((a, b)) = self.link_at()
        {
            f.link_text = self.index().text_between(&spot(&story, a), &spot(&story, b));
        }
        f
    }

    /// The selection, or the word at the caret (a format with no selection).
    fn target(&mut self) -> Option<(String, u32, u32)> {
        match self.range() {
            (s, a, b) if a < b => Some((s, a, b)),
            _ => self.word_at(),
        }
    }

    /// The selection's paragraphs.
    fn selected(&mut self) -> ParaSelector {
        let (story, a, b) = self.range();
        ParaSelector::Range(StoryRange::new(&story, a, b.max(a)))
    }

    pub(super) fn format(&mut self, f: Fmt) -> Result<(), String> {
        let para = |w: &mut Word, delta: ParaAttrDelta| {
            let sel = w.selected();
            w.layout.engine.doc().set_paragraph_attrs(&w.ctx(), &sel, &delta).map(drop).map_err(|e| e.to_string())
        };
        match f {
            Fmt::Bullets => return self.toggle_list(1),
            Fmt::Numbers => return self.toggle_list(2),
            Fmt::Indent(more) => return self.indent(more),
            Fmt::Align(a) => return para(self, ParaAttrDelta { alignment: Patch::Set(a.to_owned()), ..Default::default() }),
            Fmt::LineSpacing(t) => return para(self, ParaAttrDelta { line_spacing: Patch::Set(f64::from(t)), line_spacing_rule: Patch::Set("auto".to_owned()), ..Default::default() }),
            _ => {}
        }
        let Some((story, a, b)) = self.target() else { return Ok(()) };
        let range = StoryRange::new(&story, a, b);
        let ctx = self.ctx();
        let doc = self.layout.engine.doc();
        let toggle = |k| doc.toggle_format(&ctx, range.clone(), k).map(drop);
        match f {
            Fmt::Bold => toggle(SimpleFormat::Bold),
            Fmt::Italic => toggle(SimpleFormat::Italic),
            Fmt::Underline => toggle(SimpleFormat::Underline),
            Fmt::Strike => toggle(SimpleFormat::Strike),
            Fmt::Superscript => toggle(SimpleFormat::Superscript),
            Fmt::Subscript => toggle(SimpleFormat::Subscript),
            Fmt::Clear => doc.clear_formatting(&ctx, range.clone()).map(drop),
            Fmt::Size(up) => {
                let size = self.formats().size;
                let next = if up { crate::office::ui::select::size_up(size) } else { crate::office::ui::select::size_down(size) };
                let delta = InlineFormatDelta { font_size: Patch::Set(f64::from(next)), ..Default::default() };
                self.layout.engine.doc().format_range(&ctx, range, &delta).map(drop)
            }
            _ => Ok(()),
        }
        .map_err(|e| e.to_string())
    }

    /// A run format over the selection (or the caret's word).
    pub(super) fn format_delta(&mut self, delta: InlineFormatDelta) -> Result<(), String> {
        let Some((story, a, b)) = self.target() else { return Ok(()) };
        self.layout.engine.doc().format_range(&self.ctx(), StoryRange::new(&story, a, b), &delta).map(drop).map_err(|e| e.to_string())
    }

    /// The paragraph style (`applyParagraphStyle`), the runs keeping their
    /// own formatting.
    pub(super) fn restyle(&mut self, id: &str) -> Result<(), String> {
        let sel = self.selected();
        let ctx = self.ctx();
        self.styles();
        let resolver = self.styles.as_ref().and_then(|s| s.resolver.as_ref());
        let doc = self.layout.engine.doc();
        match resolver {
            Some(r) => doc.restyle_paragraphs(&ctx, &sel, id, r),
            None => doc.restyle_paragraphs(&ctx, &sel, id, &docx_edit::StyleResolver::default()),
        }
        .map(drop)
        .map_err(|e| e.to_string())
    }

    /// Each selected paragraph: its index entry and its properties.
    fn selected_paras(&mut self) -> Vec<(super::super::index::Para, BTreeMap<String, Any>)> {
        let (story, a, b) = self.range();
        let props: HashMap<String, BTreeMap<String, Any>> = self
            .layout
            .engine
            .doc()
            .story_segments(&story)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| match s.content {
                SegmentContent::Pilcrow(p) => Some((p.para_id.to_string(), p.values)),
                _ => None,
            })
            .collect();
        self.index()
            .paras
            .iter()
            .filter(|p| p.story == story && p.start <= b && p.start + p.len >= a)
            .map(|p| (p.clone(), props.get(&p.para_id).cloned().unwrap_or_default()))
            .collect()
    }

    fn set_para(&self, p: &super::super::index::Para, delta: &ParaAttrDelta) -> Result<(), String> {
        let one = ParaSelector::Range(StoryRange::new(&p.story, p.start, p.start));
        self.layout.engine.doc().set_paragraph_attrs(&self.ctx(), &one, delta).map(drop).map_err(|e| e.to_string())
    }

    /// The list buttons (`f2`): list `num` (1 bullets, 2 numbers) on each
    /// selected paragraph at its own level, or off when the caret's
    /// paragraph is in it already.
    pub(super) fn toggle_list(&mut self, num: u32) -> Result<(), String> {
        let off = self.formats().list.map(|l| l.0) == Some(num);
        for (p, props) in self.selected_paras() {
            let mut d = ParaAttrDelta::default();
            if off {
                for k in ["numPr", "listIsBullet", "listNumFmt", "listMarker"] {
                    d.other.insert(k.to_owned(), None);
                }
            } else {
                let lvl = n_of(field(props.get("numPr"), "ilvl")).unwrap_or(0.0) as u32;
                d.other.insert("numPr".to_owned(), Some(save::num_pr(num, lvl)));
                d.other.insert("listIsBullet".to_owned(), Some(Any::Bool(num == 1)));
                d.other.insert("listNumFmt".to_owned(), if num == 1 { None } else { Some(Any::from("decimal")) });
                d.other.insert("listMarker".to_owned(), None);
            }
            self.set_para(&p, &d)?;
        }
        Ok(())
    }

    /// Indent and outdent (`g2`): a list paragraph's level one in or out
    /// (out of its list from the first level), else each paragraph's left
    /// indent by 720 twips.
    pub(super) fn indent(&mut self, more: bool) -> Result<(), String> {
        if let Some((num, lvl)) = self.formats().list {
            let Some((_, p)) = self.head_para() else { return Ok(()) };
            let mut d = ParaAttrDelta { indent_left: Patch::Clear, indent_first_line: Patch::Clear, hanging_indent: Patch::Clear, ..Default::default() };
            match (more, lvl) {
                (true, l) if l < 8 => drop(d.other.insert("numPr".to_owned(), Some(save::num_pr(num, l + 1)))),
                (false, l) if l > 0 => drop(d.other.insert("numPr".to_owned(), Some(save::num_pr(num, l - 1)))),
                (false, _) => {
                    for k in ["numPr", "listIsBullet", "listNumFmt", "listMarker"] {
                        d.other.insert(k.to_owned(), None);
                    }
                }
                _ => return Ok(()),
            }
            return self.set_para(&p, &d);
        }
        for (p, props) in self.selected_paras() {
            let left = n_of(props.get("indentLeft")).unwrap_or(0.0);
            let next = if more { left + 720.0 } else { (left - 720.0).max(0.0) };
            let d = ParaAttrDelta { indent_left: if next > 0.0 { Patch::Set(next) } else { Patch::Clear }, ..Default::default() };
            self.set_para(&p, &d)?;
        }
        Ok(())
    }

    /// The link run around the caret (`Ia`): strictly inside first, then
    /// at an edge; its whole extent over neighbours of the same address.
    fn link_at(&mut self) -> Option<(u32, u32)> {
        let head = self.head.clone();
        let segments = self.layout.engine.doc().story_segments(&head.story).ok()?;
        let mut runs = Vec::new();
        let mut at = 0u32;
        for s in &segments {
            let n = match &s.content {
                SegmentContent::Text(t) => t.encode_utf16().count() as u32,
                _ => 1,
            };
            runs.push((at, at + n, s_of(field(s.attributes.get("hyperlink"), "href"))));
            at += n;
        }
        let o = head.offset;
        let i = runs.iter().position(|r| r.2.is_some() && o > r.0 && o < r.1).or_else(|| runs.iter().position(|r| r.2.is_some() && (o == r.0 || o == r.1)))?;
        let href = runs[i].2.clone();
        let (mut a, mut b) = (i, i);
        while a > 0 && runs[a - 1].1 == runs[a].0 && runs[a - 1].2 == href {
            a -= 1;
        }
        while b + 1 < runs.len() && runs[b].1 == runs[b + 1].0 && runs[b + 1].2 == href {
            b += 1;
        }
        Some((runs[a].0, runs[b].1))
    }

    /// The Hyperlink dialog's link (`setHyperlink`): on the selection, or
    /// the link being edited; a display text given for a caret (or an
    /// edit) goes in first and takes the link.
    pub(super) fn set_link(&mut self, l: &Link) -> Result<(), String> {
        let (story, a, b) = self.range();
        let existing = if l.edit { self.link_at() } else { None };
        let (mut s, mut e) = existing.unwrap_or((a, b));
        let ctx = self.ctx();
        let doc = self.layout.engine.doc();
        if let Some(t) = l.text.as_deref().filter(|t| !t.is_empty())
            && (s == e || existing.is_some())
        {
            if existing.is_some() && s < e {
                doc.delete_range(&ctx, StoryRange::new(&story, s, e)).map_err(|x| x.to_string())?;
            }
            doc.insert_text(&ctx, Position::new(&story, s), t, docx_edit::FormatPolicy::Inherit).map_err(|x| x.to_string())?;
            e = s + t.encode_utf16().count() as u32;
        }
        if s == e {
            return Ok(());
        }
        let tooltip = l.tooltip.as_deref().map_or(Any::Null, Any::from);
        let value = Any::Map(Arc::new(HashMap::from([("href".to_owned(), Any::from(l.href.as_str())), ("tooltip".to_owned(), tooltip), ("rId".to_owned(), Any::Null)])));
        doc.set_hyperlink(&ctx, StoryRange::new(&story, s, e), Some(value)).map_err(|x| x.to_string())?;
        s = e;
        self.set_caret(spot(&story, s));
        Ok(())
    }

    /// The link taken off (`removeHyperlink`): the selection's, or the
    /// caret's whole link.
    pub(super) fn unlink(&mut self) -> Result<(), String> {
        let (story, a, b) = self.range();
        let Some((s, e)) = (if a < b { Some((a, b)) } else { self.link_at() }) else { return Ok(()) };
        self.layout.engine.doc().set_hyperlink(&self.ctx(), StoryRange::new(&story, s, e), None).map(drop).map_err(|x| x.to_string())
    }

    /// The caret's cell where it is now: its table (by the table's place
    /// in its story) and its row and grid column (a story's name keeps
    /// where the cell was made).
    pub(super) fn cell(&self) -> Option<(CellLoc, BTreeMap<String, Any>)> {
        let story = &self.head.story;
        let (parent, _, _, _) = cell_of(story)?;
        let segments = self.layout.engine.doc().story_segments(&parent).ok()?;
        let mut ordinal = 0u32;
        for s in segments {
            let SegmentContent::OtherEmbed { kind, payload } = s.content else { continue };
            if kind != "table" {
                continue;
            }
            if let Some(Any::Array(rows)) = payload.get("rows") {
                for (r, row) in rows.iter().enumerate() {
                    let Some(Any::Array(cells)) = field(Some(row), "cells") else { continue };
                    let mut col = 0u32;
                    for c in cells.iter() {
                        if s_of(field(Some(c), "story")).as_deref() == Some(story) {
                            return Some((CellLoc::new(&parent, ordinal, r as u32, col), payload));
                        }
                        col += n_of(field(field(Some(c), "tcPr"), "colspan")).unwrap_or(1.0) as u32;
                    }
                }
            }
            ordinal += 1;
        }
        None
    }

    pub(super) fn cell_info(&self) -> Option<CellInfo> {
        let (at, payload) = self.cell()?;
        let rows = match payload.get("rows") {
            Some(Any::Array(r)) => r.clone(),
            _ => return None,
        };
        let cols = match payload.get("grid") {
            Some(Any::Array(g)) => g.len() as u32,
            _ => 0,
        };
        let cell = rows.get(at.row as usize).and_then(|r| match field(Some(r), "cells") {
            Some(Any::Array(cells)) => cells.iter().find(|c| s_of(field(Some(c), "story")).as_deref() == Some(self.head.story.as_str())).cloned(),
            _ => None,
        });
        let tc = cell.as_ref().and_then(|c| field(Some(c), "tcPr")).cloned();
        let span = |k: &str| n_of(field(tc.as_ref(), k)).unwrap_or(1.0);
        let border = ["top", "left", "bottom", "right"].iter().find_map(|s| s_of(field(field(field(field(tc.as_ref(), "borders"), s), "color"), "rgb")));
        Some(CellInfo {
            rows: rows.len() as u32,
            cols,
            multi: self.whole_table && (rows.len() > 1 || cols > 1),
            can_split: span("colspan") > 1.0 || span("rowspan") > 1.0,
            fill: s_of(field(tc.as_ref(), "backgroundColor")),
            border,
            justification: s_of(field(payload.get("tblPr"), "justification")),
            width: n_of(field(payload.get("tblPr"), "width")),
            width_type: s_of(field(payload.get("tblPr"), "widthType")),
        })
    }

    /// The Table group's command; the story it changed.
    pub(super) fn table_op(&mut self, op: TableOp) -> Result<String, String> {
        let Some((at, _)) = self.cell() else { return Err("Place the cursor in a table first.".to_owned()) };
        let info = self.cell_info().unwrap_or_default();
        let parent = at.story.clone();
        let table = at.table();
        let range = if self.whole_table {
            TableRange::new(CellLoc::new(&parent, at.table_index, 0, 0), CellLoc::new(&parent, at.table_index, info.rows.saturating_sub(1), info.cols.saturating_sub(1)))
        } else {
            TableRange::cell(at.clone())
        };
        let ctx = self.ctx();
        let doc = self.layout.engine.doc();
        let e = |x: docx_edit::OpError| x.to_string();
        let mut gone = false;
        match op {
            TableOp::RowAbove => drop(doc.insert_row(&ctx, &at, false).map_err(e)?),
            TableOp::RowBelow => drop(doc.insert_row(&ctx, &at, true).map_err(e)?),
            TableOp::ColumnLeft => drop(doc.insert_column(&ctx, &at, false).map_err(e)?),
            TableOp::ColumnRight => drop(doc.insert_column(&ctx, &at, true).map_err(e)?),
            TableOp::Merge => {
                if !info.multi {
                    return Err("Select more than one cell.".to_owned());
                }
                doc.merge_cells(&ctx, &range).map_err(e)?;
            }
            TableOp::Split => {
                if !info.can_split {
                    return Err("This cell can't be split.".to_owned());
                }
                doc.split_cell(&ctx, &at).map_err(e)?;
            }
            TableOp::SelectTable => {
                self.whole_table = true;
                return Ok(parent);
            }
            TableOp::DeleteRow => {
                if info.rows <= 1 {
                    return Err("A table needs at least one row.".to_owned());
                }
                gone = doc.delete_row(&ctx, &range).map_err(e)?.deleted_table;
            }
            TableOp::DeleteColumn => {
                if info.cols <= 1 {
                    return Err("A table needs at least one column.".to_owned());
                }
                gone = doc.delete_column(&ctx, &range).map_err(e)?.deleted_table;
            }
            TableOp::DeleteTable => {
                doc.delete_table(&ctx, &table).map_err(e)?;
                gone = true;
            }
            TableOp::Borders(choice) => drop(doc.set_cell_borders(&ctx, &range, &border_sides(choice, &self.border)).map_err(e)?),
            TableOp::BorderColor(c) => {
                self.border.color = c;
                drop(doc.set_cell_borders(&ctx, &range, &border_sides("all", &self.border)).map_err(e)?)
            }
            TableOp::BorderWidth(size) => {
                self.border.size = size;
                drop(doc.set_cell_borders(&ctx, &range, &border_sides("all", &self.border)).map_err(e)?)
            }
            TableOp::Fill(c) => drop(doc.set_cell_shading(&ctx, &range, c.as_deref()).map_err(e)?),
            TableOp::Justify(j) => drop(doc.set_table_properties(&ctx, &table, &HashMap::from([("justification".to_owned(), Any::from(j))])).map_err(e)?),
            TableOp::Properties { width, width_type, justification } => {
                let props = HashMap::from([
                    ("width".to_owned(), width.map_or(Any::Null, Any::Number)),
                    ("widthType".to_owned(), Any::from(width_type.as_str())),
                    ("justification".to_owned(), Any::from(justification.as_str())),
                ]);
                drop(doc.set_table_properties(&ctx, &table, &props).map_err(e)?)
            }
        }
        self.whole_table = false;
        if gone {
            // the caret back to the table's story, where the table was
            self.set_caret(spot(&parent, 0));
        } else if let Some(story) = self.story_at(&table, at.row as usize, at.column) {
            // the caret stays with its cell
            self.set_caret(spot(&story, 0));
        }
        Ok(parent)
    }

    /// The story of the cell at `row` and grid column `col` (or the last
    /// there is) of `table`.
    fn story_at(&self, table: &TableLocator, row: usize, col: u32) -> Option<String> {
        let payload = self.layout.engine.doc().table_payload(table).ok()?;
        let Some(Any::Array(rows)) = payload.get("rows") else { return None };
        let r = rows.get(row.min(rows.len().checked_sub(1)?))?;
        let Some(Any::Array(cells)) = field(Some(r), "cells") else { return None };
        let mut at = 0u32;
        let mut last = None;
        for c in cells.iter() {
            last = s_of(field(Some(c), "story"));
            at += n_of(field(field(Some(c), "tcPr"), "colspan")).unwrap_or(1.0) as u32;
            if at > col {
                break;
            }
        }
        last
    }

    /// The Image group's command on the selected picture.
    pub(super) fn picture(&mut self, op: Picture) -> Result<String, String> {
        let (story, a, b) = self.range();
        let segments = self.layout.engine.doc().story_segments(&story).map_err(|e| e.to_string())?;
        let mut at = 0u32;
        let mut attrs = None;
        for s in segments {
            let n = match &s.content {
                SegmentContent::Text(t) => t.encode_utf16().count() as u32,
                _ => 1,
            };
            if at == a && b == a + 1
                && let SegmentContent::OtherEmbed { kind, payload } = s.content
                && kind == "image"
            {
                attrs = Some(payload);
                break;
            }
            at += n;
        }
        let Some(attrs) = attrs else { return Err("Select an image first.".to_owned()) };
        let entries: Vec<(String, Any)> = match op {
            Picture::Wrap(value) => {
                let Some(target) = layout_target(value) else { return Ok(story) };
                wrap_attrs(target, &attrs).into_iter().map(|(k, v)| (k.to_owned(), v.unwrap_or(Any::Null))).collect()
            }
            Picture::Transform(action) => {
                let now = s_of(attrs.get("transform")).unwrap_or_default();
                vec![("transform".to_owned(), transformed(&now, action).map_or(Any::Null, Any::from))]
            }
        };
        self.layout.engine.doc().set_embed_attrs(&self.ctx(), Position::new(&story, a), entries).map_err(|e| e.to_string())?;
        Ok(story)
    }

    /// A press on a page: the picture under it, as `(page pixels)` → its
    /// body position.
    pub(super) fn picture_at(&self, page: usize, x: f32, y: f32) -> Option<i64> {
        let (x, y) = (f64::from(x), f64::from(y));
        self.list(|l| {
            l.pages.get(page)?.primitives.iter().rev().find_map(|p| match p {
                Primitive::Image(i) => {
                    let (ix, iy, iw, ih) = (i.x.as_f64()?, i.y.as_f64()?, i.w.as_f64()?, i.h.as_f64()?);
                    (x >= ix && x <= ix + iw && y >= iy && y <= iy + ih).then_some(i.attrs.doc_start).flatten()
                }
                _ => None,
            })
        })
        .flatten()
    }

    /// The selected picture's box on its page (page pixels).
    pub(super) fn picture_box(&self, pm: i64) -> Option<(usize, f32, f32, f32, f32)> {
        self.list(|l| {
            l.pages.iter().enumerate().find_map(|(i, p)| {
                p.primitives.iter().find_map(|q| match q {
                    Primitive::Image(im) if im.attrs.doc_start == Some(pm) => Some((i, im.x.as_f64()? as f32, im.y.as_f64()? as f32, im.w.as_f64()? as f32, im.h.as_f64()? as f32)),
                    _ => None,
                })
            })
        })
        .flatten()
    }

    /// The whole table's cells, as body ranges (the selection "Select
    /// entire table" draws).
    pub(super) fn table_ranges(&mut self) -> Vec<(i64, i64)> {
        let Some((at, _)) = self.cell() else { return Vec::new() };
        let Ok(payload) = self.layout.engine.doc().table_payload(&at.table()) else { return Vec::new() };
        let Some(Any::Array(rows)) = payload.get("rows") else { return Vec::new() };
        let stories: Vec<String> = rows
            .iter()
            .filter_map(|r| match field(Some(r), "cells") {
                Some(Any::Array(c)) => Some(c.iter().filter_map(|c| s_of(field(Some(c), "story"))).collect::<Vec<_>>()),
                _ => None,
            })
            .flatten()
            .collect();
        let index = self.index();
        stories
            .iter()
            .filter_map(|s| {
                let first = index.paras.iter().find(|p| &p.story == s)?;
                let last = index.paras.iter().rev().find(|p| &p.story == s)?;
                Some((first.pm, last.pm + i64::from(last.len)))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms_turn_and_flip_as_the_webs() {
        assert_eq!(transformed("", "rotateCW").as_deref(), Some("rotate(90deg)"));
        assert_eq!(transformed("rotate(90deg)", "rotateCCW"), None);
        assert_eq!(transformed("", "rotateCCW").as_deref(), Some("rotate(270deg)"));
        assert_eq!(transformed("rotate(270deg)", "flipH").as_deref(), Some("rotate(270deg) scaleX(-1)"));
        assert_eq!(transformed("scaleX(-1)", "flipV").as_deref(), Some("scaleX(-1) scaleY(-1)"));
        assert_eq!(transformed("scaleY(-1)", "flipV"), None);
    }

    #[test]
    fn borders_expand_the_current_border_to_their_sides() {
        let b = Border { size: 12, color: "FF0000".into() };
        let all = border_sides("all", &b);
        assert_eq!(all.len(), 6);
        assert_eq!(border_sides("outside", &b).len(), 4);
        let inside = border_sides("inside", &b);
        assert!(inside.contains_key("insideH") && inside.contains_key("insideV"));
        let top = border_sides("top", &b);
        assert_eq!(n_of(field(top.get("top"), "size")), Some(12.0));
        assert_eq!(s_of(field(field(top.get("top"), "color"), "rgb")).as_deref(), Some("FF0000"));
        assert_eq!(s_of(field(border_sides("none", &b).get("left"), "style")).as_deref(), Some("none"));
    }

    #[test]
    fn wraps_take_the_webs_attributes() {
        let none = BTreeMap::new();
        let left = wrap_attrs("squareLeft", &none);
        assert!(left.contains(&("cssFloat", Some(Any::from("left")))) && left.contains(&("wrapText", Some(Any::from("right")))));
        let mut floated = BTreeMap::new();
        for (k, v) in left.into_iter().filter_map(|(k, v)| Some((k, v?))) {
            floated.insert(k.to_owned(), v);
        }
        assert_eq!(wrap_of(&floated), "wrapRight");
        assert_eq!(layout_target("wrapLeft"), Some("squareRight"));
        let inline = wrap_attrs("inline", &floated);
        assert!(inline.contains(&("position", None)));
        assert_eq!(wrap_of(&BTreeMap::new()), "inline");
    }

    #[test]
    fn the_style_list_reads_the_packages_styles() {
        let s = Styles::read(&super::super::super::fixture::paragraphs(&["a"]));
        let ids: Vec<&str> = s.paragraph.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"Normal") && ids.contains(&"Heading1"), "{ids:?}");
        let h = s.paragraph.iter().find(|p| p.id == "Heading1").unwrap();
        assert_eq!((h.size, h.bold, h.color.as_deref()), (Some(32.0), Some(true), Some("1F3864")));
        assert_eq!((s.family.as_str(), s.size), ("Arial", 11.0));
    }
}
