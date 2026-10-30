// Port of src/layout/layered.ts: layered (Sugiyama-style) layout of one flat graph. Ranks run
// down the y axis; callers rotate the result for other directions. The aims the layout weighs,
// and where each is carried out, are set out at the top of the upstream file.

use crate::js;
use std::cmp::Ordering;

pub mod kind {
    pub const NODE: u8 = 0;
    pub const DUMMY: u8 = 1;
    pub const LABEL: u8 = 2;
    pub const START_PORT: u8 = 3;
    pub const END_PORT: u8 = 4;
}
use kind::*;

#[derive(Clone, Debug)]
pub struct LNode {
    pub w: f64,
    pub h: f64,
    pub kind: u8,
    pub pin: i32,
    // Port's cross-flow destination; NaN if unknown.
    pub hint: f64,
    // Initial cross-flow position; NaN if unknown.
    pub key: f64,
    // How much of the side that edges meet may be shared out between them.
    pub span: f64,
    pub x: f64,
    pub y: f64,
    pub rank: i32,
}

#[derive(Clone, Debug)]
pub struct LEdge {
    pub tail: usize,
    pub head: usize,
    pub minlen: f64,
    pub label_w: f64,
    pub label_h: f64,
    pub tail_dx: f64,
    pub head_dx: f64,
    pub tail_fixed: bool,
    pub head_fixed: bool,
    pub reversed: bool,
    pub points: Vec<f64>,
    pub label_x: f64,
    pub label_y: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Groups {
    pub of: Vec<i32>,
    pub parent: Vec<i32>,
}

#[derive(Clone, Debug, Default)]
pub struct LayeredOptions {
    pub node_sep: f64,
    pub edge_sep: f64,
    pub rank_sep: f64,
    pub port_sep: f64,
    pub tracks: bool,
    pub straight: bool,
    pub apart: bool,
    pub head_room: f64,
    pub groups: Option<Groups>,
    pub order_only: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayeredResult {
    pub width: f64,
    pub height: f64,
    pub crossings: Option<f64>,
    pub declared: Option<f64>,
}

pub fn lnode(w: f64, h: f64, kind: u8) -> LNode {
    LNode { w, h, kind, pin: 0, hint: f64::NAN, key: f64::NAN, span: 0.0, x: 0.0, y: 0.0, rank: 0 }
}

pub fn ledge(tail: usize, head: usize, minlen: f64, label_w: f64, label_h: f64) -> LEdge {
    LEdge { tail, head, minlen, label_w, label_h, tail_dx: 0.0, head_dx: 0.0, tail_fixed: false, head_fixed: false, reversed: false, points: Vec::new(), label_x: 0.0, label_y: 0.0 }
}

const MAX_SWEEPS: usize = 24;
const MAX_DUMMIES: i64 = 20000;
const PORT_PITCH: f64 = 16.0;
const MIN_PITCH: f64 = 8.0;
const MIN_SLOPE: f64 = 0.125;
const MAX_GAP: f64 = 3.0;
const TRACK_MARGIN: f64 = 18.0;
const TRACK_PITCH: f64 = 10.0;
const TRACK_SPREAD: f64 = 14.0;
const LEVEL_WIDE: f64 = 2.0;
const LEVEL_MARGIN: f64 = 28.0;
const LEVEL_PITCH: f64 = 12.0;
const LEVEL_LEAD: f64 = 12.0;
const LEVEL_CLEAR: f64 = 8.0;
const LEVEL_TOUCH: f64 = 4.0;
const CROSSED_SLOPE: f64 = 0.5;
const IN_LINE: f64 = 0.5;
const TWIN: f64 = 1.0;
const SNAP: f64 = 12.0;
const BEND_SNAP: f64 = 24.0;
const LEAN_CLEAR: f64 = 40.0;
const MAX_WEIGHED: usize = 400;
const SAMPLES: usize = 16;

// What an end of a segment is to the node it meets.
const HELD: u8 = 0;
const LEAVES: u8 = 1;
const ARRIVES: u8 = 2;

fn is_dummy(k: u8) -> bool {
    k == DUMMY || k == LABEL
}

fn sort_range(list: &mut [usize], from: usize, to: usize, key: &[i64]) {
    for i in from + 1..to {
        let item = list[i];
        let k = key[item];
        let mut j = i as isize - 1;
        while j >= from as isize && (key[list[j as usize]] > k || (key[list[j as usize]] == k && list[j as usize] > item)) {
            list[(j + 1) as usize] = list[j as usize];
            j -= 1;
        }
        list[(j + 1) as usize] = item;
    }
}

fn sort_by_bary(list: &mut [usize], count: usize, bary: &[f64], pos: &[usize], right: bool, spare: &mut [usize]) {
    let before = |a: usize, b: usize| bary[a] < bary[b] || (bary[a] == bary[b] && if right { pos[a] > pos[b] } else { pos[a] < pos[b] });
    const RUN: usize = 8;
    let mut s = 0;
    while s < count {
        let end = count.min(s + RUN);
        for i in s + 1..end {
            let item = list[i];
            let mut j = i as isize - 1;
            while j >= s as isize && before(item, list[j as usize]) {
                list[(j + 1) as usize] = list[j as usize];
                j -= 1;
            }
            list[(j + 1) as usize] = item;
        }
        s += RUN;
    }
    let mut from_is_list = true;
    let mut width = RUN;
    while width < count {
        {
            let (from, to): (&mut [usize], &mut [usize]) = if from_is_list { (&mut *list, &mut *spare) } else { (&mut *spare, &mut *list) };
            let mut s = 0;
            while s < count {
                let mid = count.min(s + width);
                let end = count.min(s + 2 * width);
                let (mut i, mut j, mut k) = (s, mid, s);
                while i < mid && j < end {
                    if before(from[j], from[i]) {
                        to[k] = from[j];
                        j += 1;
                    } else {
                        to[k] = from[i];
                        i += 1;
                    }
                    k += 1;
                }
                while i < mid {
                    to[k] = from[i];
                    k += 1;
                    i += 1;
                }
                while j < end {
                    to[k] = from[j];
                    k += 1;
                    j += 1;
                }
                s += 2 * width;
            }
        }
        from_is_list = !from_is_list;
        width *= 2;
    }
    if !from_is_list {
        list[..count].copy_from_slice(&spare[..count]);
    }
}

pub struct Graph {
    n: usize,
    real: usize,
    max_rank: usize,
    w: Vec<f64>,
    kind: Vec<u8>,
    rank: Vec<i32>,
    // Segment endpoints, and where each segment attaches on them.
    ef: Vec<usize>,
    et: Vec<usize>,
    efd: Vec<f64>,
    etd: Vec<f64>,
    succ_start: Vec<usize>,
    succ_e: Vec<usize>,
    pred_start: Vec<usize>,
    pred_e: Vec<usize>,
    layer_start: Vec<usize>,
    order: Vec<usize>,
    pos: Vec<usize>,
    offsets: bool,
    // Ranks whose order must remain fixed.
    fixed: Vec<u8>,
    grp: Option<Vec<i32>>,
    group_parent: Option<Vec<i32>>,
}

struct Pieces {
    from: Vec<f64>,
    to: Vec<f64>,
    rank: Vec<usize>,
    lower: Vec<i64>,
    track: Vec<i64>,
    levels: Vec<i64>,
    wide: Vec<u8>,
}

pub fn layered(nodes: &mut [LNode], edges: &mut [LEdge], opt: &LayeredOptions) -> LayeredResult {
    let real = nodes.len();
    if real == 0 {
        return LayeredResult::default();
    }
    let m = edges.len();
    if real == 1 && m == 0 && nodes[0].kind == NODE {
        let node = &mut nodes[0];
        node.rank = 0;
        node.x = node.w / 2.0;
        node.y = node.h / 2.0;
        return LayeredResult { width: node.w, height: node.h, ..Default::default() };
    }

    // Every edge gets a middle rank of its own when labels or parallel edges need the room.
    let mut step: i32 = 1;
    if m > 0 {
        let mut seen = std::collections::HashSet::new();
        for e in edges.iter() {
            let a = e.tail.min(e.head);
            let b = e.tail.max(e.head);
            let key = a * real + b;
            if e.label_w > 0.0 || seen.contains(&key) {
                step = 2;
                break;
            }
            seen.insert(key);
        }
    }

    orient(nodes, edges);
    rank(nodes, edges, step);

    let mut span = vec![0i64; m];
    let mut dummies: i64 = 0;
    for ei in 0..m {
        let e = &edges[ei];
        span[ei] = (nodes[e.head].rank as i64 - nodes[e.tail].rank as i64).abs();
        if span[ei] > 1 {
            dummies += span[ei] - 1;
        }
    }
    let mut direct = vec![0u8; m];
    if dummies > MAX_DUMMIES {
        let mut longest: Vec<usize> = (0..m).filter(|&ei| span[ei] > 1).collect();
        longest.sort_by(|&a, &b| span[b].cmp(&span[a]).then(a.cmp(&b)));
        for ei in longest {
            if dummies <= MAX_DUMMIES {
                break;
            }
            direct[ei] = 1;
            dummies -= span[ei] - 1;
        }
    }

    let n = real + dummies as usize;
    let mut w = vec![0f64; n];
    let mut hh = vec![0f64; n];
    let mut kinds = vec![0u8; n];
    let mut ranks = vec![0i32; n];
    let mut max_rank: usize = 0;
    for i in 0..real {
        let node = &nodes[i];
        w[i] = node.w;
        hh[i] = node.h;
        kinds[i] = node.kind;
        ranks[i] = node.rank;
        if node.rank as usize > max_rank {
            max_rank = node.rank as usize;
        }
    }

    // The graph that gets ordered and positioned: every edge cut into rank-to-rank segments.
    let mut segments = 0usize;
    for ei in 0..m {
        if direct[ei] == 0 {
            segments += span[ei] as usize;
        }
    }
    let mut ef = vec![0usize; segments];
    let mut et = vec![0usize; segments];
    let mut efd = vec![0f64; segments];
    let mut etd = vec![0f64; segments];
    let mut first_dummy = vec![-1i64; m];
    // Which way the edges with a label in each rank head: 1 down, 2 up, 3 both.
    let mut heads = vec![0u8; max_rank + 1];
    let mut label_node = vec![-1i64; m];
    let spans = (0..real).any(|i| nodes[i].span > 0.0 && nodes[i].kind == NODE);
    let mut efk: Option<Vec<u8>> = if spans { Some(vec![0u8; segments]) } else { None };
    let mut etk: Option<Vec<u8>> = if spans { Some(vec![0u8; segments]) } else { None };
    let mut first_seg = vec![-1i64; m];
    let mut last_seg = vec![-1i64; m];
    let mut key = vec![f64::NAN; n];
    for i in 0..real {
        key[i] = nodes[i].key;
    }
    let mut offsets = false;
    {
        let mut next = real;
        let mut ce = 0usize;
        for ei in 0..m {
            if direct[ei] != 0 {
                continue;
            }
            let e = &edges[ei];
            let a = if e.reversed { e.head } else { e.tail };
            let b = if e.reversed { e.tail } else { e.head };
            let a_dx = if e.reversed { e.head_dx } else { e.tail_dx };
            let b_dx = if e.reversed { e.tail_dx } else { e.head_dx };
            if a_dx != 0.0 || b_dx != 0.0 {
                offsets = true;
            }
            let ra = ranks[a];
            let rb = ranks[b];
            let mut label_rank: i32 = -1;
            if e.label_w > 0.0 {
                // The label goes halfway along: see upstream for how a rank is chosen.
                label_rank = ra + ((rb - ra) >> 1);
                if step == 2 && rb - ra >= 4 {
                    if label_rank & 1 == 1 {
                        label_rank += if e.reversed { 1 } else { -1 };
                    }
                } else if step == 2 && label_rank & 1 == 0 {
                    label_rank += if e.reversed { 1 } else { -1 };
                }
            }
            let mut prev = a;
            if rb - ra > 1 {
                first_dummy[ei] = next as i64;
            }
            first_seg[ei] = ce as i64;
            if let Some(k) = efk.as_mut() {
                if !(if e.reversed { e.head_fixed } else { e.tail_fixed }) {
                    k[ce] = if e.reversed { ARRIVES } else { LEAVES };
                }
            }
            for r in ra + 1..rb {
                let d = next;
                next += 1;
                ranks[d] = r;
                key[d] = key[a] + ((key[b] - key[a]) * (r - ra) as f64) / (rb - ra) as f64;
                if r == label_rank {
                    w[d] = e.label_w;
                    hh[d] = e.label_h;
                    kinds[d] = LABEL;
                    label_node[ei] = d as i64;
                    heads[r as usize] |= if e.reversed { 2 } else { 1 };
                } else {
                    kinds[d] = DUMMY;
                }
                ef[ce] = prev;
                et[ce] = d;
                efd[ce] = if prev == a { a_dx } else { 0.0 };
                ce += 1;
                prev = d;
            }
            ef[ce] = prev;
            et[ce] = b;
            efd[ce] = if prev == a { a_dx } else { 0.0 };
            etd[ce] = b_dx;
            if let Some(k) = etk.as_mut() {
                if !(if e.reversed { e.tail_fixed } else { e.head_fixed }) {
                    k[ce] = if e.reversed { LEAVES } else { ARRIVES };
                }
            }
            last_seg[ei] = ce as i64;
            ce += 1;
        }
    }

    // Bends belong to the innermost group containing both edge endpoints.
    let mut grp: Option<Vec<i32>> = None;
    if let Some(groups) = &opt.groups {
        let of = &groups.of;
        let parent = &groups.parent;
        let mut g = vec![0i32; n];
        g[..real].copy_from_slice(&of[..real]);
        let depth = |mut gr: i32| {
            let mut d = 0;
            while gr >= 0 {
                d += 1;
                gr = parent[gr as usize];
            }
            d
        };
        let lca = |mut x: i32, mut y: i32| {
            let mut dx = depth(x);
            let mut dy = depth(y);
            while dx > dy {
                x = parent[x as usize];
                dx -= 1;
            }
            while dy > dx {
                y = parent[y as usize];
                dy -= 1;
            }
            while x != y {
                x = parent[x as usize];
                y = parent[y as usize];
            }
            x
        };
        for ei in 0..m {
            if first_dummy[ei] < 0 {
                continue;
            }
            let e = &edges[ei];
            let gr = lca(of[e.tail], of[e.head]);
            let d0 = first_dummy[ei] as usize;
            for d in d0..d0 + span[ei] as usize - 1 {
                g[d] = gr;
            }
        }
        grp = Some(g);
    }

    let mut succ_start = vec![0usize; n + 1];
    let mut pred_start = vec![0usize; n + 1];
    for e in 0..segments {
        succ_start[ef[e] + 1] += 1;
        pred_start[et[e] + 1] += 1;
    }
    for i in 0..n {
        succ_start[i + 1] += succ_start[i];
        pred_start[i + 1] += pred_start[i];
    }
    let mut succ_e = vec![0usize; segments];
    let mut pred_e = vec![0usize; segments];
    {
        let mut sf = succ_start[..n].to_vec();
        let mut pf = pred_start[..n].to_vec();
        for e in 0..segments {
            succ_e[sf[ef[e]]] = e;
            sf[ef[e]] += 1;
            pred_e[pf[et[e]]] = e;
            pf[et[e]] += 1;
        }
    }

    let mut layer_start = vec![0usize; max_rank + 2];
    for i in 0..n {
        layer_start[ranks[i] as usize + 1] += 1;
    }
    for r in 0..=max_rank {
        layer_start[r + 1] += layer_start[r];
    }
    let mut order = vec![0usize; n];
    let pos = vec![0usize; n];

    // Initial order: depth-first from the top so that subtrees start out contiguous.
    {
        let mut by_rank = vec![0usize; n];
        let mut fill = layer_start[..max_rank + 1].to_vec();
        for i in 0..n {
            let r = ranks[i] as usize;
            by_rank[fill[r]] = i;
            fill[r] += 1;
        }
        fill[..(max_rank + 1)].copy_from_slice(&layer_start[..(max_rank + 1)]);
        let mut visited = vec![0u8; n];
        let mut stack: Vec<usize> = Vec::new();
        for k in 0..n {
            if visited[by_rank[k]] != 0 {
                continue;
            }
            stack.push(by_rank[k]);
            while let Some(v) = stack.pop() {
                if visited[v] != 0 {
                    continue;
                }
                visited[v] = 1;
                let r = ranks[v] as usize;
                order[fill[r]] = v;
                fill[r] += 1;
                let mut j = succ_start[v + 1] as isize - 1;
                while j >= succ_start[v] as isize {
                    let w2 = et[succ_e[j as usize]];
                    if visited[w2] == 0 {
                        stack.push(w2);
                    }
                    j -= 1;
                }
            }
        }
    }

    let sort_rank = |order: &mut Vec<usize>, r: usize, key: &dyn Fn(usize) -> f64| -> bool {
        let s = layer_start[r];
        let e = layer_start[r + 1];
        if e - s < 2 {
            return false;
        }
        for i in s..e {
            if key(order[i]).is_nan() {
                return false;
            }
        }
        let mut layer: Vec<(usize, usize)> = order[s..e].iter().enumerate().map(|(i, &v)| (v, i)).collect();
        layer.sort_by(|a, b| js::ord(js::or(key(a.0) - key(b.0), || a.1 as f64 - b.1 as f64)));
        for (i, (v, _)) in layer.into_iter().enumerate() {
            order[s + i] = v;
        }
        true
    };
    for r in 0..=max_rank {
        sort_rank(&mut order, r, &|v| key[v]);
    }

    let pinned = (0..real).any(|i| nodes[i].pin != 0);
    if pinned {
        for r in 0..=max_rank {
            let s = layer_start[r];
            let mut layer: Vec<(usize, usize)> = order[s..layer_start[r + 1]].iter().enumerate().map(|(i, &v)| (v, i)).collect();
            let pin_of = |v: usize| if v < real { nodes[v].pin } else { 0 };
            layer.sort_by(|a, b| pin_of(a.0).cmp(&pin_of(b.0)).then(a.1.cmp(&b.1)));
            for (i, (v, _)) in layer.into_iter().enumerate() {
                order[s + i] = v;
            }
        }
    }
    // Preserve port order to avoid crossings outside the group.
    let mut fixed = vec![0u8; max_rank + 1];
    {
        let hint = |v: usize| if v < real && (nodes[v].kind == START_PORT || nodes[v].kind == END_PORT) { nodes[v].hint } else { f64::NAN };
        for r in [0, max_rank] {
            if sort_rank(&mut order, r, &hint) {
                fixed[r] = 1;
            }
        }
    }

    let mut g = Graph {
        n,
        real,
        max_rank,
        w,
        kind: kinds,
        rank: ranks,
        ef,
        et,
        efd,
        etd,
        succ_start,
        succ_e,
        pred_start,
        pred_e,
        layer_start,
        order,
        pos,
        offsets,
        fixed,
        grp,
        group_parent: opt.groups.as_ref().map(|gr| gr.parent.clone()),
    };
    set_positions(&mut g);

    let mut crossings = 0f64;
    let mut declared = 0f64;
    if segments > 0 && max_rank > 0 {
        let (c, d) = reduce_crossings(&mut g, nodes);
        crossings = c as f64;
        declared = d as f64;
        set_positions(&mut g);
    }
    if opt.order_only {
        // Points are (rank, cross-flow position) pairs.
        let at = position(&mut g, opt);
        for i in 0..real {
            nodes[i].x = at[i];
        }
        for ei in 0..m {
            let e = &mut edges[ei];
            let a = if e.reversed { e.head } else { e.tail };
            let b = if e.reversed { e.tail } else { e.head };
            let mut pts = vec![g.rank[a] as f64, at[a]];
            if direct[ei] == 0 && first_dummy[ei] >= 0 {
                let d0 = first_dummy[ei] as usize;
                for d in d0..d0 + span[ei] as usize - 1 {
                    pts.push(g.rank[d] as f64);
                    pts.push(at[d]);
                }
            }
            pts.push(g.rank[b] as f64);
            pts.push(at[b]);
            e.points = pts;
        }
        return LayeredResult { width: 0.0, height: 0.0, crossings: Some(crossings), declared: Some(declared) };
    }

    if let (Some(fk), Some(tk)) = (&efk, &etk) {
        spread(&mut g, nodes, fk, tk);
    }

    let mut xs = position(&mut g, opt);

    let empty = empty_ranks(&g);
    if let (Some(fk), Some(tk)) = (&efk, &etk) {
        let free = free_ends(&g, fk, tk);
        align_labels(&g, nodes, &mut xs, &free, opt, empty.as_deref());
        settle(&mut g, nodes, &xs, fk, tk, &free, empty.as_deref());
        snap(&g, &mut xs, empty.as_deref(), opt);
        for ei in 0..m {
            if direct[ei] != 0 {
                continue;
            }
            let e = &mut edges[ei];
            let a_dx = g.efd[first_seg[ei] as usize];
            let b_dx = g.etd[last_seg[ei] as usize];
            e.tail_dx = if e.reversed { b_dx } else { a_dx };
            e.head_dx = if e.reversed { a_dx } else { b_dx };
        }
    }

    align_ports(&g, &mut xs, empty.as_deref());

    // A rank that holds nothing but bends: its bends move onto the straight line between the
    // ends of their edges, and one that gets there is left out of the route.
    let mut on_line = vec![0u8; n];
    if let Some(em) = &empty {
        if !opt.tracks {
            straighten(&g, &mut xs, em, &first_dummy, &span, if opt.apart { 0.0 } else { opt.edge_sep }, &mut on_line);
        }
    }
    if !opt.tracks {
        past_labels(&g, &mut xs, &first_dummy, &span, opt.edge_sep, &mut on_line);
    }
    // An edge is cut where it passes a rank that holds something, and each piece crosses one gap.
    let mut piece_start = vec![0usize; m + 1];
    let mut pieces: Option<Pieces> = if opt.tracks || (efk.is_some() && !opt.straight) {
        Some(Pieces { from: vec![], to: vec![], rank: vec![], lower: vec![], track: vec![], levels: vec![], wide: vec![] })
    } else {
        None
    };
    let mut track_count = vec![0i64; max_rank + 1];
    if let Some(p) = pieces.as_mut() {
        for ei in 0..m {
            piece_start[ei] = p.from.len();
            if direct[ei] != 0 {
                continue;
            }
            let d0 = first_dummy[ei];
            let end = if d0 < 0 { d0 } else { d0 + span[ei] - 1 };
            // An edge that keeps a bend to stay clear of its twin is drawn through the bend, as it is.
            let mut twin = false;
            let mut d = d0;
            while d < end {
                // So is one that has a bend left out in a rank of labels: its curve crosses that rank.
                let du = d as usize;
                let bare = empty.as_ref().is_some_and(|em| em[g.rank[du] as usize] == 1);
                if !opt.tracks && bare != (on_line[du] == 1) {
                    twin = true;
                }
                d += 1;
            }
            if twin {
                continue;
            }
            let fs = first_seg[ei] as usize;
            let mut x = xs[g.ef[fs]] + g.efd[fs];
            let mut rank = g.rank[g.ef[fs]] as usize;
            let mut d = d0;
            while d < end {
                let du = d as usize;
                if empty.as_ref().is_some_and(|em| em[g.rank[du] as usize] != 0) {
                    d += 1;
                    continue;
                }
                p.from.push(x);
                p.to.push(xs[du]);
                p.rank.push(rank);
                p.lower.push(d);
                x = xs[du];
                rank = g.rank[du] as usize;
                d += 1;
            }
            let ls = last_seg[ei] as usize;
            p.from.push(x);
            p.to.push(xs[g.et[ls]] + g.etd[ls]);
            p.rank.push(rank);
            p.lower.push(-1);
        }
        piece_start[m] = p.from.len();
    }

    // Ranks that hold only border ports or their pass-through dummies need little room of their own.
    let port_only = |r: usize, k: u8| {
        let mut found = false;
        for i in g.layer_start[r]..g.layer_start[r + 1] {
            let kk = g.kind[g.order[i]];
            if kk == k {
                found = true;
            } else if kk != DUMMY {
                return false;
            }
        }
        found
    };
    let start_ranks = if port_only(0, START_PORT) { step as usize } else { 0 };
    let end_ranks = if port_only(max_rank, END_PORT) { step as usize } else { 0 };
    // The longest way across that any edge travels in the gap below each rank.
    let mut run = vec![0f64; max_rank + 1];
    for e in 0..segments {
        let across = (xs[g.ef[e]] + g.efd[e] - xs[g.et[e]] - g.etd[e]).abs();
        let r = g.rank[g.ef[e]] as usize;
        if across > run[r] {
            run[r] = across;
        }
    }
    let mut band_top = vec![0f64; max_rank + 1];
    let mut band_bottom = vec![0f64; max_rank + 1];
    let stepf = step as f64;
    let usual = |r: usize| (if r < start_ranks || r as isize >= max_rank as isize - end_ranks as isize { opt.port_sep } else { opt.rank_sep }) / stepf;
    // The room under each rank.
    let mut gap_under = vec![0f64; max_rank + 1];
    for r in 0..max_rank {
        gap_under[r] = if pieces.is_some() { usual(r) } else { usual(r).max(((MAX_GAP * opt.rank_sep) / stepf).min(run[r] * MIN_SLOPE)) };
    }
    // A rank between two ranks of nodes, which holds labels and bends only.
    let mut between = vec![1u8; max_rank + 1];
    for i in 0..real {
        between[g.rank[i] as usize] = 0;
    }
    for r in 1..max_rank {
        if opt.tracks || between[r] == 0 || (heads[r] != 1 && heads[r] != 2) {
            continue;
        }
        let shift = (opt.head_room / 2.0).min(gap_under[if heads[r] == 1 { r - 1 } else { r }] / 2.0);
        gap_under[r - 1] += if heads[r] == 1 { -shift } else { shift };
        gap_under[r] += if heads[r] == 1 { shift } else { -shift };
    }
    let margin = if opt.tracks { TRACK_MARGIN } else { LEVEL_MARGIN };
    let least_pitch = if opt.tracks { TRACK_PITCH } else { LEVEL_PITCH };
    let most_pitch = if opt.tracks { TRACK_SPREAD } else { LEVEL_PITCH };
    let mut below = vec![0usize; max_rank + 1];
    if let Some(p) = pieces.as_mut() {
        let mut room = vec![0f64; max_rank + 1];
        for r in 0..max_rank {
            if empty.as_ref().is_some_and(|em| em[r] != 0) {
                continue;
            }
            let mut next = r + 1;
            room[r] = gap_under[r];
            while next < max_rank && empty.as_ref().is_some_and(|em| em[next] != 0) {
                room[r] += gap_under[next];
                next += 1;
            }
            below[r] = next;
        }
        for k in 0..p.from.len() {
            let wide = (p.from[k] - p.to[k]).abs() > LEVEL_WIDE * room[p.rank[k]];
            p.wide.push(wide as u8);
        }
        let mut tall = vec![0f64; max_rank + 1];
        assign_tracks(p, &mut track_count, !opt.tracks, &room, &mut tall);
        for r in 0..max_rank {
            if track_count[r] > 0 {
                gap_under[r] += (2.0 * margin + (track_count[r] - 1) as f64 * least_pitch - room[r]).max(0.0);
            } else if tall[r] > 0.0 {
                gap_under[r] += ((MAX_GAP * opt.rank_sep).min(tall[r]) - room[r]).max(0.0);
            }
        }
    }
    let mut y = 0.0;
    for r in 0..=max_rank {
        let mut h = 0f64;
        for i in g.layer_start[r]..g.layer_start[r + 1] {
            if hh[g.order[i]] > h {
                h = hh[g.order[i]];
            }
        }
        band_top[r] = y;
        band_bottom[r] = y + h;
        y += h + gap_under[r];
    }
    let height = y;
    // The part of its rank that an edge runs straight through at a bend or label.
    let inset = |d: usize| {
        let r = g.rank[d] as usize;
        if g.kind[d] == DUMMY && between[r] != 0 && !opt.tracks {
            (band_bottom[r] - band_top[r]) / 4.0
        } else {
            0.0
        }
    };
    let track_y = |p: &Pieces, piece: usize, on_track: i64| {
        let r = p.rank[piece];
        let top = band_bottom[r];
        let bottom = band_top[below[r]];
        let count = if opt.tracks { track_count[r] } else { p.levels[piece] };
        let pitch = if count > 1 { most_pitch.min((bottom - top - 2.0 * margin) / (count - 1) as f64) } else { 0.0 };
        (top + bottom) / 2.0 + (on_track as f64 - (count - 1) as f64 / 2.0) * pitch
    };

    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    for i in 0..n {
        let half = g.w[i] / 2.0;
        if xs[i] - half < min_x {
            min_x = xs[i] - half;
        }
        if xs[i] + half > max_x {
            max_x = xs[i] + half;
        }
    }
    // A rank whose nodes only send edges on stands on one line, and one whose nodes only
    // receive them hangs from one. In any other rank the nodes are centered.
    let mut arrives = vec![0u8; max_rank + 1];
    let mut leaves = vec![0u8; max_rank + 1];
    for i in 0..real {
        if g.kind[i] != NODE || efk.is_none() {
            continue;
        }
        if g.pred_start[i + 1] > g.pred_start[i] {
            arrives[g.rank[i] as usize] = 1;
        }
        if g.succ_start[i + 1] > g.succ_start[i] {
            leaves[g.rank[i] as usize] = 1;
        }
    }
    for i in 0..real {
        let node = &mut nodes[i];
        let r = node.rank as usize;
        node.x = xs[i] - min_x;
        if g.kind[i] == NODE && leaves[r] != 0 && arrives[r] == 0 {
            node.y = band_bottom[r] - node.h / 2.0;
        } else if g.kind[i] == NODE && arrives[r] != 0 && leaves[r] == 0 {
            node.y = band_top[r] + node.h / 2.0;
        } else {
            node.y = (band_top[r] + band_bottom[r]) / 2.0;
        }
    }

    for ei in 0..m {
        let (fx, fy, fh, lx, ly, lh) = {
            let e = &edges[ei];
            let first = &nodes[if e.reversed { e.head } else { e.tail }];
            let last = &nodes[if e.reversed { e.tail } else { e.head }];
            (first.x, first.y, first.h, last.x, last.y, last.h)
        };
        let e = &mut edges[ei];
        let mut pts: Vec<f64> = vec![fx + if e.reversed { e.head_dx } else { e.tail_dx }, fy + fh / 2.0];
        let d0 = first_dummy[ei];
        if pieces.as_ref().is_some_and(|_| piece_start[ei + 1] > piece_start[ei]) {
            let p = pieces.as_ref().unwrap();
            for k in piece_start[ei]..piece_start[ei + 1] {
                if p.track[k] >= 0 && (opt.tracks || p.wide[k] == 1) {
                    let across = track_y(p, k, p.track[k]);
                    pts.extend_from_slice(&[p.from[k] - min_x, across, p.to[k] - min_x, across]);
                } else if p.track[k] > 0 {
                    // A short piece among levels runs straight until it is past the level before
                    // its own, and turns in one S from there.
                    pts.push(p.from[k] - min_x);
                    pts.push(track_y(p, k, p.track[k] - 1));
                }
                let d = p.lower[k];
                if d < 0 {
                    continue;
                }
                let d = d as usize;
                let r = g.rank[d] as usize;
                pts.push(xs[d] - min_x);
                pts.push(band_top[r] + inset(d));
                if band_bottom[r] > band_top[r] {
                    pts.push(xs[d] - min_x);
                    pts.push(band_bottom[r] - inset(d));
                }
            }
        } else if d0 >= 0 {
            let d0 = d0 as usize;
            for d in d0..d0 + span[ei] as usize - 1 {
                if on_line[d] != 0 {
                    continue;
                }
                let x = xs[d] - min_x;
                let r = g.rank[d] as usize;
                let top = band_top[r];
                let bottom = band_bottom[r];
                pts.push(x);
                pts.push(top + inset(d));
                if bottom > top {
                    pts.push(x);
                    pts.push(bottom - inset(d));
                }
            }
        }
        pts.push(lx + if e.reversed { e.tail_dx } else { e.head_dx });
        pts.push(ly - lh / 2.0);
        if e.reversed {
            let (mut i, mut j) = (0usize, pts.len() - 2);
            while i < j {
                pts.swap(i, j);
                pts.swap(i + 1, j + 1);
                i += 2;
                j -= 2;
            }
        }
        let ln = label_node[ei];
        if ln >= 0 {
            let ln = ln as usize;
            let r = g.rank[ln] as usize;
            e.label_x = xs[ln] - min_x;
            e.label_y = (band_top[r] + band_bottom[r]) / 2.0;
        } else if e.label_w > 0.0 {
            e.label_x = (pts[0] + pts[pts.len() - 2]) / 2.0;
            e.label_y = (pts[1] + pts[pts.len() - 1]) / 2.0;
        }
        e.points = pts;
    }

    LayeredResult { width: max_x - min_x, height, ..Default::default() }
}

fn set_positions(g: &mut Graph) {
    for r in 0..=g.max_rank {
        let s = g.layer_start[r];
        for i in s..g.layer_start[r + 1] {
            g.pos[g.order[i]] = i - s;
        }
    }
}

// Reverses the edges that close a cycle, found by depth-first search in declaration order.
pub fn orient(nodes: &[LNode], edges: &mut [LEdge]) {
    let n = nodes.len();
    let m = edges.len();
    let mut start = vec![0usize; n + 1];
    for e in edges.iter_mut() {
        e.reversed = nodes[e.head].kind == START_PORT || nodes[e.tail].kind == END_PORT;
        if !e.reversed {
            start[e.tail + 1] += 1;
        }
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut out = vec![0usize; start[n]];
    {
        let mut fill = start[..n].to_vec();
        for i in 0..m {
            if !edges[i].reversed {
                out[fill[edges[i].tail]] = i;
                fill[edges[i].tail] += 1;
            }
        }
    }
    let mut state = vec![0u8; n];
    let mut cursor = start[..n].to_vec();
    let mut stack: Vec<usize> = Vec::new();
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        stack.push(root);
        state[root] = 1;
        while let Some(&v) = stack.last() {
            if cursor[v] < start[v + 1] {
                let ei = out[cursor[v]];
                cursor[v] += 1;
                let w = edges[ei].head;
                if state[w] == 1 {
                    edges[ei].reversed = true;
                } else if state[w] == 0 {
                    state[w] = 1;
                    stack.push(w);
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }
}

pub fn rank(nodes: &mut [LNode], edges: &[LEdge], step: i32) {
    let n = nodes.len();
    let m = edges.len();
    let mut from = vec![0usize; m];
    let mut to = vec![0usize; m];
    let mut len = vec![0i64; m];
    let mut out_start = vec![0usize; n + 1];
    let mut in_start = vec![0usize; n + 1];
    for i in 0..m {
        let e = &edges[i];
        let a = if e.reversed { e.head } else { e.tail };
        let b = if e.reversed { e.tail } else { e.head };
        from[i] = a;
        to[i] = b;
        // Int32Array truncates.
        len[i] = (1f64.max(e.minlen) * step as f64) as i64;
        out_start[a + 1] += 1;
        in_start[b + 1] += 1;
    }
    for i in 0..n {
        out_start[i + 1] += out_start[i];
        in_start[i + 1] += in_start[i];
    }
    let mut out_e = vec![0usize; m];
    let mut in_e = vec![0usize; m];
    {
        let mut of = out_start[..n].to_vec();
        let mut inf = in_start[..n].to_vec();
        for i in 0..m {
            out_e[of[from[i]]] = i;
            of[from[i]] += 1;
            in_e[inf[to[i]]] = i;
            inf[to[i]] += 1;
        }
    }

    let mut r = vec![0i64; n];
    let mut indeg: Vec<i64> = (0..n).map(|i| (in_start[i + 1] - in_start[i]) as i64).collect();
    let mut topo = vec![0usize; n];
    let mut count = 0;
    for i in 0..n {
        if indeg[i] == 0 {
            topo[count] = i;
            count += 1;
        }
    }
    let mut q = 0;
    while q < count {
        let v = topo[q];
        for k in out_start[v]..out_start[v + 1] {
            let ei = out_e[k];
            let w = to[ei];
            if r[v] + len[ei] > r[w] {
                r[w] = r[v] + len[ei];
            }
            indeg[w] -= 1;
            if indeg[w] == 0 {
                topo[count] = w;
                count += 1;
            }
        }
        q += 1;
    }

    // Longest-path ranking leaves loosely attached nodes far from their neighbours.
    // Move each node toward the side that has more edges.
    for _pass in 0..8 {
        let mut changed = false;
        for q in (0..count).rev() {
            let v = topo[q];
            let ins = in_start[v + 1] - in_start[v];
            let outs = out_start[v + 1] - out_start[v];
            if ins == 0 && outs == 0 {
                continue;
            }
            let mut lo = i64::MIN;
            let mut hi = i64::MAX;
            for k in in_start[v]..in_start[v + 1] {
                let ei = in_e[k];
                lo = lo.max(r[from[ei]] + len[ei]);
            }
            for k in out_start[v]..out_start[v + 1] {
                let ei = out_e[k];
                hi = hi.min(r[to[ei]] - len[ei]);
            }
            let mut next = r[v];
            if outs > ins && hi != i64::MAX {
                next = hi;
            } else if ins > outs && lo != i64::MIN {
                next = lo;
            }
            if next != r[v] {
                r[v] = next;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut min = i64::MAX;
    let mut max = i64::MIN;
    let mut start_ports = false;
    for i in 0..n {
        let k = nodes[i].kind;
        if k == START_PORT {
            start_ports = true;
        }
        if k == START_PORT || k == END_PORT {
            continue;
        }
        min = min.min(r[i]);
        max = max.max(r[i]);
    }
    if min == i64::MAX {
        min = 0;
        max = 0;
    }
    let step = step as i64;
    for i in 0..n {
        if nodes[i].kind == START_PORT {
            r[i] = min - step;
        } else if nodes[i].kind == END_PORT {
            r[i] = max + step;
        }
    }
    if start_ports {
        min -= step;
    }
    for i in 0..n {
        nodes[i].rank = (r[i] - min) as i32;
    }
}

// The ends of edges that may move along one side of a node (0 the side its edges arrive at, 1
// the other), ordered by their opposite nodes. Returns the sum of the gaps.
fn side_ends(g: &Graph, v: usize, side: usize, span: f64, kinds: &[u8], ids: &mut Vec<usize>, room: &mut Vec<f64>) -> f64 {
    let start = if side == 0 { &g.pred_start } else { &g.succ_start };
    let list = if side == 0 { &g.pred_e } else { &g.succ_e };
    let other = if side == 0 { &g.ef } else { &g.et };
    let pos = &g.pos;
    for k in start[v]..start[v + 1] {
        if kinds[list[k]] != HELD {
            ids.push(list[k]);
        }
    }
    if ids.len() < 2 {
        return 0.0;
    }
    ids.sort_by(|&a, &b| pos[other[a]].cmp(&pos[other[b]]).then(a.cmp(&b)));
    let fill = |room: &mut Vec<f64>, share: &dyn Fn(bool, bool) -> f64| {
        let mut total = 0.0;
        room.clear();
        room.resize(ids.len(), 0.0);
        room[0] = 0.0;
        for k in 1..ids.len() {
            let same = kinds[ids[k]] == kinds[ids[k - 1]];
            room[k] = share(same && kinds[ids[k]] == LEAVES, same);
            total += room[k];
        }
        total
    };
    let mut total = fill(room, &|_, _| 1.0);
    if span / total < MIN_PITCH {
        total = fill(room, &|leaves, _| if leaves { 0.5 } else { 1.0 });
    }
    if span / total < MIN_PITCH {
        total = fill(room, &|_, same| if same { 0.0 } else { 1.0 });
    }
    if total == 0.0 {
        return 0.0;
    }
    let pitch = PORT_PITCH.min(span / total);
    for k in 1..ids.len() {
        room[k] *= pitch;
    }
    total * pitch
}

// Gives the edges that meet one side of a node places of their own along it, around its middle.
fn spread(g: &mut Graph, nodes: &[LNode], efk: &[u8], etk: &[u8]) {
    for v in 0..g.real {
        let span = nodes[v].span.min(g.w[v]);
        if span <= 0.0 || nodes[v].kind != NODE {
            continue;
        }
        for side in 0..2 {
            let mut ids = Vec::new();
            let mut room = Vec::new();
            let total = side_ends(g, v, side, span, if side == 0 { etk } else { efk }, &mut ids, &mut room);
            if total == 0.0 {
                continue;
            }
            let dx = if side == 0 { &mut g.etd } else { &mut g.efd };
            let mut at = -total / 2.0;
            for k in 0..ids.len() {
                at += room[k];
                dx[ids[k]] = at;
            }
        }
    }
}

// The segment of an edge that reaches the next point held in place, on the way up from segment
// `e` (side 0) or down: past the bends in ranks that hold nothing else.
fn follow(g: &Graph, empty: Option<&[u8]>, mut e: usize, side: usize) -> usize {
    let Some(empty) = empty else { return e };
    for _ in 0..100000 {
        let u = if side == 0 { g.ef[e] } else { g.et[e] };
        if g.kind[u] != DUMMY || empty[g.rank[u] as usize] == 0 {
            break;
        }
        e = if side == 0 { g.pred_e[g.pred_start[u]] } else { g.succ_e[g.succ_start[u]] };
    }
    e
}

// Aligns border ports with the ends on their nodes so edges run straight across the gap.
fn align_ports(g: &Graph, xs: &mut [f64], empty: Option<&[u8]>) {
    for (r, k) in [(0, START_PORT), (g.max_rank, END_PORT)] {
        let s = g.layer_start[r];
        let end = g.layer_start[r + 1];
        let mut to: Vec<f64> = Vec::new();
        for i in s..end {
            let v = g.order[i];
            let start = k == START_PORT;
            let degree = if start { g.succ_start[v + 1] - g.succ_start[v] } else { g.pred_start[v + 1] - g.pred_start[v] };
            if g.kind[v] != k || degree != 1 {
                break;
            }
            let e = follow(g, empty, if start { g.succ_e[g.succ_start[v]] } else { g.pred_e[g.pred_start[v]] }, if start { 1 } else { 0 });
            let at = if start { xs[g.et[e]] + g.etd[e] } else { xs[g.ef[e]] + g.efd[e] };
            if let Some(&last) = to.last() {
                if at < last + TWIN {
                    break;
                }
            }
            to.push(at);
        }
        if to.is_empty() || to.len() != end - s {
            continue;
        }
        for i in s..end {
            xs[g.order[i]] = to[i - s];
        }
    }
}

// How many ends that may move along it each node has on the side its edges arrive at, and on the other.
fn free_ends(g: &Graph, efk: &[u8], etk: &[u8]) -> [Vec<i64>; 2] {
    let mut free = [vec![0i64; g.n], vec![0i64; g.n]];
    for e in 0..g.ef.len() {
        if etk[e] != HELD {
            free[0][g.et[e]] += 1;
        }
        if efk[e] != HELD {
            free[1][g.ef[e]] += 1;
        }
    }
    free
}

// How far along the flow the handles of a plain S reach, as a share of the way it travels along it.
pub fn ease(across: f64, along: f64) -> f64 {
    0.25f64.max(0.5f64.min((4.0 * along.abs()) / js::or(across.abs(), || 1.0)))
}

// Brings the labels and bends of each edge into line with an end of the edge.
fn align_labels(g: &Graph, nodes: &[LNode], xs: &mut [f64], free: &[Vec<i64>; 2], opt: &LayeredOptions, empty: Option<&[u8]>) {
    let sep = opt.edge_sep;
    let placed = |v: usize| g.kind[v] == LABEL || (g.kind[v] == DUMMY && !empty.is_some_and(|em| em[g.rank[v] as usize] == 1));
    let end = |xs: &[f64], e: usize, side: usize| -> (f64, i64) {
        let u = if side == 0 { g.ef[e] } else { g.et[e] };
        let x = if side == 0 { xs[u] + g.efd[e] } else { xs[u] + g.etd[e] };
        (x, if u < g.real && nodes[u].kind == NODE { free[1 - side][u] } else { 0 })
    };
    // The room the neighbours in its rank leave a label or bend.
    let room = |xs: &[f64], v: usize| -> (f64, f64) {
        let r = g.rank[v] as usize;
        let s = g.layer_start[r];
        let i = s + g.pos[v];
        let clear = |other: usize| g.w[other] / 2.0 + g.w[v] / 2.0 + if g.kind[other] == NODE { opt.node_sep / 2.0 + sep / 2.0 } else if g.kind[other] == g.kind[v] { sep } else { sep / 2.0 };
        (
            if i > s { xs[g.order[i - 1]] + clear(g.order[i - 1]) } else { f64::NEG_INFINITY },
            if i + 1 < g.layer_start[r + 1] { xs[g.order[i + 1]] - clear(g.order[i + 1]) } else { f64::INFINITY },
        )
    };
    // The place between `lo` and `hi` that is in line with one of two ends, or else nearest the first choice.
    let choose = |x: f64, up: (f64, i64), down: (f64, i64), lo: f64, hi: f64| -> f64 {
        let up_first = if up.1 != down.1 { up.1 < down.1 } else { (up.0 - x).abs() <= (down.0 - x).abs() };
        let choices = if up_first { [up.0, down.0] } else { [down.0, up.0] };
        choices.iter().copied().find(|&c| c >= lo && c <= hi).unwrap_or_else(|| lo.max(hi.min(choices[0])))
    };
    let mut done = vec![0u8; g.n];
    let mut chain: Vec<usize> = Vec::new();
    for r in 1..g.max_rank {
        for i in g.layer_start[r]..g.layer_start[r + 1] {
            if !placed(g.order[i]) || done[g.order[i]] != 0 {
                continue;
            }
            // The edge's labels and bends from its first node to its last, and the room they share.
            let mut top = g.order[i];
            while is_dummy(g.kind[g.ef[g.pred_e[g.pred_start[top]]]]) {
                top = g.ef[g.pred_e[g.pred_start[top]]];
            }
            let mut bottom = top;
            let mut lo = f64::NEG_INFINITY;
            let mut hi = f64::INFINITY;
            chain.clear();
            let mut d = top;
            while is_dummy(g.kind[d]) {
                bottom = d;
                if placed(d) {
                    done[d] = 1;
                    chain.push(d);
                    let (least, most) = room(xs, d);
                    if least > lo {
                        lo = least;
                    }
                    if most < hi {
                        hi = most;
                    }
                }
                d = g.et[g.succ_e[g.succ_start[d]]];
            }
            if lo <= hi {
                let x = choose(xs[chain[0]], end(xs, g.pred_e[g.pred_start[top]], 0), end(xs, g.succ_e[g.succ_start[bottom]], 1), lo, hi);
                for &d in &chain {
                    xs[d] = x;
                }
                continue;
            }
            for &d in &chain {
                let (least, most) = room(xs, d);
                if least > most {
                    continue;
                }
                let up = end(xs, follow(g, empty, g.pred_e[g.pred_start[d]], 0), 0);
                let down = end(xs, follow(g, empty, g.succ_e[g.succ_start[d]], 1), 1);
                xs[d] = choose(xs[d], up, down, least, most);
            }
        }
    }
}

struct SideSet {
    v: usize,
    side: usize,
    span: f64,
    ids: Vec<usize>,
    room: Vec<f64>,
    all: Vec<usize>,
}

// Once the nodes have their places, moves an end along its side when that lets its edge run straight.
fn settle(g: &mut Graph, nodes: &[LNode], xs: &[f64], efk: &[u8], etk: &[u8], free: &[Vec<i64>; 2], empty: Option<&[u8]>) {
    let real = g.real;
    let mut sides: Vec<SideSet> = Vec::new();
    for v in 0..real {
        let span = nodes[v].span.min(g.w[v]);
        if span <= 0.0 || nodes[v].kind != NODE {
            continue;
        }
        for side in 0..2 {
            let mut ids = Vec::new();
            let mut room = Vec::new();
            side_ends(g, v, side, span, if side == 0 { etk } else { efk }, &mut ids, &mut room);
            if ids.is_empty() {
                continue;
            }
            if ids.len() == 1 {
                room.clear();
                room.push(0.0);
            }
            let start = if side == 0 { &g.pred_start } else { &g.succ_start };
            let list = if side == 0 { &g.pred_e } else { &g.succ_e };
            let other = if side == 0 { &g.ef } else { &g.et };
            let mut all: Vec<usize> = (start[v]..start[v + 1]).map(|k| list[k]).collect();
            all.sort_by(|&a, &b| g.pos[other[a]].cmp(&g.pos[other[b]]).then(a.cmp(&b)));
            sides.push(SideSet { v, side, span, ids, room, all });
        }
    }
    // Where the end of segment `e` on node `v` should be, from the middle of its side.
    let wanted = |g: &Graph, e: usize, side: usize, v: usize, span: f64, again: bool| -> f64 {
        if free[side][v] < 2 {
            return 0.0;
        }
        let e = follow(g, empty, e, side);
        let u = if side == 0 { g.ef[e] } else { g.et[e] };
        let point = if side == 0 { xs[u] + g.efd[e] } else { xs[u] + g.etd[e] };
        // Once the other end has its place, this one comes into line with it if it can.
        if again && (point - xs[v]).abs() <= span / 2.0 + IN_LINE {
            return (-span / 2.0).max((span / 2.0).min(point - xs[v]));
        }
        let mut middle = point;
        let mut width = 0.0;
        if u < real && nodes[u].kind == NODE && (if side == 0 { efk } else { etk })[e] != HELD && free[1 - side][u] >= 2 {
            width = 0f64.max(nodes[u].span.min(g.w[u]));
            if width > 0.0 {
                middle = xs[u];
            }
        }
        let lo = (xs[v] - span / 2.0).max(middle - width / 2.0);
        let hi = (xs[v] + span / 2.0).min(middle + width / 2.0);
        // Two sides with no part in line: the end leans towards the other, if its side is long enough.
        if lo > hi + IN_LINE {
            let lean = 0f64.max(span / 2.0 - LEAN_CLEAR);
            return (-lean).max(lean.min(point - xs[v]));
        }
        let place = (xs[v] * width + middle * span) / (width + span);
        lo.max(hi.min(place)) - xs[v]
    };
    let mut moved = true;
    let mut pass = 0;
    while moved && pass < 6 {
        moved = false;
        for set in &sides {
            let SideSet { v, side, span, ids, room, all } = set;
            let (v, side, span) = (*v, *side, *span);
            let kinds = if side == 0 { etk } else { efk };
            let mut k = 0;
            let mut lo = -span / 2.0;
            for i in 0..=all.len() {
                let held = i < all.len() && kinds[all[i]] == HELD;
                if i < all.len() && !held {
                    continue;
                }
                // Free ends since the last held end occupy ids[k..k + size).
                let mut size = 0;
                let mut j = i as isize - 1;
                while j >= 0 && kinds[all[j as usize]] != HELD {
                    size += 1;
                    j -= 1;
                }
                let hi = if held { (if side == 0 { &g.etd } else { &g.efd })[all[i]] - PORT_PITCH } else { span / 2.0 };
                if size > 0 {
                    // Isotonic fitting preserves order and spacing by pooling neighbours that are too close.
                    let mut sum: Vec<f64> = Vec::new();
                    let mut count: Vec<f64> = Vec::new();
                    let mut before = 0.0;
                    for j in 0..size {
                        if j > 0 {
                            before += room[k + j];
                        }
                        sum.push(wanted(g, ids[k + j], side, v, span, pass > 0) - before);
                        count.push(1.0);
                        // Keep ends together when spacing forced them to share a place.
                        let mut together = j > 0 && room[k + j] == 0.0;
                        while sum.len() > 1 && (together || sum[sum.len() - 2] / count[count.len() - 2] > sum[sum.len() - 1] / count[count.len() - 1]) {
                            together = false;
                            let top = sum.pop().unwrap();
                            let pooled = count.pop().unwrap();
                            *sum.last_mut().unwrap() += top;
                            *count.last_mut().unwrap() += pooled;
                        }
                    }
                    let total = before;
                    let mut j = 0;
                    before = 0.0;
                    for b in 0..sum.len() {
                        // Center the run if it cannot fit between held ends.
                        let level = if hi - total < lo { (lo + hi - total) / 2.0 } else { lo.max((hi - total).min(sum[b] / count[b])) };
                        let mut c = 0.0;
                        while c < count[b] {
                            if j > 0 {
                                before += room[k + j];
                            }
                            let dx = if side == 0 { &mut g.etd } else { &mut g.efd };
                            if (dx[ids[k + j]] - level - before).abs() > 0.01 {
                                moved = true;
                            }
                            dx[ids[k + j]] = level + before;
                            c += 1.0;
                            j += 1;
                        }
                    }
                    k += size;
                }
                if held {
                    lo = (if side == 0 { &g.etd } else { &g.efd })[all[i]] + PORT_PITCH;
                }
            }
        }
        pass += 1;
    }
}

// Half the room a node, bend or port takes across the flow, with its share of the gap beside it.
fn half_room(g: &Graph, opt: &LayeredOptions, v: usize) -> f64 {
    g.w[v] / 2.0 + (if g.kind[v] == NODE { opt.node_sep } else { opt.edge_sep }) / 2.0
}

// Brings edges that are nearly straight into line, by moving a node or bend a little across the flow.
fn snap(g: &Graph, xs: &mut [f64], empty: Option<&[u8]>, opt: &LayeredOptions) {
    let passed = |u: usize| empty.is_some_and(|em| g.kind[u] == DUMMY && em[g.rank[u] as usize] == 1);
    let mut a: Vec<usize> = Vec::new();
    let mut b: Vec<usize> = Vec::new();
    let mut da: Vec<f64> = Vec::new();
    let mut db: Vec<f64> = Vec::new();
    for e in 0..g.ef.len() {
        if passed(g.ef[e]) {
            continue;
        }
        let f = follow(g, empty, e, 1);
        a.push(g.ef[e]);
        b.push(g.et[f]);
        da.push(g.efd[e]);
        db.push(g.etd[f]);
    }
    let off = |xs: &[f64], k: usize| xs[b[k]] + db[k] - xs[a[k]] - da[k];
    // The nodes in line with each other move as one.
    let mut block: Vec<usize> = (0..g.n).collect();
    let mut members: Vec<Vec<usize>> = (0..g.n).map(|i| vec![i]).collect();
    let join = |block: &mut Vec<usize>, members: &mut Vec<Vec<usize>>, x: usize, y: usize| {
        let mut keep = block[x];
        let mut drop = block[y];
        if keep == drop {
            return;
        }
        if members[keep].len() < members[drop].len() {
            std::mem::swap(&mut keep, &mut drop);
        }
        let moved = std::mem::take(&mut members[drop]);
        for &v in &moved {
            block[v] = keep;
        }
        members[keep].extend(moved);
    };
    let mut near: Vec<usize> = Vec::new();
    for k in 0..a.len() {
        let d = off(xs, k).abs();
        if d < 0.01 {
            join(&mut block, &mut members, a[k], b[k]);
        } else if d <= BEND_SNAP {
            near.push(k);
        }
    }
    if near.is_empty() {
        return;
    }
    near.sort_by(|&x, &y| js::ord(js::or(off(xs, x).abs() - off(xs, y).abs(), || x as f64 - y as f64)));
    let gap = |v: usize| half_room(g, opt, v);
    for k in near {
        let d = off(xs, k);
        if d.abs() < 0.01 {
            join(&mut block, &mut members, a[k], b[k]);
            continue;
        }
        if d.abs() > BEND_SNAP || block[a[k]] == block[b[k]] {
            continue;
        }
        // The smaller of the two blocks moves, if it can; else the other.
        let upper = block[a[k]];
        let lower = block[b[k]];
        let first = if members[upper].len() <= members[lower].len() { upper } else { lower };
        for id in [first, if first == upper { lower } else { upper }] {
            let shift = if id == upper { d } else { -d };
            if d.abs() > SNAP && !members[id].iter().all(|&v| g.kind[v] == DUMMY) {
                continue;
            }
            // Whether every node of a block can move by `shift` and keep its room from those beside it.
            let free = members[id].iter().all(|&v| {
                let s = g.layer_start[g.rank[v] as usize];
                let end = g.layer_start[g.rank[v] as usize + 1];
                let i = s + g.pos[v];
                if i > s && block[g.order[i - 1]] != id && xs[v] + shift - xs[g.order[i - 1]] < gap(v) + gap(g.order[i - 1]) - 0.01 {
                    return false;
                }
                if i + 1 < end && block[g.order[i + 1]] != id && xs[g.order[i + 1]] - xs[v] - shift < gap(v) + gap(g.order[i + 1]) - 0.01 {
                    return false;
                }
                true
            });
            if !free {
                continue;
            }
            for &v in &members[id] {
                xs[v] += shift;
            }
            join(&mut block, &mut members, a[k], b[k]);
            break;
        }
    }
}

// Marks the ranks that hold nothing but the bends of edges, if there are any.
fn empty_ranks(g: &Graph) -> Option<Vec<u8>> {
    let mut empty = vec![0u8; g.max_rank + 1];
    let mut any = false;
    for r in 1..g.max_rank {
        let mut only = g.layer_start[r + 1] > g.layer_start[r];
        let mut i = g.layer_start[r];
        while only && i < g.layer_start[r + 1] {
            if g.kind[g.order[i]] != DUMMY {
                only = false;
            }
            i += 1;
        }
        if only {
            empty[r] = 1;
            any = true;
        }
    }
    if any {
        Some(empty)
    } else {
        None
    }
}

/// Insertion-ordered map from rank to its pieces (upstream's Map).
struct ByRank(Vec<(usize, Vec<usize>)>);

// Gives every piece that does not run straight down a track across its gap, and counts the
// tracks of each gap. See upstream for the rules.
fn assign_tracks(p: &mut Pieces, track_count: &mut [i64], curved: bool, room: &[f64], tall: &mut [f64]) {
    let total = p.from.len();
    let mut by_rank = ByRank(Vec::new());
    for k in 0..total {
        p.track.push(-1);
        p.levels.push(0);
        if (p.from[k] - p.to[k]).abs() < IN_LINE {
            continue;
        }
        match by_rank.0.iter_mut().find(|(r, _)| *r == p.rank[k]) {
            Some((_, list)) => list.push(k),
            None => by_rank.0.push((p.rank[k], vec![k])),
        }
    }
    if by_rank.0.is_empty() {
        return;
    }
    let lo_: Vec<f64> = (0..total).map(|k| p.from[k].min(p.to[k])).collect();
    let hi_: Vec<f64> = (0..total).map(|k| p.from[k].max(p.to[k])).collect();
    let from = p.from.clone();
    let to = p.to.clone();
    let wide = p.wide.clone();
    let within = |x: f64, k: usize| -> i32 { (x >= lo_[k] - IN_LINE && x <= hi_[k] + IN_LINE) as i32 };
    let mates = |a: usize, b: usize| (from[a] - from[b]).abs() < IN_LINE || (to[a] - to[b]).abs() < IN_LINE;
    let overlap = |a: usize, b: usize| lo_[a] < hi_[b] - IN_LINE && lo_[b] < hi_[a] - IN_LINE;
    let clash = |a: usize, b: usize| overlap(a, b) && !mates(a, b);
    let meet = |a: usize, b: usize| overlap(a, b) || within(to[a], b) + within(from[b], a) + within(to[b], a) + within(from[a], b) > 0;
    let crossings = |a: usize, b: usize| within(to[a], b) + within(from[b], a);
    // Returns pairs of list indices, smaller first. Omitted pairs cannot meet or clash.
    let near_pairs = |list: &[usize]| -> Vec<usize> {
        let mut by_lo: Vec<usize> = (0..list.len()).collect();
        by_lo.sort_by(|&a, &b| js::ord(lo_[list[a]] - lo_[list[b]]));
        let mut pairs = Vec::new();
        for x in 0..by_lo.len() {
            let i = by_lo[x];
            let reach = hi_[list[i]] + 4.0 * IN_LINE;
            let mut y = x + 1;
            while y < by_lo.len() && lo_[list[by_lo[y]]] <= reach {
                let j = by_lo[y];
                pairs.push(i.min(j));
                pairs.push(i.max(j));
                y += 1;
            }
        }
        pairs
    };

    let lay = |list: &mut Vec<usize>, track: &mut Vec<i64>, levels: &mut Vec<i64>| -> i64 {
        // A plain order to start from: the pieces that reach furthest go first.
        list.sort_by(|&a, &b| js::ord(js::or(hi_[b] - lo_[b] - (hi_[a] - lo_[a]), || js::or(lo_[a] - lo_[b], || a as f64 - b as f64))));
        let count = list.len();
        let mut above: Vec<Vec<usize>> = Vec::new();
        let mut group: Vec<usize> = (0..count).collect();
        fn find(group: &mut [usize], mut i: usize) -> usize {
            while group[i] != i {
                group[i] = group[group[i]];
                i = group[i];
            }
            i
        }
        let weighed = count <= MAX_WEIGHED;
        let pairs = if weighed { near_pairs(list) } else { Vec::new() };
        let mut clashes: Vec<Vec<usize>> = if weighed { vec![Vec::new(); count] } else { Vec::new() };
        let mut q = 0;
        while q < pairs.len() {
            let (i, j) = (pairs[q], pairs[q + 1]);
            q += 2;
            if !clash(list[i], list[j]) {
                continue;
            }
            clashes[i].push(j);
            clashes[j].push(i);
        }
        let mut sequence: Vec<usize> = (0..count).collect();
        if weighed {
            let mut waiting = vec![0i64; count];
            let mut after: Vec<Vec<usize>> = vec![Vec::new(); count];
            above = vec![Vec::new(); count];
            let mut q = 0;
            while q < pairs.len() {
                let (i, j) = (pairs[q], pairs[q + 1]);
                q += 2;
                let a = list[i];
                let b = list[j];
                if !meet(a, b) {
                    continue;
                }
                let fi = find(&mut group, i);
                let fj = find(&mut group, j);
                group[fi] = fj;
                let ab = crossings(a, b);
                let ba = crossings(b, a);
                if ab == ba {
                    continue;
                }
                let first = if ab < ba { i } else { j };
                let second = if ab < ba { j } else { i };
                above[second].push(first);
                after[first].push(second);
                waiting[second] += 1;
            }
            let mut placed = vec![0u8; count];
            sequence = Vec::with_capacity(count);
            for _ in 0..count {
                let mut pick: isize = -1;
                for i in 0..count {
                    if placed[i] != 0 {
                        continue;
                    }
                    if pick < 0 || waiting[i] < waiting[pick as usize] {
                        pick = i as isize;
                    }
                    if waiting[i] == 0 {
                        break;
                    }
                }
                let pick = pick as usize;
                placed[pick] = 1;
                sequence.push(pick);
                for &j in &after[pick] {
                    waiting[j] -= 1;
                }
            }
        }
        let mut taken: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let mut on_track: Vec<Vec<usize>> = Vec::new();
        let mut rows: i64 = 0;
        for &at in &sequence {
            let k = list[at];
            let mut t: i64 = 0;
            if let Some(ab) = above.get(at) {
                for &i in ab {
                    if track[list[i]] >= t {
                        t = track[list[i]] + 1;
                    }
                }
            }
            if weighed {
                taken.clear();
                for &i in &clashes[at] {
                    taken.insert(track[list[i]]);
                }
                while taken.contains(&t) {
                    t += 1;
                }
            } else {
                while on_track.get(t as usize).is_some_and(|row| row.iter().any(|&other| clash(k, other))) {
                    t += 1;
                }
                while on_track.len() <= t as usize {
                    on_track.push(Vec::new());
                }
                on_track[t as usize].push(k);
            }
            track[k] = t;
            if t >= rows {
                rows = t + 1;
            }
        }
        let mut first = vec![rows; count];
        let mut last = vec![-1i64; count];
        for i in 0..count {
            let root = find(&mut group, i);
            first[root] = first[root].min(track[list[i]]);
            last[root] = last[root].max(track[list[i]]);
        }
        for i in 0..count {
            let root = find(&mut group, i);
            levels[list[i]] = last[root] - first[root] + 1;
            if curved {
                track[list[i]] -= first[root];
            }
        }
        rows
    };

    let sample = |k: usize, h: f64, out: &mut [f64; 2 * SAMPLES + 2]| {
        let c = ease(to[k] - from[k], h);
        for i in 0..=SAMPLES {
            let t = i as f64 / SAMPLES as f64;
            let u = 1.0 - t;
            out[2 * i] = from[k] + (to[k] - from[k]) * t * t * (3.0 - 2.0 * t);
            out[2 * i + 1] = h * (3.0 * c * t * u * u + 3.0 * (1.0 - c) * t * t * u + t * t * t);
        }
    };
    let reaches = |points: &[f64], line: &[f64], limit: f64| -> bool {
        // Allow rounding error in the cheap distance check before calling hypot.
        let bound = limit * limit * (1.0 + 1e-9);
        for i in 0..=SAMPLES {
            let px = points[2 * i];
            let py = points[2 * i + 1];
            for j in 0..SAMPLES {
                let dx = line[2 * j + 2] - line[2 * j];
                let dy = line[2 * j + 3] - line[2 * j + 1];
                let t = 0f64.max(1f64.min(((px - line[2 * j]) * dx + (py - line[2 * j + 1]) * dy) / js::or(dx * dx + dy * dy, || 1.0)));
                let ex = px - line[2 * j] - t * dx;
                let ey = py - line[2 * j + 1] - t * dy;
                if ex * ex + ey * ey < bound && ex.hypot(ey) < limit {
                    return true;
                }
            }
        }
        false
    };
    let near = |a: usize, b: usize, height: f64, limit: f64| -> bool {
        let h = (height / 2.0).max(height - 2.0 * LEVEL_LEAD);
        let mut la = [0f64; 2 * SAMPLES + 2];
        let mut lb = [0f64; 2 * SAMPLES + 2];
        sample(a, h, &mut la);
        sample(b, h, &mut lb);
        reaches(&la, &lb, limit) || reaches(&lb, &la, limit)
    };

    for (r, all) in by_rank.0.iter_mut() {
        let r = *r;
        if !curved {
            track_count[r] = lay(all, &mut p.track, &mut p.levels);
            continue;
        }
        if all.len() > MAX_WEIGHED {
            continue;
        }
        // The pieces that turn onto levels.
        let mut far: Vec<usize> = all.iter().copied().filter(|&k| wide[k] == 1).collect();
        lay(&mut far, &mut p.track, &mut p.levels);
        let mut level: std::collections::HashSet<usize> = far.iter().copied().filter(|&k| p.levels[k] > 1).collect();
        let pairs = near_pairs(all);
        let mut q = 0;
        while q < pairs.len() {
            let a = all[pairs[q]];
            let b = all[pairs[q + 1]];
            q += 2;
            if (level.contains(&a) && level.contains(&b)) || !clash(a, b) || (from[a] < to[a]) != (from[b] < to[b]) {
                continue;
            }
            if near(a, b, room[r], if wide[a] == 1 || wide[b] == 1 { LEVEL_CLEAR } else { LEVEL_TOUCH }) {
                level.insert(a);
                level.insert(b);
            }
        }
        for &k in &far {
            p.track[k] = -1;
            p.levels[k] = 0;
        }
        if level.is_empty() {
            continue;
        }
        let mut crossing = 0;
        let mut alongside = 0;
        let mut beside: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut q = 0;
        while q < pairs.len() {
            let a = all[pairs[q]];
            let b = all[pairs[q + 1]];
            q += 2;
            if level.contains(&a) && level.contains(&b) {
                if !overlap(a, b) {
                    continue;
                }
                if (from[a] < to[a]) != (from[b] < to[b]) {
                    crossing += 1;
                } else if !mates(a, b) {
                    alongside += 1;
                }
            } else if (level.contains(&a) || level.contains(&b)) && meet(a, b) {
                beside.insert(a);
                beside.insert(b);
            }
        }
        if crossing > alongside {
            for &k in all.iter() {
                if (hi_[k] - lo_[k]) * CROSSED_SLOPE > tall[r] {
                    tall[r] = (hi_[k] - lo_[k]) * CROSSED_SLOPE;
                }
            }
            continue;
        }
        // With them go the short pieces that could cross one of them.
        let mut members: Vec<usize> = all.iter().copied().filter(|k| level.contains(k) || (wide[*k] != 1 && beside.contains(k))).collect();
        lay(&mut members, &mut p.track, &mut p.levels);
        let mut most = 0;
        for &k in &members {
            if p.levels[k] > most {
                most = p.levels[k];
            }
        }
        track_count[r] = most;
    }
}

// Moves the bends in ranks that hold only bends onto the straight line between the nearest points
// of their edges that are held in place, and marks the ones that can be left out of the route.
#[allow(clippy::too_many_arguments)]
fn straighten(g: &Graph, xs: &mut [f64], empty: &[u8], first_dummy: &[i64], span: &[i64], sep: f64, on_line: &mut [u8]) {
    let len = xs.len();
    let mut want = vec![0f64; len];
    let mut from = vec![0f64; len];
    let mut to = vec![0f64; len];
    for ei in 0..first_dummy.len() {
        let d0 = first_dummy[ei];
        if d0 < 0 {
            continue;
        }
        let d0 = d0 as usize;
        let end = d0 + span[ei] as usize - 1;
        // A bend's rank is its place in the run, counted from the rank after the edge's first node.
        let base = g.rank[d0] as usize;
        let mut d = d0;
        while d < end {
            if empty[base + d - d0] == 0 {
                d += 1;
                continue;
            }
            let mut last = d;
            while last + 1 < end && empty[base + last + 1 - d0] != 0 {
                last += 1;
            }
            let above = g.pred_e[g.pred_start[d]];
            let below = g.succ_e[g.succ_start[last]];
            let x0 = xs[g.ef[above]] + g.efd[above];
            let x1 = xs[g.et[below]] + g.etd[below];
            let steps = (last - d + 2) as f64;
            for k in d..=last {
                want[k] = x0 + ((x1 - x0) * (k - d + 1) as f64) / steps;
                from[k] = x0;
                to[k] = x1;
            }
            d = last + 1;
        }
    }

    // Only twins stay: bends of edges that run between the same two points. They take places a
    // little apart, around the line.
    let mut twins: Vec<usize> = Vec::new();
    for r in 1..g.max_rank {
        if empty[r] == 0 {
            continue;
        }
        let s = g.layer_start[r];
        let end = g.layer_start[r + 1];
        let mut row: Vec<usize> = g.order[s..end].to_vec();
        row.sort_by(|&a, &b| js::ord(js::or(from[a] - from[b], || js::or(to[a] - to[b], || a as f64 - b as f64))));
        let mut i = 0;
        while i < row.len() {
            twins.clear();
            let mut j = i;
            while j < row.len() && (from[row[j]] - from[row[i]]).abs() < TWIN && (to[row[j]] - to[row[i]]).abs() < TWIN {
                twins.push(row[j]);
                j += 1;
            }
            // The twins keep the order the rank gave them.
            twins.sort_by(|&a, &b| g.pos[a].cmp(&g.pos[b]));
            for k in 0..twins.len() {
                let d = twins[k];
                xs[d] = want[d] + (k as f64 - (twins.len() as f64 - 1.0) / 2.0) * sep;
                if twins.len() == 1 || sep == 0.0 {
                    on_line[d] = 1;
                }
            }
            i = j;
        }
    }
}

// In a rank that holds labels and bends but no nodes, a bend that would pass clear of every
// label anyway is left out, and the edge turns in the whole height between the ranks either side.
fn past_labels(g: &Graph, xs: &mut [f64], first_dummy: &[i64], span: &[i64], clear: f64, on_line: &mut [u8]) {
    let mut labelled = vec![0u8; g.max_rank + 1];
    let mut any = false;
    for r in 1..g.max_rank {
        let mut labels = false;
        let mut others = false;
        for i in g.layer_start[r]..g.layer_start[r + 1] {
            let k = g.kind[g.order[i]];
            if k == LABEL {
                labels = true;
            } else if k != DUMMY {
                others = true;
            }
        }
        if labels && !others {
            labelled[r] = 1;
            any = true;
        }
    }
    if !any {
        return;
    }
    for ei in 0..first_dummy.len() {
        let d0 = first_dummy[ei];
        if d0 < 0 {
            continue;
        }
        let d0 = d0 as usize;
        let end = d0 + span[ei] as usize - 1;
        for d in d0..end {
            if g.kind[d] != DUMMY || labelled[g.rank[d] as usize] == 0 {
                continue;
            }
            // The points on either side that the edge is held at, past any bends already left out.
            let mut first = d;
            let mut last = d;
            while first > d0 && on_line[first - 1] != 0 {
                first -= 1;
            }
            while last + 1 < end && on_line[last + 1] != 0 {
                last += 1;
            }
            let above = g.pred_e[g.pred_start[first]];
            let below = g.succ_e[g.succ_start[last]];
            let from = xs[g.ef[above]] + g.efd[above];
            let to = xs[g.et[below]] + g.etd[below];
            let lo = from.min(to) - clear;
            let hi = from.max(to) + clear;
            let r = g.rank[d] as usize;
            let mut free = true;
            let mut i = g.layer_start[r];
            while free && i < g.layer_start[r + 1] {
                let label = g.order[i];
                if g.kind[label] == LABEL && xs[label] + g.w[label] / 2.0 > lo && xs[label] - g.w[label] / 2.0 < hi {
                    free = false;
                }
                i += 1;
            }
            if !free {
                continue;
            }
            on_line[d] = 1;
            // The bends left out between the two points lie on the line between them.
            let steps = (last - first + 2) as f64;
            for k in first..=last {
                xs[k] = from + ((to - from) * (k - first + 1) as f64) / steps;
            }
        }
    }
}

// Returns [final crossings, crossings before grouping or reordering].
fn reduce_crossings(g: &mut Graph, nodes: &[LNode]) -> (i64, i64) {
    let n = g.n;
    let real = g.real;
    let max_rank = g.max_rank;
    let mut widest = 0;
    for r in 0..=max_rank {
        widest = widest.max(g.layer_start[r + 1] - g.layer_start[r]);
    }
    if widest < 2 {
        return (0, 0);
    }
    let segs = g.ef.len();
    let offsets = g.offsets;
    // With offsets, edges that reach one node at different offsets take separate places along it.
    let mut place: Option<Vec<usize>> = if offsets { Some(vec![0; segs]) } else { None };
    let mut place_in = vec![0usize; if offsets { segs } else { 0 }];
    let mut places = vec![0usize; if offsets { n } else { 0 }];
    let mut leaving = g.succ_e.clone();
    let mut mixed = vec![0u8; if offsets { n } else { 0 }];
    if offsets {
        let mut arriving = g.pred_e.clone();
        let by_offset = |list: &mut [usize], from: usize, to: usize, dx: &[f64]| -> bool {
            for k in from + 1..to {
                if dx[list[k]] == dx[list[from]] {
                    continue;
                }
                list[from..to].sort_by(|&a, &b| js::ord(dx[a] - dx[b]));
                return true;
            }
            false
        };
        for v in 0..n {
            places[v] = 1;
            if by_offset(&mut arriving, g.pred_start[v], g.pred_start[v + 1], &g.etd) {
                for k in g.pred_start[v] + 1..g.pred_start[v + 1] {
                    if g.etd[arriving[k]] != g.etd[arriving[k - 1]] {
                        places[v] += 1;
                    }
                    place_in[arriving[k]] = places[v] - 1;
                }
            }
            if by_offset(&mut leaving, g.succ_start[v], g.succ_start[v + 1], &g.efd) {
                mixed[v] = 1;
            }
        }
    }
    let mut size = 1;
    while size < if offsets { widest + segs } else { widest } {
        size <<= 1;
    }
    let mut tree = vec![0i64; 2 * size];
    let mut targets = vec![0usize; segs];

    let count = |g: &Graph, place: &mut Option<Vec<usize>>, tree: &mut Vec<i64>, targets: &mut Vec<usize>| -> i64 {
        let mut total = 0i64;
        for r in 0..max_rank {
            let mut south_len = g.layer_start[r + 2] - g.layer_start[r + 1];
            if let Some(place) = place.as_mut() {
                let mut next = 0;
                for i in g.layer_start[r + 1]..g.layer_start[r + 2] {
                    let v = g.order[i];
                    for k in g.pred_start[v]..g.pred_start[v + 1] {
                        place[g.pred_e[k]] = next + place_in[g.pred_e[k]];
                    }
                    next += places[v];
                }
                south_len = next;
            }
            if south_len < 2 {
                continue;
            }
            let mut t = 0;
            for i in g.layer_start[r]..g.layer_start[r + 1] {
                let v = g.order[i];
                let from = g.succ_start[v];
                let to = g.succ_start[v + 1];
                let t0 = t;
                for k in from..to {
                    targets[t] = match place.as_ref() {
                        Some(p) => p[leaving[k]],
                        None => g.pos[g.et[g.succ_e[k]]],
                    };
                    t += 1;
                }
                let d = t - t0;
                if d < 2 {
                    continue;
                }
                if place.is_some() && mixed[v] != 0 {
                    // Sort equal-offset edges by destination to avoid counting shared endpoints as crossings.
                    for a in t0 + 1..t {
                        let item = targets[a];
                        let dx = g.efd[leaving[from + a - t0]];
                        let mut b = a as isize - 1;
                        while b >= t0 as isize && g.efd[leaving[from + b as usize - t0]] == dx && targets[b as usize] > item {
                            targets[(b + 1) as usize] = targets[b as usize];
                            b -= 1;
                        }
                        targets[(b + 1) as usize] = item;
                    }
                } else if d <= 12 {
                    for a in t0 + 1..t {
                        let item = targets[a];
                        let mut b = a as isize - 1;
                        while b >= t0 as isize && targets[b as usize] > item {
                            targets[(b + 1) as usize] = targets[b as usize];
                            b -= 1;
                        }
                        targets[(b + 1) as usize] = item;
                    }
                } else {
                    targets[t0..t].sort_unstable();
                }
            }
            if t < 2 {
                continue;
            }
            let mut leaves = 1;
            while leaves < south_len {
                leaves <<= 1;
            }
            for x in tree[..2 * leaves].iter_mut() {
                *x = 0;
            }
            for k in 0..t {
                let mut i = targets[k] + leaves;
                tree[i] += 1;
                while i > 1 {
                    if i & 1 == 0 {
                        total += tree[i + 1];
                    }
                    i >>= 1;
                    tree[i] += 1;
                }
            }
        }
        total
    };

    let declared = count(g, &mut place, &mut tree, &mut targets);
    let mut bary = vec![0f64; n];
    let mut moving = vec![0usize; widest];
    let mut spare = vec![0usize; widest];

    // Keep groups contiguous, ordering each by its mean key.
    let group_parent = g.group_parent.clone();
    let grp = g.grp.clone();
    let groups = group_parent.as_ref().map_or(0, |p| p.len());
    let mut depth = vec![0i32; groups];
    if let Some(gp) = &group_parent {
        for gr in 0..groups {
            let mut p = gr as i32;
            while p >= 0 {
                depth[gr] += 1;
                p = gp[p as usize];
            }
        }
    }
    let mut group_sum = vec![0f64; groups];
    let mut group_count = vec![0i64; groups];
    let mut group_first = vec![0usize; groups];
    let arrange = |g: &mut Graph, r: usize, key: &[f64], group_sum: &mut Vec<f64>, group_count: &mut Vec<i64>, group_first: &mut Vec<usize>| {
        let grp = grp.as_ref().unwrap();
        let gp = group_parent.as_ref().unwrap();
        let s = g.layer_start[r];
        let e = g.layer_start[r + 1];
        for i in s..e {
            let mut gr = grp[g.order[i]];
            while gr >= 0 {
                group_sum[gr as usize] = 0.0;
                group_count[gr as usize] = 0;
                gr = gp[gr as usize];
            }
        }
        for i in s..e {
            let v = g.order[i];
            let mut gr = grp[v];
            while gr >= 0 {
                let gu = gr as usize;
                if group_count[gu] == 0 {
                    group_first[gu] = i;
                }
                group_sum[gu] += key[v];
                group_count[gu] += 1;
                gr = gp[gu];
            }
        }
        // Compare siblings in their nearest common group; group items use n + group ID.
        let value = |item: usize| if item >= n { group_sum[item - n] / group_count[item - n] as f64 } else { key[item] };
        let first = |item: usize, pos: &[usize]| if item >= n { group_first[item - n] as f64 } else { (s + pos[item]) as f64 };
        let pos = g.pos.clone();
        let compare = |a: usize, b: usize| -> Ordering {
            let mut ia = a;
            let mut ib = b;
            let mut ca = grp[a];
            let mut cb = grp[b];
            let mut da = if ca < 0 { 0 } else { depth[ca as usize] };
            let mut db = if cb < 0 { 0 } else { depth[cb as usize] };
            while da > db {
                ia = n + ca as usize;
                ca = gp[ca as usize];
                da -= 1;
            }
            while db > da {
                ib = n + cb as usize;
                cb = gp[cb as usize];
                db -= 1;
            }
            while ca != cb {
                ia = n + ca as usize;
                ca = gp[ca as usize];
                ib = n + cb as usize;
                cb = gp[cb as usize];
            }
            js::ord(js::or(value(ia) - value(ib), || first(ia, &pos) - first(ib, &pos)))
        };
        let mut row: Vec<usize> = g.order[s..e].to_vec();
        row.sort_by(|&a, &b| compare(a, b));
        g.order[s..e].copy_from_slice(&row);
        for i in s..e {
            g.pos[g.order[i]] = i - s;
        }
    };
    if grp.is_some() {
        let mut at = vec![0f64; n];
        for r in 0..=max_rank {
            for i in g.layer_start[r]..g.layer_start[r + 1] {
                at[g.order[i]] = (i - g.layer_start[r]) as f64;
            }
            arrange(g, r, &at, &mut group_sum, &mut group_count, &mut group_first);
        }
    }
    let mut best = if grp.is_some() { count(g, &mut place, &mut tree, &mut targets) } else { declared };
    if best == 0 {
        return (0, declared);
    }
    let mut best_order = g.order.clone();
    // Require improvements over both the grouped and ungrouped starting orders.
    let start = best.min(declared);

    let settle_positions = |g: &mut Graph| set_positions(g);
    let pin_of = |v: usize| if v < real { nodes[v].pin } else { 0 };

    let initial = g.order.clone();
    let small = n <= 600;
    let mut chosen = best_order.clone();
    let mut chosen_out = usize::MAX;
    for attempt in 0..(if small { 2 } else { 1 }) {
        if attempt > 0 {
            g.order.copy_from_slice(&initial);
            settle_positions(g);
        }
        let mut found = start;
        let mut found_order = initial.clone();
        let mut i = 0;
        let mut stale = 0;
        while stale < 4 && i < MAX_SWEEPS {
            // sweep((i + attempt) % 2 === 0, i % 4 >= 2)
            let down = (i + attempt) % 2 == 0;
            let bias_right = i % 4 >= 2;
            for step in 1..=max_rank {
                let r = if down { step } else { max_rank - step };
                if g.fixed[r] != 0 {
                    continue;
                }
                let s = g.layer_start[r];
                let e = g.layer_start[r + 1];
                let mut movable = 0;
                for i in s..e {
                    let v = g.order[i];
                    let pin = pin_of(v);
                    if pin != 0 {
                        bary[v] = pin as f64 * 1e9;
                        moving[movable] = v;
                        movable += 1;
                        continue;
                    }
                    let (from, to) = if down { (g.pred_start[v], g.pred_start[v + 1]) } else { (g.succ_start[v], g.succ_start[v + 1]) };
                    if from == to {
                        if grp.is_some() {
                            // Preserve relative position when there are no neighbours in the adjacent rank.
                            let across = if down { g.layer_start[r] - g.layer_start[r - 1] } else { g.layer_start[r + 2] - g.layer_start[r + 1] };
                            bary[v] = ((i - s) * across) as f64 / (e - s) as f64;
                            continue;
                        }
                        bary[v] = -1.0;
                        continue;
                    }
                    let mut sum = 0.0;
                    for k in from..to {
                        let seg = if down { g.pred_e[k] } else { g.succ_e[k] };
                        let u = if down { g.ef[seg] } else { g.et[seg] };
                        sum += g.pos[u] as f64;
                        if offsets && g.w[u] > 0.0 {
                            sum += (if down { g.efd[seg] } else { g.etd[seg] }) / g.w[u];
                        }
                    }
                    bary[v] = sum / (to - from) as f64;
                    moving[movable] = v;
                    movable += 1;
                }
                if grp.is_some() {
                    arrange(g, r, &bary, &mut group_sum, &mut group_count, &mut group_first);
                    continue;
                }
                if movable < 2 {
                    continue;
                }
                sort_by_bary(&mut moving, movable, &bary, &g.pos, bias_right, &mut spare);
                let mut k = 0;
                for i in s..e {
                    if bary[g.order[i]] != -1.0 {
                        g.order[i] = moving[k];
                        k += 1;
                    }
                }
                for i in s..e {
                    g.pos[g.order[i]] = i - s;
                }
            }
            let c = count(g, &mut place, &mut tree, &mut targets);
            if c < found {
                found = c;
                found_order.copy_from_slice(&g.order);
                stale = 0;
                if c == 0 {
                    break;
                }
            }
            i += 1;
            stale += 1;
        }
        g.order.copy_from_slice(&found_order);
        if small && found > 0 {
            settle_positions(g);
            transpose(g, nodes);
            let c = count(g, &mut place, &mut tree, &mut targets);
            if c < found {
                found = c;
                found_order.copy_from_slice(&g.order);
            }
        }
        let out = if small { out_of_order(g) } else { 0 };
        if attempt == 0 || found < best || (found == best && out < chosen_out) {
            best = found;
            chosen = found_order;
            chosen_out = out;
        }
    }
    g.order.copy_from_slice(&chosen);
    settle_positions(g);
    best_order.copy_from_slice(&chosen);
    // The retained grouped order may exceed the ungrouped threshold; recount it.
    if grp.is_some() {
        best = count(g, &mut place, &mut tree, &mut targets);
    }

    // A drawing and its mirror image cross as often too. Of those two, likewise.
    if !small {
        return (best, declared);
    }
    if (0..real).any(|i| nodes[i].pin != 0) {
        return (best, declared);
    }
    if chosen_out == 0 || g.fixed[0] != 0 || g.fixed[max_rank] != 0 {
        return (best, declared);
    }
    for r in 0..=max_rank {
        let (s, e) = (g.layer_start[r], g.layer_start[r + 1]);
        g.order[s..e].reverse();
    }
    settle_positions(g);
    if count(g, &mut place, &mut tree, &mut targets) > best || out_of_order(g) >= chosen_out {
        g.order.copy_from_slice(&best_order);
    }
    (best, declared)
}

// How many pairs of nodes in one rank are not in the order they were given.
fn out_of_order(g: &Graph) -> usize {
    let mut total = 0;
    for r in 0..=g.max_rank {
        for i in g.layer_start[r]..g.layer_start[r + 1] {
            if g.order[i] >= g.real {
                continue;
            }
            for j in i + 1..g.layer_start[r + 1] {
                if g.order[j] < g.order[i] {
                    total += 1;
                }
            }
        }
    }
    total
}

// Adjacent swaps can remove crossings missed by barycenter sweeps.
fn transpose(g: &mut Graph, nodes: &[LNode]) {
    let real = g.real;
    let pin_of = |v: usize| if v < real { nodes[v].pin } else { 0 };
    let at = |g: &Graph, e: usize, up: bool| {
        let u = if up { g.ef[e] } else { g.et[e] };
        if g.offsets && g.w[u] > 0.0 {
            g.pos[u] as f64 + (if up { g.efd[e] } else { g.etd[e] }) / g.w[u]
        } else {
            g.pos[u] as f64
        }
    };
    // Crossings between a and b, assuming a is left of b.
    let between = |g: &Graph, a: usize, b: usize| {
        let mut c = 0;
        for k in g.pred_start[a]..g.pred_start[a + 1] {
            let x = at(g, g.pred_e[k], true);
            for j in g.pred_start[b]..g.pred_start[b + 1] {
                if x > at(g, g.pred_e[j], true) {
                    c += 1;
                }
            }
        }
        for k in g.succ_start[a]..g.succ_start[a + 1] {
            let x = at(g, g.succ_e[k], false);
            for j in g.succ_start[b]..g.succ_start[b + 1] {
                if x > at(g, g.succ_e[j], false) {
                    c += 1;
                }
            }
        }
        c
    };
    for _pass in 0..8 {
        let mut improved = false;
        for r in 0..=g.max_rank {
            if g.fixed[r] != 0 {
                continue;
            }
            let s = g.layer_start[r];
            let mut i = s;
            while i + 1 < g.layer_start[r + 1] {
                let a = g.order[i];
                let b = g.order[i + 1];
                let same_group = g.grp.as_ref().is_none_or(|grp| grp[a] == grp[b]);
                if pin_of(a) != pin_of(b) || !same_group {
                    i += 1;
                    continue;
                }
                if between(g, b, a) >= between(g, a, b) {
                    i += 1;
                    continue;
                }
                g.order[i] = b;
                g.order[i + 1] = a;
                g.pos[b] = i - s;
                g.pos[a] = i + 1 - s;
                improved = true;
                i += 1;
            }
        }
        if !improved {
            break;
        }
    }
}

// Brandes-Köpf horizontal coordinate assignment: four aligned layouts, balanced.
fn position(g: &mut Graph, opt: &LayeredOptions) -> Vec<f64> {
    let n = g.n;
    let max_rank = g.max_rank;
    let segments = g.ef.len();
    let gap: Vec<f64> = (0..n).map(|i| half_room(g, opt, i)).collect();

    // Neighbours in left-to-right order, so the medians can be read off directly.
    let mut key = vec![0i64; segments];
    for e in 0..segments {
        key[e] = g.pos[g.et[e]] as i64;
    }
    for v in 0..n {
        if g.succ_start[v + 1] - g.succ_start[v] > 1 {
            let (a, b) = (g.succ_start[v], g.succ_start[v + 1]);
            sort_range(&mut g.succ_e, a, b, &key);
        }
    }
    for e in 0..segments {
        key[e] = g.pos[g.ef[e]] as i64;
    }
    for v in 0..n {
        if g.pred_start[v + 1] - g.pred_start[v] > 1 {
            let (a, b) = (g.pred_start[v], g.pred_start[v + 1]);
            sort_range(&mut g.pred_e, a, b, &key);
        }
    }
    let g: &Graph = g;

    // An edge between two real nodes may not cross a dummy-to-dummy segment; mark those that would.
    let mut conflict = vec![0u8; segments];
    for r in 1..=max_rank {
        let prev_len = g.layer_start[r] - g.layer_start[r - 1];
        let s = g.layer_start[r];
        let end = g.layer_start[r + 1];
        let mut k0: usize = 0;
        let mut scan = s;
        for i in s..end {
            let v = g.order[i];
            let mut inner: isize = -1;
            if is_dummy(g.kind[v]) {
                let u = g.ef[g.pred_e[g.pred_start[v]]];
                if is_dummy(g.kind[u]) {
                    inner = u as isize;
                }
            }
            let k1 = if inner >= 0 { g.pos[inner as usize] } else { prev_len };
            if inner >= 0 || i == end - 1 {
                for j in scan..=i {
                    let w = g.order[j];
                    let w_dummy = is_dummy(g.kind[w]);
                    for k in g.pred_start[w]..g.pred_start[w + 1] {
                        let e = g.pred_e[k];
                        let up = g.pos[g.ef[e]];
                        if (up < k0 || k1 < up) && !(w_dummy && is_dummy(g.kind[g.ef[e]])) {
                            conflict[e] = 1;
                        }
                    }
                }
                scan = i + 1;
                k0 = k1;
            }
        }
    }

    let mut results: Vec<Vec<f64>> = Vec::with_capacity(4);
    let mut root = vec![0usize; n];
    let mut align = vec![0usize; n];
    // Offset of each node from its block's root, so that aligned edges meet at their attachment points.
    let mut shift = vec![0f64; n];
    let mut e_from = vec![0usize; n];
    let mut e_to = vec![0usize; n];
    let mut e_w = vec![0f64; n];
    let mut indeg = vec![0i64; n];
    let mut out_start = vec![0usize; n + 1];
    let mut out_e = vec![0usize; n];
    let mut fill = vec![0usize; n];
    let mut topo = vec![0usize; n];
    let mut x = vec![0f64; n];

    for variant in 0..4 {
        let up = variant < 2;
        let right = variant & 1 == 1;
        let start = if up { &g.pred_start } else { &g.succ_start };
        let list = if up { &g.pred_e } else { &g.succ_e };
        let other = if up { &g.ef } else { &g.et };
        let own_dx = if up { &g.etd } else { &g.efd };
        let other_dx = if up { &g.efd } else { &g.etd };
        for i in 0..n {
            root[i] = i;
            align[i] = i;
            shift[i] = 0.0;
            indeg[i] = 0;
            out_start[i + 1] = 0;
            x[i] = 0.0;
        }

        for step in 0..=max_rank {
            let r = if up { step } else { max_rank - step };
            let s = g.layer_start[r];
            let end = g.layer_start[r + 1];
            let adjacent: isize = if up { r as isize - 1 } else { r as isize + 1 };
            let adjacent_last: isize = if adjacent >= 0 && adjacent as usize <= max_rank { (g.layer_start[adjacent as usize + 1] - g.layer_start[adjacent as usize]) as isize - 1 } else { 0 };
            let mut prev_idx: isize = -1;
            let indices: Box<dyn Iterator<Item = usize>> = if right { Box::new((s..end).rev()) } else { Box::new(s..end) };
            for i in indices {
                let v = g.order[i];
                let from = start[v];
                let d = start[v + 1] - from;
                if d == 0 {
                    continue;
                }
                let lo = from + ((d - 1) >> 1);
                let hi = from + (d >> 1);
                for pass in 0..2 {
                    if pass == 1 && lo == hi {
                        break;
                    }
                    let e = list[if (pass == 0) != right { lo } else { hi }];
                    let w = other[e];
                    let idx: isize = if right { adjacent_last - g.pos[w] as isize } else { g.pos[w] as isize };
                    if align[v] == v && prev_idx < idx && conflict[e] == 0 {
                        align[w] = v;
                        root[v] = root[w];
                        align[v] = root[v];
                        shift[v] = shift[w] + other_dx[e] - own_dx[e];
                        prev_idx = idx;
                    }
                }
            }
        }

        let mut block_edges = 0;
        for r in 0..=max_rank {
            let s = g.layer_start[r];
            let end = g.layer_start[r + 1];
            for i in s + 1..end {
                let a = if right { g.order[i] } else { g.order[i - 1] };
                let b = if right { g.order[i - 1] } else { g.order[i] };
                let sep = gap[a] + gap[b];
                e_from[block_edges] = root[a];
                e_to[block_edges] = root[b];
                e_w[block_edges] = if right { sep - shift[a] + shift[b] } else { sep + shift[a] - shift[b] };
                indeg[root[b]] += 1;
                out_start[root[a] + 1] += 1;
                block_edges += 1;
            }
        }
        for i in 0..n {
            out_start[i + 1] += out_start[i];
        }
        fill[..n].copy_from_slice(&out_start[..n]);
        for e in 0..block_edges {
            out_e[fill[e_from[e]]] = e;
            fill[e_from[e]] += 1;
        }

        let mut count = 0;
        for i in 0..n {
            if root[i] == i && indeg[i] == 0 {
                topo[count] = i;
                count += 1;
            }
        }
        let mut q = 0;
        while q < count {
            let v = topo[q];
            for k in out_start[v]..out_start[v + 1] {
                let e = out_e[k];
                let w = e_to[e];
                if x[v] + e_w[e] > x[w] {
                    x[w] = x[v] + e_w[e];
                }
                indeg[w] -= 1;
                if indeg[w] == 0 {
                    topo[count] = w;
                    count += 1;
                }
            }
            q += 1;
        }
        for q in (0..count).rev() {
            let v = topo[q];
            let mut min = f64::INFINITY;
            for k in out_start[v]..out_start[v + 1] {
                let e = out_e[k];
                let cand = x[e_to[e]] - e_w[e];
                if cand < min {
                    min = cand;
                }
            }
            if min != f64::INFINITY && min > x[v] {
                x[v] = min;
            }
        }
        let xs: Vec<f64> = (0..n).map(|i| (if right { -x[root[i]] } else { x[root[i]] }) + shift[i]).collect();
        results.push(xs);
    }

    let mut narrowest = 0;
    let mut best_width = f64::INFINITY;
    let mut mins = Vec::new();
    let mut maxs = Vec::new();
    for (k, xs) in results.iter().enumerate() {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for i in 0..n {
            let half = g.w[i] / 2.0;
            if xs[i] - half < lo {
                lo = xs[i] - half;
            }
            if xs[i] + half > hi {
                hi = xs[i] + half;
            }
        }
        mins.push(lo);
        maxs.push(hi);
        if hi - lo < best_width {
            best_width = hi - lo;
            narrowest = k;
        }
    }
    for k in 0..4 {
        let delta = if k & 1 == 0 { mins[narrowest] - mins[k] } else { maxs[narrowest] - maxs[k] };
        if delta != 0.0 {
            for v in results[k].iter_mut() {
                *v += delta;
            }
        }
    }

    let mut out = vec![0f64; n];
    for i in 0..n {
        let mut a = results[0][i];
        let mut b = results[1][i];
        let mut c = results[2][i];
        let mut d = results[3][i];
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        if c > d {
            std::mem::swap(&mut c, &mut d);
        }
        if a > c {
            std::mem::swap(&mut a, &mut c);
        }
        if b > d {
            std::mem::swap(&mut b, &mut d);
        }
        if b > c {
            std::mem::swap(&mut b, &mut c);
        }
        out[i] = (b + c) / 2.0;
    }
    out
}
