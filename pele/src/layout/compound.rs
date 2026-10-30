// Port of src/layout/compound.ts: lays out a graph whose nodes may be nested in groups. Each
// group is laid out on its own, in its own direction, and then placed in its parent as a single
// box. An edge that crosses a group's border passes through a port on the side facing its other end.

use super::layered::{self, ease, kind, layered, ledge, lnode, orient, rank, Groups, LEdge, LNode, LayeredOptions, LayeredResult};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    TB,
    BT,
    LR,
    RL,
}

// The direction a diagram names, or top to bottom for anything else.
pub fn direction(dir: Option<&str>) -> Dir {
    match dir {
        Some("BT") => Dir::BT,
        Some("LR") => Dir::LR,
        Some("RL") => Dir::RL,
        _ => Dir::TB,
    }
}

#[derive(Clone, Debug)]
pub struct CNode {
    pub w: f64,
    pub h: f64,
    pub parent: i64,
    pub is_group: bool,
    pub dir: Option<Dir>,
    pub pad_x: f64,
    pub pad_top: f64,
    pub pad_bottom: f64,
    pub min_w: f64,
    // Declaration order; a group takes the smallest value among its members.
    pub seq: f64,
    // How much of a side its edges may spread along, measured across the flow.
    pub span: f64,
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug)]
pub struct CEdge {
    pub src: usize,
    pub dst: usize,
    pub minlen: f64,
    pub label_w: f64,
    pub label_h: f64,
    // x, y, axis triples; axis is 0 where the edge runs vertically into the point, 1 horizontally.
    pub route: Vec<f64>,
    pub label_x: f64,
    pub label_y: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CompoundResult {
    pub width: f64,
    pub height: f64,
}

const TOP: u8 = 0;
const RIGHT: u8 = 1;
const BOTTOM: u8 = 2;
const LEFT: u8 = 3;

#[derive(Clone, Debug)]
struct PortEdge {
    e: usize,
    inner: usize,
    out: bool,
    side: u8,
    // Cross-flow position outside the group from a previous layout; NaN if unknown.
    hint: f64,
    px: f64,
    py: f64,
    route: Vec<f64>,
}

#[derive(Clone, Debug)]
struct LevelEdge {
    e: usize,
    a: usize,
    b: usize,
}

#[derive(Clone, Debug)]
struct Level {
    id: i64,
    dir: Dir,
    items: Vec<usize>,
    edges: Vec<LevelEdge>,
    ports: Vec<PortEdge>,
    port_of: HashMap<usize, usize>,
    routes: HashMap<usize, Vec<f64>>,
    x: f64,
    y: f64,
}

const MAX_PORTS: i64 = 40000;
// Limit repeated layouts to small graphs.
const MAX_ORDERED_PORTS: i64 = 400;
const MAX_ORDERED_EDGES: usize = 500;
// Crossing tolerance before trying independent group ordering.
const FLAT_SLACK: f64 = 1.5;
const FLAT_SPARE: f64 = 4.0;

struct Snapshot {
    nodes: Vec<f64>,
    routes: Vec<Vec<f64>>,
    labels: Vec<f64>,
    levels: Vec<f64>,
    total: CompoundResult,
}

fn snapshot(nodes: &[CNode], edges: &[CEdge], levels: &[Level], total: CompoundResult) -> Snapshot {
    let mut at = Vec::new();
    for c in nodes {
        at.extend_from_slice(&[c.x, c.y, c.w, c.h]);
    }
    let mut labels = Vec::new();
    for e in edges {
        labels.push(e.label_x);
        labels.push(e.label_y);
    }
    let mut places = Vec::new();
    for level in levels {
        places.push(level.x);
        places.push(level.y);
        for port in &level.ports {
            places.push(port.px);
            places.push(port.py);
        }
    }
    Snapshot { nodes: at, routes: edges.iter().map(|e| e.route.clone()).collect(), labels, levels: places, total }
}

fn restore(s: &Snapshot, nodes: &mut [CNode], edges: &mut [CEdge], levels: &mut [Level]) -> CompoundResult {
    for (i, c) in nodes.iter_mut().enumerate() {
        c.x = s.nodes[4 * i];
        c.y = s.nodes[4 * i + 1];
        c.w = s.nodes[4 * i + 2];
        c.h = s.nodes[4 * i + 3];
    }
    for (i, e) in edges.iter_mut().enumerate() {
        e.route = s.routes[i].clone();
        e.label_x = s.labels[2 * i];
        e.label_y = s.labels[2 * i + 1];
    }
    let mut k = 0;
    for level in levels.iter_mut() {
        level.x = s.levels[k];
        level.y = s.levels[k + 1];
        k += 2;
        for port in level.ports.iter_mut() {
            port.px = s.levels[k];
            port.py = s.levels[k + 1];
            k += 2;
        }
    }
    s.total
}

// Ignore crossings within this distance of a shared node.
const FAN: f64 = 24.0;
// Tolerance for collinear points.
const IN_LINE: f64 = 0.5;
const CURVE_PIECES: usize = 3;
const CORNER_PIECES: usize = 2;
// Allow for a step, an S-curve, or a straight segment plus a rounded corner.
const STEP_PIECES: usize = 3;
// Match the renderer's corner handles.
const HANDLE: f64 = 0.62;
const MAX_INSERT: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Curved,
    Stepped,
    Linear,
}

pub fn style_of(opt: &LayeredOptions) -> Style {
    if opt.tracks {
        Style::Stepped
    } else if opt.straight {
        Style::Linear
    } else {
        Style::Curved
    }
}

struct Segs {
    ax: Vec<f64>,
    ay: Vec<f64>,
    bx: Vec<f64>,
    by: Vec<f64>,
    low: Vec<f64>,
    high: Vec<f64>,
    owner: Vec<usize>,
    // Endpoint flags: 1 for the first segment, 2 for the last.
    ends: Vec<u8>,
    count: usize,
    top: f64,
    bottom: f64,
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
}

impl Segs {
    #[allow(clippy::too_many_arguments)]
    fn add(&mut self, ei: usize, part: u8, mut x0: f64, mut y0: f64, x1: f64, y1: f64) {
        let on = x0 == self.to_x && y0 == self.to_y && ((y1 == y0 && self.from_y == y0) || (x1 == x0 && self.from_x == x0));
        if on && self.count > 0 {
            let p = self.count - 1;
            if self.owner[p] == ei && self.ends[p] == part && (x0 - self.from_x) * (x1 - x0) + (y0 - self.from_y) * (y1 - y0) > 0.0 {
                self.count -= 1;
                x0 = self.from_x;
                y0 = self.from_y;
            }
        }
        self.from_x = x0;
        self.from_y = y0;
        self.to_x = x1;
        self.to_y = y1;
        let c = self.count;
        self.ends[c] = part;
        self.ax[c] = x0.min(x1);
        self.bx[c] = x0.max(x1);
        self.ay[c] = if x0 <= x1 { y0 } else { y1 };
        self.by[c] = if x0 <= x1 { y1 } else { y0 };
        self.low[c] = y0.min(y1);
        self.high[c] = y0.max(y1);
        if self.low[c] < self.top {
            self.top = self.low[c];
        }
        if self.high[c] > self.bottom {
            self.bottom = self.high[c];
        }
        self.owner[c] = ei;
        self.count += 1;
    }
}

pub fn route_crossings(nodes: &[CNode], edges: &[CEdge], style: Style) -> i64 {
    let mut size = 0;
    for e in edges {
        if e.src != e.dst && e.route.len() > 3 {
            size += (e.route.len() / 3 - 1) * STEP_PIECES;
        }
    }
    let mut s = Segs {
        ax: vec![0.0; size],
        ay: vec![0.0; size],
        bx: vec![0.0; size],
        by: vec![0.0; size],
        low: vec![0.0; size],
        high: vec![0.0; size],
        owner: vec![0; size],
        ends: vec![0; size],
        count: 0,
        top: f64::INFINITY,
        bottom: f64::NEG_INFINITY,
        from_x: f64::NAN,
        from_y: f64::NAN,
        to_x: f64::NAN,
        to_y: f64::NAN,
    };
    for (ei, e) in edges.iter().enumerate() {
        if e.src == e.dst {
            continue;
        }
        let r = &e.route;
        let aligned = |k: usize| (r[k] - r[k - 3]).abs() < 0.01 || (r[k + 1] - r[k - 2]).abs() < 0.01;
        let length = |k: usize| (r[k] - r[k - 3]).abs() + (r[k + 1] - r[k - 2]).abs();
        let turns = |k: usize| {
            style == Style::Curved
                && k > 0
                && k + 3 < r.len()
                && aligned(k)
                && aligned(k + 3)
                && length(k) >= 0.01
                && length(k + 3) >= 0.01
                && (((r[k] - r[k - 3]).abs() < 0.01) != ((r[k + 3] - r[k]).abs() < 0.01))
        };
        let mut at_x = r[0];
        let mut at_y = r[1];
        let mut k = 3;
        while k < r.len() {
            let part: u8 = (if k == 3 { 1 } else { 0 }) | (if k == r.len() - 3 { 2 } else { 0 });
            let x0 = at_x;
            let y0 = at_y;
            let x1 = r[k];
            let y1 = r[k + 1];
            at_x = x1;
            at_y = y1;
            if aligned(k) || style == Style::Linear {
                if !turns(k) {
                    s.add(ei, part, x0, y0, x1, y1);
                    k += 3;
                    continue;
                }
                // Adjacent corners share a leg, so each gets half.
                let before = if turns(k - 3) { length(k) / 2.0 } else { length(k) };
                let after = if turns(k + 3) { length(k + 3) / 2.0 } else { length(k + 3) };
                let ux = (x1 - r[k - 3]) / length(k);
                let uy = (y1 - r[k - 2]) / length(k);
                let vx = (r[k + 3] - x1) / length(k + 3);
                let vy = (r[k + 4] - y1) / length(k + 3);
                let sx = x1 - ux * before;
                let sy = y1 - uy * before;
                s.add(ei, part, x0, y0, sx, sy);
                let c1x = x1 - ux * before * (1.0 - HANDLE);
                let c1y = y1 - uy * before * (1.0 - HANDLE);
                let c2x = x1 + vx * after * (1.0 - HANDLE);
                let c2y = y1 + vy * after * (1.0 - HANDLE);
                at_x = x1 + vx * after;
                at_y = y1 + vy * after;
                let turn = part | if k + 3 == r.len() - 3 { 2 } else { 0 };
                let mut px = sx;
                let mut py = sy;
                for i in 1..=CORNER_PIECES {
                    let t = i as f64 / CORNER_PIECES as f64;
                    let u = 1.0 - t;
                    let x = u * u * u * sx + 3.0 * u * u * t * c1x + 3.0 * u * t * t * c2x + t * t * t * at_x;
                    let y = u * u * u * sy + 3.0 * u * u * t * c1y + 3.0 * u * t * t * c2y + t * t * t * at_y;
                    s.add(ei, turn, px, py, x, y);
                    px = x;
                    py = y;
                }
                k += 3;
                continue;
            }
            if style == Style::Stepped {
                let down = r[k + 2] == 0.0;
                let mx = if down { x0 } else { (x0 + x1) / 2.0 };
                let my = if down { (y0 + y1) / 2.0 } else { y0 };
                s.add(ei, part, x0, y0, mx, my);
                s.add(ei, part, mx, my, if down { x1 } else { mx }, if down { my } else { y1 });
                s.add(ei, part, if down { x1 } else { mx }, if down { my } else { y1 }, x1, y1);
                k += 3;
                continue;
            }
            // Approximate the rendered S-curve with straight segments.
            let down = r[k + 2] == 0.0;
            let c = if down { ease(x1 - x0, y1 - y0) } else { ease(y1 - y0, x1 - x0) };
            let mut px = x0;
            let mut py = y0;
            for i in 1..=CURVE_PIECES {
                let t = i as f64 / CURVE_PIECES as f64;
                let u = 1.0 - t;
                let across = t * t * (3.0 - 2.0 * t);
                let along = 3.0 * c * t * u * u + 3.0 * (1.0 - c) * t * t * u + t * t * t;
                let x = x0 + (x1 - x0) * if down { across } else { along };
                let y = y0 + (y1 - y0) * if down { along } else { across };
                s.add(ei, part, px, py, x, y);
                px = x;
                py = y;
            }
            k += 3;
        }
    }
    let count = s.count;
    let (top, bottom) = (s.top, s.bottom);
    // Count each pair only in the first band shared by both segments.
    let bands: usize = if bottom > top { crate::js::round((count as f64).sqrt()) as usize } else { 1 };
    let scale = if bottom > top { bands as f64 / (bottom - top) } else { 0.0 };
    let band_of = |y: f64| -> i64 { ((bands as i64) - 1).min(((y - top) * scale).floor() as i64) };
    let mut first = vec![0i64; count];
    let mut band_start = vec![0usize; bands + 1];
    for p in 0..count {
        first[p] = band_of(s.low[p]);
        let last = band_of(s.high[p]);
        let mut b = first[p];
        while b <= last {
            band_start[(b + 1) as usize] += 1;
            b += 1;
        }
    }
    for b in 0..bands {
        band_start[b + 1] += band_start[b];
    }
    let mut fill = band_start[..bands].to_vec();
    let mut in_band = vec![0usize; band_start[bands]];
    for p in 0..count {
        let last = band_of(s.high[p]);
        let mut b = first[p];
        while b <= last {
            in_band[fill[b as usize]] = p;
            fill[b as usize] += 1;
            b += 1;
        }
    }
    // Edges merging into a shared endpoint run do not count as crossings.
    let mut runs = vec![0f64; edges.len() * 8];
    let in_line = |r: &[f64], a: usize, b: usize, c: usize| ((r[a] - r[b]).abs() < IN_LINE && (r[a] - r[c]).abs() < IN_LINE) || ((r[a + 1] - r[b + 1]).abs() < IN_LINE && (r[a + 1] - r[c + 1]).abs() < IN_LINE);
    for (ei, e) in edges.iter().enumerate() {
        let r = &e.route;
        if r.len() < 6 {
            continue;
        }
        let mut k = 3;
        while k + 3 < r.len() && in_line(r, 0, k, k + 3) {
            k += 3;
        }
        let last = r.len() - 3;
        let mut j = last as isize - 3;
        while j >= 3 && in_line(r, last, j as usize, j as usize - 3) {
            j -= 3;
        }
        let j = j as usize;
        runs[ei * 8..ei * 8 + 8].copy_from_slice(&[r[0], r[1], r[k], r[k + 1], r[j], r[j + 1], r[last], r[last + 1]]);
    }
    let on_run = |ei: usize, end: u8, x: f64, y: f64| {
        let o = ei * 8 + if end == 1 { 0 } else { 4 };
        let x0 = runs[o];
        let y0 = runs[o + 1];
        let dx = runs[o + 2] - x0;
        let dy = runs[o + 3] - y0;
        let length = dx * dx + dy * dy;
        let t = if length > 0.0 { 0f64.max(1f64.min(((x - x0) * dx + (y - y0) * dy) / length)) } else { 0.0 };
        (x - x0 - t * dx).hypot(y - y0 - t * dy) < IN_LINE
    };
    let mut total = 0;
    for band in 0..bands {
        let band_end = band_start[band + 1];
        let bs = band_start[band];
        if band_end - bs > MAX_INSERT {
            in_band[bs..band_end].sort_by(|&p, &q| crate::js::ord(crate::js::or(s.ax[p] - s.ax[q], || p as f64 - q as f64)));
        } else {
            for i in bs + 1..band_end {
                let p = in_band[i];
                let mut j = i;
                while j > bs && s.ax[in_band[j - 1]] > s.ax[p] {
                    in_band[j] = in_band[j - 1];
                    j -= 1;
                }
                in_band[j] = p;
            }
        }
        for si in bs..band_end {
            let p = in_band[si];
            let dx = s.bx[p] - s.ax[p];
            let dy = s.by[p] - s.ay[p];
            let here = first[p] == band as i64;
            for t in si + 1..band_end {
                let q = in_band[t];
                if s.ax[q] > s.bx[p] {
                    break;
                }
                if s.high[q] < s.low[p] || s.low[q] > s.high[p] || s.owner[p] == s.owner[q] || (!here && first[q] != band as i64) {
                    continue;
                }
                let d1 = dx * (s.ay[q] - s.ay[p]) - dy * (s.ax[q] - s.ax[p]);
                let d2 = dx * (s.by[q] - s.ay[p]) - dy * (s.bx[q] - s.ax[p]);
                // Treat points on the line as one side to count segment-boundary crossings once.
                if (d1 > 0.0) == (d2 > 0.0) {
                    continue;
                }
                let ex = s.bx[q] - s.ax[q];
                let ey = s.by[q] - s.ay[q];
                let d3 = ex * (s.ay[p] - s.ay[q]) - ey * (s.ax[p] - s.ax[q]);
                let d4 = ex * (s.by[p] - s.ay[q]) - ey * (s.bx[p] - s.ax[q]);
                if (d3 > 0.0) == (d4 > 0.0) {
                    continue;
                }
                let ep = &edges[s.owner[p]];
                let eq = &edges[s.owner[q]];
                let at = d3 / (d3 - d4);
                let x = s.ax[p] + at * dx;
                let y = s.ay[p] + at * dy;
                // Exclude fan-in and fan-out at shared nodes.
                let fan = |v: usize, pe: u8, qe: u8| {
                    ((s.ends[p] & pe) != 0 && (s.ends[q] & qe) != 0)
                        || ((x - nodes[v].x).abs() < nodes[v].w / 2.0 + FAN && (y - nodes[v].y).abs() < nodes[v].h / 2.0 + FAN)
                        || (on_run(s.owner[p], pe, x, y) && on_run(s.owner[q], qe, x, y))
                };
                if ep.src == eq.src && fan(ep.src, 1, 1) {
                    continue;
                }
                if ep.src == eq.dst && fan(ep.src, 1, 2) {
                    continue;
                }
                if ep.dst == eq.src && fan(ep.dst, 2, 1) {
                    continue;
                }
                if ep.dst == eq.dst && fan(ep.dst, 2, 2) {
                    continue;
                }
                total += 1;
            }
        }
    }
    total
}

// Match Mermaid: groups crossed by edges inherit their parent's direction.
pub fn inherit_crossed_directions(nodes: &mut [CNode], ends: &[(i64, i64)]) {
    let ancestors = |nodes: &[CNode], i: usize| {
        let mut set: Vec<usize> = Vec::new();
        let mut p = nodes[i].parent;
        while p >= 0 && !set.contains(&(p as usize)) {
            set.push(p as usize);
            p = nodes[p as usize].parent;
        }
        set
    };
    for &(src, dst) in ends {
        if src < 0 || dst < 0 || src == dst {
            continue;
        }
        let a = ancestors(nodes, src as usize);
        let b = ancestors(nodes, dst as usize);
        for &g in &a {
            if !b.contains(&g) {
                nodes[g].dir = None;
            }
        }
        for &g in &b {
            if !a.contains(&g) {
                nodes[g].dir = None;
            }
        }
    }
}

pub fn shift_layout(nodes: &mut [CNode], edges: &mut [CEdge], ox: f64, oy: f64) {
    for c in nodes.iter_mut() {
        c.x += ox;
        c.y += oy;
    }
    for e in edges.iter_mut() {
        let mut k = 0;
        while k < e.route.len() {
            e.route[k] += ox;
            e.route[k + 1] += oy;
            k += 3;
        }
        e.label_x += ox;
        e.label_y += oy;
    }
}

pub fn cnode(w: f64, h: f64) -> CNode {
    CNode { w, h, parent: -1, is_group: false, dir: None, pad_x: 0.0, pad_top: 0.0, pad_bottom: 0.0, min_w: 0.0, seq: 0.0, span: 0.0, x: 0.0, y: 0.0 }
}

fn start_side(dir: Dir) -> u8 {
    match dir {
        Dir::TB => TOP,
        Dir::BT => BOTTOM,
        Dir::LR => LEFT,
        Dir::RL => RIGHT,
    }
}

fn opposite(side: u8) -> u8 {
    (side + 2) & 3
}

fn vertical(dir: Dir) -> bool {
    dir == Dir::TB || dir == Dir::BT
}

struct Ctx<'a> {
    nodes: &'a mut [CNode],
    edges: &'a mut [CEdge],
    order: Vec<Level>,
    // Level index by level id (-1 for the root).
    level_of: HashMap<i64, usize>,
    local: Vec<usize>,
    key: Vec<f64>,
    keyed: bool,
    far: Option<FarHint>,
}

struct FarHint {
    ledge_of: Vec<i64>,
    points: Vec<Vec<f64>>,
    last: Vec<i64>,
}

impl Ctx<'_> {
    fn level(&self, id: i64) -> usize {
        self.level_of[&id]
    }

    fn endpoint_below(&self, item: usize, e: usize, out: bool) -> bool {
        (if out { self.edges[e].src } else { self.edges[e].dst }) != item
    }

    // Order outgoing ports at the group's last rank; break ties using the next rank.
    fn far_hint(&self, level: &Level, port: &PortEdge) -> f64 {
        let f = self.far.as_ref().unwrap();
        let le = f.ledge_of[port.e];
        if le < 0 || level.id < 0 || !port.out || port.side != opposite(start_side(level.dir)) {
            return f64::NAN;
        }
        let points = &f.points[le as usize];
        let mut inside = self.key[self.edges[port.e].src];
        let mut outside = 0.0;
        let last = f.last[level.id as usize] as f64;
        let mut k = 0;
        while k < points.len() {
            if points[k] == last {
                inside = points[k + 1];
            }
            if points[k] == last + 1.0 {
                outside = points[k + 1];
            }
            k += 2;
        }
        inside + outside * 1e-6
    }

    fn build(&mut self, li: usize, sized: bool) -> (Vec<LNode>, Vec<LEdge>, Vec<i64>) {
        let level = &self.order[li];
        let vert = vertical(level.dir);
        let start = start_side(level.dir);
        let mut lnodes: Vec<LNode> = Vec::new();
        let mut ledges: Vec<LEdge> = Vec::new();
        for (i, &item) in level.items.iter().enumerate() {
            self.local[item] = i;
            let node = &self.nodes[item];
            let mut ln = if sized { lnode(if vert { node.w } else { node.h }, if vert { node.h } else { node.w }, kind::NODE) } else { lnode(0.0, 0.0, kind::NODE) };
            ln.span = if node.is_group { node.span.min(ln.w - 2.0 * node.pad_x) } else { node.span };
            ln.key = if self.keyed { self.key[item] } else { f64::NAN };
            lnodes.push(ln);
        }
        let held = |ctx: &Ctx, item: usize, ei: usize, out: bool| ctx.nodes[item].is_group && ctx.endpoint_below(item, ei, out);
        let offset = |ctx: &Ctx, item: usize, ei: usize, out: bool| -> f64 {
            if !sized || !ctx.nodes[item].is_group || !ctx.endpoint_below(item, ei, out) {
                return 0.0;
            }
            let lv = &ctx.order[ctx.level(item as i64)];
            let port = &lv.ports[lv.port_of[&(ei * 2 + if out { 0 } else { 1 })]];
            if vert {
                port.px - ctx.nodes[item].w / 2.0
            } else {
                port.py - ctx.nodes[item].h / 2.0
            }
        };
        for le in &level.edges {
            let e = &self.edges[le.e];
            let mut edge = ledge(self.local[le.a], self.local[le.b], e.minlen, 0.0, 0.0);
            if sized {
                edge.label_w = if vert { e.label_w } else { e.label_h };
                edge.label_h = if vert { e.label_h } else { e.label_w };
                edge.tail_dx = offset(self, le.a, le.e, true);
                edge.head_dx = offset(self, le.b, le.e, false);
                edge.tail_fixed = held(self, le.a, le.e, true);
                edge.head_fixed = held(self, le.b, le.e, false);
            }
            ledges.push(edge);
        }
        let mut dummies = vec![-1i64; level.ports.len()];
        for (i, port) in level.ports.iter().enumerate() {
            if port.side == start || port.side == opposite(start) {
                let d = lnodes.len();
                let mut ln = lnode(0.0, 0.0, if port.side == start { kind::START_PORT } else { kind::END_PORT });
                ln.key = if self.keyed { self.key[if port.out { self.edges[port.e].dst } else { self.edges[port.e].src }] } else { f64::NAN };
                ln.hint = if port.hint.is_nan() && self.keyed && self.far.is_some() { self.far_hint(level, port) } else { port.hint };
                lnodes.push(ln);
                dummies[i] = d as i64;
                let mut edge = if port.out { ledge(self.local[port.inner], d, 1.0, 0.0, 0.0) } else { ledge(d, self.local[port.inner], 1.0, 0.0, 0.0) };
                if port.out {
                    edge.tail_dx = offset(self, port.inner, port.e, true);
                    edge.tail_fixed = held(self, port.inner, port.e, true);
                } else {
                    edge.head_dx = offset(self, port.inner, port.e, false);
                    edge.head_fixed = held(self, port.inner, port.e, false);
                }
                ledges.push(edge);
            } else {
                lnodes[self.local[port.inner]].pin = if port.side == TOP || port.side == LEFT { -1 } else { 1 };
            }
        }
        (lnodes, ledges, dummies)
    }
}

struct Laid {
    given: Vec<f64>,
    lnodes: Vec<LNode>,
    ledges: Vec<LEdge>,
    size: LayeredResult,
}

fn same_value(a: f64, b: f64) -> bool {
    // Object.is
    if a.is_nan() && b.is_nan() {
        return true;
    }
    a == b && a.is_sign_negative() == b.is_sign_negative()
}

pub fn compound_layout(nodes: &mut [CNode], edges: &mut [CEdge], root_dir: Dir, opt: &LayeredOptions) -> CompoundResult {
    let n = nodes.len();
    let mut order: Vec<Level> = Vec::new();
    let mut level_of: HashMap<i64, usize> = HashMap::new();
    let make_level = |order: &mut Vec<Level>, level_of: &mut HashMap<i64, usize>, id: i64, dir: Dir| {
        level_of.insert(id, order.len());
        order.push(Level { id, dir, items: Vec::new(), edges: Vec::new(), ports: Vec::new(), port_of: HashMap::new(), routes: HashMap::new(), x: 0.0, y: 0.0 });
        order.len() - 1
    };
    make_level(&mut order, &mut level_of, -1, root_dir);

    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut root_items = Vec::new();
    for i in 0..n {
        let p = nodes[i].parent;
        if p >= 0 && (p as usize) < n && p as usize != i && nodes[p as usize].is_group {
            children[p as usize].push(i);
        } else {
            nodes[i].parent = -1;
            root_items.push(i);
        }
    }
    order[0].items = root_items;
    let mut depth = vec![0i64; n];
    let mut placed = vec![0u8; n];
    let descend = |order: &mut Vec<Level>, level_of: &mut HashMap<i64, usize>, nodes: &[CNode], children: &[Vec<usize>], depth: &mut Vec<i64>, placed: &mut Vec<u8>, from: usize| {
        let mut q = from;
        while q < order.len() {
            let items = order[q].items.clone();
            let (id, dir) = (order[q].id, order[q].dir);
            for item in items {
                placed[item] = 1;
                depth[item] = if id < 0 { 0 } else { depth[id as usize] + 1 };
                if nodes[item].is_group {
                    let child = make_level(order, level_of, item as i64, nodes[item].dir.unwrap_or(dir));
                    order[child].items = children[item].clone();
                }
            }
            q += 1;
        }
    };
    descend(&mut order, &mut level_of, nodes, &children, &mut depth, &mut placed, 0);
    // Groups that contain each other are unreachable from the root; lift them out.
    for i in 0..n {
        if placed[i] != 0 {
            continue;
        }
        let former = nodes[i].parent as usize;
        children[former].retain(|&c| c != i);
        nodes[i].parent = -1;
        order[0].items.push(i);
        placed[i] = 1;
        depth[i] = 0;
        if nodes[i].is_group {
            let from = order.len();
            let child = make_level(&mut order, &mut level_of, i as i64, nodes[i].dir.unwrap_or(root_dir));
            order[child].items = children[i].clone();
            descend(&mut order, &mut level_of, nodes, &children, &mut depth, &mut placed, from);
        }
    }

    for q in (1..order.len()).rev() {
        let mut seq = f64::INFINITY;
        for &item in &order[q].items {
            if nodes[item].seq < seq {
                seq = nodes[item].seq;
            }
        }
        if seq != f64::INFINITY {
            nodes[order[q].id as usize].seq = seq;
        }
    }
    for level in order.iter_mut() {
        level.items.sort_by(|&a, &b| crate::js::ord(crate::js::or(nodes[a].seq - nodes[b].seq, || a as f64 - b as f64)));
    }

    // Every group border an edge crosses costs a port. Past the budget, an edge is drawn between
    // the groups that hold its ends, at the level they share.
    let mut ports: i64 = 0;
    let parent = |nodes: &[CNode], i: usize| nodes[i].parent as usize;
    for e in edges.iter_mut() {
        let mut a = e.src;
        let mut b = e.dst;
        let mut crossed = 0;
        while depth[a] > depth[b] {
            a = parent(nodes, a);
            crossed += 1;
        }
        while depth[b] > depth[a] {
            b = parent(nodes, b);
            crossed += 1;
        }
        while a != b && nodes[a].parent != nodes[b].parent {
            a = parent(nodes, a);
            b = parent(nodes, b);
            crossed += 2;
        }
        if a == b {
            continue;
        }
        if ports + crossed > MAX_PORTS {
            e.src = a;
            e.dst = b;
        } else {
            ports += crossed;
        }
    }

    let mut loops: Vec<usize> = Vec::new();
    let mut stray: Vec<usize> = Vec::new();
    for ei in 0..edges.len() {
        edges[ei].route = Vec::new();
        if edges[ei].src == edges[ei].dst {
            loops.push(ei);
            continue;
        }
        let mut a = edges[ei].src;
        let mut b = edges[ei].dst;
        let mut a_path = Vec::new();
        let mut b_path = Vec::new();
        while depth[a] > depth[b] {
            a_path.push(a);
            a = parent(nodes, a);
        }
        while depth[b] > depth[a] {
            b_path.push(b);
            b = parent(nodes, b);
        }
        if a == b {
            stray.push(ei);
            continue;
        }
        while nodes[a].parent != nodes[b].parent {
            a_path.push(a);
            b_path.push(b);
            a = parent(nodes, a);
            b = parent(nodes, b);
        }
        let li = level_of[&nodes[a].parent];
        order[li].edges.push(LevelEdge { e: ei, a, b });
        for (path, out) in [(a_path, true), (b_path, false)] {
            for inner in path {
                let li = level_of[&nodes[inner].parent];
                let level = &mut order[li];
                level.ports.push(PortEdge { e: ei, inner, out, side: TOP, hint: f64::NAN, px: 0.0, py: 0.0, route: Vec::new() });
                let idx = level.ports.len() - 1;
                level.port_of.insert(ei * 2 + if out { 0 } else { 1 }, idx);
            }
        }
    }

    let ordered = ports > 1 && ports <= MAX_ORDERED_PORTS && edges.len() <= MAX_ORDERED_EDGES;

    let mut ctx = Ctx { nodes, edges, order, level_of, local: vec![0; n], key: vec![f64::NAN; n], keyed: false, far: None };

    // Use a flat layout to coordinate node and port order across groups.
    let mut flat_crossings = 0f64;
    let mut flat = ctx.order.len() > 1 && ordered;
    for level in &ctx.order {
        if level.dir != root_dir {
            flat = false;
        }
    }
    if flat {
        let vert = vertical(root_dir);
        let nodes = &*ctx.nodes;
        let edges = &*ctx.edges;
        let mut leaf = vec![-1i64; n];
        let mut group = vec![-1i32; n];
        let mut lnodes: Vec<LNode> = Vec::new();
        let mut of: Vec<i32> = Vec::new();
        let mut gparent: Vec<i32> = Vec::new();
        for i in 0..n {
            if !nodes[i].is_group {
                continue;
            }
            group[i] = gparent.len() as i32;
            gparent.push(-1);
        }
        for i in 0..n {
            let p = nodes[i].parent;
            if nodes[i].is_group {
                gparent[group[i] as usize] = if p >= 0 { group[p as usize] } else { -1 };
            } else {
                leaf[i] = lnodes.len() as i64;
                lnodes.push(lnode(if vert { nodes[i].w } else { nodes[i].h }, if vert { nodes[i].h } else { nodes[i].w }, kind::NODE));
                of.push(if p >= 0 { group[p as usize] } else { -1 });
            }
        }
        let mut ledges: Vec<LEdge> = Vec::new();
        let mut ledge_of = vec![-1i64; edges.len()];
        for (ei, e) in edges.iter().enumerate() {
            if e.src == e.dst || leaf[e.src] < 0 || leaf[e.dst] < 0 {
                continue;
            }
            ledge_of[ei] = ledges.len() as i64;
            ledges.push(ledge(leaf[e.src] as usize, leaf[e.dst] as usize, e.minlen, if vert { e.label_w } else { e.label_h }, if vert { e.label_h } else { e.label_w }));
        }
        let mut flat_opt = opt.clone();
        flat_opt.groups = Some(Groups { of, parent: gparent });
        flat_opt.order_only = true;
        let flat_layout = layered(&mut lnodes, &mut ledges, &flat_opt);
        flat_crossings = flat_layout.crossings.unwrap_or(0.0).min(flat_layout.declared.unwrap_or(0.0));
        let mut sum = vec![0f64; n];
        let mut count = vec![0i64; n];
        // Last rank occupied by each group.
        let mut last = vec![-1i64; n];
        for i in 0..n {
            if leaf[i] < 0 {
                continue;
            }
            let ln = &lnodes[leaf[i] as usize];
            ctx.key[i] = ln.x;
            let mut p = nodes[i].parent;
            while p >= 0 {
                let pu = p as usize;
                sum[pu] += ctx.key[i];
                count[pu] += 1;
                last[pu] = last[pu].max(ln.rank as i64);
                p = nodes[pu].parent;
            }
        }
        for i in 0..n {
            if count[i] > 0 {
                ctx.key[i] = sum[i] / count[i] as f64;
            }
        }
        ctx.far = Some(FarHint { ledge_of, points: ledges.into_iter().map(|e| e.points).collect(), last });
    }

    // Reuse unchanged levels across layout attempts.
    let mut laid: HashMap<i64, Vec<Laid>> = HashMap::new();

    let mut total = CompoundResult::default();
    let style = layered_style(opt);
    ctx.keyed = flat;
    place(&mut ctx, opt, ordered, &mut laid, &mut total);
    let mut crossings: i64 = -1;
    if flat {
        crossings = route_crossings(ctx.nodes, ctx.edges, style);
        if crossings as f64 > flat_crossings * FLAT_SLACK + FLAT_SPARE {
            let first = snapshot(ctx.nodes, ctx.edges, &ctx.order, total);
            ctx.keyed = false;
            place(&mut ctx, opt, ordered, &mut laid, &mut total);
            let own = route_crossings(ctx.nodes, ctx.edges, style);
            if own >= crossings {
                ctx.keyed = true;
                total = restore(&first, ctx.nodes, ctx.edges, &mut ctx.order);
            } else {
                crossings = own;
            }
        }
    }

    // Refine port order using routes outside each group.
    if ctx.keyed == flat && ordered {
        let mut again = false;
        for li in 0..ctx.order.len() {
            let level_id = ctx.order[li].id;
            if level_id < 0 {
                continue;
            }
            let bx_node = ctx.nodes[level_id as usize].clone();
            let vert = vertical(ctx.order[li].dir);
            let end = opposite(start_side(ctx.order[li].dir));
            let (lx, ly) = (ctx.order[li].x, ctx.order[li].y);
            let mut seen: Vec<(f64, f64)> = Vec::new();
            for pi in 0..ctx.order[li].ports.len() {
                let port = &ctx.order[li].ports[pi];
                if port.side != end {
                    continue;
                }
                // Skip the straight run out of the port to find where the edge turns.
                let route = &ctx.edges[port.e].route;
                let bx = lx + port.px;
                let by = ly + port.py;
                let mut at: isize = -1;
                let mut k = 0;
                while k < route.len() && (at < 0 || port.out) {
                    if (route[k] - bx).abs() < 0.01 && (route[k + 1] - by).abs() < 0.01 {
                        at = k as isize;
                    }
                    k += 3;
                }
                if at < 0 {
                    continue;
                }
                let a: isize = if vert { 0 } else { 1 };
                let stepk: isize = if port.out { 3 } else { -3 };
                let mut next = at + stepk;
                while next >= 0 && (next as usize) < route.len() && (route[(next + a) as usize] - route[(at + a) as usize]).abs() < IN_LINE {
                    next += stepk;
                }
                if next < 0 || next as usize >= route.len() {
                    continue;
                }
                let hint = route[(next + a) as usize] - if vert { bx_node.x - bx_node.w / 2.0 } else { bx_node.y - bx_node.h / 2.0 };
                let pxy = if vert { port.px } else { port.py };
                ctx.order[li].ports[pi].hint = hint;
                seen.push((pxy, hint));
            }
            seen.sort_by(|p, q| crate::js::ord(p.0 - q.0));
            for i in 1..seen.len() {
                if seen[i - 1].1 > seen[i].1 {
                    again = true;
                }
            }
        }
        if again {
            let before = if crossings >= 0 { crossings } else { route_crossings(ctx.nodes, ctx.edges, style) };
            if before > 0 {
                let first = snapshot(ctx.nodes, ctx.edges, &ctx.order, total);
                place(&mut ctx, opt, ordered, &mut laid, &mut total);
                if route_crossings(ctx.nodes, ctx.edges, style) >= before {
                    total = restore(&first, ctx.nodes, ctx.edges, &mut ctx.order);
                }
            }
        }
    }

    let (nodes, edges) = (ctx.nodes, ctx.edges);
    for ei in stray {
        let (a, b) = (nodes[edges[ei].src].clone(), nodes[edges[ei].dst].clone());
        let e = &mut edges[ei];
        e.route = vec![a.x, a.y, 0.0, b.x, b.y, 0.0];
        e.label_x = (a.x + b.x) / 2.0;
        e.label_y = (a.y + b.y) / 2.0;
    }
    for ei in loops {
        let node = nodes[edges[ei].src].clone();
        let e = &mut edges[ei];
        e.label_x = node.x + node.w / 2.0 + 28.0 + e.label_w / 2.0;
        e.label_y = node.y;
    }
    total
}

fn layered_style(opt: &LayeredOptions) -> Style {
    style_of(opt)
}

fn lay_out(ctx: &Ctx, li: usize, mut lnodes: Vec<LNode>, mut ledges: Vec<LEdge>, opt: &LayeredOptions, ordered: bool, laid: &mut HashMap<i64, Vec<Laid>>) -> (Vec<LNode>, Vec<LEdge>, LayeredResult) {
    if !ordered {
        let size = layered(&mut lnodes, &mut ledges, opt);
        return (lnodes, ledges, size);
    }
    let mut given = Vec::new();
    for ln in &lnodes {
        given.extend_from_slice(&[ln.w, ln.h, ln.kind as f64, ln.pin as f64, ln.hint, ln.key, ln.span]);
    }
    for le in &ledges {
        given.extend_from_slice(&[le.tail as f64, le.head as f64, le.minlen, le.label_w, le.label_h, le.tail_dx, le.head_dx, le.tail_fixed as u8 as f64, le.head_fixed as u8 as f64]);
    }
    let past = laid.entry(ctx.order[li].id).or_default();
    for p in past.iter() {
        if p.given.len() == given.len() && p.given.iter().zip(&given).all(|(a, b)| same_value(*a, *b)) {
            return (p.lnodes.clone(), p.ledges.clone(), p.size);
        }
    }
    let size = layered(&mut lnodes, &mut ledges, opt);
    past.push(Laid { given, lnodes: lnodes.clone(), ledges: ledges.clone(), size });
    (lnodes, ledges, size)
}

fn place(ctx: &mut Ctx, opt: &LayeredOptions, ordered: bool, laid: &mut HashMap<i64, Vec<Laid>>, total: &mut CompoundResult) {
    // Rank parents first to determine which group sides edges cross.
    for li in 0..ctx.order.len() {
        for pi in 0..ctx.order[li].ports.len() {
            let port = ctx.order[li].ports[pi].clone();
            if ctx.nodes[port.inner].is_group && ctx.endpoint_below(port.inner, port.e, port.out) {
                let inner = ctx.level(port.inner as i64);
                let idx = ctx.order[inner].port_of[&(port.e * 2 + if port.out { 0 } else { 1 })];
                ctx.order[inner].ports[idx].side = port.side;
            }
        }
        if ctx.order[li].edges.is_empty() {
            continue;
        }
        let mut needed = false;
        for le in &ctx.order[li].edges {
            let e = &ctx.edges[le.e];
            if (ctx.nodes[le.a].is_group && e.src != le.a) || (ctx.nodes[le.b].is_group && e.dst != le.b) {
                needed = true;
            }
        }
        if !needed {
            continue;
        }
        let (mut lnodes, mut ledges, _) = ctx.build(li, false);
        orient(&lnodes, &mut ledges);
        rank(&mut lnodes, &ledges, 1);
        let start = start_side(ctx.order[li].dir);
        let level_edges = ctx.order[li].edges.clone();
        for (i, le) in level_edges.iter().enumerate() {
            let e = &ctx.edges[le.e];
            let a_above = !ledges[i].reversed;
            if ctx.nodes[le.a].is_group && e.src != le.a {
                let lv = ctx.level(le.a as i64);
                let idx = ctx.order[lv].port_of[&(le.e * 2)];
                ctx.order[lv].ports[idx].side = if a_above { opposite(start) } else { start };
            }
            if ctx.nodes[le.b].is_group && e.dst != le.b {
                let lv = ctx.level(le.b as i64);
                let idx = ctx.order[lv].port_of[&(le.e * 2 + 1)];
                ctx.order[lv].ports[idx].side = if a_above { start } else { opposite(start) };
            }
        }
    }

    // Lay out children first so their sizes are known.
    for q in (0..ctx.order.len()).rev() {
        let id = ctx.order[q].id;
        let dir = ctx.order[q].dir;
        let group = if id >= 0 { Some(ctx.nodes[id as usize].clone()) } else { None };
        let vert = vertical(dir);
        let flip = dir == Dir::BT || dir == Dir::RL;
        let (bl, be, dummies) = ctx.build(q, true);
        let (lnodes, ledges, size) = lay_out(ctx, q, bl, be, opt, ordered, laid);
        let content_w = if vert { size.width } else { size.height };
        let content_h = if vert { size.height } else { size.width };
        let pad_x = group.as_ref().map_or(0.0, |g| g.pad_x);
        let pad_top = group.as_ref().map_or(0.0, |g| g.pad_top);
        let box_w = (content_w + 2.0 * pad_x).max(group.as_ref().map_or(0.0, |g| g.min_w));
        let box_h = content_h + pad_top + group.as_ref().map_or(0.0, |g| g.pad_bottom);
        let off_x = (box_w - content_w) / 2.0;
        let off_y = pad_top;
        let tx = |lx: f64, ly: f64| off_x + if vert { lx } else if flip { size.height - ly } else { ly };
        let ty = |lx: f64, ly: f64| off_y + if vert { if flip { size.height - ly } else { ly } } else { lx };
        let axis = if vert { 0.0 } else { 1.0 };
        let convert = |points: &[f64]| {
            let mut out = Vec::with_capacity(points.len() / 2 * 3);
            let mut i = 0;
            while i < points.len() {
                out.push(tx(points[i], points[i + 1]));
                out.push(ty(points[i], points[i + 1]));
                out.push(axis);
                i += 2;
            }
            out
        };

        let items = ctx.order[q].items.clone();
        for (i, &item) in items.iter().enumerate() {
            ctx.nodes[item].x = tx(lnodes[i].x, lnodes[i].y);
            ctx.nodes[item].y = ty(lnodes[i].x, lnodes[i].y);
        }
        let level_edges = ctx.order[q].edges.clone();
        for (i, le) in level_edges.iter().enumerate() {
            let edge = &ledges[i];
            ctx.order[q].routes.insert(le.e, convert(&edge.points));
            let e = &mut ctx.edges[le.e];
            if e.label_w > 0.0 {
                e.label_x = tx(edge.label_x, edge.label_y);
                e.label_y = ty(edge.label_x, edge.label_y);
            }
        }
        let mut k = level_edges.len();
        for i in 0..ctx.order[q].ports.len() {
            let port = ctx.order[q].ports[i].clone();
            let item = ctx.nodes[port.inner].clone();
            if dummies[i] >= 0 {
                let mut route = convert(&ledges[k].points);
                k += 1;
                let at = if port.out { route.len() - 3 } else { 0 };
                let mut bx = route[at];
                let mut by = route[at + 1];
                match port.side {
                    TOP => by = 0.0,
                    BOTTOM => by = box_h,
                    LEFT => bx = 0.0,
                    _ => bx = box_w,
                }
                if port.out {
                    route.extend_from_slice(&[bx, by, axis]);
                } else {
                    route.splice(0..0, [bx, by, axis]);
                }
                let p = &mut ctx.order[q].ports[i];
                p.px = bx;
                p.py = by;
                p.route = route;
                continue;
            }
            let mut ix = item.x;
            let mut iy = item.y;
            if item.is_group && ctx.endpoint_below(port.inner, port.e, port.out) {
                let lv = ctx.level(port.inner as i64);
                let inner = &ctx.order[lv].ports[ctx.order[lv].port_of[&(port.e * 2 + if port.out { 0 } else { 1 })]];
                ix += inner.px - item.w / 2.0;
                iy += inner.py - item.h / 2.0;
            } else if port.side == TOP {
                iy -= item.h / 2.0;
            } else if port.side == BOTTOM {
                iy += item.h / 2.0;
            } else if port.side == LEFT {
                ix -= item.w / 2.0;
            } else {
                ix += item.w / 2.0;
            }
            let mut bx = ix;
            let mut by = iy;
            match port.side {
                TOP => by = 0.0,
                BOTTOM => by = box_h,
                LEFT => bx = 0.0,
                _ => bx = box_w,
            }
            let a = if port.side == TOP || port.side == BOTTOM { 0.0 } else { 1.0 };
            let p = &mut ctx.order[q].ports[i];
            p.px = bx;
            p.py = by;
            p.route = if port.out { vec![ix, iy, a, bx, by, a] } else { vec![bx, by, a, ix, iy, a] };
        }

        if id >= 0 {
            ctx.nodes[id as usize].w = box_w;
            ctx.nodes[id as usize].h = box_h;
        } else {
            *total = CompoundResult { width: box_w, height: box_h };
        }
    }

    for li in 0..ctx.order.len() {
        let ox = ctx.order[li].x;
        let oy = ctx.order[li].y;
        let items = ctx.order[li].items.clone();
        for item in items {
            ctx.nodes[item].x += ox;
            ctx.nodes[item].y += oy;
            if ctx.nodes[item].is_group {
                let child = ctx.level(item as i64);
                let node = &ctx.nodes[item];
                ctx.order[child].x = node.x - node.w / 2.0;
                ctx.order[child].y = node.y - node.h / 2.0;
            }
        }
    }

    let append = |route: &mut Vec<f64>, part: &[f64], ox: f64, oy: f64| {
        let mut i = 0;
        while i < part.len() {
            let x = part[i] + ox;
            let y = part[i + 1] + oy;
            let len = route.len();
            if !(len >= 3 && (route[len - 3] - x).abs() < 0.01 && (route[len - 2] - y).abs() < 0.01) {
                route.extend_from_slice(&[x, y, part[i + 2]]);
            }
            i += 3;
        }
    };

    for li in 0..ctx.order.len() {
        let level_id = ctx.order[li].id;
        for le in ctx.order[li].edges.clone() {
            let (src, dst) = (ctx.edges[le.e].src, ctx.edges[le.e].dst);
            let mut route: Vec<f64> = Vec::new();
            let mut outward: Vec<usize> = Vec::new();
            let mut item = src;
            while ctx.nodes[item].parent != level_id {
                outward.push(ctx.level(ctx.nodes[item].parent));
                item = ctx.nodes[item].parent as usize;
            }
            for lv in outward {
                let l = &ctx.order[lv];
                append(&mut route, &l.ports[l.port_of[&(le.e * 2)]].route, l.x, l.y);
            }
            let l = &ctx.order[li];
            append(&mut route, &l.routes[&le.e], l.x, l.y);
            let mut inward: Vec<usize> = Vec::new();
            let mut item = dst;
            while ctx.nodes[item].parent != level_id {
                inward.push(ctx.level(ctx.nodes[item].parent));
                item = ctx.nodes[item].parent as usize;
            }
            for &lv in inward.iter().rev() {
                let l = &ctx.order[lv];
                append(&mut route, &l.ports[l.port_of[&(le.e * 2 + 1)]].route, l.x, l.y);
            }
            let (lx, ly) = (ctx.order[li].x, ctx.order[li].y);
            let e = &mut ctx.edges[le.e];
            e.route = route;
            if e.label_w > 0.0 {
                e.label_x += lx;
                e.label_y += ly;
            }
        }
    }
}

#[allow(dead_code)]
fn _uses(_: layered::Groups) {}
