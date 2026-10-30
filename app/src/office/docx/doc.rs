//! The Word document on the office worker ([`crate::office::session`]):
//! BetterOffice's editing engine, its pages drawn for the view, the caret
//! and the selection, and every edit.
//!
//! A document opens in two steps: the first page is laid out and drawn at
//! once, the rest of the document is laid out on the next tick. An edit
//! goes into the engine's stories and the engine lays out again from the
//! edited paragraph on (its resident pass), rebuilding the display lists
//! of the pages in view; a page whose list came out the same keeps its
//! drawing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use docx_edit::{EditCtx, FormatPolicy, InlineFormatDelta, MergeDirection, Patch, Position, SimpleFormat, StoryRange, UndoSession};
use docx_layout::display_list::{DisplayList, DisplayPage};
use docx_layout::hit::{self, VerticalDirection};

use super::draw::{self, Drawn};
use super::index::{Index, Spot, cell_of};
use super::layout::Layout;
use super::save;
use crate::office::session::Document;

/// Caret moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Move {
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp,
    PageDown,
}

/// Inline and paragraph formats.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Fmt {
    Bold,
    Italic,
    Underline,
    /// The font size up (`true`) or down a step.
    Size(bool),
    Bullets,
    Numbers,
}

pub(crate) enum Cmd {
    /// The pages in and near view at `zoom`: drawn when not yet drawn for
    /// this layout and zoom.
    View { pages: Vec<usize>, zoom: f32 },
    /// A click at (`x`, `y`) on page `page` (page pixels); `extend` moves
    /// only the selection's head; `clicks` 2 selects a word, 3 a paragraph.
    Click { page: usize, x: f32, y: f32, extend: bool, clicks: u8 },
    Move { to: Move, extend: bool },
    SelectAll,
    /// Typed text replaces the selection.
    Type(String),
    Backspace,
    Delete,
    Enter,
    Tab,
    Format(Fmt),
    /// A row above or below the caret's table row, or the row deleted.
    Row(Option<bool>),
    Undo,
    Redo,
    /// Find `query` (empty clears), and go to the match after (or before) the caret.
    Find { query: String, back: bool },
    /// The selection's text; `cut` deletes it.
    Copy { cut: bool },
    Paste(String),
    /// The next save goes to this file (a `.doc` saved as `.docx`).
    SaveTo(PathBuf),
    /// The document as HTML, for printing.
    Print,
}

/// The caret's and selection's boxes on their pages (page pixels).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Caret {
    /// (page, x, y, height)
    pub caret: Option<(usize, f32, f32, f32)>,
    /// (page, x, y, w, h)
    pub sel: Vec<(usize, f32, f32, f32, f32)>,
    /// Find's other matches.
    pub found: Vec<(usize, f32, f32, f32, f32)>,
    /// The page to bring into view (the caret moved).
    pub reveal: Option<(usize, f32)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Meta {
    /// Bumped by every relayout: pages drawn before it may be stale.
    pub rev: u64,
    /// Page sizes, page pixels.
    pub pages: Vec<(f32, f32)>,
    /// Only the first pages are laid out so far.
    pub partial: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    /// The selection's formats: bold, italic, underline, size (points).
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub size: f32,
    /// Find's matches and the current one (1-based; 0 none).
    pub found: (usize, usize),
    /// Tracked changes and comments: (author, kind, text).
    pub review: Vec<(String, String, String)>,
}

pub(crate) enum Out {
    Meta(Meta),
    /// Page `index` drawn for layout `rev` at the zoom it was asked at.
    Page { index: usize, rev: u64, zoom: f32, drawn: Arc<Drawn> },
    Caret(Caret),
    /// Picture bytes by (part, relationship id), once each.
    Media(Vec<((Option<String>, String), Arc<Vec<u8>>)>),
    Copied(String),
    /// The document as HTML to print.
    Print(String),
    Note(String),
}

pub(crate) struct Word {
    layout: Layout,
    undo: UndoSession,
    /// The `.docx` the document came from (a `.doc` converted).
    source: Vec<u8>,
    /// A `.doc`: saved as `.docx`.
    legacy: bool,
    rev: u64,
    index: Option<Index>,
    anchor: Spot,
    head: Spot,
    /// The x a run of up and down moves keeps.
    goal_x: Option<f64>,
    zoom: f32,
    want: Vec<usize>,
    /// What was drawn of each page: its list as drawn, the layout and the zoom.
    drawn: HashMap<usize, (Arc<DisplayPage>, u64, f32)>,
    /// Pictures already sent.
    media_sent: std::collections::HashSet<(Option<String>, String)>,
    media: Option<Vec<(String, Vec<u8>)>>,
    found: Vec<(Spot, Spot)>,
    found_at: usize,
    query: String,
    meta_sent: bool,
    /// The whole document laid out (after the first page).
    complete: bool,
    /// Edits since the open or the last save (the engine's own history can't tell).
    pub(crate) edited: bool,
    /// Where the next save goes, when not the file itself.
    target: Option<PathBuf>,
}

fn ctx() -> EditCtx {
    EditCtx::local("Rox", "1970-01-01T00:00:00Z")
}

fn spot(story: &str, offset: u32) -> Spot {
    Spot { story: story.to_owned(), offset }
}

impl Word {
    pub(crate) fn from_docx(bytes: Vec<u8>, legacy: bool) -> Result<Word, String> {
        let mut layout = Layout::open(&bytes)?;
        layout.first(1)?;
        let undo = UndoSession::new();
        undo.track(layout.engine.doc());
        let mut w = Word {
            layout,
            undo,
            source: bytes,
            legacy,
            rev: 1,
            index: None,
            anchor: spot("body", 0),
            head: spot("body", 0),
            goal_x: None,
            zoom: 1.0,
            want: vec![0],
            drawn: HashMap::new(),
            media_sent: Default::default(),
            media: None,
            found: Vec::new(),
            found_at: 0,
            query: String::new(),
            meta_sent: false,
            complete: false,
            edited: false,
            target: None,
        };
        // the caret starts in the first paragraph
        let first = w.index().paras.first().map(|p| spot(&p.story, p.start));
        if let Some(s) = first {
            w.set_caret(s);
        }
        Ok(w)
    }

    fn index(&mut self) -> &Index {
        if self.index.is_none() {
            self.index = Some(Index::build(self.layout.engine.doc()));
        }
        self.index.as_ref().expect("built")
    }

    fn list<T>(&self, f: impl FnOnce(&DisplayList) -> T) -> Option<T> {
        self.layout.engine.with_display_list(f)
    }

    fn meta(&mut self) -> Meta {
        let pages = self.layout.with_pages(|ps| ps.iter().map(|p| (p.width.as_f64().unwrap_or(816.0) as f32, p.height.as_f64().unwrap_or(1056.0) as f32)).collect()).unwrap_or_default();
        let (bold, italic, underline, size) = self.formats();
        let review = self.review();
        Meta {
            rev: self.rev,
            pages,
            partial: !self.complete,
            can_undo: self.undo.can_undo(),
            can_redo: self.undo.can_redo(),
            bold,
            italic,
            underline,
            size,
            found: (self.found.len(), if self.found.is_empty() { 0 } else { self.found_at + 1 }),
            review,
        }
    }

    /// The tracked changes and comments, in order, for the review list.
    fn review(&mut self) -> Vec<(String, String, String)> {
        self.index();
        let index = self.index.as_ref().expect("built");
        let doc = self.layout.engine.doc();
        let mut out = Vec::new();
        let text = |r: &docx_edit::LocRange| -> String {
            let Some(p) = index.paras.iter().find(|p| p.para_id == r.start.para) else { return String::new() };
            let (a, b) = (r.start.offset as usize, if r.end.para == r.start.para { r.end.offset as usize } else { p.text.len() });
            String::from_utf16_lossy(p.text.get(a.min(b)..b.min(p.text.len())).unwrap_or(&[]))
        };
        for c in doc.list_changes("body").unwrap_or_default() {
            let kind = match c.kind {
                docx_edit::ChangeKind::Insertion => "Inserted",
                docx_edit::ChangeKind::Deletion => "Deleted",
                _ => "Changed",
            };
            out.push((c.author.clone(), kind.to_owned(), text(&c.range)));
        }
        for c in &self.layout.comments {
            out.push((c["author"].as_str().unwrap_or("").to_owned(), "Comment".to_owned(), comment_text(&c["content"].to_string())));
        }
        out
    }

    /// Bold, italic, underline and size at the selection's start.
    fn formats(&self) -> (bool, bool, bool, f32) {
        let s = if self.anchor.offset <= self.head.offset { &self.anchor } else { &self.head };
        let Ok(segments) = self.layout.engine.doc().story_segments(&s.story) else { return (false, false, false, 11.0) };
        let mut at = 0u32;
        let mut attrs = None;
        for seg in &segments {
            let n = match &seg.content {
                docx_edit::SegmentContent::Text(t) => t.encode_utf16().count() as u32,
                _ => 1,
            };
            // the text the caret follows, as typing continues it
            if at < s.offset.max(1) && s.offset.max(1) <= at + n {
                attrs = Some(&seg.attributes);
                break;
            }
            at += n;
        }
        let Some(a) = attrs else { return (false, false, false, 11.0) };
        let on = |k: &str| matches!(a.get(k), Some(yrs::Any::Bool(true)));
        let underline = a.get("underline").is_some_and(|v| !matches!(v, yrs::Any::Null | yrs::Any::Bool(false)));
        // half-points
        let size = match a.get("fontSize") {
            Some(yrs::Any::Map(m)) => match m.get("size") {
                Some(yrs::Any::Number(n)) => *n as f32 / 2.0,
                Some(yrs::Any::BigInt(n)) => *n as f32 / 2.0,
                _ => 11.0,
            },
            _ => 11.0,
        };
        (on("bold"), on("italic"), underline, size)
    }

    /// Lays out again after an edit in `story` and redraws what changed.
    fn relayout(&mut self, story: &str, outs: &mut Vec<Out>) {
        if !self.complete {
            self.finish(outs);
        }
        let _scope = self.layout.fonts.enter();
        if let Err(e) = self.layout.engine.apply_and_layout(story, 0) {
            log::warn!("word: relayout: {e}");
            outs.push(Out::Note(format!("Couldn't lay the page out again: {e}")));
        }
        drop(_scope);
        self.rev += 1;
        self.edited = true;
        self.index = None;
        // matches move with the text
        if !self.query.is_empty() {
            let q = self.query.clone();
            self.found = self.index().find(&q);
            self.found_at = self.found_at.min(self.found.len().saturating_sub(1));
        }
        outs.push(Out::Meta(self.meta()));
        self.redraw(outs);
    }

    /// The rest of the document laid out (the first page came at open).
    fn finish(&mut self, outs: &mut Vec<Out>) {
        let window = self.window();
        if let Err(e) = self.layout.full(window) {
            outs.push(Out::Note(format!("Couldn't lay the document out: {e}")));
        }
        self.layout.engine.set_windowed_incremental_builds(true);
        self.complete = true;
        self.rev += 1;
        self.index = None;
        outs.push(Out::Meta(self.meta()));
        self.redraw(outs);
    }

    fn window(&self) -> std::ops::Range<usize> {
        let lo = self.want.iter().copied().min().unwrap_or(0);
        let hi = self.want.iter().copied().max().unwrap_or(0);
        lo..hi + 1
    }

    /// Draws the pages in view that aren't drawn for this layout and zoom.
    fn redraw(&mut self, outs: &mut Vec<Out>) {
        let count = self.layout.page_count();
        let want: Vec<usize> = self.want.iter().copied().filter(|p| *p < count).collect();
        self.layout.engine.set_display_window(Some(self.window()));
        let unbuilt: Vec<usize> = want.iter().copied().filter(|i| self.layout.with_pages(|ps| ps[*i].unbuilt).unwrap_or(false)).collect();
        if !unbuilt.is_empty()
            && let Err(e) = self.layout.build(&unbuilt)
        {
            log::warn!("word: building pages {unbuilt:?}: {e}");
        }
        let mut pictures = Vec::new();
        for i in want {
            let Some(page) = self.layout.page(i) else { continue };
            if page.unbuilt {
                continue;
            }
            if let Some((old, rev, zoom)) = self.drawn.get_mut(&i) {
                if *zoom == self.zoom && (*rev == self.rev || **old == page) {
                    // the same page: its drawing stands
                    if *rev != self.rev {
                        *rev = self.rev;
                        outs.push(Out::Page { index: i, rev: self.rev, zoom: self.zoom, drawn: Arc::new(Drawn::default()) });
                    }
                    continue;
                }
            }
            let drawn = draw::page(&page, &self.layout.fonts, self.zoom);
            pictures.extend(drawn.pictures.iter().map(|p| (p.part.clone(), p.rel_id.clone())));
            self.drawn.insert(i, (Arc::new(page), self.rev, self.zoom));
            outs.push(Out::Page { index: i, rev: self.rev, zoom: self.zoom, drawn: Arc::new(drawn) });
        }
        self.send_media(pictures, outs);
    }

    /// The bytes of pictures not sent yet.
    fn send_media(&mut self, wanted: Vec<(Option<String>, String)>, outs: &mut Vec<Out>) {
        let fresh: Vec<_> = wanted.into_iter().filter(|k| !self.media_sent.contains(k)).collect();
        if fresh.is_empty() {
            return;
        }
        let media = self.media.get_or_insert_with(|| save::media(&self.source));
        let mut out = Vec::new();
        for key in fresh {
            self.media_sent.insert(key.clone());
            if let Some(bytes) = save::picture(&self.source, media, key.0.as_deref(), &key.1) {
                out.push((key, Arc::new(bytes)));
            }
        }
        if !out.is_empty() {
            outs.push(Out::Media(out));
        }
    }

    /// The caret and selection boxes.
    fn caret(&mut self, reveal: bool) -> Out {
        let index = self.index.take().unwrap_or_else(|| Index::build(self.layout.engine.doc()));
        let a = index.pm_of(&self.anchor);
        let h = index.pm_of(&self.head);
        let found: Vec<(i64, i64)> = self.found.iter().filter_map(|(s, e)| Some((index.pm_of(s)?, index.pm_of(e)?))).collect();
        self.index = Some(index);
        let mut c = Caret::default();
        let rect = |r: &hit::RangeRect| (r.page_index, r.x as f32, r.y as f32, r.width as f32, r.height as f32);
        self.list(|l| {
            if let Some(h) = h {
                c.caret = hit::caret_rect(l, h).map(|r| (r.page_index, r.x as f32, r.y as f32, r.height as f32));
                if let Some(a) = a
                    && a != h
                {
                    c.sel = hit::range_rects(l, a.min(h), a.max(h)).iter().map(rect).collect();
                }
            }
            for (s, e) in &found {
                c.found.extend(hit::range_rects(l, *s, *e).iter().map(rect));
            }
        });
        if reveal {
            c.reveal = c.caret.map(|(p, _, y, _)| (p, y));
        }
        Out::Caret(c)
    }

    fn collapsed(&self) -> bool {
        self.anchor == self.head
    }

    /// The selection within one story: (story, from, to). A selection
    /// across stories (a table and the text around it) is cut at the
    /// head's story.
    fn range(&self) -> (String, u32, u32) {
        let s = self.head.story.clone();
        if self.anchor.story != s {
            return (s, self.head.offset, self.head.offset);
        }
        (s, self.anchor.offset.min(self.head.offset), self.anchor.offset.max(self.head.offset))
    }

    fn set_caret(&mut self, s: Spot) {
        self.anchor = s.clone();
        self.head = s;
    }

    /// Deletes the selection; whether there was one.
    fn delete_selection(&mut self) -> Result<bool, String> {
        let (story, a, b) = self.range();
        if a == b {
            return Ok(false);
        }
        self.layout.engine.doc().delete_range(&ctx(), StoryRange::new(&story, a, b)).map_err(|e| e.to_string())?;
        self.set_caret(spot(&story, a));
        Ok(true)
    }

    fn insert(&mut self, text: &str) -> Result<(), String> {
        self.delete_selection()?;
        let doc = self.layout.engine.doc();
        let story = self.head.story.clone();
        let mut at = self.head.offset;
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                doc.split_paragraph(&ctx(), Position::new(&story, at), None).map_err(|e| e.to_string())?;
                at += 1;
            }
            let line = line.trim_end_matches('\r');
            if !line.is_empty() {
                doc.insert_text(&ctx(), Position::new(&story, at), line, FormatPolicy::Inherit).map_err(|e| e.to_string())?;
                at += line.encode_utf16().count() as u32;
            }
        }
        self.set_caret(spot(&story, at));
        Ok(())
    }

    /// The paragraph of the head and its index.
    fn head_para(&mut self) -> Option<(usize, super::index::Para)> {
        let head = self.head.clone();
        let index = self.index();
        let i = index.para_of(&head)?;
        Some((i, index.paras[i].clone()))
    }

    fn backspace(&mut self) -> Result<(), String> {
        if self.delete_selection()? {
            return Ok(());
        }
        let Some((_, p)) = self.head_para() else { return Ok(()) };
        let off = self.head.offset;
        if off > p.start {
            let k = (off - p.start) as usize;
            // a surrogate pair goes whole
            let n = if k >= 2 && (0xDC00..0xE000).contains(&p.text[k - 1]) { 2 } else { 1 };
            self.layout.engine.doc().delete_range(&ctx(), StoryRange::new(&p.story, off - n, off)).map_err(|e| e.to_string())?;
            self.set_caret(spot(&p.story, off - n));
        } else if p.start > 0 {
            // joins the paragraph before, in the same story
            let before = self.index().paras.iter().rev().find(|q| q.story == p.story && q.start + q.len + 1 == p.start).cloned();
            if let Some(q) = before {
                self.layout.engine.doc().merge_paragraphs(&ctx(), &p.para_id, MergeDirection::Backward).map_err(|e| e.to_string())?;
                self.set_caret(spot(&q.story, q.start + q.len));
            }
        }
        Ok(())
    }

    fn delete(&mut self) -> Result<(), String> {
        if self.delete_selection()? {
            return Ok(());
        }
        let Some((_, p)) = self.head_para() else { return Ok(()) };
        let off = self.head.offset;
        if off < p.start + p.len {
            let k = (off - p.start) as usize;
            let n = if (0xD800..0xDC00).contains(&p.text[k]) { 2 } else { 1 };
            self.layout.engine.doc().delete_range(&ctx(), StoryRange::new(&p.story, off, off + n)).map_err(|e| e.to_string())?;
        } else {
            let next = self.index().paras.iter().any(|q| q.story == p.story && q.start == p.start + p.len + 1);
            if next {
                self.layout.engine.doc().merge_paragraphs(&ctx(), &p.para_id, MergeDirection::Forward).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// The word around the caret (for formatting with no selection).
    fn word_at(&mut self) -> Option<(String, u32, u32)> {
        let (_, p) = self.head_para()?;
        let k = (self.head.offset - p.start) as usize;
        let is_word = |u: &u16| char::from_u32(*u as u32).is_some_and(|c| c.is_alphanumeric());
        let mut a = k;
        while a > 0 && is_word(&p.text[a - 1]) {
            a -= 1;
        }
        let mut b = k;
        while b < p.text.len() && is_word(&p.text[b]) {
            b += 1;
        }
        (a < b).then(|| (p.story.clone(), p.start + a as u32, p.start + b as u32))
    }

    fn format(&mut self, f: Fmt) -> Result<(), String> {
        let (story, a, b) = match self.range() {
            (s, a, b) if a < b => (s, a, b),
            _ => match self.word_at() {
                Some(w) => w,
                None => return Ok(()),
            },
        };
        let doc = self.layout.engine.doc();
        let range = StoryRange::new(&story, a, b);
        match f {
            Fmt::Bold => doc.toggle_format(&ctx(), range, SimpleFormat::Bold).map(drop),
            Fmt::Italic => doc.toggle_format(&ctx(), range, SimpleFormat::Italic).map(drop),
            Fmt::Underline => doc.toggle_format(&ctx(), range, SimpleFormat::Underline).map(drop),
            Fmt::Size(up) => {
                let size = self.formats().3;
                let next = size_step(size, up);
                let delta = InlineFormatDelta { font_size: Patch::Set(next as f64), ..Default::default() };
                doc.format_range(&ctx(), range, &delta).map(drop)
            }
            Fmt::Bullets | Fmt::Numbers => return self.make_list(f == Fmt::Numbers),
        }
        .map_err(|e| e.to_string())
    }

    /// The selected paragraphs as a list (or out of one, when they are one
    /// of this kind already).
    fn make_list(&mut self, numbered: bool) -> Result<(), String> {
        let (story, a, b) = self.range();
        let selector = docx_edit::ParaSelector::Range(StoryRange::new(&story, a, b.max(a)));
        let num = save::list_num(&self.source, numbered);
        if num == 0 {
            return Err(format!("This document defines no {} list to use.", if numbered { "numbered" } else { "bulleted" }));
        }
        let current = self.list_of_head();
        let mut delta = docx_edit::ParaAttrDelta::default();
        // the list's marker attributes, as a paragraph already in it has them
        let donor = self.list_attrs(num);
        if current == Some(num) {
            delta.other.insert("numPr".to_owned(), None);
            for k in donor.keys() {
                delta.other.insert(k.clone(), None);
            }
        } else {
            for (k, v) in donor {
                delta.other.insert(k, Some(v));
            }
            delta.other.insert("numPr".to_owned(), Some(save::num_pr(num, 0)));
        }
        self.layout.engine.doc().set_paragraph_attrs(&ctx(), &selector, &delta).map(drop).map_err(|e| e.to_string())
    }

    /// The `list…` attributes of a paragraph in list `num` (none when no
    /// paragraph is in it yet: the marker then comes with the next layout
    /// of a saved file).
    fn list_attrs(&self, num: u32) -> std::collections::BTreeMap<String, yrs::Any> {
        let Ok(segments) = self.layout.engine.doc().story_segments("body") else { return Default::default() };
        segments
            .iter()
            .find_map(|s| match &s.content {
                docx_edit::SegmentContent::Pilcrow(props) if props.values.get("numPr").and_then(save::num_id) == Some(num) => {
                    Some(props.values.iter().filter(|(k, _)| k.starts_with("list")).map(|(k, v)| (k.clone(), v.clone())).collect())
                }
                _ => None,
            })
            .unwrap_or_default()
    }

    /// The list (`numId`) the head's paragraph is in.
    fn list_of_head(&mut self) -> Option<u32> {
        let (_, p) = self.head_para()?;
        let segments = self.layout.engine.doc().story_segments(&p.story).ok()?;
        segments.iter().find_map(|s| match &s.content {
            docx_edit::SegmentContent::Pilcrow(props) if props.para_id == p.para_id => save::num_id(props.values.get("numPr")?),
            _ => None,
        })
    }

    fn row(&mut self, at: Option<bool>) -> Result<String, String> {
        let story = self.head.story.clone();
        let Some((parent, table, row, col)) = cell_of(&story) else { return Err("The caret isn't in a table.".to_owned()) };
        let cell = docx_edit::CellLoc::new(&parent, table, row, col);
        let doc = self.layout.engine.doc();
        match at {
            Some(after) => doc.insert_row(&ctx(), &cell, after).map(drop),
            None => {
                let r = doc.delete_row(&ctx(), &docx_edit::TableRange::cell(cell)).map(drop);
                // the caret's cell is gone: back to the table's story
                self.set_caret(spot(&parent, 0));
                r
            }
        }
        .map_err(|e| e.to_string())?;
        Ok(parent)
    }

    fn click(&mut self, page: usize, x: f32, y: f32, extend: bool, clicks: u8) {
        let Some(pm) = self.list(|l| hit::hit_test(l, page, x as f64, y as f64)).flatten() else { return };
        let Some(s) = self.index().spot_of_pm(pm) else { return };
        self.goal_x = None;
        if extend {
            if s.story == self.anchor.story {
                self.head = s;
            }
            return;
        }
        self.set_caret(s);
        match clicks {
            2 => {
                if let Some((story, a, b)) = self.word_at() {
                    self.anchor = spot(&story, a);
                    self.head = spot(&story, b);
                }
            }
            3.. => {
                if let Some((_, p)) = self.head_para() {
                    self.anchor = spot(&p.story, p.start);
                    self.head = spot(&p.story, p.start + p.len);
                }
            }
            _ => {}
        }
    }

    fn step(&mut self, to: Move, extend: bool) {
        let head = self.head.clone();
        let next = self.moved(&head, to);
        if let Some(n) = next {
            if extend {
                if n.story == self.anchor.story {
                    self.head = n;
                }
            } else if !self.collapsed() && matches!(to, Move::Left | Move::Right) {
                // an arrow collapses a selection to its side
                let (_, a, b) = self.range();
                let story = self.head.story.clone();
                self.set_caret(spot(&story, if to == Move::Left { a } else { b }));
            } else {
                self.set_caret(n);
            }
        }
        if !matches!(to, Move::Up | Move::Down | Move::PageUp | Move::PageDown) {
            self.goal_x = None;
        }
    }

    fn moved(&mut self, s: &Spot, to: Move) -> Option<Spot> {
        let index = self.index.take().unwrap_or_else(|| Index::build(self.layout.engine.doc()));
        let r = self.moved_in(&index, s, to);
        self.index = Some(index);
        r
    }

    fn moved_in(&self, index: &Index, s: &Spot, to: Move) -> Option<Spot> {
        let i = index.para_of(s)?;
        let p = &index.paras[i];
        let k = (s.offset - p.start) as usize;
        let at = |p: &super::index::Para, k: usize| spot(&p.story, p.start + k as u32);
        let is_word = |u: u16| char::from_u32(u as u32).is_some_and(|c| c.is_alphanumeric());
        match to {
            Move::Left if k > 0 => Some(at(p, k - if k >= 2 && (0xDC00..0xE000).contains(&p.text[k - 1]) { 2 } else { 1 })),
            Move::Left => index.paras.get(i.checked_sub(1)?).map(|q| at(q, q.len as usize)),
            Move::Right if k < p.len as usize => Some(at(p, k + if (0xD800..0xDC00).contains(&p.text[k]) { 2 } else { 1 })),
            Move::Right => index.paras.get(i + 1).map(|q| at(q, 0)),
            Move::WordLeft => {
                if k == 0 {
                    return self.moved_in(index, s, Move::Left);
                }
                let mut j = k;
                while j > 0 && !is_word(p.text[j - 1]) {
                    j -= 1;
                }
                while j > 0 && is_word(p.text[j - 1]) {
                    j -= 1;
                }
                Some(at(p, j))
            }
            Move::WordRight => {
                if k >= p.len as usize {
                    return self.moved_in(index, s, Move::Right);
                }
                let mut j = k;
                while j < p.text.len() && !is_word(p.text[j]) {
                    j += 1;
                }
                while j < p.text.len() && is_word(p.text[j]) {
                    j += 1;
                }
                Some(at(p, j))
            }
            Move::DocStart => index.paras.first().map(|q| at(q, 0)),
            Move::DocEnd => index.paras.last().map(|q| at(q, q.len as usize)),
            Move::Up | Move::Down | Move::PageUp | Move::PageDown => {
                let pm = index.pm_of(s)?;
                let down = matches!(to, Move::Down | Move::PageDown);
                let lines = if matches!(to, Move::PageUp | Move::PageDown) { 40 } else { 1 };
                let goal = self.goal_x.unwrap_or(f64::NAN);
                let mut pos = pm;
                let mut goal_x = goal;
                self.list(|l| {
                    for _ in 0..lines {
                        let Some(m) = hit::vertical_move(l, pos, if down { VerticalDirection::Down } else { VerticalDirection::Up }, goal_x.is_finite().then_some(goal_x)) else { break };
                        goal_x = m.goal_x;
                        if m.position == pos {
                            break;
                        }
                        pos = m.position;
                    }
                });
                index.spot_of_pm(pos)
            }
            Move::LineStart | Move::LineEnd => {
                let pm = index.pm_of(s)?;
                let pos = self
                    .list(|l| {
                        let c = hit::caret_rect(l, pm)?;
                        let x = if to == Move::LineStart { 0.0 } else { 1e6 };
                        hit::hit_test(l, c.page_index, x, c.y + c.height / 2.0)
                    })
                    .flatten()?;
                index.spot_of_pm(pos)
            }
        }
    }

    /// Up and down keep the x they started from.
    fn keep_goal(&mut self) {
        if self.goal_x.is_none() {
            let head = self.head.clone();
            let pm = self.index().pm_of(&head);
            self.goal_x = pm.and_then(|pm| self.list(|l| hit::caret_rect(l, pm).map(|c| c.x)).flatten());
        }
    }

    fn find(&mut self, query: String, back: bool) {
        if query != self.query {
            self.query = query.clone();
            self.found = self.index().find(&query);
            // the first match at or after the caret
            let head = self.head.clone();
            let pm = self.index().pm_of(&head).unwrap_or(0);
            let index = self.index.take().expect("built");
            self.found_at = self.found.iter().position(|(s, _)| index.pm_of(s).unwrap_or(0) >= pm).unwrap_or(0);
            self.index = Some(index);
        } else if !self.found.is_empty() {
            let n = self.found.len();
            self.found_at = if back { (self.found_at + n - 1) % n } else { (self.found_at + 1) % n };
        }
        if let Some((s, e)) = self.found.get(self.found_at).cloned() {
            self.anchor = s;
            self.head = e;
        }
    }

    fn undo(&mut self, redo: bool) -> Option<String> {
        let story = self.undo.changed_stories().into_iter().next().unwrap_or_else(|| "body".to_owned());
        let done = if redo { self.undo.redo() } else { self.undo.undo() };
        if !done {
            return None;
        }
        // the caret stays where it can
        let len = self.layout.engine.doc().story_len(&self.head.story).unwrap_or(0);
        if self.head.offset > len || self.anchor.story != self.head.story {
            self.head.offset = self.head.offset.min(len);
            self.anchor = self.head.clone();
        }
        self.anchor.offset = self.anchor.offset.min(len);
        Some(story)
    }
}

/// Word's font size steps.
fn size_step(size: f32, up: bool) -> f32 {
    const STEPS: [f32; 16] = [8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 36.0, 48.0, 72.0, 96.0, 144.0];
    if up { STEPS.iter().copied().find(|s| *s > size + 0.01).unwrap_or(size + 12.0) } else { STEPS.iter().rev().copied().find(|s| *s < size - 0.01).unwrap_or((size - 1.0).max(1.0)) }
}

/// A comment's body (its paragraphs as JSON) as plain text.
fn comment_text(json: &str) -> String {
    fn walk(v: &serde_json::Value, out: &mut String) {
        match v {
            serde_json::Value::String(s) => out.push_str(s),
            serde_json::Value::Array(a) => a.iter().for_each(|v| walk(v, out)),
            serde_json::Value::Object(o) => {
                for k in ["text", "content", "paragraphs", "runs", "children"] {
                    if let Some(v) = o.get(k) {
                        walk(v, out);
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
        walk(&v, &mut out);
    }
    out
}

impl Document for Word {
    type Cmd = Cmd;
    type Out = Out;

    fn open(bytes: Vec<u8>, path: &Path) -> Result<Self, String> {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ext == "doc" {
            return Word::from_docx(save::doc_to_docx(&bytes)?, true);
        }
        Word::from_docx(bytes, false)
    }

    fn run(&mut self, cmd: Cmd) -> (Vec<Out>, bool) {
        let mut outs = Vec::new();
        let edit = |w: &mut Word, outs: &mut Vec<Out>, r: Result<String, String>| -> bool {
            match r {
                Ok(story) => {
                    w.relayout(&story, outs);
                    outs.push(w.caret(true));
                    true
                }
                Err(e) => {
                    outs.push(Out::Note(e));
                    false
                }
            }
        };
        let story = self.head.story.clone();
        let changed = match cmd {
            Cmd::View { pages, zoom } => {
                let zoomed = zoom != self.zoom;
                self.want = pages;
                self.zoom = zoom;
                self.redraw(&mut outs);
                if zoomed {
                    outs.push(self.caret(false));
                }
                false
            }
            Cmd::Click { page, x, y, extend, clicks } => {
                self.click(page, x, y, extend, clicks);
                outs.push(self.caret(false));
                outs.push(Out::Meta(self.meta()));
                false
            }
            Cmd::Move { to, extend } => {
                if matches!(to, Move::Up | Move::Down | Move::PageUp | Move::PageDown) {
                    self.keep_goal();
                }
                self.step(to, extend);
                outs.push(self.caret(true));
                outs.push(Out::Meta(self.meta()));
                false
            }
            Cmd::SelectAll => {
                let head = self.head.story.clone();
                let index = self.index();
                let story = index.paras.iter().find(|p| p.story == head).map(|p| p.story.clone()).unwrap_or_else(|| "body".to_owned());
                let first = index.paras.iter().find(|p| p.story == story).map(|p| p.start).unwrap_or(0);
                let last = index.paras.iter().rev().find(|p| p.story == story).map(|p| p.start + p.len).unwrap_or(0);
                self.anchor = spot(&story, first);
                self.head = spot(&story, last);
                outs.push(self.caret(false));
                false
            }
            Cmd::Type(t) => {
                let r = self.insert(&t).map(|_| story);
                edit(self, &mut outs, r)
            }
            Cmd::Paste(t) => {
                let r = self.insert(&t).map(|_| story);
                edit(self, &mut outs, r)
            }
            Cmd::Backspace => {
                let r = self.backspace().map(|_| story);
                edit(self, &mut outs, r)
            }
            Cmd::Delete => {
                let r = self.delete().map(|_| story);
                edit(self, &mut outs, r)
            }
            Cmd::Enter => {
                let r = self.insert("\n").map(|_| story);
                edit(self, &mut outs, r)
            }
            Cmd::Tab => {
                let r = self.delete_selection().and_then(|_| {
                    let at = self.head.clone();
                    self.layout.engine.doc().insert_tab(&ctx(), Position::new(&at.story, at.offset)).map_err(|e| e.to_string())?;
                    self.set_caret(spot(&at.story, at.offset + 1));
                    Ok(story)
                });
                edit(self, &mut outs, r)
            }
            Cmd::Format(f) => {
                let r = self.format(f).map(|_| story);
                edit(self, &mut outs, r)
            }
            Cmd::Row(at) => {
                let r = self.row(at);
                edit(self, &mut outs, r)
            }
            Cmd::Undo => match self.undo(false) {
                Some(story) => edit(self, &mut outs, Ok(story)),
                None => false,
            },
            Cmd::Redo => match self.undo(true) {
                Some(story) => edit(self, &mut outs, Ok(story)),
                None => false,
            },
            Cmd::Find { query, back } => {
                self.find(query, back);
                outs.push(self.caret(true));
                outs.push(Out::Meta(self.meta()));
                false
            }
            Cmd::SaveTo(path) => {
                self.target = Some(path);
                false
            }
            Cmd::Print => {
                let index = self.index.take().unwrap_or_else(|| Index::build(self.layout.engine.doc()));
                outs.push(Out::Print(save::html(self.layout.engine.doc(), &index)));
                self.index = Some(index);
                false
            }
            Cmd::Copy { cut } => {
                let (story, a, b) = self.range();
                let text = self.index().text_between(&spot(&story, a), &spot(&story, b));
                outs.push(Out::Copied(text));
                if cut && a < b {
                    let r = self.delete_selection().map(|_| story);
                    edit(self, &mut outs, r)
                } else {
                    false
                }
            }
        };
        (outs, changed)
    }

    fn tick(&mut self) -> (Vec<Out>, bool) {
        let mut outs = Vec::new();
        if !self.meta_sent {
            // the first page, then the rest on the next tick
            self.meta_sent = true;
            outs.push(Out::Meta(self.meta()));
            self.redraw(&mut outs);
            outs.push(self.caret(false));
        } else if !self.complete {
            self.finish(&mut outs);
            outs.push(self.caret(false));
        }
        (outs, false)
    }

    fn busy(&self) -> bool {
        !self.meta_sent || !self.complete
    }

    fn save(&mut self, path: &Path) -> Result<(Vec<u8>, PathBuf), String> {
        let bytes = if self.edited || self.legacy { save::docx(&self.source, self.layout.engine.doc())? } else { self.source.clone() };
        let to = match &self.target {
            Some(t) => t.clone(),
            None if self.legacy => path.with_extension("docx"),
            None => path.to_owned(),
        };
        // saved as .docx, the document is one from now on
        self.legacy = false;
        self.edited = false;
        Ok((bytes, to))
    }
}
