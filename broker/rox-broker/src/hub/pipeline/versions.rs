//! The version tree (`headOf`, `verSlug`, `verFile`, `versionsOf`,
//! `writeVersion`, `setHead`, `walkFiles`): `generated/<name>/<slug>.v<k>.tsx`
//! is immutable, `heads` is the current pointer, and `generated/<name>.tsx`
//! (and a published `widgets/<name>.tsx`) mirror the head.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use super::chunks::{chunk_key, ChunkJobs};
use super::nsh::NshCli;
use super::Dirs;
use crate::hub::ctx::HubCtx;
use crate::hub::traits::Nsh;
use crate::js;

fn nsh_of(ctx: &HubCtx) -> Arc<dyn Nsh> {
    ctx.nsh.clone().unwrap_or_else(|| Arc::new(NshCli::from_env()))
}

/// Directory entries by name, as Node's readdirSync lists them (libuv
/// sorts them bytewise).
pub fn read_dir_sorted(dir: &Path) -> std::io::Result<Vec<std::fs::DirEntry>> {
    let mut list: Vec<std::fs::DirEntry> = std::fs::read_dir(dir)?.filter_map(Result::ok).collect();
    list.sort_by_key(|e| e.file_name());
    Ok(list)
}

/// `s.replace(/\.tsx$/i, "")`
fn strip_tsx(s: &str) -> &str {
    match s.len().checked_sub(4).and_then(|at| s.get(at..).map(|tail| (at, tail))) {
        Some((at, tail)) if tail.eq_ignore_ascii_case(".tsx") => &s[..at],
        _ => s,
    }
}

/// state.mjs's `slugFile`: a display filename, kebab-case plus ".tsx", or
/// "" when nothing usable is left.
pub fn slug_file(s: &str) -> String {
    let t = s.trim_matches(js::is_space);
    let t = strip_tsx(t);
    let lower = t.to_lowercase();
    let mut slug = String::new();
    let mut dash = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
            dash = false;
        } else if !dash {
            slug.push('-');
            dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    let slug: String = slug.chars().take(40).collect();
    let slug = slug.trim_end_matches('-');
    let ok = slug.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) && slug.len() <= 40;
    if ok {
        format!("{slug}.tsx")
    } else {
        String::new()
    }
}

pub fn head_of(ctx: &HubCtx, name: &str) -> Result<Option<i64>, String> {
    ctx.db.with(|db| db.query_row("SELECT v FROM heads WHERE name=?", [name], |r| r.get::<_, Option<i64>>(0)).optional().map(Option::flatten))
}

/// `verSlug(name)`: the app's file label, else its title's slug, else the
/// name, without ".tsx".
pub fn ver_slug(ctx: &HubCtx, name: &str) -> Result<String, String> {
    let (file, title) = ctx
        .db
        .with(|db| db.query_row("SELECT file,title FROM apps WHERE name=?", [name], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?))).optional())?
        .unwrap_or((None, None));
    let pick = file.filter(|f| !f.is_empty()).unwrap_or_else(|| slug_file(title.as_deref().unwrap_or("")));
    let pick = if pick.is_empty() { format!("{name}.tsx") } else { pick };
    Ok(strip_tsx(&pick).to_owned())
}

/// `verFile(name, v)`: the first file in `generated/<name>/` ending
/// `.v<v>.tsx`.
pub fn ver_file(dirs: &Dirs, name: &str, v: &str) -> Option<PathBuf> {
    let dir = dirs.gen.join(name);
    let tail = format!(".v{v}.tsx");
    read_dir_sorted(&dir).ok()?.into_iter().map(|e| e.file_name().to_string_lossy().into_owned()).find(|f| f.ends_with(&tail)).map(|f| dir.join(f))
}

/// `versionsOf(name)`: `[{v, parent, created_at, label}]` by v.
pub fn versions_of(ctx: &HubCtx, name: &str) -> Result<Vec<Value>, String> {
    ctx.db.with(|db| {
        let mut st = db.prepare("SELECT v,parent,created_at,label FROM versions WHERE name=? ORDER BY v")?;
        let rows = st.query_map([name], |r| {
            Ok(json!({ "v": r.get::<_, Option<i64>>(0)?, "parent": r.get::<_, Option<i64>>(1)?, "created_at": r.get::<_, Option<i64>>(2)?, "label": r.get::<_, Option<String>>(3)? }))
        })?;
        rows.collect()
    })
}

fn published(ctx: &HubCtx, name: &str) -> Result<bool, String> {
    ctx.db.with(|db| db.query_row("SELECT 1 FROM widgets WHERE name=?", [name], |_| Ok(())).optional().map(|r| r.is_some()))
}

/// `writeVersion(name, code, parent, prompt, label)`: the next version's
/// file, its row, the head moved to it, the mirrors, and its two chunks
/// (`R6v3.js` and `R6.js`) started. → the new v, and the chunk builds.
pub fn write_version(ctx: &HubCtx, name: &str, code: &str, parent: Option<i64>, prompt: Option<&str>, label: Option<&str>) -> Result<(i64, ChunkJobs), String> {
    let dirs = Dirs::of(&ctx.data);
    let dir = dirs.gen.join(name);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let max: Option<i64> = ctx.db.with(|db| db.query_row("SELECT MAX(v) AS m FROM versions WHERE name=?", [name], |r| r.get(0)))?;
    let v = max.filter(|m| *m != 0).unwrap_or(0) + 1;
    let v_file = dir.join(format!("{}.v{v}.tsx", ver_slug(ctx, name)?));
    std::fs::write(&v_file, code).map_err(|e| e.to_string())?;
    let prompt = js::slice_utf16(prompt.unwrap_or(""), 500);
    let label = js::slice_utf16(label.unwrap_or("").trim_matches(js::is_space), 40);
    let now = ctx.clock.now() as i64;
    ctx.db.with(|db| {
        db.execute(
            "INSERT INTO versions(name,v,parent,prompt,created_at,label) VALUES(?,?,?,?,?,?)",
            params![name, v, parent, (!prompt.is_empty()).then_some(&prompt), now, (!label.is_empty()).then_some(&label)],
        )?;
        db.execute("INSERT OR REPLACE INTO heads(name,v) VALUES(?,?)", params![name, v])
    })?;
    let head = dirs.gen.join(format!("{name}.tsx"));
    std::fs::write(&head, code).map_err(|e| e.to_string())?;
    // a published Mini App is the same artifact: its file mirrors head too
    if published(ctx, name)? {
        std::fs::write(dirs.widgets.join(format!("{name}.tsx")), code).map_err(|e| e.to_string())?;
    }
    let nsh = nsh_of(ctx);
    let mut jobs = ChunkJobs::default();
    jobs.spawn(&nsh, &dirs.stage, &dirs.chunks, chunk_key(&format!("{name}@v{v}")), v_file);
    jobs.spawn(&nsh, &dirs.stage, &dirs.chunks, name.to_owned(), head);
    Ok((v, jobs))
}

/// `setHead(name, v)`: the pointer moves, nothing is rewritten. `None`
/// (Node's false) when that version has no file.
pub fn set_head(ctx: &HubCtx, name: &str, v: i64) -> Result<Option<ChunkJobs>, String> {
    let dirs = Dirs::of(&ctx.data);
    let Some(file) = ver_file(&dirs, name, &v.to_string()).filter(|f| f.exists()) else { return Ok(None) };
    ctx.db.with(|db| db.execute("INSERT OR REPLACE INTO heads(name,v) VALUES(?,?)", params![name, v]))?;
    let code = std::fs::read(&file).map_err(|e| e.to_string())?;
    let head = dirs.gen.join(format!("{name}.tsx"));
    std::fs::write(&head, &code).map_err(|e| e.to_string())?;
    if published(ctx, name)? {
        std::fs::write(dirs.widgets.join(format!("{name}.tsx")), &code).map_err(|e| e.to_string())?;
    }
    let mut jobs = ChunkJobs::default();
    jobs.spawn(&nsh_of(ctx), &dirs.stage, &dirs.chunks, name.to_owned(), head);
    ctx.store.write_index();
    if let Err(e) = ctx.store.write_shelf() {
        eprintln!("rox-broker: widgets/index.tsx: {e}");
    }
    Ok(Some(jobs))
}

/// `walkFiles(dir)`: every file under `dir`, recursively, as
/// `{name, rel}`; a dir that can't be read is skipped.
pub fn walk_files(dir: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<Value>) {
    let Ok(list) = read_dir_sorted(dir) else { return };
    for e in list {
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&p, base, out);
        } else {
            let rel = p.strip_prefix(base).map(|r| r.display().to_string()).unwrap_or_default();
            out.push(json!({ "name": e.file_name().to_string_lossy(), "rel": rel }));
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::nsh::tests::{calls, stub, temp};
    use super::*;
    use crate::hub::traits::Clock;

    /// Node's CREATE statements for what the pipeline reads and writes.
    pub fn schema(ctx: &HubCtx) {
        ctx.db
            .with(|db| {
                db.execute_batch(
                    "CREATE TABLE IF NOT EXISTS widgets(name TEXT PRIMARY KEY, title TEXT, prompt TEXT, published_at TEXT);
                     CREATE TABLE IF NOT EXISTS apps(name TEXT PRIMARY KEY, title TEXT, file TEXT);
                     CREATE TABLE IF NOT EXISTS versions(name TEXT NOT NULL, v INTEGER NOT NULL, parent INTEGER, prompt TEXT, created_at INTEGER, PRIMARY KEY(name, v));
                     CREATE TABLE IF NOT EXISTS heads(name TEXT PRIMARY KEY, v INTEGER NOT NULL);
                     ALTER TABLE versions ADD COLUMN label TEXT;",
                )
            })
            .unwrap();
    }

    fn make(dir: &Path, mode: &str) -> HubCtx {
        for d in ["generated", ".stage", "widgets"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        let mut ctx = HubCtx::new(dir.to_owned(), Clock::manual());
        ctx.nsh = Some(Arc::new(stub(&dir.join("stubs.log"), mode)));
        schema(&ctx);
        ctx
    }

    fn rows(ctx: &HubCtx, sql: &str) -> Vec<Vec<Value>> {
        ctx.db
            .with(|db| {
                let mut st = db.prepare(sql)?;
                let n = st.column_count();
                let rows = st.query_map([], |r| (0..n).map(|i| Ok(cell(r.get_ref(i)?))).collect::<rusqlite::Result<Vec<Value>>>())?;
                rows.collect()
            })
            .unwrap()
    }

    fn cell(v: rusqlite::types::ValueRef<'_>) -> Value {
        use rusqlite::types::ValueRef::*;
        match v {
            Integer(i) => json!(i),
            Real(f) => json!(f),
            Text(t) => json!(String::from_utf8_lossy(t)),
            Null | Blob(_) => Value::Null,
        }
    }

    fn read(p: PathBuf) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    fn names(dir: PathBuf) -> Vec<String> {
        read_dir_sorted(&dir).unwrap().iter().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
    }

    fn sorted(mut v: Vec<Vec<String>>) -> Vec<Vec<String>> {
        v.sort();
        v
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    const CODE: &str = "import { Text } from \"nsh\";\n\nexport default function R() {\n  return <Text>hi</Text>;\n}\n";

    #[tokio::test]
    async fn pipe_06_write_version_files_rows_and_two_chunks() {
        let dir = temp("write");
        std::fs::create_dir_all(dir.join("chunks")).unwrap();
        let ctx = make(&dir, "pass");
        let (v, jobs) = write_version(&ctx, "R5", CODE, None, Some("make a counter"), Some("")).unwrap();
        jobs.done().await;
        let gen = dir.join("generated");
        assert_eq!(v, 1);
        assert_eq!(names(gen.join("R5")), ["R5.v1.tsx"]);
        assert_eq!(read(gen.join("R5/R5.v1.tsx")), CODE);
        assert_eq!(read(gen.join("R5.tsx")), CODE);
        assert_eq!(rows(&ctx, "SELECT * FROM versions"), vec![vec![json!("R5"), json!(1), Value::Null, json!("make a counter"), json!(0), Value::Null]]);
        assert_eq!(rows(&ctx, "SELECT * FROM heads"), vec![vec![json!("R5"), json!(1)]]);
        let stage = dir.join(".stage");
        let p = |p: PathBuf| p.display().to_string();
        assert_eq!(
            sorted(calls(&dir.join("stubs.log"))),
            sorted(vec![s(&["build", &p(gen.join("R5/R5.v1.tsx")), "--out", &p(stage.join("chunk-R5v1"))]), s(&["build", &p(gen.join("R5.tsx")), "--out", &p(stage.join("chunk-R5"))])])
        );
        assert_eq!(names(dir.join("chunks")), ["R5.js", "R5v1.js"]);
        assert_eq!(read(dir.join("chunks/R5v1.js")), "// nsh stub bundle of R5.v1.tsx\nexport default null;\n");
        assert_eq!(read(dir.join("chunks/R5.js")), "// nsh stub bundle of R5.tsx\nexport default null;\n");
        assert!(!dir.join("widgets/R5.tsx").exists());
    }

    #[tokio::test]
    async fn pipe_07_next_versions_parents_labels_slugs_and_the_published_mirror() {
        let dir = temp("write2");
        std::fs::create_dir_all(dir.join("chunks")).unwrap();
        let ctx = make(&dir, "pass");
        let (a, j1) = write_version(&ctx, "R6", "one", None, Some(""), None).unwrap();
        ctx.db.with(|db| db.execute_batch("INSERT INTO apps(name,title,file) VALUES('R6','My Counter!',NULL); INSERT INTO widgets(name,title) VALUES('R6','My Counter!');")).unwrap();
        let (b, j2) = write_version(&ctx, "R6", "two", Some(1), Some(&"p".repeat(600)), Some("  slow-ball and a very long branch name here  ")).unwrap();
        ctx.db.with(|db| db.execute("UPDATE apps SET file='ticker.tsx' WHERE name='R6'", [])).unwrap();
        let (c, j3) = write_version(&ctx, "R6", "three", Some(1), Some("again"), Some("x")).unwrap();
        for j in [j1, j2, j3] {
            j.done().await;
        }
        let gen = dir.join("generated");
        assert_eq!((a, b, c, head_of(&ctx, "R6").unwrap()), (1, 2, 3, Some(3)));
        assert_eq!(
            versions_of(&ctx, "R6")
                .unwrap()
                .into_iter()
                .map(|mut r| {
                    r["created_at"] = json!("number");
                    r
                })
                .collect::<Vec<_>>(),
            vec![
                json!({"v": 1, "parent": null, "created_at": "number", "label": null}),
                json!({"v": 2, "parent": 1, "created_at": "number", "label": "slow-ball and a very long branch name he"}),
                json!({"v": 3, "parent": 1, "created_at": "number", "label": "x"}),
            ]
        );
        assert_eq!(ver_slug(&ctx, "R6").unwrap(), "ticker");
        let dirs = Dirs::of(&dir);
        assert_eq!(ver_file(&dirs, "R6", "2"), Some(gen.join("R6/my-counter.v2.tsx")));
        assert_eq!(ver_file(&dirs, "R6", "9"), None);
        assert_eq!(names(gen.join("R6")), ["R6.v1.tsx", "my-counter.v2.tsx", "ticker.v3.tsx"]);
        assert_eq!(read(gen.join("R6.tsx")), "three");
        assert_eq!(read(dir.join("widgets/R6.tsx")), "three");
        assert_eq!(
            rows(&ctx, "SELECT name, v, length(prompt) FROM versions ORDER BY v"),
            vec![vec![json!("R6"), json!(1), Value::Null], vec![json!("R6"), json!(2), json!(500)], vec![json!("R6"), json!(3), json!(5)]]
        );
        assert_eq!(names(dir.join("chunks")), ["R6.js", "R6v1.js", "R6v2.js", "R6v3.js"]);
    }

    #[tokio::test]
    async fn pipe_08_no_chunks_dir_or_a_failing_chunk_build_lands_no_chunk() {
        // BUG: nothing makes <data>/chunks; every copy then fails silently
        let dir = temp("nochunks");
        let ctx = make(&dir, "pass");
        write_version(&ctx, "R3", "a", None, Some("p"), None).unwrap().1.done().await;
        assert!(!dir.join("chunks").exists());
        assert_eq!(read(dir.join("generated/R3/R3.v1.tsx")), "a");
        assert_eq!(head_of(&ctx, "R3").unwrap(), Some(1));

        let dir = temp("chunkfail");
        std::fs::create_dir_all(dir.join("chunks")).unwrap();
        let ctx = make(&dir, "fail");
        write_version(&ctx, "R4", "b", None, Some("p"), None).unwrap().1.done().await;
        assert!(names(dir.join("chunks")).is_empty());
        assert_eq!(read(dir.join("generated/R4.tsx")), "b");
    }

    #[tokio::test]
    async fn pipe_09_set_head_moves_the_pointer_and_the_mirrors() {
        let dir = temp("head");
        std::fs::create_dir_all(dir.join("chunks")).unwrap();
        let ctx = make(&dir, "pass");
        write_version(&ctx, "R8", "one", None, Some("p"), None).unwrap().1.done().await;
        write_version(&ctx, "R8", "two", Some(1), Some("p"), None).unwrap().1.done().await;
        ctx.db.with(|db| db.execute("INSERT INTO widgets(name,title) VALUES('R8','t')", [])).unwrap();
        let n = calls(&dir.join("stubs.log")).len();
        assert!(set_head(&ctx, "R8", 5).unwrap().is_none());
        set_head(&ctx, "R8", 1).unwrap().expect("v1 has a file").done().await;
        assert_eq!((head_of(&ctx, "R8").unwrap(), head_of(&ctx, "R9").unwrap()), (Some(1), None));
        let gen = dir.join("generated");
        assert_eq!(read(gen.join("R8.tsx")), "one");
        assert_eq!(read(dir.join("widgets/R8.tsx")), "one");
        let p = |p: PathBuf| p.display().to_string();
        assert_eq!(calls(&dir.join("stubs.log"))[n..], [s(&["build", &p(gen.join("R8.tsx")), "--out", &p(dir.join(".stage/chunk-R8"))])]);
        assert_eq!(read(dir.join("chunks/R8.js")), "// nsh stub bundle of R8.tsx\nexport default null;\n");
    }

    #[test]
    fn pipe_12_walk_files() {
        let dir = temp("walk");
        let root = dir.join("w");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("top.txt"), "").unwrap();
        std::fs::write(root.join("a/b/deep.md"), "").unwrap();
        assert_eq!(walk_files(&root), vec![json!({"name": "deep.md", "rel": "a/b/deep.md"}), json!({"name": "top.txt", "rel": "top.txt"})]);
        assert!(walk_files(&dir.join("nope")).is_empty());
    }

    #[test]
    fn slug_files_as_state_mjs_makes_them() {
        assert_eq!(slug_file("My Counter!"), "my-counter.tsx");
        assert_eq!(slug_file("  Ticker.TSX "), "ticker.tsx");
        assert_eq!(slug_file("!!!"), "");
        assert_eq!(slug_file(""), "");
        assert_eq!(slug_file(&format!("{}-b", "a".repeat(39))), format!("{}.tsx", "a".repeat(39)));
    }
}
