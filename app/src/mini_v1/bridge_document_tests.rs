//! `_meta["buwiz/document"]`, `buwiz/open_with` and `buwiz/save` through a
//! real call: an app's `buwiz.call`, the signed envelope, the fake broker's
//! result, the open's checks. The path rules themselves are `document.rs`'s
//! and `system_open.rs`'s tests.

use std::fs;

use super::*;

const TOOLS: [&str; 4] = ["fx_document", "fx_document_fail", "fx_document_big", "fx_account_use"];

/// A plugin's folder holding `open-1f/1/a.docx` and `open-1f/1/b.xlsx`, and a
/// docx outside it.
struct Bed {
    base: PathBuf,
    folder: PathBuf,
}

impl Bed {
    fn new() -> Bed {
        let base = crate::mini_v1::storage::scratch_root("bridge-document");
        let folder = base.join(FIXTURE);
        fs::create_dir_all(folder.join("open-1f/1")).unwrap();
        for name in ["open-1f/1/a.docx", "open-1f/1/b.xlsx", "open-1f/1/IMG_4250.png", "open-1f/1/Invoice.pdf", "open-1f/1/run.exe", "../outside.docx", "../outside.png"] {
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

    /// A call from the person's surface in a first-party plugin's open: what
    /// the check can see, which isn't proof of a press.
    fn call_on_the_persons_surface(&self, file: &str) -> Open {
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

/// What reached the viewer, by kind and name.
fn viewed(o: &Open) -> Vec<(String, String)> {
    o.ui.documents.lock().unwrap().iter().map(|d| d.attach()).map(|a| (a.kind, a.name)).collect()
}

fn opened_with(o: &Open) -> Vec<String> {
    o.ui.opened_with.lock().unwrap().iter().map(|f| f.name().to_owned()).collect()
}

fn saved(o: &Open) -> Vec<String> {
    o.ui.saved.lock().unwrap().iter().map(|f| f.name().to_owned()).collect()
}

impl Bed {
    /// An open whose app calls `tool` with `args` as it mounts, just after
    /// the person's press `press`, if any.
    fn pressed(&self, first_party: bool, agent: bool, press: Option<u64>, args: Value) -> Open {
        let script = format!("return await buwiz.call(\"fx_document\", {args});");
        let folder = self.folder.clone();
        Open::start_with(FIXTURE, json!({"tools": TOOLS}), &script, Limits::default(), move |ui| {
            ui.first_party.store(first_party, Ordering::SeqCst);
            ui.agent.store(agent, Ordering::SeqCst);
            *ui.folder.lock().unwrap() = Some(folder);
            *ui.hand_press.lock().unwrap() = press;
        })
    }

    /// A call, just after a press, asking to open `file` in the default app.
    fn open_with(&self, first_party: bool, agent: bool, file: &str) -> Open {
        let args = json!({"open_with": {"path": self.folder.join(file)}});
        self.pressed(first_party, agent, Some(1), args)
    }

    /// A call, just after a press, asking to save `file` to Downloads.
    fn save(&self, first_party: bool, agent: bool, file: &str) -> Open {
        let args = json!({"save": {"path": self.folder.join(file), "name": "shown.png"}});
        self.pressed(first_party, agent, Some(1), args)
    }
}

/// Waits until the fake broker is inside the call.
fn in_flight(o: &Open) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while o.broker.running.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the call never reached the broker");
        std::thread::yield_now();
    }
}

/// A switch that failed is about the switch: what the call opened still opens.
#[test]
fn a_failed_switch_still_opens_the_document_beside_it() {
    let bed = Bed::new();
    let args = json!({"field": {"account": "google", "label": "gone"}, "document": bed.field("open-1f/1/a.docx")});
    let mut o = bed.open(true, false, "fx_account_use", args);
    assert!(o.wait("{").contains("needs_signin"));
    assert_eq!(opened(&o), ["a.docx"]);
}

#[test]
fn a_call_call_on_the_persons_surface_in_a_first_party_app_opens_a_docx_and_an_xlsx() {
    let bed = Bed::new();
    for (file, name) in [("open-1f/1/a.docx", "a.docx"), ("open-1f/1/b.xlsx", "b.xlsx")] {
        let mut o = bed.call_on_the_persons_surface(file);
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
    let mut o = bed.open(false, false, "fx_document", json!({"field": bed.field("open-1f/1/a.docx")}));
    assert_eq!(o.wait("{"), r#"{"opened":true}"#, "its call still answers");
    assert!(opened(&o).is_empty());
}

#[test]
fn a_call_the_ai_made_opens_nothing() {
    let bed = Bed::new();
    let mut o = bed.open(true, true, "fx_document", json!({"field": bed.field("open-1f/1/a.docx")}));
    assert_eq!(o.wait("{"), r#"{"opened":true}"#);
    assert_eq!(o.broker.calls.lock().unwrap()[0].0["actor"], "agent");
    assert!(opened(&o).is_empty());
}

#[test]
fn the_actor_is_who_made_the_call_not_who_acts_when_the_result_comes() {
    let bed = Bed::new();
    let args = json!({"field": bed.field("open-1f/1/a.docx"), "ms": 300});
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
    let mut o = bed.open(true, false, "fx_document", json!({"field": bed.field("open-1f/1/a.docx"), "ms": 300}));
    in_flight(&o);
    o.ctx.close();
    o.wait("{");
    assert!(opened(&o).is_empty());
}

#[test]
fn the_viewers_job_asks_the_session_when_it_runs_not_at_the_check() {
    let bed = Bed::new();
    let live = |o: &Open| o.ui.lives.lock().unwrap().pop().expect("the bridge handed it over")();
    let mut o = bed.call_on_the_persons_surface("open-1f/1/a.docx");
    o.wait("{");
    assert!(live(&o), "the open lives");
    let mut o = bed.call_on_the_persons_surface("open-1f/1/a.docx");
    o.wait("{");
    o.ctx.close();
    assert!(!live(&o), "it closed between the check and the job");
}

#[test]
fn a_failed_or_oversize_result_opens_nothing() {
    let bed = Bed::new();
    let args = json!({"field": bed.field("open-1f/1/a.docx")});
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
    std::os::unix::fs::symlink(bed.folder.join("open-1f/1/a.docx"), bed.folder.join("open-1f/1/link.docx")).unwrap();
    fs::write(bed.folder.join("open-1f/1/run.exe"), "x").unwrap();
    for file in ["../outside.docx", "open-1f/1/link.docx", "open-1f/1", "open-1f/1/gone.docx", "open-1f/1/run.exe"] {
        let mut o = bed.call_on_the_persons_surface(file);
        assert_eq!(o.wait("{"), r#"{"opened":true}"#, "{file}");
        assert!(opened(&o).is_empty(), "{file}");
    }
}

#[test]
fn a_host_that_knows_no_folder_opens_nothing() {
    let bed = Bed::new();
    let field = bed.field("open-1f/1/a.docx");
    let script = format!("return await buwiz.call(\"fx_document\", {});", json!({"field": field}));
    let mut o = Open::start_with(FIXTURE, json!({"tools": TOOLS}), &script, Limits::default(), |ui| ui.first_party.store(true, Ordering::SeqCst));
    assert_eq!(o.wait("{"), r#"{"opened":true}"#);
    assert!(opened(&o).is_empty());
}

#[test]
fn a_picture_goes_to_the_lightbox_and_a_pdf_to_the_pdf_view() {
    let bed = Bed::new();
    for (file, kind, name) in [("open-1f/1/IMG_4250.png", "image", "IMG_4250.png"), ("open-1f/1/Invoice.pdf", "pdf", "Invoice.pdf"), ("open-1f/1/a.docx", "file", "a.docx")] {
        let mut o = bed.call_on_the_persons_surface(file);
        assert_eq!(o.wait("{"), r#"{"opened":true}"#);
        assert_eq!(viewed(&o), [(kind.to_owned(), name.to_owned())]);
    }
}

#[test]
fn the_persons_press_opens_or_saves_a_file_by_its_name_on_disk() {
    let bed = Bed::new();
    let mut o = bed.open_with(true, false, "open-1f/1/IMG_4250.png");
    assert_eq!(o.wait("{"), r#"{"opened":true}"#, "the app gets its answer, and nothing of the keys");
    assert_eq!(opened_with(&o), ["IMG_4250.png"]);
    assert!(opened(&o).is_empty(), "the viewer is buwiz/document's alone");
    let mut o = bed.save(true, false, "open-1f/1/IMG_4250.png");
    o.wait("{");
    assert_eq!(saved(&o), ["IMG_4250.png"], "the field's name is ignored");
}

#[test]
fn a_call_with_no_press_opens_and_saves_nothing_though_it_is_the_persons() {
    let bed = Bed::new();
    let path = bed.folder.join("open-1f/1/IMG_4250.png");
    let mut o = bed.pressed(true, false, None, json!({"open_with": {"path": path}, "save": {"path": path}}));
    assert_eq!(o.wait("{"), r#"{"opened":true}"#);
    assert_eq!(o.broker.calls.lock().unwrap()[0].0["actor"], "person", "no agent has acted");
    assert!(opened_with(&o).is_empty() && saved(&o).is_empty());
}

#[test]
fn one_press_hands_off_exactly_one_file() {
    let bed = Bed::new();
    let path = bed.folder.join("open-1f/1/IMG_4250.png");
    let script = format!(
        "await buwiz.call(\"fx_document\", {}); return await buwiz.call(\"fx_document\", {});",
        json!({"open_with": {"path": path}}),
        json!({"save": {"path": path}})
    );
    let folder = bed.folder.clone();
    let mut o = Open::start_with(FIXTURE, json!({"tools": TOOLS}), &script, Limits::default(), move |ui| {
        ui.first_party.store(true, Ordering::SeqCst);
        *ui.folder.lock().unwrap() = Some(folder);
        *ui.hand_press.lock().unwrap() = Some(7);
    });
    o.wait("{");
    assert_eq!(opened_with(&o), ["IMG_4250.png"]);
    assert!(saved(&o).is_empty(), "the second call's press was spent");
    // one result asking for both is one press too
    let mut o = bed.pressed(true, false, Some(8), json!({"open_with": {"path": path}, "save": {"path": path}}));
    o.wait("{");
    assert_eq!(opened_with(&o).len() + saved(&o).len(), 1);
}

#[test]
fn the_ais_call_or_a_third_party_plugins_opens_and_saves_nothing() {
    let bed = Bed::new();
    for (first_party, agent) in [(true, true), (false, false)] {
        let mut o = bed.open_with(first_party, agent, "open-1f/1/IMG_4250.png");
        assert_eq!(o.wait("{"), r#"{"opened":true}"#, "its call still answers");
        let mut s = bed.save(first_party, agent, "open-1f/1/IMG_4250.png");
        s.wait("{");
        assert!(opened_with(&o).is_empty() && saved(&s).is_empty(), "first party {first_party}, agent {agent}");
    }
}

#[test]
fn an_exe_opens_nothing_even_where_it_may_save() {
    let bed = Bed::new();
    let mut o = bed.open_with(true, false, "open-1f/1/run.exe");
    o.wait("{");
    assert!(opened_with(&o).is_empty());
    let mut o = bed.save(true, false, "open-1f/1/run.exe");
    o.wait("{");
    assert_eq!(saved(&o), ["run.exe"], "a save is a download: any kind, by its own name");
}

#[test]
fn a_link_or_a_file_outside_the_folder_opens_and_saves_nothing() {
    let bed = Bed::new();
    std::os::unix::fs::symlink(bed.folder.join("open-1f/1/IMG_4250.png"), bed.folder.join("open-1f/1/link.png")).unwrap();
    fs::write(bed.folder.join("store.png"), "x").unwrap();
    for file in ["../outside.png", "open-1f/1/link.png", "open-1f/1", "open-1f/1/gone.png", "store.png"] {
        let mut o = bed.open_with(true, false, file);
        assert_eq!(o.wait("{"), r#"{"opened":true}"#, "{file}");
        let mut s = bed.save(true, false, file);
        s.wait("{");
        assert!(opened_with(&o).is_empty() && saved(&s).is_empty(), "{file}");
    }
}

#[test]
fn a_file_after_the_screen_closed_opens_and_saves_nothing() {
    let bed = Bed::new();
    let path = bed.folder.join("open-1f/1/IMG_4250.png");
    let mut o = bed.pressed(true, false, Some(1), json!({"open_with": {"path": path}, "save": {"path": path}, "ms": 300}));
    in_flight(&o);
    o.ctx.close();
    o.wait("{");
    assert!(opened_with(&o).is_empty() && saved(&o).is_empty());
}

fn told(o: &Open) -> Vec<&'static str> {
    o.ui.refusals.lock().unwrap().clone()
}

#[test]
fn a_hand_off_the_persons_press_made_that_does_nothing_says_so() {
    let bed = Bed::new();
    let mut o = bed.open_with(true, false, "open-1f/1/run.exe");
    o.wait("{");
    assert_eq!(told(&o), [OPEN_REFUSED]);
    let mut o = bed.save(true, false, "open-1f/1/gone.png");
    o.wait("{");
    assert_eq!(told(&o), [SAVE_REFUSED]);
    let mut o = bed.pressed(true, false, Some(1), json!({"field": bed.field("open-1f/1/run.exe")}));
    o.wait("{");
    assert_eq!(told(&o), [OPEN_REFUSED], "the viewer's too");
}

#[test]
fn a_refusal_with_no_press_behind_it_is_only_logged() {
    let bed = Bed::new();
    let path = bed.folder.join("open-1f/1/run.exe");
    let mut o = bed.pressed(true, false, None, json!({"open_with": {"path": path}, "save": {"path": path}, "field": bed.field("open-1f/1/run.exe")}));
    o.wait("{");
    assert!(told(&o).is_empty(), "an app's own code can't put up the Bot's errors");
    // nor the AI's call
    let mut o = bed.open_with(true, true, "open-1f/1/run.exe");
    o.wait("{");
    assert!(told(&o).is_empty());
}

#[test]
fn a_name_that_would_show_as_another_is_refused_out_loud() {
    let bed = Bed::new();
    let name = "open-1f/1/invoice\u{202E}fdp.command";
    fs::write(bed.folder.join(name), "x").unwrap();
    let mut o = bed.save(true, false, name);
    o.wait("{");
    assert!(saved(&o).is_empty());
    assert_eq!(told(&o), [SAVE_REFUSED]);
    let pdf = "open-1f/1/a\u{200B}.pdf";
    fs::write(bed.folder.join(pdf), "x").unwrap();
    let mut o = bed.pressed(true, false, Some(1), json!({"field": bed.field(pdf)}));
    o.wait("{");
    assert!(opened(&o).is_empty(), "the viewer never shows the name");
    assert_eq!(told(&o), [OPEN_REFUSED]);
}
