//! ```mermaid fences drawn natively: Pele ported to Rust (`pele/`) lays the
//! diagram out off the UI thread, and nsh's canvas draws the scene in
//! Rox's palette — no Node, no web view. A type the port does not cover
//! yet, or a diagram it rejects, takes the old way (`web.rs`: the Node
//! helper, then mermaid.js in a web view).

mod draw;
pub(crate) mod palette;
pub(crate) mod web;

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use nsh_core::prelude::*;

pub(crate) use draw::Drawn;
pub(crate) use web::Diagram;

use crate::live::store;
use palette::PelePalette;

/// How many scenes, and drawings, are kept.
const KEEP: usize = 256;

/// A least-recently-used map.
pub(crate) struct Lru<K, V> {
    map: HashMap<K, V>,
    order: VecDeque<K>,
    cap: usize,
}

impl<K: Hash + Eq + Clone, V: Clone> Lru<K, V> {
    pub fn new(cap: usize) -> Self {
        Lru { map: HashMap::new(), order: VecDeque::new(), cap }
    }

    pub fn get(&mut self, k: &K) -> Option<V> {
        let v = self.map.get(k)?.clone();
        if let Some(i) = self.order.iter().position(|o| o == k) {
            self.order.remove(i);
        }
        self.order.push_back(k.clone());
        Some(v)
    }

    pub fn put(&mut self, k: K, v: V) {
        if self.map.insert(k.clone(), v).is_some() {
            if let Some(i) = self.order.iter().position(|o| *o == k) {
                self.order.remove(i);
            }
        }
        self.order.push_back(k);
        while self.order.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }
}

/// Why a fence is not drawn natively.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Unported {
    /// A type the port does not cover yet, or text Pele rejects (its message).
    Pele(String),
}

/// A diagram as Pele laid it out: the scene, with upstream's SVG for "copy".
pub(crate) struct Laid {
    pub scene: pele::Scene,
}

fn code_hash(code: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut h);
    h.finish()
}

fn scenes() -> &'static Mutex<Lru<u64, Arc<Result<Laid, String>>>> {
    static SCENES: OnceLock<Mutex<Lru<u64, Arc<Result<Laid, String>>>>> = OnceLock::new();
    SCENES.get_or_init(|| Mutex::new(Lru::new(KEEP)))
}

/// Lays `code` out (cached by its hash). Runs on any thread.
pub(crate) fn lay_out(code: &str) -> Arc<Result<Laid, String>> {
    let key = code_hash(code);
    if let Some(hit) = scenes().lock().unwrap().get(&key) {
        return hit;
    }
    let laid = match pele::render(code) {
        Ok(r) => pele::Scene::parse(&r.svg).map(|scene| Laid { scene }).ok_or_else(|| "unreadable scene".to_owned()),
        Err(e) => Err(e.message),
    };
    let laid = Arc::new(laid);
    scenes().lock().unwrap().put(key, laid.clone());
    laid
}

/// The palette for the mode, with the user's overrides.
pub(crate) fn palette(dark: bool) -> PelePalette {
    PelePalette::new(dark, &store().mermaid_colors.get())
}

pub(crate) type Native = Signal<Option<Result<Arc<Drawn>, Unported>>>;

thread_local! {
    static DRAWINGS: std::cell::RefCell<Lru<(u64, u64), Native>> = std::cell::RefCell::new(Lru::new(KEEP));
}

/// The drawing for `code` in the dark or light palette: `None` while Pele
/// lays it out (on a blocking thread), then the drawing, or why it is not
/// drawn natively (the caller takes the old way then).
pub(crate) fn diagram(code: &str, dark: bool) -> Native {
    let pal = palette(dark);
    let key = (code_hash(code), pal.hash64());
    if let Some(sig) = DRAWINGS.with(|d| d.borrow_mut().get(&key)) {
        return sig;
    }
    let sig = store().in_store(|| signal(None));
    DRAWINGS.with(|d| d.borrow_mut().put(key, sig));
    let code = code.to_owned();
    Owner::detached().run(|| {
        spawn(async move {
            let result = blocking(move || {
                let laid = lay_out(&code);
                match &*laid {
                    Ok(l) => Ok(Arc::new(draw::draw(&l.scene, &pal))),
                    Err(e) => Err(Unported::Pele(e.clone())),
                }
            })
            .await
            .unwrap_or_else(|e| Err(Unported::Pele(e.to_string())));
            if let Err(Unported::Pele(e)) = &result {
                log::debug!("mermaid: not native: {e}");
            }
            sig.set(Some(result));
        });
    });
    sig
}

#[cfg(test)]
mod tests {
    use super::draw::draw;
    use super::palette::{Overrides, PelePalette};
    use nsh_core::canvas::{CanvasOp, Num};
    use pele::scene::{Element, Node};
    use std::path::Path;

    fn goldens() -> Vec<(String, String)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../pele/tests/golden/flowchart");
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "svg") {
                out.push((p.file_stem().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(&p).unwrap()));
            }
        }
        out.sort();
        out
    }

    fn fill_texts(ops: &[CanvasOp]) -> Vec<(String, f32, f32)> {
        ops.iter()
            .filter_map(|op| match op {
                CanvasOp::FillText { text, x: Num::Const(x), y: Num::Const(y), .. } => Some((text.to_string(), *x, *y)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_flowchart_golden_translates_completely() {
        let pal = PelePalette::new(true, &Overrides::default());
        let all = goldens();
        assert!(all.len() > 300, "goldens: {}", all.len());
        for (name, svg) in all {
            let scene = pele::Scene::parse(&svg).unwrap_or_else(|| panic!("{name}: scene"));
            let d = draw(&scene, &pal);
            assert!(d.unknown.is_empty(), "{name}: unknown {:?}", d.unknown);
            assert!(d.w > 0.0 && d.h > 0.0, "{name}");
        }
    }

    // The x and y of every text that sets its own (a one-line label, or a
    // tspan that starts a line) are where the canvas draws it.
    fn positioned(el: &Element, tx: f32, ty: f32, out: &mut Vec<(f32, f32)>) {
        let (mut tx, mut ty) = (tx, ty);
        if let Some(t) = el.attr("transform").and_then(|t| t.strip_prefix("translate(")).and_then(|t| t.strip_suffix(')')) {
            let (x, y) = t.split_once(',').unwrap();
            tx += x.parse::<f32>().unwrap();
            ty += y.parse::<f32>().unwrap();
        }
        if el.name == "text" {
            let only_text = el.children.iter().all(|c| matches!(c, Node::Text(_)));
            if only_text {
                out.push((el.attr("x").unwrap().parse().unwrap(), el.attr("y").unwrap().parse().unwrap()));
            }
        }
        for c in &el.children {
            if let Node::Element(e) = c {
                positioned(e, tx, ty, out);
            }
        }
    }

    #[test]
    fn single_line_labels_draw_at_the_goldens_x_and_y() {
        let pal = PelePalette::new(false, &Overrides::default());
        for (name, svg) in goldens() {
            let scene = pele::Scene::parse(&svg).unwrap();
            let mut want = Vec::new();
            positioned(&scene.root, 0.0, 0.0, &mut want);
            let d = draw(&scene, &pal);
            let got: Vec<(f32, f32)> = fill_texts(&d.drawing.ops).iter().map(|t| (t.1, t.2)).collect();
            for w in &want {
                assert!(got.contains(w), "{name}: no text at {w:?} in {got:?}");
            }
        }
    }

    #[test]
    fn a_palette_override_changes_the_ops() {
        let svg = pele::render("flowchart LR\n  A-->B\n").unwrap().svg;
        let scene = pele::Scene::parse(&svg).unwrap();
        let plain = draw(&scene, &PelePalette::new(true, &Overrides::default()));
        let mut o = Overrides::default();
        o.dark.insert("surface".into(), "#ff0000".into());
        let red = draw(&scene, &PelePalette::new(true, &o));
        assert_ne!(plain.drawing.ops, red.drawing.ops);
        let fills = |ops: &[CanvasOp]| ops.iter().filter(|op| matches!(op, CanvasOp::FillStyle(nsh_core::canvas::Paint::Color(c)) if c.0 == 0xff0000ff)).count();
        assert_eq!(fills(&plain.drawing.ops), 0);
        assert!(fills(&red.drawing.ops) >= 2, "both node boxes take the surface color");
    }

    #[test]
    fn click_links_become_link_areas() {
        let svg = pele::render("flowchart LR\n  A[Docs] --> B\n  click A \"https://example.com\"\n").unwrap().svg;
        let d = draw(&pele::Scene::parse(&svg).unwrap(), &PelePalette::new(true, &Overrides::default()));
        assert_eq!(d.links.len(), 1);
        assert_eq!(d.links[0].1, "https://example.com/");
        assert!(d.links[0].0.size.width.0 > 10.0);
    }

    // Times of Pele's layout and of the translation to canvas ops, per
    // flowchart sample. Ignored; run in release:
    //   cargo test --release -p rox --lib mermaid::tests::translate_times -- --ignored --nocapture
    #[test]
    #[ignore]
    fn translate_times() {
        use std::time::Instant;
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../pele/tests/samples");
        let mut names: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("fc-")).collect();
        names.sort();
        let pal = PelePalette::new(true, &Overrides::default());
        let stats = |mut t: Vec<f64>| {
            t.sort_by(|a, b| a.partial_cmp(b).unwrap());
            (t[t.len() / 2], t[t.len() * 95 / 100])
        };
        println!("| sample | render median ms | render p95 ms | translate median ms | translate p95 ms |");
        println!("|---|---:|---:|---:|---:|");
        for p in names {
            let code = std::fs::read_to_string(&p).unwrap();
            let (mut render, mut translate) = (Vec::new(), Vec::new());
            for _ in 0..200 {
                let t = Instant::now();
                let svg = pele::render(&code).unwrap().svg;
                let scene = pele::Scene::parse(&svg).unwrap();
                render.push(t.elapsed().as_secs_f64() * 1000.0);
                let t = Instant::now();
                let _ = draw(&scene, &pal);
                translate.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            let ((rm, rp), (tm, tp)) = (stats(render), stats(translate));
            println!("| {} | {:.3} | {:.3} | {:.3} | {:.3} |", p.file_stem().unwrap().to_string_lossy(), rm, rp, tm, tp);
        }
    }
}
