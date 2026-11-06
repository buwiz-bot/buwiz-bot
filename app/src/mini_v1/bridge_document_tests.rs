//! `_meta["buwiz/document"]` through a real call: an app's `buwiz.call`, the
//! signed envelope, the fake broker's result, the open's checks. The path
//! rules themselves are `document.rs`'s tests.

use std::fs;

use super::*;

const TOOLS: [&str; 3] = ["fx_document", "fx_document_fail", "fx_document_big"];

/// A plugin's folder holding `open/1/a.docx` and `open/1/b.xlsx`, and a
/// docx outside it.
struct Bed {
    base: PathBuf,
    folder: PathBuf,
}

impl Bed {
    fn new() -> Bed {
        let base = crate::mini_v1::storage::scratch_root("bridge-document");
        let folder = base.join(FIXTURE);
        fs::create_dir_all(folder.join("open/1")).unwrap();
        for name in ["open/1/a.docx", "open/1/b.xlsx", "../outside.docx"] {
            fs::write(folder.join(name), "x").unwrap();
        }
        Bed { base, folder }
    }

    fn field(&self, file: &str) -> Value {
        json!({"path": self.folder.join(file), "name": "shown.docx", "mime": "application/x"})
    }

    /// An open whose app calls `tool` with `args` as it mounts.
    fn open(&self, first_party: bool, agent: bool, tool: &str, args: Value) -> Open {
        let script = format!("return await buwiz.call({tool:?}, {args});");
        let folder = self.folder.clone();
        Open::start_with(FIXTURE, json!({"tools": TOOLS}), &script, Limits::default(), move |ui| {
            ui.first_party.store(first_party, Ordering::SeqCst);
            ui.agent.store(agent, Ordering::SeqCst);
            *ui.folder.lock().unwrap() = Some(folder);
        })
    }

    /// The person's press in a first-party plugin's open.
    fn pressed(&self, file: &str) -> Open {
        self.open(true, false, "fx_document", json!({"field": self.field(file)}))
    }
}

impl Drop for Bed {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn opened(o: &Open) -> Vec<String> {
    o.ui.documents.lock().unwrap().iter().map(|d| d.attach().name).collect()
}

/// Waits until the fake broker is inside the call.
fn in_flight(o: &Open) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while o.broker.running.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the call never reached the broker");
        std::thread::yield_now();
    }
}

#[test]
fn the_persons_press_in_a_first_party_app_opens_a_docx_and_an_xlsx() {
    let bed = Bed::new();
    for (file, name) in [("open/1/a.docx", "a.docx"), ("open/1/b.xlsx", "b.xlsx")] {
        let mut o = bed.pressed(file);
        assert_eq!(o.wait("{"), r#"{"opened":true}"#, "the app gets its answer, and nothing of the path");
        assert_eq!(opened(&o), [name]);
    }
}

#[test]
fn an_older_plugin_with_no_field_changes_nothing() {
    let bed = Bed::new();
    let mut o = bed.open(true, false, "fx_document", json!({}));
    assert_eq!(o.wait("{"), r#"{"opened":true}"#);
    assert!(opened(&o).is_empty());
}

#[test]
fn a_third_party_plugins_field_is_ignored() {
    let bed = Bed::new();
    let mut o = bed.open(false, false, "fx_document", json!({"field": bed.field("open/1/a.docx")}));
    assert_eq!(o.wait("{"), r#"{"opened":true}"#, "its call still answers");
    assert!(opened(&o).is_empty());
}

#[test]
fn a_call_the_ai_made_opens_nothing() {
    let bed = Bed::new();
    let mut o = bed.open(true, true, "fx_document", json!({"field": bed.field("open/1/a.docx")}));
    assert_eq!(o.wait("{"), r#"{"opened":true}"#);
    assert_eq!(o.broker.calls.lock().unwrap()[0].0["actor"], "agent");
    assert!(opened(&o).is_empty());
}

#[test]
fn the_actor_is_who_made_the_call_not_who_acts_when_the_result_comes() {
    let bed = Bed::new();
    let args = json!({"field": bed.field("open/1/a.docx"), "ms": 300});
    // the AI's call, and the person's input while it is out
    let mut o = bed.open(true, true, "fx_document", args.clone());
    in_flight(&o);
    o.ui.agent.store(false, Ordering::SeqCst);
    o.wait("{");
    assert!(opened(&o).is_empty(), "the person's input takes nothing back");
    // the person's call, and the AI acting while it is out
    let mut o = bed.open(true, false, "fx_document", args);
    in_flight(&o);
    o.ui.agent.store(true, Ordering::SeqCst);
    o.wait("{");
    assert_eq!(opened(&o), ["a.docx"]);
}

#[test]
fn a_result_after_the_screen_closed_opens_nothing() {
    let bed = Bed::new();
    let mut o = bed.open(true, false, "fx_document", json!({"field": bed.field("open/1/a.docx"), "ms": 300}));
    in_flight(&o);
    o.ctx.close();
    o.wait("{");
    assert!(opened(&o).is_empty());
}

#[test]
fn the_viewers_job_asks_the_session_when_it_runs_not_at_the_check() {
    let bed = Bed::new();
    let live = |o: &Open| o.ui.lives.lock().unwrap().pop().expect("the bridge handed it over")();
    let mut o = bed.pressed("open/1/a.docx");
    o.wait("{");
    assert!(live(&o), "the open lives");
    let mut o = bed.pressed("open/1/a.docx");
    o.wait("{");
    o.ctx.close();
    assert!(!live(&o), "it closed between the check and the job");
}

#[test]
fn a_failed_or_oversize_result_opens_nothing() {
    let bed = Bed::new();
    let args = json!({"field": bed.field("open/1/a.docx")});
    let mut o = bed.open(true, false, "fx_document_fail", args.clone());
    assert!(o.wait("threw").starts_with("threw tool_error"));
    assert!(opened(&o).is_empty());
    let mut o = bed.open(true, false, "fx_document_big", args);
    assert!(o.wait("threw").starts_with("threw result_too_large"));
    assert!(opened(&o).is_empty());
}

#[test]
fn a_path_the_check_refuses_never_reaches_the_viewer() {
    let bed = Bed::new();
    std::os::unix::fs::symlink(bed.folder.join("open/1/a.docx"), bed.folder.join("open/1/link.docx")).unwrap();
    fs::write(bed.folder.join("open/1/run.exe"), "x").unwrap();
    for file in ["../outside.docx", "open/1/link.docx", "open/1", "open/1/gone.docx", "open/1/run.exe"] {
        let mut o = bed.pressed(file);
        assert_eq!(o.wait("{"), r#"{"opened":true}"#, "{file}");
        assert!(opened(&o).is_empty(), "{file}");
    }
}

#[test]
fn a_host_that_knows_no_folder_opens_nothing() {
    let bed = Bed::new();
    let field = bed.field("open/1/a.docx");
    let script = format!("return await buwiz.call(\"fx_document\", {});", json!({"field": field}));
    let mut o = Open::start_with(FIXTURE, json!({"tools": TOOLS}), &script, Limits::default(), |ui| ui.first_party.store(true, Ordering::SeqCst));
    assert_eq!(o.wait("{"), r#"{"opened":true}"#);
    assert!(opened(&o).is_empty());
}
