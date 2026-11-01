//! ```mermaid fences drawn natively: Pele ported to Rust (hexuria/pele-rs) lays the
//! diagram out off the UI thread, and nsh's canvas draws the scene in
//! Rox's palette — no Node, no web view. A diagram Pele does not draw
//! shows Pele's message instead.

mod draw;
pub(crate) mod palette;

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use nsh_core::prelude::*;

pub(crate) use draw::Drawn;

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
        if self.map.insert(k.clone(), v).is_some()
            && let Some(i) = self.order.iter().position(|o| *o == k) {
                self.order.remove(i);
        }
        self.order.push_back(k);
        while self.order.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }
}

/// Why Pele does not draw a fence: its message, and the source line
/// (1-based; 0 when Pele gives none).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Unported {
    pub message: String,
    pub line: usize,
}

impl Unported {
    fn from(e: &pele::PeleError) -> Self {
        Unported { message: e.message.clone(), line: e.line }
    }

    /// The note shown in the diagram's place.
    pub(crate) fn note(&self) -> String {
        match self.line {
            0 => format!("can't draw: {}", self.message),
            line => format!("can't draw: {} (line {line})", self.message),
        }
    }
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

fn scenes() -> &'static Mutex<Lru<u64, Arc<Result<Laid, Unported>>>> {
    type SceneCache = Mutex<Lru<u64, Arc<Result<Laid, Unported>>>>;
    static SCENES: OnceLock<SceneCache> = OnceLock::new();
    SCENES.get_or_init(|| Mutex::new(Lru::new(KEEP)))
}

/// Lays `code` out (cached by its hash). Runs on any thread.
pub(crate) fn lay_out(code: &str) -> Arc<Result<Laid, Unported>> {
    let key = code_hash(code);
    if let Some(hit) = scenes().lock().unwrap().get(&key) {
        return hit;
    }
    let laid = match pele::render(code) {
        Ok(r) => pele::Scene::parse(&r.svg)
            .map(|scene| Laid { scene })
            .ok_or_else(|| Unported { message: "unreadable scene".to_owned(), line: 0 }),
        Err(e) => Err(Unported::from(&e)),
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
/// drawn (the caller shows its note).
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
                    Err(e) => Err(e.clone()),
                }
            })
            .await
            .unwrap_or_else(|e| Err(Unported { message: e.to_string(), line: 0 }));
            if let Err(e) = &result {
                log::debug!("mermaid: not native: {e:?}");
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

    // Every flowchart input of the port's own tests that Pele renders, as the
    // scene it laid out. The scene must write back Pele's SVG byte for byte,
    // so a drawing checked against it is checked against the oracle.
    fn flowchart_scenes() -> Vec<(String, pele::Scene)> {
        let mut out = Vec::new();
        for (name, code) in pele::corpus::flowchart() {
            let Ok(r) = pele::render(&code) else { continue };
            let scene = pele::Scene::parse(&r.svg).unwrap_or_else(|| panic!("{name}: scene"));
            assert_eq!(pele::svg::write::write(&scene), r.svg, "{name}: scene round trip");
            out.push((name, scene));
        }
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
    fn every_flowchart_input_translates_completely() {
        let pal = PelePalette::new(true, &Overrides::default());
        let all = flowchart_scenes();
        assert!(all.len() > 300, "flowchart inputs: {}", all.len());
        for (name, scene) in all {
            let d = draw(&scene, &pal);
            assert!(d.unknown.is_empty(), "{name}: unknown {:?}", d.unknown);
            assert!(d.w > 0.0 && d.h > 0.0, "{name}");
        }
    }

    // Every type the port registers: each input that renders draws with no
    // element or attribute the translation does not know.
    #[test]
    fn every_ported_type_translates_completely() {
        let pal = PelePalette::new(true, &Overrides::default());
        let all = pele::corpus::all();
        let types: Vec<&str> = all.iter().map(|(ty, _)| *ty).collect();
        assert_eq!(types, pele::PORTED);
        let mut unknown: std::collections::BTreeMap<String, (usize, String)> = Default::default();
        for (ty, inputs) in all {
            let mut seen = 0;
            for (name, code) in inputs {
                let Ok(r) = pele::render(&code) else { continue };
                let scene = pele::Scene::parse(&r.svg).unwrap_or_else(|| panic!("{ty} {name}: scene"));
                let d = draw(&scene, &pal);
                assert!(d.w > 0.0 && d.h > 0.0, "{ty} {name}");
                for u in d.unknown {
                    let e = unknown.entry(format!("{ty}: {u}")).or_insert((0, name.clone()));
                    e.0 += 1;
                }
                seen += 1;
            }
            assert!(seen > 0, "{ty}: no input renders");
        }
        assert!(unknown.is_empty(), "unknown (uses, first input): {unknown:#?}");
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
    fn single_line_labels_draw_at_the_scenes_x_and_y() {
        let pal = PelePalette::new(false, &Overrides::default());
        for (name, scene) in flowchart_scenes() {
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
    fn a_group_opacity_fades_what_it_holds() {
        // Over a fill the diagram chose, an entity's row stripes are a tint of it: a group at 0.6.
        let svg = pele::render("erDiagram\n  CUSTOMER:::red {\n    string name\n    string email\n  }\n  classDef red fill:#ff0000\n").unwrap().svg;
        assert!(svg.contains(" opacity=\"0.6\""));
        let pal = PelePalette::new(true, &Overrides::default());
        let faded = |svg: &str| {
            let d = draw(&pele::Scene::parse(svg).unwrap(), &pal);
            d.drawing.ops.iter().filter(|op| matches!(op, CanvasOp::FillStyle(nsh_core::canvas::Paint::Color(c)) if c.0 & 0xff == 153)).count()
        };
        assert!(faded(&svg) >= 1);
        assert_eq!(faded(&svg.replace(" opacity=\"0.6\"", "")), 0);
    }

    #[test]
    fn an_underlined_span_draws_its_rule() {
        let rules = |code: &str| {
            let svg = pele::render(code).unwrap().svg;
            let d = draw(&pele::Scene::parse(&svg).unwrap(), &PelePalette::new(false, &Overrides::default()));
            d.drawing.ops.iter().filter(|op| matches!(op, CanvasOp::FillRect(_))).count()
        };
        // A static member is underlined.
        assert_eq!(rules("classDiagram\n  class Counter {\n    +total$\n  }\n"), 1);
        assert_eq!(rules("classDiagram\n  class Counter {\n    +total\n  }\n"), 0);
    }

    fn hand_drawn(body: &str) -> super::Drawn {
        let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\" width=\"100\" height=\"100\">{body}</svg>");
        draw(&pele::Scene::parse(&svg).unwrap(), &PelePalette::new(false, &Overrides::default()))
    }

    fn transforms(ops: &[CanvasOp]) -> Vec<[f32; 6]> {
        ops.iter()
            .filter_map(|op| match op {
                CanvasOp::Transform(m) => Some(m.map(|v| if let Num::Const(v) = v { v } else { f32::NAN })),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_rotation_turns_about_its_centre() {
        // gitGraph sets each commit label at -45 degrees about its own point.
        let svg = pele::render("gitGraph\n  commit\n").unwrap().svg;
        assert!(svg.contains("transform=\"rotate(-45 3 16.36)\""), "{svg}");
        let d = draw(&pele::Scene::parse(&svg).unwrap(), &PelePalette::new(true, &Overrides::default()));
        let m = transforms(&d.drawing.ops);
        let (s, c) = (-45f32).to_radians().sin_cos();
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        let turn = m.iter().find(|m| near(m[0], c) && near(m[1], s) && near(m[2], -s) && near(m[3], c)).expect("a rotation");
        // The centre stays where it is.
        let (x, y) = (3.0, 16.36);
        assert!(near(turn[0] * x + turn[2] * y + turn[4], x) && near(turn[1] * x + turn[3] * y + turn[5], y), "{turn:?}");
        assert!(d.unknown.is_empty(), "{:?}", d.unknown);
    }

    #[test]
    fn text_anchor_is_inherited_from_a_group() {
        let aligns = |d: &super::Drawn| -> Vec<nsh_core::canvas::CanvasTextAlign> {
            d.drawing.ops.iter().filter_map(|op| if let CanvasOp::TextAlign(a) = op { Some(*a) } else { None }).collect()
        };
        use nsh_core::canvas::CanvasTextAlign::{Center, Left, Right};
        assert_eq!(aligns(&hand_drawn("<g text-anchor=\"middle\"><text x=\"50\" y=\"20\">Hi</text></g>")), vec![Center]);
        assert_eq!(aligns(&hand_drawn("<g text-anchor=\"middle\"><text x=\"50\" y=\"20\" text-anchor=\"end\">Hi</text></g>")), vec![Right]);
        assert_eq!(aligns(&hand_drawn("<g><text x=\"50\" y=\"20\">Hi</text></g>")), vec![Left]);
    }

    #[test]
    fn a_nested_svg_maps_its_view_box() {
        // An icon: 24-unit artwork in a 12-point box at (10,20).
        let d = hand_drawn("<svg class=\"pele-icon\" x=\"10\" y=\"20\" width=\"12\" height=\"12\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\"><path d=\"M0,0L24,24\"/></svg>");
        assert_eq!(transforms(&d.drawing.ops), vec![[0.5, 0.0, 0.0, 0.5, 10.0, 20.0]]);
        assert!(d.unknown.is_empty(), "{:?}", d.unknown);
    }

    #[test]
    fn a_rounding_may_be_the_smaller_of_two() {
        let radius = |style: &str| {
            let d = hand_drawn(&format!("<rect x=\"0\" y=\"0\" width=\"40\" height=\"20\" fill=\"red\" style=\"{style}\"/>"));
            assert!(d.unknown.is_empty(), "{:?}", d.unknown);
            d.drawing.ops.iter().find_map(|op| match op {
                CanvasOp::Path(nsh_core::canvas::PathOp::RoundRect(_, [Num::Const(r), ..])) => Some(*r),
                _ => None,
            })
        };
        assert_eq!(radius("rx:min(6px,3px)"), Some(3.0));
        assert_eq!(radius("rx:min(var(--nope,2px),3px)"), Some(2.0));
    }

    #[test]
    fn what_pele_does_not_draw_says_why() {
        use super::lay_out;
        let note = |code: &str| match &*lay_out(code) {
            Ok(_) => panic!("{code}: renders"),
            Err(e) => e.note(),
        };
        // No diagram type: no line.
        assert_eq!(note("not a diagram\n"), "can't draw: No diagram type detected for the given text.");
        let n = note("flowchart LR\n  A-->\n");
        assert!(n.starts_with("can't draw: ") && n.contains("(line "), "{n}");
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
    //   cargo test --release -p buwiz-bot --lib mermaid::tests::translate_times -- --ignored --nocapture
    #[test]
    #[ignore]
    fn translate_times() {
        use std::time::Instant;
        let samples: Vec<_> = pele::corpus::samples().into_iter().filter(|(name, _)| name.starts_with("fc-")).collect();
        let pal = PelePalette::new(true, &Overrides::default());
        let stats = |mut t: Vec<f64>| {
            t.sort_by(|a, b| a.partial_cmp(b).unwrap());
            (t[t.len() / 2], t[t.len() * 95 / 100])
        };
        println!("| sample | render median ms | render p95 ms | translate median ms | translate p95 ms |");
        println!("|---|---:|---:|---:|---:|");
        for (name, code) in samples {
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
            println!("| {} | {:.3} | {:.3} | {:.3} | {:.3} |", name, rm, rp, tm, tp);
        }
    }
}
