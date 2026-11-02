//! The avatar checks against Node's own (server/src/avatar-shape.mjs): the
//! same outlines, recipes and replies give the same errors and candidates,
//! and both read one system prompt.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

use serde_json::{json, Value};

use super::*;

fn server() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../server")
}

/// Node's answer to `code` (an ES module body that prints JSON), given
/// `input` as the global `input`.
fn node(code: &str, input: &Value) -> Value {
    let file = std::env::temp_dir().join(format!("rox-avatar-shape-{}.json", crate::hub::store::uuidv7()));
    std::fs::write(&file, input.to_string()).unwrap();
    let o = Command::new("node")
        .args(["--input-type=module", "-e", &format!("import {{ readFileSync }} from \"node:fs\";\nconst input = JSON.parse(readFileSync(process.env.AVD_INPUT, \"utf8\"));\nconst m = await import(\"./src/avatar-shape.mjs\");\n{code}")])
        .current_dir(server())
        .env("AVD_INPUT", &file)
        .output()
        .expect("node runs");
    let _ = std::fs::remove_file(&file);
    assert!(o.status.success(), "node failed: {}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).expect("node prints JSON")
}

const GOOD: [&str; 8] = [
    MOCK_PEBBLE,
    MOCK_CAT,
    MOCK_RECEIPT,
    "m50 15 q35 0 35 35 q0 35 -35 35 q-35 0 -35 -35 q0 -35 35 -35 z",
    "M50 2 Q60 40 98 50 Q60 60 50 98 Q40 60 2 50 Q40 40 50 2 Z",
    "M50 4 L58 36 L90 27 L66 50 L90 73 L58 64 L50 96 L42 64 L10 73 L34 50 L10 27 L42 36 Z",
    "M50 85 C20 65 10 45 15 30 C20 15 40 12 50 28 C60 12 80 15 85 30 C90 45 80 65 50 85 Z",
    "M15 15 85 15 85 85 15 85 Z",
];

/// AVD-02's bad cases, and the edges of the parser.
fn bad() -> Vec<String> {
    let mut v: Vec<String> = [
        "M10 10 H90 V90 H10 Z",
        "M10 10 A40 40 0 1 1 90 90 Z",
        "M1e1 10 L90 10 L90 90 Z",
        "M10 10 L90 10 L90",
        "L10 10 L90 10 L90 90 Z",
        "M10.5.5 10 L90 10 L90 90 Z",
        "M-20 10 L90 10 L90 90 L10 90 Z",
        "M15 15 L85 15 L85 85 L15 85",
        "M10 10 L40 10 L40 40 Z M60 60 L90 60 L90 90 Z",
        "M40 40 L60 40 L60 60 L40 60 Z",
        "M5 40 L95 40 L95 70 L5 70 Z",
        "M60 10 C20 10 10 50 30 80 C40 95 70 95 85 75 C55 80 40 60 45 40 C50 20 60 15 60 10 Z",
        "M15 15 L85 85 L85 15 L15 85 Z",
        "M30 20 L70 20 L70 48 L98 48 L98 51 L70 51 L70 80 L30 80 Z",
        "M10 10 L90 10 Z 5",
        "M10 10 L90 10 Z L5 5",
        "M10 10 - 5 L90 90 Z",
        "M10,10,90,10,90,90,10,90z",
        "M 50 50 Z",
        "M50 10 L50 10 L50 10 Z",
        "M10 10 L90 10 L50 10 Z",
        "M10 40 L90 40 L90 41 L10 41 Z",
        "M50 50 L50 90",
        "Z",
        "   ",
        "",
        "M10 10\u{a0}L90 10 L90 90 Z",
        "m10 10 l80 0 l0 80 l-80 0 z",
        "M10 10 L90 10 L90 90 L10 90 Z m5 5",
        ".5.5",
        "M.5 .5 L90 .5 L90 90 L.5 90 Z",
        "M-5 -5 L105 -5 L105 105 L-5 105 Z",
        "M-5.1 -5 L105 -5 L105 105 L-5 105 Z",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    v.push(format!("M10 10 {}Z", "L90 10 L90 90 L10 90 ".repeat(200)));
    v
}

/// Pseudo-random outlines: polar polygons and curves around the centre,
/// some spiky, some off the box, so every check fires somewhere.
fn random_outlines(n: usize) -> Vec<String> {
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..n)
        .map(|i| {
            let k = 3 + (rnd() * 12.0) as usize;
            let (cx, cy) = (50.0 + (rnd() - 0.5) * 20.0, 50.0 + (rnd() - 0.5) * 20.0);
            let base = 15.0 + rnd() * 40.0;
            let pts: Vec<(f64, f64)> = (0..k)
                .map(|j| {
                    let th = j as f64 / k as f64 * std::f64::consts::TAU + rnd() * 0.3;
                    let r = base * (0.4 + rnd() * if i % 3 == 0 { 1.6 } else { 0.8 });
                    (((cx + r * th.cos()) * 10.0).round() / 10.0, ((cy + r * th.sin()) * 10.0).round() / 10.0)
                })
                .collect();
            let mut d = format!("M{} {}", pts[0].0, pts[0].1);
            for (j, p) in pts.iter().enumerate().skip(1) {
                match (i + j) % 3 {
                    0 => d.push_str(&format!(" L{} {}", p.0, p.1)),
                    1 => d.push_str(&format!(" Q{} {} {} {}", ((cx + p.0) / 2.0 + rnd() * 10.0).round(), ((cy + p.1) / 2.0).round(), p.0, p.1)),
                    _ => d.push_str(&format!(" C{} {} {} {} {} {}", (p.0 + rnd() * 8.0).round(), p.1, p.0, (p.1 - rnd() * 8.0).round(), p.0, p.1)),
                }
            }
            if i % 7 != 0 {
                d.push_str(" Z");
            }
            d
        })
        .collect()
}

#[test]
fn avatar_outline_errors_match_nodes() {
    let mut all: Vec<String> = GOOD.iter().map(|s| s.to_string()).collect();
    all.extend(bad());
    all.extend(random_outlines(300));
    let theirs = node("console.log(JSON.stringify(input.map((o) => m.outlineErrors(o))));", &json!(all));
    let ours: Vec<Vec<String>> = all.iter().map(|o| outline_errors(Some(o))).collect();
    for (k, (o, t)) in ours.iter().zip(theirs.as_array().unwrap()).enumerate() {
        assert_eq!(&json!(o), t, "outline {k}: {}", all[k]);
    }
    for g in GOOD {
        assert_eq!(outline_errors(Some(g)), Vec::<String>::new(), "{g}");
    }
    let fired: std::collections::HashSet<String> = ours.iter().flatten().map(|e| e.split(' ').take(3).collect::<Vec<_>>().join(" ")).collect();
    assert!(fired.len() >= 12, "the battery reaches most checks: {fired:?}");
}

#[test]
fn avatar_validate_matches_nodes() {
    let recipes = json!([
        {"shape": "blob", "color": "#1084FE"}, {"shape": "blob", "color": "#ff309b"}, {"shape": "moon", "color": "#1084FE"},
        {"color": "#1084FE"}, {"shape": "blob", "color": "#FFF"}, {"shape": "blob", "color": "red"}, {"shape": "blob"},
        {"shape": "custom", "color": "#1084FE"}, {"shape": "custom", "color": "#1084FE", "outline": MOCK_CAT},
        {"shape": "heart", "color": "#FF263C", "outline": "nonsense"}, {"shape": 5, "color": 7, "outline": 9},
        {"shape": "custom", "color": "#12345G", "outline": "M0 0 Z"},
        // parts
        {"shape": "custom", "color": "#1E1E1E", "outline": MOCK_CAT, "parts": [{"outline": "M40 30 L60 30 L60 50 L40 50 Z", "color": "#7A4A22", "z": -1}]},
        {"shape": "heart", "color": "#1E1E1E", "parts": []},
        {"shape": "custom", "color": "#1E1E1E", "outline": MOCK_CAT, "parts": "x"},
        {"shape": "custom", "color": "#1E1E1E", "outline": MOCK_CAT, "parts": [5, null, [], {"outline": "M30 40 L70 40 L70 46 L30 46 Z", "color": "#E8C85A"}, {"outline": "M44 44 L56 44 L56 56 L44 56 Z", "color": "brown", "z": 2, "layers": [{"path": "M1 1 Z", "fill": "#000000"}]}, {"outline": "M40 30 L60 30 L60 50 L40 50 Z", "color": "#7A4A22", "layers": "x", "z": null}, {"outline": "M40 30 L60 30 L60 50 L40 50 Z", "color": "#7A4A22", "layers": []}]},
        // rich: layers and eyes
        {"shape": "custom", "color": "#3E8E2F", "outline": MOCK_CAT, "layers": [{"path": "M30 60 L70 60 L50 80 Z", "fill": "#F5EBD6"}], "eyes": "#58D63A"},
        {"shape": "heart", "color": "#FF0000", "layers": [{"path": "M30 60 L70 60 L50 80 Z", "fill": "#F5EBD6"}], "eyes": "#FFFFFF"},
        {"shape": "custom", "color": "#3E8E2F", "outline": MOCK_CAT, "eyes": "#58D63A"},
        {"shape": "custom", "color": "#3E8E2F", "outline": MOCK_CAT, "layers": [], "eyes": null},
        {"shape": "custom", "color": "#3E8E2F", "outline": MOCK_CAT, "layers": [{"path": "M30 60 L70 60 L50 80 Z", "fill": "#F5EBD6"}]},
        {"shape": "custom", "color": "#3E8E2F", "outline": MOCK_CAT, "eyes": "nope", "layers": [
            {"path": "M10 10 L20 10 L20 20 Z", "fill": "red"}, {"path": "M10 10 L20 10 L20 20 Z", "fill": "#FF0000", "opacity": 2},
            {"path": "M10 10 H20 V20 Z", "fill": "#FF0000"}, {"path": "M10 10 L20 10", "fill": "#FF0000", "opacity": null},
            5, null, [], {"fill": "#FF0000"}, {"path": "  ", "fill": "#FF0000"}, {"path": "M-9 10 L20 10 L20 20 Z", "fill": "#FF0000", "opacity": 0.5},
            {"path": "M10 10 L20 10 L20", "fill": "#FF0000"}, {"path": "M10 10 L10.5 10 L10 10.5 Z", "fill": "#ff0000"},
        ]},
    ]);
    let theirs = node("console.log(JSON.stringify(input.map((r) => m.validateAvatar(r))));", &recipes);
    for (r, t) in recipes.as_array().unwrap().iter().zip(theirs.as_array().unwrap()) {
        let s = |k: &str| r.get(k).and_then(Value::as_str);
        assert_eq!(json!(validate_avatar(s("shape"), s("color"), s("outline"), r.get("layers"), r.get("eyes"), r.get("parts"))), *t, "{r}");
    }
}

#[test]
fn avatar_design_system_is_nodes() {
    let theirs = node("console.log(JSON.stringify(m.DESIGN_SYSTEM));", &json!(null));
    assert_eq!(design_system(), theirs.as_str().unwrap());
    for (k, d) in KIT {
        assert!(design_system().contains(&format!("\n- {k}: {d}\n")), "{k}");
    }
}

#[test]
fn avatar_parse_candidates_and_mocks_match_nodes() {
    let c = json!({"name": "Fox", "shape": "custom", "color": "#ff6700", "outline": format!("  {MOCK_CAT}\n"), "why": "A fox."});
    let texts = json!([
        format!("Here you go:\n```json\n{}\n```\nEnjoy!", json!({"candidates": [c]})),
        json!([c]).to_string(),
        json!([{"name": "Fox", "shape": "", "outline": MOCK_CAT, "colour": "#00c972"}, {"shape": "Heart", "color": "#ff6700", "outline": "x", "name": "x".repeat(50), "why": "y".repeat(300)}]).to_string(),
        "no json", "{broken", "{\"shape\":\"star\",\"color\":\"#FF309B\"}", "[1, null, [], {\"name\": 3, \"shape\": true}]",
        "{\"candidates\": 5, \"shape\": \"hex\"}", "```JSON {\"candidates\":[{\"shape\":\"custom\",\"outline\":\"M1  2\\t\\nL3 4\"}]} ```",
    ]);
    let theirs = node("console.log(JSON.stringify(input.map((t) => m.parseCandidates(t))));", &texts);
    for (t, n) in texts.as_array().unwrap().iter().zip(theirs.as_array().unwrap()) {
        let ours: Vec<Value> = parse_candidates(t.as_str().unwrap()).iter().map(Candidate::json).collect();
        assert_eq!(json!(ours), *n, "{t}");
    }
    let mocks = node("console.log(JSON.stringify([m.mockCandidates({}), m.mockCandidates({ image: true })]));", &json!(null));
    let ours = json!([mock_candidates(false).iter().map(Candidate::json).collect::<Vec<_>>(), mock_candidates(true).iter().map(Candidate::json).collect::<Vec<_>>()]);
    assert_eq!(ours, mocks);
    for (prompt, name, image, rich) in [("files my BIR forms", "Tax Fox", false, false), ("keep the ears", "", true, false), ("", "", true, false), ("", "Giant", true, true), ("a green giant", "", false, true)] {
        let theirs = node(&format!("console.log(JSON.stringify(m.designRequest({{ prompt: {}, name: {}, image: {image}, rich: {rich} }})));", json!(prompt), json!(name)), &json!(null));
        assert_eq!(json!(design_request(prompt, name, image, rich)), theirs, "{prompt:?} {image} {rich}");
    }
    let rich = node("console.log(JSON.stringify([m.RICH_SYSTEM, m.MOCK_RICH, m.MOCK_RICH.map((c) => m.validateAvatar(c))]));", &json!(null));
    assert_eq!(rich[0], json!(rich_system()));
    assert_eq!(rich[1], json!(mock_rich()));
    assert_eq!(rich[2], json!([[], [], []]));
    // rich replies: layers and eyes carried (custom only), tidied
    let rich_texts = json!([
        json!({"candidates": mock_rich()}).to_string(),
        json!([{"shape": "custom", "outline": MOCK_CAT, "color": "#1e1e1e", "parts": [{"outline": " M40 30 L60 30\n L60 50 L40 50 Z ", "colour": "#7a4a22", "z": -1, "layers": [["M42 32 L58 32 L50 40 Z", "#ffffff", 0.5]]}, 5, [], {"color": "#E8C85A", "z": "x", "layers": "x"}]}, {"shape": "blob", "parts": [{"outline": "M1 1 Z", "color": "#000000"}]}]).to_string(),
        json!([{"shape": "custom", "outline": MOCK_CAT, "layers": [{"path": "  M1 2\n L3 4 ", "color": "#abcdef", "opacity": 0.5}, 5, [], {"fill": "#123456", "opacity": "x"}], "eyes": " #ffeedd "}, {"shape": "blob", "layers": [], "eyes": "#FFFFFF"}, {"shape": "custom", "outline": MOCK_CAT, "layers": "x", "eyes": 5}]).to_string(),
    ]);
    let theirs = node("console.log(JSON.stringify(input.map((t) => m.parseCandidates(t))));", &rich_texts);
    for (t, n) in rich_texts.as_array().unwrap().iter().zip(theirs.as_array().unwrap()) {
        let ours: Vec<Value> = parse_candidates(t.as_str().unwrap()).iter().map(Candidate::json).collect();
        assert_eq!(json!(ours), *n, "{t}");
    }
}

/// The design loop over scripted replies, in Rust and in Node: the same
/// candidates, the same messages each call got, the same picks.
#[test]
fn avatar_design_candidates_match_nodes() {
    let kit = |n: &str, s: &str, c: &str| json!({"name": n, "shape": s, "color": c, "why": format!("{n}.")});
    let cust = |n: &str, o: &str| json!({"name": n, "shape": "custom", "color": "#00C972", "outline": o, "why": format!("{n}.")});
    let img = json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}});
    let wrap = |v: Value| json!(json!({"candidates": v}).to_string());
    let cases = json!([
        {"prompt": "files my BIR forms", "name": "Tax Fox", "replies": [wrap(json!([kit("A", "blob", "#1084FE"), cust("B", MOCK_PEBBLE), cust("C", MOCK_CAT)]))]},
        {"prompt": "x", "replies": [wrap(json!([kit("A", "blob", "#1084FE"), cust("Thin", "M15 15 L85 85 L85 15 L15 85 Z"), kit("C", "moon", "#1084FE")])), wrap(json!([kit("A", "blob", "#1084FE"), cust("B", MOCK_PEBBLE), kit("C", "star", "#1084FE")]))]},
        {"prompt": "x", "replies": [wrap(json!([kit("A", "blob", "#1084FE"), cust("B", "M0 0 Z"), cust("C", "M0 0 Z")])), wrap(json!([cust("B2", "M0 0 Z"), cust("C2", MOCK_PEBBLE), kit("D", "hex", "#FFF")]))]},
        {"prompt": "reads receipts", "replies": ["I cannot draw.", "  "]},
        {"prompt": "y", "replies": [wrap(json!([cust("A", MOCK_PEBBLE), cust("B", MOCK_CAT), cust("C", MOCK_RECEIPT)])), wrap(json!([cust("A", MOCK_PEBBLE), cust("B", MOCK_CAT), cust("C", MOCK_RECEIPT)]))]},
        {"prompt": "keep the ears", "image": img, "replies": [wrap(json!([kit("A", "blob", "#1084FE"), cust("B", MOCK_PEBBLE), cust("C", MOCK_CAT)])), wrap(json!([cust("Subject", MOCK_CAT), kit("A", "blob", "#1084FE"), cust("C", MOCK_PEBBLE)]))]},
        {"prompt": "z", "image": img, "replies": ["nothing", "still nothing"]},
        {"prompt": "", "image": img, "replies": [wrap(json!([kit("A", "blob", "#1084FE"), cust("B", MOCK_PEBBLE), cust("C", MOCK_CAT)])), wrap(json!([cust("Subject", MOCK_CAT), kit("A", "blob", "#1084FE"), cust("C", MOCK_PEBBLE)]))]},
        // rich: no kit rule; a candidate without layers is retried, then dropped; none left is an error
        {"prompt": "a green giant", "rich": true, "replies": [wrap(json!(mock_rich()))]},
        {"prompt": "a green giant", "rich": true, "replies": [wrap(json!([mock_rich()[0], cust("Plain", MOCK_PEBBLE), mock_rich()[2]])), wrap(json!([mock_rich()[0], cust("Plain", MOCK_PEBBLE)]))]},
        {"prompt": "x", "rich": true, "image": img, "replies": ["nothing", wrap(json!([cust("Plain", MOCK_PEBBLE)]))]},
        // the rich self-check: the previews as SVG text, or as image blocks; a passing fix replaces, a failing one keeps
        {"prompt": "a green giant", "rich": true, "check": true, "replies": [wrap(json!(mock_rich())), wrap(json!([
            {"name": "Fixed", "shape": "custom", "color": "#3E8E2F", "outline": MOCK_CAT, "layers": [["M30 60 L70 60 L50 80 Z", "#f5ebd6", 0.5]], "eyes": {"c": "#58d63a", "at": [50, 50], "gap": 20, "s": 1.3, "rim": "#111111", "rw": 0.4}},
            {"name": "Broken", "shape": "custom", "color": "#3E8E2F", "outline": MOCK_CAT, "layers": [], "eyes": "#FFFFFF"},
        ]))]},
        {"prompt": "", "image": img, "rich": true, "check": true, "images": true, "replies": [wrap(json!(mock_rich())), "no json"]},
        {"prompt": "y", "rich": true, "check": true, "replies": [wrap(json!([mock_rich()[1]]))]},
    ]);
    let theirs = node(
        r##"const out = [];
        for (const c of input) {
          const calls = [], picks = [];
          let n = 0;
          let cands;
          try { cands = await m.designCandidates({ prompt: c.prompt, name: c.name || "", image: c.image || null, rich: !!c.rich }, {
            selfCheck: !!c.check,
            preview: async (svgs) => (c.images ? svgs.map((_, k) => ({ type: "image", source: { type: "base64", media_type: "image/png", data: "P" + k } })) : null),
            ask: async (msgs) => (calls.push(structuredClone(msgs)), c.replies[n++]),
            pick: async (p) => (picks.push(p), { shape: "cloud", color: "#00BCA6" }),
          }); } catch (e) { cands = { error: e.message }; }
          out.push({ cands, calls, picks });
        }
        console.log(JSON.stringify(out));"##,
        &cases,
    );
    for (c, t) in cases.as_array().unwrap().iter().zip(theirs.as_array().unwrap()) {
        let calls = Mutex::new(Vec::<Value>::new());
        let picks = Mutex::new(Vec::<String>::new());
        let replies: Vec<String> = c["replies"].as_array().unwrap().iter().map(|r| r.as_str().unwrap().to_owned()).collect();
        let fut = design_candidates(
            c["prompt"].as_str().unwrap(),
            c.get("name").and_then(Value::as_str).unwrap_or(""),
            c.get("image").cloned(),
            c.get("rich").and_then(Value::as_bool).unwrap_or(false),
            c.get("check").and_then(Value::as_bool).unwrap_or(false),
            |msgs: Vec<Value>| {
                let mut calls = calls.lock().unwrap();
                calls.push(json!(msgs));
                // a call past the script answers nothing, as Node's undefined reply parses
                let r = replies.get(calls.len() - 1).cloned().unwrap_or_default();
                async move { Ok::<_, String>(r) }
            },
            |p: String| {
                picks.lock().unwrap().push(p);
                async { "cloud".to_owned() }
            },
            |svgs: Vec<String>| {
                let images = c.get("images").is_some();
                async move { images.then(|| svgs.iter().enumerate().map(|(k, _)| json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": format!("P{k}")}})).collect()) }
            },
        );
        let cands = match futures_lite_block_on(fut) {
            Ok(c) => json!(c.iter().map(Candidate::json).collect::<Vec<_>>()),
            Err(e) => json!({"error": e}),
        };
        let ours = json!({"cands": cands, "calls": *calls.lock().unwrap(), "picks": *picks.lock().unwrap()});
        assert_eq!(ours, *t, "{}", c["prompt"]);
    }
}

fn futures_lite_block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
}

#[test]
fn avatar_picture_prompts_match_nodes() {
    let cases = json!([["", true], ["  make it blue  ", true], ["a green giant", false], ["", false]]);
    let theirs = node(
        r#"const d = await import("./src/avatar-design.mjs");
        console.log(JSON.stringify(input.map(([prompt, image]) => d.pictureRequest({ prompt, image })).concat([d.PICTURE_STYLE])));"#,
        &cases,
    );
    let mut ours: Vec<Value> = cases.as_array().unwrap().iter().map(|c| json!(picture_request(c[0].as_str().unwrap(), c[1].as_bool().unwrap()))).collect();
    ours.push(json!(picture_style()));
    assert_eq!(json!(ours), theirs);
    assert!(picture_request("", false).starts_with("Draw the character described below as a minimal flat \"Bot\" avatar.\n\nKeep"));
}

#[test]
fn avatar_picture_svg_is_refused_with_scripts_or_outside_images() {
    assert_eq!(picture_svg("Here: <svg viewBox=\"0 0 1 1\"><rect/></svg> done").unwrap(), "<svg viewBox=\"0 0 1 1\"><rect/></svg>");
    assert_eq!(picture_svg("no markup").unwrap_err(), "the model returned no SVG");
    for bad in ["<svg><SCRIPT>x</SCRIPT></svg>", "<svg><image href=\"a.png\"/></svg>", "<svg><foreignObject/></svg>", "<svg><use href = 'https://x/y#a'/></svg>", "<svg><a xlink:HREF=\"http://x\"/></svg>"] {
        assert_eq!(picture_svg(bad).unwrap_err(), "the SVG used scripts or outside images", "{bad}");
    }
    assert!(picture_svg("<svg><use href=\"#a\"/></svg>").is_ok());
}

#[test]
fn avatar_eyes_and_previews_match_nodes() {
    let outline = MOCK_CAT;
    let eyes = json!([
        "#58D63A", "red", 5, null, [1], {"c": "#58D63A"}, {"c": "#58D63A", "at": [50, 58], "gap": 20, "s": 1.2, "rim": "#111111", "rw": 0.3},
        {"c": "red", "at": [500, 5], "gap": 0, "s": 3, "rim": 1, "rw": 2}, {"c": "#FFFFFF", "at": [1]}, {"c": "#FFFFFF", "at": [50, "x"]}, {"c": "#FFFFFF", "gap": null, "s": null},
        {"c": "#FFFFFF", "at": [18, 14]}, {"c": "#FFFFFF", "at": [17.9, 50]}, {"c": "#FFFFFF", "s": 0.6, "rw": 0, "gap": 100}, {"c": "#FFFFFF", "s": 2.21, "rw": 1.21, "gap": 100.5},
    ]);
    let theirs = node(&format!("console.log(JSON.stringify(input.map((e) => m.eyeErrors(e === '__undef' ? undefined : e, {}))));", json!(outline)), &eyes);
    let ours: Vec<Vec<String>> = eyes.as_array().unwrap().iter().map(|e| eye_errors(Some(e), Some(outline))).collect();
    assert_eq!(json!(ours), theirs);
    // previews: the mocks, the default placement and a placed, rimmed set
    let mut cands: Vec<Value> = mock_rich();
    let mut placed = mock_rich()[0].clone();
    placed["eyes"] = json!({"c": "#58D63A", "at": [50, 58], "gap": 22, "s": 1.4, "rim": "#111111", "rw": 0.3});
    cands.push(placed);
    let mut pebble = mock_rich()[2].clone();
    pebble["outline"] = json!(MOCK_PEBBLE);
    pebble["eyes"] = json!({"c": "#2B1D12", "s": 0.7});
    cands.push(pebble);
    let theirs = node("console.log(JSON.stringify(input.map((c) => m.previewSvg(c))));", &json!(cands));
    let ours: Vec<String> = cands.iter().map(|c| preview_svg(&parse_candidates(&json!([c]).to_string())[0])).collect();
    assert_eq!(json!(ours), theirs);
    assert!(ours[3].contains("fill=\"#111111\"/>"), "{}", ours[3]);
}

#[test]
fn avatar_to_fixed_rounds_ties_up() {
    assert_eq!(to_fixed2(0.125), "0.13");
    assert_eq!(to_fixed2(4.0), "4.00");
    assert_eq!(to_fixed2(2.0 / 3.0), "0.67");
    assert_eq!(to_fixed2(f64::INFINITY), "Infinity");
}
