//! Avatars drawn by the real mark engine, as SVG files.
//!
//! `mark-sheet spin <design.json> <out dir> [box px]`: the design at 16
//! yaws, then looking up and down at four yaws, then at rest, gazing,
//! blinking and poked.
//! `mark-sheet dump <design.json> <out file>`: every frame of a fixed run
//! as one text file, to compare two builds byte for byte.
//! `mark-sheet time <design.json>...`: microseconds per frame.
//!
//! A design is the JSON the app saves: `outline`, `color`, `layers`,
//! `eyes`, `parts`, `solids`.
#![allow(dead_code, unused_imports, unused_variables)]

#[path = "../../../src"]
mod app {
    pub(crate) mod mark;
}

use app::mark::*;
use serde_json::{json, Value};

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
}

fn svg(tree: &MarkTree, out: &mut String) {
    out.push('<');
    out.push_str(tree.tag);
    for (k, v) in &tree.attrs {
        out.push_str(&format!(" {}=\"{}\"", k, escape(v)));
    }
    out.push('>');
    for c in &tree.children {
        svg(c, out);
    }
    out.push_str(&format!("</{}>", tree.tag));
}

fn draw(frame: &Frame, tab: &std::rc::Rc<Tables>, box_: f64) -> MarkTree {
    let extent = (box_ * (0.92 / 0.6) * 100.0f64).round() / 100.0;
    uh(
        frame,
        &PaintOpts {
            cutout: true,
            ink_paint: Some("#1884FC".to_owned()),
            ..Default::default()
        },
        extent,
        "m",
        &UhSize {
            display_size: Some(box_),
            extent,
        },
        tab,
    )
}

/// The file a frame goes to, with room around the mark: the app's canvas is
/// wider than the mark's extent, and an avatar may reach into it.
fn write(frame: &Frame, tab: &std::rc::Rc<Tables>, box_: f64, path: &str) {
    let mut s = String::new();
    svg(&draw(frame, tab, box_), &mut s);
    let extent = (box_ * (0.92 / 0.6) * 100.0f64).round() / 100.0;
    let view = extent * 1.5 / 1.257;
    let pad = (view - extent) / 2.0;
    let vb = s.split("viewBox=\"0 0 ").nth(1).and_then(|r| r.split(' ').next()).unwrap_or("1");
    let vb: f64 = vb.parse().unwrap_or(extent);
    let k = vb / extent;
    let s = s.replacen(
        &format!("viewBox=\"0 0 {} {}\"", un2(vb), un2(vb)),
        &format!("viewBox=\"{} {} {} {}\"", -pad * k, -pad * k, view * k, view * k),
        1,
    );
    let s = s.replacen(&format!("width=\"{}\"", (extent * 100.0).round() / 100.0), &format!("width=\"{view}\""), 1);
    let s = s.replacen(&format!("height=\"{}\"", (extent * 100.0).round() / 100.0), &format!("height=\"{view}\""), 1);
    std::fs::write(path, s).unwrap();
}

/// The viewBox number as `kc` writes it.
fn un2(v: f64) -> String {
    let s = format!("{v}");
    s
}

fn layer(x: &Value) -> Value {
    let mut l = vec![
        x["path"].clone(),
        x["fill"].clone(),
        x.get("opacity").and_then(|o| o.as_f64()).unwrap_or(1.0).into(),
    ];
    if x.get("over").and_then(|o| o.as_bool()).unwrap_or(false) {
        l.push("over".into());
    }
    l.into()
}

/// A design as the engine's recipe (what `store::shape_of` builds).
fn recipe(d: &Value) -> String {
    let layers = |v: &Value| -> Vec<Value> {
        v["layers"].as_array().map(|l| l.iter().map(layer).collect()).unwrap_or_default()
    };
    let mut r = json!({"o": d["outline"], "c": d["color"], "l": layers(d), "e": d["eyes"]});
    if let Some(ps) = d["parts"].as_array().filter(|p| !p.is_empty()) {
        let parts: Vec<Value> = ps
            .iter()
            .map(|p| {
                let mut q = json!({"o": p["outline"], "c": p["color"], "l": layers(p), "z": p.get("z").and_then(|z| z.as_f64()).unwrap_or(0.0)});
                if p["d"].is_number() {
                    q["d"] = p["d"].clone();
                }
                q
            })
            .collect();
        r["p"] = parts.into();
    }
    if let Some(s) = d.get("solids").filter(|s| s.as_array().is_some_and(|a| !a.is_empty())) {
        r["s"] = s.clone();
    }
    r.to_string()
}

fn load(path: &str) -> (&'static str, Value) {
    let d: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let name = rich_body(&recipe(&d)).unwrap_or_else(|e| panic!("{path}: {e}"));
    (name, d)
}

/// The rotation that turns a mark `yaw` then `pitch` degrees.
fn posed(frame: &mut Frame, yaw: f64, pitch: f64) {
    frame.quat = ib(pitch * MN, yaw * MN, 0.0);
    frame.turn = [pitch, yaw, 0.0];
}

fn spin(path: &str, out: &str, box_: f64) {
    let (body, _) = load(path);
    let tab = shared_tables();
    std::fs::create_dir_all(out).unwrap();
    let mut mark = Mark::new(&tab, Some(body), 1.0, false, false, 7);
    for _ in 0..30 {
        mark.advance(1.0 / 30.0);
    }
    write(&mark.frame(), &tab, box_, &format!("{out}/rest.svg"));
    let mut frame = mark.frame();
    for i in 0..16 {
        let yaw = i as f64 * 22.5;
        posed(&mut frame, yaw, 0.0);
        write(&frame, &tab, box_, &format!("{out}/yaw{:03}.svg", i * 225 / 10));
    }
    for pitch in [-55, -30, 30, 55] {
        for yaw in [0, 45, 135, 225, 315] {
            posed(&mut frame, yaw as f64, pitch as f64);
            let p = if pitch < 0 { format!("m{}", -pitch) } else { format!("p{pitch}") };
            write(&frame, &tab, box_, &format!("{out}/pitch{p}-yaw{yaw:03}.svg"));
        }
    }
    // gaze and blink
    for (x, y, l) in [(-1.0, 0.0, "l"), (1.0, 0.0, "r"), (0.0, -1.0, "u"), (0.0, 1.0, "d")] {
        mark.set_gaze(Some([x, y]));
        for _ in 0..60 {
            mark.advance(1.0 / 30.0);
        }
        write(&mark.frame(), &tab, box_, &format!("{out}/gaze-{l}.svg"));
    }
    // the states and a spin, frame by frame
    for state in ["thinking", "happy", "laughing"] {
        let name = state_name(state);
        let mut m = Mark::new(&tab, Some(body), 1.0, false, false, 11);
        let mut d = Director::new(&mut m, name, 11, false, false, None);
        for f in 0..150 {
            d.advance(&mut m, 1.0 / 30.0);
            m.advance(1.0 / 30.0);
            if f % 50 == 49 {
                write(&m.frame(), &tab, box_, &format!("{out}/state-{state}-{f:03}.svg"));
            }
        }
    }
    let mut m = Mark::new(&tab, Some(body), 1.0, false, false, 5);
    let mut d = Director::new(&mut m, "idle", 5, false, false, None);
    for _ in 0..10 {
        m.advance(1.0 / 30.0);
    }
    m.poke("spin", 1.0);
    for f in 0..48 {
        d.advance(&mut m, 1.0 / 30.0);
        m.advance(1.0 / 30.0);
        if f % 3 == 1 {
            write(&m.frame(), &tab, box_, &format!("{out}/poke-{f:02}.svg"));
        }
    }
}

/// Every frame of a fixed run, as text.
fn dump(path: &str, out: &str) {
    let (body, _) = load(path);
    let tab = shared_tables();
    let mut text = String::new();
    let mut mark = Mark::new(&tab, Some(body), 1.0, false, false, 7);
    let mut dir = Director::new(&mut mark, "idle", 7, false, true, None);
    for box_ in [22.96, 52.48, 78.72] {
        for f in 0..40 {
            mark.advance(1.0 / 30.0);
            if f % 4 == 0 {
                svg(&draw(&mark.frame(), &tab, box_), &mut text);
                text.push('\n');
            }
        }
    }
    dir.set_resting(&mut mark, false);
    for state in ["thinking", "happy", "laughing", "surprised"] {
        dir.set_state_pub(&mut mark, state);
        for f in 0..60 {
            dir.advance(&mut mark, 1.0 / 30.0);
            mark.advance(1.0 / 30.0);
            if f % 3 == 0 {
                svg(&draw(&mark.frame(), &tab, 52.48), &mut text);
                text.push('\n');
            }
        }
    }
    mark.poke("spin", 1.0);
    for _ in 0..45 {
        dir.advance(&mut mark, 1.0 / 30.0);
        mark.advance(1.0 / 30.0);
        svg(&draw(&mark.frame(), &tab, 52.48), &mut text);
        text.push('\n');
    }
    std::fs::write(out, text).unwrap();
}

/// Build-and-tree time per frame through a full turn, and the size of the
/// trees.
fn time(paths: &[String]) {
    let tab = shared_tables();
    for path in paths {
        let t = std::time::Instant::now();
        let (body, _) = load(path);
        let built = t.elapsed();
        let mark = Mark::new(&tab, Some(body), 1.0, false, false, 7);
        let mut frame = mark.frame();
        for box_ in [22.96, 52.48] {
            let mut count = (0usize, 0usize);
            let t = std::time::Instant::now();
            let n = 720;
            for i in 0..n {
                posed(&mut frame, i as f64, 12.0);
                let tree = draw(&frame, &tab, box_);
                let c = paths_in(&tree);
                count = (count.0 + c.0, count.1 + c.1);
            }
            println!(
                "{path}: register {built:?}, box {box_}: {:.0} us/frame, {:.0} paths, {:.0} path bytes",
                t.elapsed().as_micros() as f64 / n as f64,
                count.0 as f64 / n as f64,
                count.1 as f64 / n as f64,
            );
        }
    }
}

fn paths_in(tree: &MarkTree) -> (usize, usize) {
    let mut c = if tree.tag == "path" { (1, tree.attr("d").map_or(0, str::len)) } else { (0, 0) };
    for k in &tree.children {
        let (a, b) = paths_in(k);
        c = (c.0 + a, c.1 + b);
    }
    c
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("spin") => spin(&a[2], &a[3], a.get(4).map_or(300.0, |s| s.parse().unwrap())),
        Some("dump") => dump(&a[2], &a[3]),
        Some("time") => time(&a[2..]),
        _ => eprintln!("mark-sheet spin|dump|time ..."),
    }
}
