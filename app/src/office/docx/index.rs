//! Where text is: the engine edits story offsets (a story is one text, a
//! paragraph its units then its mark), the display list places body
//! positions (a ProseMirror-style count over the whole body, table cells
//! included). The index maps one to the other, paragraph by paragraph,
//! and holds each paragraph's text for find, copy and word moves. It is
//! rebuilt after each change (a few ms on a 200-page document).

use std::collections::BTreeMap;

use docx_edit::{EditingDoc, GeometryRead, SegmentContent};

/// A paragraph: its story, its first unit there, its length in units, and
/// the body position its content starts at.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Para {
    pub story: String,
    pub para_id: String,
    pub start: u32,
    pub len: u32,
    pub pm: i64,
    /// The text, a U+FFFC per unit that isn't text (a picture, a note mark).
    pub text: Vec<u16>,
}

/// A place in a story.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Spot {
    pub story: String,
    pub offset: u32,
}

#[derive(Default)]
pub(crate) struct Index {
    /// In body order.
    pub paras: Vec<Para>,
}

impl Index {
    pub(crate) fn build(doc: &EditingDoc) -> Index {
        let GeometryRead::Value(outline) = doc.geometry_position_outline("body") else { return Index::default() };
        let mut paras = Vec::new();
        for (story, o) in &outline {
            let pms: BTreeMap<&str, u64> = o.paragraphs.iter().map(|p| (p.para_id.as_str(), o.content_start + p.display_start + 1)).collect();
            let Ok(segments) = doc.story_segments(story) else { continue };
            let mut at = 0u32;
            let mut start = 0u32;
            let mut text: Vec<u16> = Vec::new();
            // units before a paragraph's text that aren't in it (a table, a
            // page break) move its start on
            let mut leading = true;
            for s in segments {
                match s.content {
                    SegmentContent::Text(t) => {
                        let u: Vec<u16> = t.encode_utf16().collect();
                        at += u.len() as u32;
                        text.extend(u);
                        leading = false;
                    }
                    SegmentContent::Pilcrow(props) => {
                        let id = props.para_id.to_string();
                        if let Some(pm) = pms.get(id.as_str()) {
                            paras.push(Para { story: story.clone(), para_id: id, start, len: at - start, pm: *pm as i64, text: std::mem::take(&mut text) });
                        }
                        text.clear();
                        at += 1;
                        start = at;
                        leading = true;
                    }
                    SegmentContent::OtherEmbed { kind, .. } => {
                        at += 1;
                        if leading && matches!(kind.as_str(), "table" | "blockSdt" | "pageBreak" | "columnBreak") {
                            start = at;
                        } else {
                            text.push(0xFFFC);
                            leading = false;
                        }
                    }
                }
            }
        }
        paras.sort_by_key(|p| p.pm);
        Index { paras }
    }

    /// The paragraph a body position falls in (or the nearest before it).
    pub(crate) fn para_at_pm(&self, pm: i64) -> Option<usize> {
        let i = self.paras.partition_point(|p| p.pm <= pm);
        if i == 0 { (!self.paras.is_empty()).then_some(0) } else { Some(i - 1) }
    }

    pub(crate) fn spot_of_pm(&self, pm: i64) -> Option<Spot> {
        let p = &self.paras[self.para_at_pm(pm)?];
        let k = (pm - p.pm).clamp(0, p.len as i64) as u32;
        Some(Spot { story: p.story.clone(), offset: p.start + k })
    }

    /// The paragraph holding `s` (its end included).
    pub(crate) fn para_of(&self, s: &Spot) -> Option<usize> {
        self.paras.iter().position(|p| p.story == s.story && s.offset >= p.start && s.offset <= p.start + p.len)
    }

    pub(crate) fn pm_of(&self, s: &Spot) -> Option<i64> {
        let p = &self.paras[self.para_of(s)?];
        Some(p.pm + (s.offset - p.start) as i64)
    }

    /// Takes a change inside one paragraph without a rebuild: at `at`,
    /// `removed` units out and `inserted` in (typing, a Backspace). False
    /// when `at` isn't inside a paragraph (the index is then rebuilt).
    pub(crate) fn patch(&mut self, at: &Spot, removed: u32, inserted: &[u16]) -> bool {
        let Some(i) = self.para_of(at) else { return false };
        let p = &mut self.paras[i];
        let k = (at.offset - p.start) as usize;
        if k + removed as usize > p.text.len() {
            return false;
        }
        p.text.splice(k..k + removed as usize, inserted.iter().copied());
        let delta = inserted.len() as i64 - removed as i64;
        p.len = (p.len as i64 + delta) as u32;
        let (story, start, pm) = (p.story.clone(), p.start, p.pm);
        for q in &mut self.paras {
            if q.story == story && q.start > start {
                q.start = (q.start as i64 + delta) as u32;
            }
            if q.pm > pm {
                q.pm += delta;
            }
        }
        true
    }

    /// The text between two spots of one story (paragraph breaks as `\n`).
    pub(crate) fn text_between(&self, a: &Spot, b: &Spot) -> String {
        let (a, b) = if a.offset <= b.offset { (a, b) } else { (b, a) };
        let mut out: Vec<u16> = Vec::new();
        let mut first = true;
        for p in self.paras.iter().filter(|p| p.story == a.story) {
            let (s, e) = (p.start, p.start + p.len);
            if e < a.offset || s > b.offset {
                continue;
            }
            if !std::mem::take(&mut first) {
                out.push(b'\n' as u16);
            }
            let from = a.offset.max(s) - s;
            let to = b.offset.min(e) - s;
            out.extend(p.text[from as usize..to as usize].iter().filter(|u| **u != 0xFFFC));
        }
        String::from_utf16_lossy(&out)
    }

    /// Every match of `needle` (case-insensitive), as story ranges, in body order.
    pub(crate) fn find(&self, needle: &str) -> Vec<(Spot, Spot)> {
        let n: Vec<u16> = needle.to_lowercase().encode_utf16().collect();
        if n.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for p in &self.paras {
            let hay: Vec<u16> = String::from_utf16_lossy(&p.text).to_lowercase().encode_utf16().collect();
            if hay.len() != p.text.len() {
                // lowercasing changed the length: match the text as it is
                continue;
            }
            let mut i = 0;
            while i + n.len() <= hay.len() {
                if hay[i..i + n.len()] == n[..] {
                    let s = Spot { story: p.story.clone(), offset: p.start + i as u32 };
                    let e = Spot { story: p.story.clone(), offset: p.start + (i + n.len()) as u32 };
                    out.push((s, e));
                    i += n.len();
                } else {
                    i += 1;
                }
            }
        }
        out
    }
}

/// The table cell a cell story is (`body:t0:r1c2`): its parent story, the
/// table's index there, its row and column.
pub(crate) fn cell_of(story: &str) -> Option<(String, u32, u32, u32)> {
    let (parent, rest) = story.rsplit_once(":t")?;
    let (table, cell) = rest.split_once(":r")?;
    let (row, col) = cell.split_once('c')?;
    // a cell made in the editor carries its client after a `~`
    let col = col.split_once('~').map_or(col, |(c, _)| c);
    Some((parent.to_owned(), table.parse().ok()?, row.parse().ok()?, col.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_stories_parse() {
        assert_eq!(cell_of("body:t0:r1c2"), Some(("body".to_owned(), 0, 1, 2)));
        assert_eq!(cell_of("body:t0:r0c1~1381"), Some(("body".to_owned(), 0, 0, 1)));
        assert_eq!(cell_of("body"), None);
    }
}
