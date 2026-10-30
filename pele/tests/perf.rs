// Render times of the flowchart samples. Ignored by default; run in release:
//   cargo test --release --test perf -- --ignored --nocapture

use std::time::Instant;

fn stats(mut t: Vec<f64>) -> (f64, f64) {
    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (t[t.len() / 2], t[(t.len() * 95 / 100).min(t.len() - 1)])
}

#[test]
#[ignore]
fn flowchart_sample_render_times() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/samples");
    let mut names: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("fc-")).collect();
    names.sort();
    println!("| sample | median ms | p95 ms | scene parse median ms |");
    println!("|---|---:|---:|---:|");
    for p in names {
        let code = std::fs::read_to_string(&p).unwrap();
        let _ = pele::render(&code);
        let mut render = Vec::new();
        let mut parse = Vec::new();
        for _ in 0..200 {
            let t = Instant::now();
            let r = pele::render(&code).unwrap();
            render.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            let _ = pele::Scene::parse(&r.svg).unwrap();
            parse.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        let (m, p95) = stats(render);
        let (pm, _) = stats(parse);
        println!("| {} | {:.3} | {:.3} | {:.3} |", p.file_stem().unwrap().to_string_lossy(), m, p95, pm);
    }
}
