//! The sheet's selection: an active cell and a range, moved by the
//! keyboard and the pointer as Excel moves them. Pure (no UI, no
//! workbook): what it needs of the sheet comes in as [`Bounds`].

/// The last row and column a sheet has (Excel's 1,048,576 × 16,384).
pub(crate) const LAST_ROW: u32 = 1_048_575;
pub(crate) const LAST_COL: u32 = 16_383;

/// What a move needs of the sheet.
pub(crate) struct Bounds<'a> {
    /// One past the last used row and column (⌘-arrows stop there).
    pub used: (u32, u32),
    /// Rows a page key moves.
    pub page: u32,
    /// Whether a row or a column is hidden (moves step over it).
    pub hidden_row: &'a dyn Fn(u32) -> bool,
    pub hidden_col: &'a dyn Fn(u32) -> bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    fn delta(self) -> (i64, i64) {
        match self {
            Dir::Up => (-1, 0),
            Dir::Down => (1, 0),
            Dir::Left => (0, -1),
            Dir::Right => (0, 1),
        }
    }
}

/// How far a key moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// One cell (an arrow).
    Cell,
    /// To the edge of the data (⌘-arrow).
    Edge,
    /// A screenful (Page Up / Page Down).
    Page,
}

/// The active cell and the range around it: `anchor` is where it started
/// (the active cell, unless Tab or Enter walk inside the range), `focus`
/// the end a Shift-move or a drag moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sel {
    pub anchor: (u32, u32),
    pub focus: (u32, u32),
    pub active: (u32, u32),
}

impl Default for Sel {
    fn default() -> Self {
        Sel::at((0, 0))
    }
}

impl Sel {
    /// One cell.
    pub(crate) fn at(cell: (u32, u32)) -> Sel {
        Sel { anchor: cell, focus: cell, active: cell }
    }

    /// The range, top-left and bottom-right.
    pub(crate) fn range(&self) -> ((u32, u32), (u32, u32)) {
        let (a, f) = (self.anchor, self.focus);
        ((a.0.min(f.0), a.1.min(f.1)), (a.0.max(f.0), a.1.max(f.1)))
    }

    pub(crate) fn is_single(&self) -> bool {
        self.anchor == self.focus
    }

    pub(crate) fn contains(&self, cell: (u32, u32)) -> bool {
        let ((r0, c0), (r1, c1)) = self.range();
        (r0..=r1).contains(&cell.0) && (c0..=c1).contains(&cell.1)
    }

    /// A press on `cell`: Shift extends from the anchor.
    pub(crate) fn press(&self, cell: (u32, u32), shift: bool) -> Sel {
        if shift { Sel { focus: cell, ..*self } } else { Sel::at(cell) }
    }

    /// A drag reaching `cell`.
    pub(crate) fn drag(&self, cell: (u32, u32)) -> Sel {
        Sel { focus: cell, ..*self }
    }

    /// ⌘A: the whole sheet, the active cell kept.
    pub(crate) fn all(&self) -> Sel {
        Sel { anchor: (0, 0), focus: (LAST_ROW, LAST_COL), active: self.active }
    }

    /// A whole row or column (a press on its header); Shift extends.
    pub(crate) fn row(&self, r: u32, shift: bool) -> Sel {
        let anchor = if shift { (self.anchor.0, 0) } else { (r, 0) };
        Sel { anchor, focus: (r, LAST_COL), active: if shift { self.active } else { (r, 0) } }
    }

    pub(crate) fn col(&self, c: u32, shift: bool) -> Sel {
        let anchor = if shift { (0, self.anchor.1) } else { (0, c) };
        Sel { anchor, focus: (LAST_ROW, c), active: if shift { self.active } else { (0, c) } }
    }

    /// An arrow (with ⌘ for `Step::Edge`, a page key for `Step::Page`):
    /// moves the active cell, or with Shift the range's moving end.
    pub(crate) fn step(&self, dir: Dir, step: Step, shift: bool, b: &Bounds) -> Sel {
        let from = if shift { self.focus } else { self.active };
        let to = moved(from, dir, step, b);
        if shift { Sel { focus: to, ..*self } } else { Sel::at(to) }
    }

    /// Tab (`Right`, Shift-Tab `Left`) and Enter (`Down`, Shift-Enter
    /// `Up`): inside a range they walk its cells and wrap, keeping it;
    /// otherwise they move one cell.
    pub(crate) fn walk(&self, dir: Dir, b: &Bounds) -> Sel {
        if self.is_single() {
            return Sel::at(moved(self.active, dir, Step::Cell, b));
        }
        let ((r0, c0), (r1, c1)) = self.range();
        let (mut r, mut c) = self.active;
        match dir {
            Dir::Right => {
                if c < c1 {
                    c += 1
                } else {
                    c = c0;
                    r = if r < r1 { r + 1 } else { r0 };
                }
            }
            Dir::Left => {
                if c > c0 {
                    c -= 1
                } else {
                    c = c1;
                    r = if r > r0 { r - 1 } else { r1 };
                }
            }
            Dir::Down => {
                if r < r1 {
                    r += 1
                } else {
                    r = r0;
                    c = if c < c1 { c + 1 } else { c0 };
                }
            }
            Dir::Up => {
                if r > r0 {
                    r -= 1
                } else {
                    r = r1;
                    c = if c > c0 { c - 1 } else { c1 };
                }
            }
        }
        Sel { active: (r, c), ..*self }
    }
}

/// Where `from` lands: hidden rows and columns are stepped over, and the
/// sheet's edges stop it.
fn moved(from: (u32, u32), dir: Dir, step: Step, b: &Bounds) -> (u32, u32) {
    let (dr, dc) = dir.delta();
    let hidden = |r: u32, c: u32| (b.hidden_row)(r) || (b.hidden_col)(c);
    let clamp = |r: i64, c: i64| (r.clamp(0, LAST_ROW as i64) as u32, c.clamp(0, LAST_COL as i64) as u32);
    match step {
        Step::Cell => {
            let (mut r, mut c) = (from.0 as i64, from.1 as i64);
            loop {
                let (nr, nc) = (r + dr, c + dc);
                if nr < 0 || nc < 0 || nr > LAST_ROW as i64 || nc > LAST_COL as i64 {
                    return clamp(r, c);
                }
                (r, c) = (nr, nc);
                if !hidden(r as u32, c as u32) {
                    return (r as u32, c as u32);
                }
            }
        }
        Step::Page => {
            let n = b.page.max(1) as i64;
            clamp(from.0 as i64 + dr * n, from.1 as i64 + dc * n)
        }
        Step::Edge => {
            // the data's edge, then the sheet's
            let (last_r, last_c) = (b.used.0.saturating_sub(1), b.used.1.saturating_sub(1));
            match dir {
                Dir::Up => (0, from.1),
                Dir::Left => (from.0, 0),
                Dir::Down => (if from.0 < last_r { last_r } else { LAST_ROW }, from.1),
                Dir::Right => (from.0, if from.1 < last_c { last_c } else { LAST_COL }),
            }
        }
    }
}

/// `range` grown over the merged ranges it touches until none sticks out
/// of it, as the web editor draws a selection (`(row, col)` corners).
pub(crate) fn expand(range: ((u32, u32), (u32, u32)), merges: &[((u32, u32), (u32, u32))]) -> ((u32, u32), (u32, u32)) {
    let ((mut r0, mut c0), (mut r1, mut c1)) = range;
    let mut grew = true;
    while grew {
        grew = false;
        for &((mr0, mc0), (mr1, mc1)) in merges {
            let touches = mr0 <= r1 && mr1 >= r0 && mc0 <= c1 && mc1 >= c0;
            if touches && (mr0 < r0 || mc0 < c0 || mr1 > r1 || mc1 > c1) {
                (r0, c0, r1, c1) = (r0.min(mr0), c0.min(mc0), r1.max(mr1), c1.max(mc1));
                grew = true;
            }
        }
    }
    ((r0, c0), (r1, c1))
}

/// Column `c`'s letters: A, Z, AA, XFD.
pub(crate) fn col_name(c: u32) -> String {
    let mut n = c as u64 + 1;
    let mut s = Vec::new();
    while n > 0 {
        let d = ((n - 1) % 26) as u8;
        s.push(b'A' + d);
        n = (n - 1) / 26;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// A cell's address (`B7`) or a range's (`A1:C3`).
pub(crate) fn address(sel: &Sel) -> String {
    let ((r0, c0), (r1, c1)) = sel.range();
    let a = |r: u32, c: u32| format!("{}{}", col_name(c), r + 1);
    if sel.is_single() {
        a(sel.active.0, sel.active.1)
    } else if c0 == 0 && c1 == LAST_COL {
        format!("{}:{}", r0 + 1, r1 + 1)
    } else if r0 == 0 && r1 == LAST_ROW {
        format!("{}:{}", col_name(c0), col_name(c1))
    } else {
        format!("{}:{}", a(r0, c0), a(r1, c1))
    }
}

/// Pasted text as rows of cells: tab-separated, quoted fields as Excel
/// writes them; a trailing line break makes no extra row.
pub(crate) fn paste_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = super::convert::parse_delimited(text, '\t');
    if rows.last().is_some_and(|r| r.len() == 1 && r[0].is_empty()) {
        rows.pop();
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Bounds<'static> {
        Bounds { used: (10, 5), page: 20, hidden_row: &|r| r == 3, hidden_col: &|c| c == 2 }
    }

    #[test]
    fn arrows_move_and_skip_hidden() {
        let b = bounds();
        let s = Sel::at((2, 1));
        assert_eq!(s.step(Dir::Down, Step::Cell, false, &b), Sel::at((4, 1)));
        assert_eq!(s.step(Dir::Right, Step::Cell, false, &b), Sel::at((2, 3)));
        assert_eq!(Sel::at((0, 0)).step(Dir::Up, Step::Cell, false, &b), Sel::at((0, 0)));
        assert_eq!(Sel::at((0, 0)).step(Dir::Left, Step::Cell, false, &b), Sel::at((0, 0)));
        assert_eq!(Sel::at((LAST_ROW, 0)).step(Dir::Down, Step::Cell, false, &b), Sel::at((LAST_ROW, 0)));
    }

    #[test]
    fn shift_extends_from_the_anchor() {
        let b = bounds();
        let s = Sel::at((1, 1)).step(Dir::Down, Step::Cell, true, &b).step(Dir::Right, Step::Cell, true, &b);
        assert_eq!(s, Sel { anchor: (1, 1), focus: (2, 3), active: (1, 1) });
        assert_eq!(s.range(), ((1, 1), (2, 3)));
        assert_eq!(address(&s), "B2:D3");
        // and back past the anchor
        let s = s.step(Dir::Up, Step::Cell, true, &b).step(Dir::Up, Step::Cell, true, &b);
        assert_eq!(s.range(), ((0, 1), (1, 3)));
        // an arrow without Shift collapses to the moved active cell
        assert_eq!(s.step(Dir::Left, Step::Cell, false, &b), Sel::at((1, 0)));
    }

    #[test]
    fn edges_pages_and_all() {
        let b = bounds();
        assert_eq!(Sel::at((2, 1)).step(Dir::Down, Step::Edge, false, &b), Sel::at((9, 1)));
        assert_eq!(Sel::at((9, 1)).step(Dir::Down, Step::Edge, false, &b), Sel::at((LAST_ROW, 1)));
        assert_eq!(Sel::at((2, 1)).step(Dir::Right, Step::Edge, false, &b), Sel::at((2, 4)));
        assert_eq!(Sel::at((5, 3)).step(Dir::Up, Step::Edge, false, &b), Sel::at((0, 3)));
        assert_eq!(Sel::at((5, 3)).step(Dir::Down, Step::Page, false, &b), Sel::at((25, 3)));
        assert_eq!(Sel::at((5, 3)).step(Dir::Up, Step::Page, false, &b), Sel::at((0, 3)));
        let all = Sel::at((4, 4)).all();
        assert_eq!((all.range(), all.active), (((0, 0), (LAST_ROW, LAST_COL)), (4, 4)));
        // ⌘⇧↓ extends to the data's edge
        assert_eq!(Sel::at((2, 1)).step(Dir::Down, Step::Edge, true, &b).range(), ((2, 1), (9, 1)));
    }

    #[test]
    fn tab_and_enter_walk_a_range_and_wrap() {
        let b = bounds();
        let s = Sel { anchor: (0, 0), focus: (1, 1), active: (0, 0) };
        let s1 = s.walk(Dir::Right, &b);
        assert_eq!(s1.active, (0, 1));
        let s2 = s1.walk(Dir::Right, &b);
        assert_eq!(s2.active, (1, 0));
        assert_eq!(s2.walk(Dir::Right, &b).walk(Dir::Right, &b).active, (0, 0));
        assert_eq!(s.walk(Dir::Down, &b).active, (1, 0));
        assert_eq!(s.walk(Dir::Up, &b).active, (1, 1));
        assert_eq!(s.walk(Dir::Left, &b).active, (1, 1));
        // the range stays
        assert_eq!(s2.range(), s.range());
        // a single cell: Enter moves down, Tab right
        assert_eq!(Sel::at((0, 0)).walk(Dir::Down, &b), Sel::at((1, 0)));
        assert_eq!(Sel::at((0, 0)).walk(Dir::Right, &b), Sel::at((0, 1)));
    }

    #[test]
    fn presses_drags_and_headers() {
        let s = Sel::at((1, 1));
        assert_eq!(s.press((4, 4), false), Sel::at((4, 4)));
        assert_eq!(s.press((4, 4), true).range(), ((1, 1), (4, 4)));
        assert_eq!(s.drag((0, 3)).range(), ((0, 1), (1, 3)));
        let row = s.row(5, false);
        assert_eq!((row.range(), address(&row)), (((5, 0), (5, LAST_COL)), "6:6".to_owned()));
        let cols = s.col(2, false).col(4, true);
        assert_eq!(address(&cols), "C:E");
        assert!(cols.contains((100, 3)) && !cols.contains((0, 5)));
    }

    #[test]
    fn a_selection_grows_over_the_merges_it_touches() {
        // A1:B2 merged, and C2:C4 reached only through B2's growth
        let merges = [((0, 0), (1, 1)), ((1, 2), (3, 2)), ((8, 8), (9, 9))];
        assert_eq!(expand(((1, 1), (1, 1)), &merges), ((0, 0), (1, 1)));
        assert_eq!(expand(((0, 1), (0, 2)), &merges), ((0, 0), (3, 2)));
        assert_eq!(expand(((5, 5), (5, 5)), &merges), ((5, 5), (5, 5)));
    }

    #[test]
    fn names_and_paste() {
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(25), "Z");
        assert_eq!(col_name(26), "AA");
        assert_eq!(col_name(LAST_COL), "XFD");
        assert_eq!(address(&Sel::at((6, 1))), "B7");
        assert_eq!(paste_rows("a\tb\n1\t\"x\ny\"\n"), vec![vec!["a".to_owned(), "b".into()], vec!["1".into(), "x\ny".into()]]);
        assert_eq!(paste_rows("solo"), vec![vec!["solo".to_owned()]]);
    }
}
