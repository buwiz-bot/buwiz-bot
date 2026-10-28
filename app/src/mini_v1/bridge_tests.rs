//! The bridge run for real: an app compiled with the shim, in QuickJS on a
//! headless engine (as `mini.rs`'s tests run widgets), its calls answered
//! by the Rust handlers, with a fake broker and a [`HostUi`] the tests
//! answer. Test names carry the spec 17 IDs they cover.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use nsh_core::{ApproxTextEngine, MountBatch, MountItem};
use nsh_engine::{App, AppError, AppErrorKind, Bundle, Engine, EngineConfig, HostChannel, MountSink};
use serde_json::{Value, json};

use super::*;
use crate::mini_v1::{
    load::{self, PluginEntry, Prepared},
    perms::Permissions,
    session::tests::CertBroker,
};

const FIXTURE: &str = "dev.goldcoders.fixture";
const TICKET: &str = "tkt_0123456789ABCDEFGHJKMNPQRS";

/// The fixture board's permissions (the fixture manifest's).
fn board_perms() -> Value {
    json!({
        "tools": ["fx_echo", "fx_text", "fx_slow", "fx_fail", "fx_big", "fx_count"],
        "open": { "apps": ["card"], "external": ["https://example.com"] },
        "attach": ["item"],
        "pick_file": { "extensions": [".xlsx", ".csv"], "max_bytes": 64, "multiple": false },
        "storage": { "quota_bytes": 1024 },
        "copy": true
    })
}

fn plugin(id: &str, perms: &Value) -> PluginEntry {
    PluginEntry::from_json(&json!({
        "id": id, "short": "fx", "title": "Fixture", "version": "1.0.0", "protocol": "1.0.0", "generation": 1,
        "scopes": [ { "name": "item", "attach": true }, { "name": "note" } ],
        "apps": [
            { "id": "board", "title": "Fixture Board", "permissions": perms,
              "props": { "type": "object", "properties": { "page": { "type": "integer" } } } },
            { "id": "card", "title": "Fixture Card",
              "props": { "type": "object", "properties": { "ticket": { "type": "string" } }, "required": ["ticket"] } }
        ]
    }))
    .unwrap()
}

/// An app that runs `script` (an async function body) at mount and shows
/// what it returns, or `threw <code>: <message>`.
fn app(script: &str) -> String {
    format!(
        r#"import {{ signal, Text }} from "nsh";
import {{ rox }} from "rox";
export default function T() {{
  const out = signal("waiting");
  const show = (v) => out.set(typeof v === "string" ? v : JSON.stringify(v));
  const fail = (e) => out.set("threw " + (e && e.code) + ": " + (e && e.message));
  (async () => {{ {script} }})().then(show, fail);
  return <Text>{{out()}}</Text>;
}}
"#
    )
}

/// `probeHost(name, data)` for a test app: the shim's own host binding,
/// which an app's source may not name (the source check refuses it), to
/// show the engine's policy refuses what the bridge did not grant.
fn probe(js: String) -> String {
    js.replacen("const __roxBridge", "const probeHost = (n, d) => __roxHost.call(n, d);\nconst __roxBridge", 1)
}

/// What the fake broker saw and how it answers.
#[derive(Default)]
struct FakeBroker {
    calls: Mutex<Vec<(Value, String)>>,
    running: AtomicUsize,
    most_running: AtomicUsize,
    closed: Mutex<Vec<String>>,
}

impl host_api::Broker for FakeBroker {
    fn request(&self, _: &str, route: &str, body: Option<Value>, _: Duration) -> Result<Value, RoxError> {
        let body = body.unwrap_or(Value::Null);
        match route {
            "/v1/host/call" => {
                let env = body["envelope"].clone();
                self.calls.lock().unwrap().push((env.clone(), body["sig"].as_str().unwrap_or_default().to_owned()));
                let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
                self.most_running.fetch_max(now, Ordering::SeqCst);
                let args = env["args"].clone();
                let result = match env["tool"].as_str().unwrap() {
                    "fx_echo" => json!({"content": [{"type": "text", "text": "echo"}], "structuredContent": args}),
                    "fx_text" => json!({"content": [{"type": "text", "text": "a"}, {"type": "image"}, {"type": "text", "text": "b"}]}),
                    "fx_fail" => json!({"isError": true, "content": [{"type": "text", "text": "the form has no period"}]}),
                    "fx_big" => json!({"content": [{"type": "text", "text": "x".repeat(2 << 20)}]}),
                    "fx_slow" | "fx_count" => {
                        std::thread::sleep(Duration::from_millis(args["ms"].as_u64().unwrap_or(150)));
                        json!({"structuredContent": {"ok": true}})
                    }
                    _ => json!({}),
                };
                self.running.fetch_sub(1, Ordering::SeqCst);
                Ok(json!({"ok": true, "result": result}))
            }
            "/v1/host/tickets/title" => Ok(json!({"valid": body["ticket"] == TICKET, "title": "Dela Cruz Jane"})),
            "/v1/host/resolve" => Ok(json!({"result": "token", "ticket": TICKET, "title": "Item One"})),
            "/v1/host/sessions/close" => {
                self.closed.lock().unwrap().push(body["open_id"].as_str().unwrap_or_default().to_owned());
                Ok(json!({"ok": true}))
            }
            _ => Err(RoxError::new("not_found", route)),
        }
    }
}

/// The person, as the tests play them.
#[derive(Default)]
struct TestUi {
    yes: AtomicBool,
    chat: AtomicBool,
    asked: Mutex<Vec<Consent>>,
    apps: Mutex<Vec<V1Key>>,
    urls: Mutex<Vec<String>>,
    attached: Mutex<Vec<String>>,
    copied: Mutex<Vec<String>>,
    picks: Mutex<Vec<PickRequest>>,
    files: Mutex<Vec<PathBuf>>,
    ended: Mutex<Vec<Ended>>,
}

impl HostUi for TestUi {
    fn consent(&self, ask: Consent, answer: Box<dyn FnOnce(bool) + Send>) {
        self.asked.lock().unwrap().push(ask);
        let yes = self.yes.load(Ordering::SeqCst);
        // the person answers later, from another thread
        std::thread::spawn(move || answer(yes));
    }
    fn open_app(&self, target: V1Key) {
        self.apps.lock().unwrap().push(target);
    }
    fn open_url(&self, url: &str) {
        self.urls.lock().unwrap().push(url.to_owned());
    }
    fn has_chat(&self) -> bool {
        self.chat.load(Ordering::SeqCst)
    }
    fn attach(&self, token: &str, _: &str) -> bool {
        let mut a = self.attached.lock().unwrap();
        let new = !a.iter().any(|t| t == token);
        if new {
            a.push(token.to_owned());
        }
        new
    }
    fn pick_files(&self, pick: PickRequest, answer: Box<dyn FnOnce(Vec<PathBuf>) + Send>) {
        self.picks.lock().unwrap().push(pick);
        answer(self.files.lock().unwrap().clone());
    }
    fn copy(&self, text: &str) {
        self.copied.lock().unwrap().push(text.to_owned());
    }
    fn session_ended(&self, _: &V1Key, why: Ended) {
        self.ended.lock().unwrap().push(why);
    }
}

struct Sink(mpsc::Sender<MountBatch>, Arc<Mutex<Vec<(AppErrorKind, String)>>>);

impl MountSink for Sink {
    fn submit(&self, batch: MountBatch) {
        let _ = self.0.send(batch);
    }
    fn report(&self, error: &AppError) {
        self.1.lock().unwrap().push((error.kind, error.message.clone()));
    }
}

/// A running open of the fixture board.
struct Open {
    engine: Option<Engine>,
    rx: mpsc::Receiver<MountBatch>,
    ctx: Arc<OpenContext>,
    broker: Arc<FakeBroker>,
    ui: Arc<TestUi>,
    channel: HostChannel,
    root: PathBuf,
    shown: Vec<String>,
}

impl Open {
    fn start(perms: Value, script: &str) -> Open {
        Open::start_with(FIXTURE, perms, script, Limits::default(), |_| {})
    }

    fn start_with(plugin_id: &str, perms: Value, script: &str, limits: Limits, ui_setup: impl FnOnce(&TestUi)) -> Open {
        Open::start_source(plugin_id, perms, &app(script), limits, ui_setup)
    }

    fn start_source(plugin_id: &str, perms: Value, source: &str, limits: Limits, ui_setup: impl FnOnce(&TestUi)) -> Open {
        let source = source.to_owned();
        let prepared = Arc::new(Prepared {
            key: V1Key { plugin: plugin_id.to_owned(), app: "board".to_owned(), ticket: None, props: Default::default() },
            plugin: plugin(plugin_id, &perms),
            title: "Fixture Board".to_owned(),
            fingerprint: load::fingerprint(&source),
            permissions: Permissions::parse(Some(&perms)).unwrap(),
            permissions_json: Some(perms.clone()),
            js: probe(load::build("board", "Fixture Board", &source).expect("the test app compiles")),
        });
        let session = Session::open(&CertBroker::honest(), plugin_id, "board", Some(&perms), Some(1)).unwrap();
        let broker = Arc::new(FakeBroker::default());
        let ui = Arc::new(TestUi::default());
        ui_setup(&ui);
        let root = crate::mini_v1::storage::scratch_root("bridge");
        let ctx = OpenContext::new(prepared.clone(), session, broker.clone(), ui.clone(), &root, limits);
        let channel = HostChannel::new();
        install(&channel, &ctx, json!({"width": 800, "height": 600, "placement": "full"}));
        let bundle = Bundle::source(prepared.js.clone()).with_policy(prepared.permissions.policy());
        let mut config = EngineConfig::new(App::Js(bundle));
        config.host = channel.clone();
        let (tx, rx) = mpsc::channel();
        let engine = Engine::start(config, || ApproxTextEngine, Sink(tx, Default::default())).expect("engine starts");
        Open { engine: Some(engine), rx, ctx, broker, ui, channel, root, shown: Vec::new() }
    }

    /// Waits until the app shows text starting with `prefix`; that text.
    fn wait(&mut self, prefix: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(t) = self.shown.iter().find(|t| t.starts_with(prefix)) {
                return t.clone();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(batch) = self.rx.recv_timeout(left) else {
                panic!("no {prefix:?}; shown: {:?}", self.shown);
            };
            for item in batch.items {
                if let MountItem::Text { text, .. } = item {
                    self.shown.push(text.text.to_string());
                }
            }
        }
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        self.engine.take();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn app_07_ungranted_methods_are_absent() {
    let mut o = Open::start(
        json!({"tools": ["fx_echo"]}),
        r#"
        const absent = ["open", "attach", "pickFile", "storage", "copy"].filter((k) => !(k in rox) && rox[k] === undefined);
        let reassigned = "kept";
        try { globalThis.rox = {}; } catch (e) { reassigned = "refused"; }
        try { rox.call = null; } catch (e) {}
        let added = "kept";
        try { rox.open = () => 1; } catch (e) { added = "refused"; }
        return [absent.join(","), typeof rox.call, typeof rox.ticket.title, Object.isFrozen(rox), globalThis.rox === rox, typeof rox.open, rox.app.app, rox.app.sdk].join(" ");
        "#,
    );
    assert_eq!(o.wait("open"), "open,attach,pickFile,storage,copy function function true true undefined board 1");
    // and at the host level: the channel handles only the granted names
    let p = &o.ctx.prepared;
    assert_eq!(p.permissions.methods(), ["rox.ticket.title", "rox.call"]);
    assert!(!p.permissions.policy().access().allows_host("rox.open"));
}

#[test]
fn app_07_a_call_of_an_ungranted_host_name_fails_in_the_engine() {
    // past the shim, at the channel itself: the policy keeps it to the
    // granted names
    let mut o = Open::start(
        json!({"tools": ["fx_echo"]}),
        r#"
        const tries = [];
        for (const name of ["rox.open", "rox.copy", "rox.storage.get", "rox.provision"]) {
          try { await probeHost(name, {}); tries.push("ran"); } catch (e) { tries.push("refused"); }
        }
        return "host " + tries.join(",");
        "#,
    );
    assert_eq!(o.wait("host"), "host refused,refused,refused,refused");
    assert!(o.ui.copied.lock().unwrap().is_empty() && o.ui.apps.lock().unwrap().is_empty());
}

#[test]
fn app_04_no_fetch_storage_or_network_globals() {
    let mut o = Open::start(
        json!({}),
        r#"
        const kinds = ["fetch", "XMLHttpRequest", "WebSocket", "localStorage", "sessionStorage"].map((k) => typeof globalThis[k]);
        let threw = "no";
        try { await fetch("http://127.0.0.1:8787/threads"); } catch (e) { threw = "yes"; }
        let ws = "no";
        try { new WebSocket("ws://127.0.0.1:8787/"); } catch (e) { ws = "yes"; }
        return "net " + kinds.join(",") + " " + threw + " " + ws;
        "#,
    );
    assert_eq!(o.wait("net"), "net undefined,undefined,undefined,undefined,undefined yes yes");
}

#[test]
fn brg_01_a_tool_not_in_permissions_is_denied_and_nothing_is_sent() {
    let mut o = Open::start(
        board_perms(),
        r#"return await rox.call("fx_secret", {});"#,
    );
    assert_eq!(o.wait("threw"), "threw permission_denied: \"fx_secret\" is not in this app's tools");
    assert!(o.broker.calls.lock().unwrap().is_empty(), "sent: {:?}", o.broker.calls.lock().unwrap());
}

#[test]
fn brg_03_a_call_goes_out_signed_by_the_opens_session_key() {
    let mut o = Open::start(
        board_perms(),
        r#"
        const a = await rox.call("fx_echo", { code: "1601C", n: 2 });
        const b = await rox.call("fx_text");
        return "ok " + JSON.stringify(a) + " " + b;
        "#,
    );
    assert_eq!(o.wait("ok"), "ok {\"code\":\"1601C\",\"n\":2} a\nb");
    let calls = o.broker.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    let s = &o.ctx.session;
    for (env, sig) in calls.iter() {
        assert_eq!(env["plugin"], FIXTURE);
        assert_eq!(env["app"], "board");
        assert_eq!(env["open_id"], s.open_id.as_str());
        keys::verify(&s.public_key(), env, sig).expect("the session key signed it");
    }
    assert_eq!(calls[1].0["args"], json!({}), "args default to {{}}");
    assert_ne!(calls[0].0["nonce"], calls[1].0["nonce"]);
}

#[test]
fn rox_call_maps_tool_errors_big_results_and_bad_args() {
    let mut o = Open::start(
        board_perms(),
        r#"
        const out = [];
        for (const [tool, args] of [["fx_fail", {}], ["fx_big", {}], ["fx_echo", [1]], ["fx_echo", { s: "x".repeat(300000) }]]) {
          try { await rox.call(tool, args); out.push("ok"); } catch (e) { out.push(e.code + (e instanceof RoxError ? "" : "?") + (e.retryable ? "!" : "")); }
        }
        try { await rox.call("fx_fail"); } catch (e) { out.push(e.message); }
        return "mapped " + out.join(",");
        "#,
    );
    assert_eq!(
        o.wait("mapped"),
        "mapped tool_error,result_too_large,invalid_args,invalid_args,the form has no period"
    );
}

#[test]
fn brg_12_the_ninth_call_waits_and_the_twenty_first_in_a_second_is_rate_limited() {
    let mut o = Open::start(
        board_perms(),
        r#"
        const all = [];
        for (let i = 0; i < 21; i++) all.push(rox.call("fx_count", { ms: 150 }));
        const done = await Promise.allSettled(all);
        const ok = done.filter((d) => d.status === "fulfilled").length;
        const limited = done.filter((d) => d.status === "rejected" && d.reason.code === "rate_limited" && d.reason.retryable).length;
        return "calls " + ok + " " + limited;
        "#,
    );
    assert_eq!(o.wait("calls"), "calls 20 1");
    assert_eq!(o.broker.most_running.load(Ordering::SeqCst), 8, "at most 8 in flight");
    assert_eq!(o.broker.calls.lock().unwrap().len(), 20, "the 21st was not sent");
}

#[test]
fn brg_11_a_call_past_the_limit_rejects_timeout() {
    let limits = Limits { timeout: Duration::from_millis(300), ..Limits::default() };
    let mut o = Open::start_with(FIXTURE, board_perms(), r#"return await rox.call("fx_slow", { ms: 1500 });"#, limits, |_| {});
    let shown = o.wait("threw");
    assert!(shown.starts_with("threw timeout: "), "{shown}");
}

#[test]
fn brg_13_open_url_is_https_to_listed_origins_and_asks_every_time() {
    let mut o = Open::start(
        board_perms(),
        r#"
        const out = [];
        for (const url of ["http://example.com/", "mailto:a@example.com", "file:///etc/passwd", "https://evil.example/x", "https://example.com/a?b=1"]) {
          try { await rox.open({ url }); out.push("opened"); } catch (e) { out.push(e.code); }
        }
        return "urls " + out.join(",");
        "#,
    );
    // the person says no (the default here)
    assert_eq!(o.wait("urls"), "urls not_supported,not_supported,not_supported,permission_denied,consent_declined");
    assert_eq!(
        *o.ui.asked.lock().unwrap(),
        [Consent::OpenUrl { url: "https://example.com/a?b=1".to_owned(), app: "Fixture Board".to_owned() }]
    );
    assert!(o.ui.urls.lock().unwrap().is_empty());

    let mut o = Open::start_with(
        FIXTURE,
        board_perms(),
        r#"await rox.open({ url: "https://Example.com/a" }); await rox.open({ url: "https://example.com/b" }); return "opened";"#,
        Limits::default(),
        |ui| ui.yes.store(true, Ordering::SeqCst),
    );
    assert_eq!(o.wait("opened"), "opened");
    assert_eq!(o.ui.asked.lock().unwrap().len(), 2, "asked each time");
    assert_eq!(*o.ui.urls.lock().unwrap(), ["https://Example.com/a", "https://example.com/b"]);
}

#[test]
fn rox_open_app_pushes_declared_apps_without_asking() {
    let mut o = Open::start(
        board_perms(),
        &format!(
            r#"
        const out = [];
        for (const t of [{{ app: "card", props: {{ ticket: "{TICKET}" }}, ticket: "{TICKET}" }}, {{ app: "board" }}, {{ app: "card", props: {{}} }}, {{ app: "card", props: {{ ticket: "t" }}, ticket: "123" }}, {{ app: "card", url: "https://example.com" }}]) {{
          try {{ await rox.open(t); out.push("pushed"); }} catch (e) {{ out.push(e.code); }}
        }}
        return "apps " + out.join(",");
        "#
        ),
    );
    assert_eq!(o.wait("apps"), "apps pushed,permission_denied,invalid_args,invalid_args,invalid_args");
    let apps = o.ui.apps.lock().unwrap();
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].to_key(), format!("v1:{FIXTURE}/card?ticket={TICKET}&props=%7B%22ticket%22%3A%22{TICKET}%22%7D"));
    assert!(o.ui.asked.lock().unwrap().is_empty(), "no consent for same-plugin apps");
}

#[test]
fn brg_14_pick_file_returns_no_path_and_read_after_close_rejects_closed() {
    let dir = crate::mini_v1::storage::scratch_root("pick");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("rates.csv");
    std::fs::write(&file, "a,b\n1").unwrap();
    let f2 = file.clone();
    let mut o = Open::start_with(
        FIXTURE,
        board_perms(),
        r#"
        const refused = [];
        for (const opts of [{ extensions: [".pdf"] }, { multiple: true }, { path: "/etc" }]) {
          try { await rox.pickFile(opts); refused.push("picked"); } catch (e) { refused.push(e.code); }
        }
        const files = await rox.pickFile({ extensions: ["CSV"] });
        const f = files[0];
        const bytes = await f.read();
        const text = String.fromCharCode(...bytes);
        return "picked " + refused.join(",") + " " + Object.keys(f).sort().join(",") + " " + f.name + " " + f.size + " " + f.type + " " + JSON.stringify(text) + " " + JSON.stringify(files).includes("rox-mini-v1-pick");
        "#,
        Limits::default(),
        move |ui| *ui.files.lock().unwrap() = vec![f2],
    );
    assert_eq!(
        o.wait("picked"),
        "picked permission_denied,permission_denied,invalid_args name,read,size,type rates.csv 5 text/csv \"a,b\\n1\" false"
    );
    assert_eq!(o.ui.picks.lock().unwrap()[0].extensions, [".csv"]);
    assert_eq!(o.ui.picks.lock().unwrap()[0].max_bytes, 64);
    // the open ends: its files can't be read any more
    o.ctx.close();
    assert!(o.ctx.files.lock().unwrap().is_empty());
    let deadline = Instant::now() + Duration::from_secs(5);
    while o.broker.closed.lock().unwrap().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(*o.broker.closed.lock().unwrap(), [o.ctx.session.open_id.clone()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn brg_14_read_after_close_rejects_closed_in_the_app() {
    let dir = crate::mini_v1::storage::scratch_root("pick2");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("big.xlsx");
    std::fs::write(&file, "x").unwrap();
    let f2 = file.clone();
    let mut o = Open::start_with(
        FIXTURE,
        board_perms(),
        r#"
        const [f] = await rox.pickFile();
        await f.read();
        for (let i = 0; i < 400; i++) {
          try { await f.read(); } catch (e) { return "after " + e.code; }
          await new Promise((r) => setTimeout(r, 10));
        }
        return "after none";
        "#,
        Limits::default(),
        move |ui| *ui.files.lock().unwrap() = vec![f2],
    );
    // let the app read once, then close the open under it
    let deadline = Instant::now() + Duration::from_secs(5);
    while o.ctx.files.lock().unwrap().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    o.ctx.close();
    assert_eq!(o.wait("after"), "after closed");
    // a file over max_bytes is not handed over
    std::fs::write(&file, "x".repeat(65)).unwrap();
    let f3 = file.clone();
    let mut o = Open::start_with(
        FIXTURE,
        board_perms(),
        r#"try { await rox.pickFile(); return "big picked"; } catch (e) { return "big " + e.code; }"#,
        Limits::default(),
        move |ui| *ui.files.lock().unwrap() = vec![f3],
    );
    assert_eq!(o.wait("big"), "big quota_exceeded");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn brg_15_attach_without_consent_is_declined_and_changes_nothing() {
    let script = r#"
        const out = [];
        for (const [scope, value] of [["note", "x"], ["item", "123"]]) {
          try { out.push(JSON.stringify(await rox.attach(scope, value))); } catch (e) { out.push(e.code); }
        }
        return "attach " + out.join(" ");
    "#;
    // no chat
    let mut o = Open::start(board_perms(), script);
    assert_eq!(o.wait("attach"), "attach permission_denied no_chat");
    // a chat, and the person says no
    let mut o = Open::start_with(FIXTURE, board_perms(), script, Limits::default(), |ui| ui.chat.store(true, Ordering::SeqCst));
    assert_eq!(o.wait("attach"), "attach permission_denied consent_declined");
    assert_eq!(
        *o.ui.asked.lock().unwrap(),
        [Consent::Attach { title: "Item One".to_owned(), app: "Fixture Board".to_owned() }]
    );
    assert!(o.ui.attached.lock().unwrap().is_empty(), "the chat is unchanged");
    // yes: attached once, the second time it is the same attachment
    let mut o = Open::start_with(
        FIXTURE,
        board_perms(),
        r#"const a = await rox.attach("item", "123"); const b = await rox.attach("item", "123"); return "yes " + JSON.stringify(a, ["token", "title", "attached"]) + " " + b.attached;"#,
        Limits::default(),
        |ui| {
            ui.chat.store(true, Ordering::SeqCst);
            ui.yes.store(true, Ordering::SeqCst);
        },
    );
    assert_eq!(o.wait("yes"), format!("yes {{\"token\":\"@fx:item:{TICKET}\",\"title\":\"Item One\",\"attached\":true}} false"));
    assert_eq!(*o.ui.attached.lock().unwrap(), [format!("@fx:item:{TICKET}")]);
}

#[test]
fn brg_16_storage_round_trips_through_the_bridge_within_its_limits() {
    let mut o = Open::start(
        board_perms(),
        r#"
        await rox.storage.set("draft.1601C", "{\"a\":1}");
        const got = await rox.storage.get("draft.1601C");
        const none = await rox.storage.get("nope");
        const out = [];
        for (const [k, v] of [["k".repeat(129), "v"], ["ok", "x".repeat(2000)], ["bad/key", "v"]]) {
          try { await rox.storage.set(k, v); out.push("set"); } catch (e) { out.push(e.code); }
        }
        await rox.storage.remove("draft.1601C");
        return "store " + got + " " + none + " " + out.join(",") + " " + JSON.stringify(await rox.storage.keys());
        "#,
    );
    assert_eq!(o.wait("store"), "store {\"a\":1} null invalid_args,quota_exceeded,invalid_args []");
    // the file is the app's own, under its plugin and app ids
    assert!(o.root.join(FIXTURE).join("apps/board/storage.json").is_file());
}

#[test]
fn copy_and_ticket_titles_go_through_the_host() {
    let mut o = Open::start(
        board_perms(),
        &format!(
            r#"
        await rox.copy("BIR 1601C, 2026-09");
        const t = await rox.ticket.title("{TICKET}");
        const gone = await rox.ticket.title("tkt_ZZZZZZZZZZZZZZZZZZZZZZZZZZ");
        let bad = "";
        try {{ await rox.ticket.title("123"); }} catch (e) {{ bad = e.code; }}
        return "host " + t + " " + gone + " " + bad;
        "#
        ),
    );
    assert_eq!(o.wait("host"), "host Dela Cruz Jane null invalid_args");
    assert_eq!(*o.ui.copied.lock().unwrap(), ["BIR 1601C, 2026-09"]);
}

#[test]
fn the_viewport_is_a_signal_the_host_moves() {
    let source = r#"import { Text } from "nsh";
import { rox } from "rox";
export default function T() {
  let frozen = "no";
  try { rox.viewport().width = 1; } catch (e) { frozen = "yes"; }
  return <Text>{"size " + rox.viewport().width + "x" + rox.viewport().height + " " + rox.viewport().placement + " " + frozen}</Text>;
}
"#;
    let mut o = Open::start_source(FIXTURE, json!({}), source, Limits::default(), |_| {});
    assert_eq!(o.wait("size"), "size 800x600 full yes");
    o.channel.send("rox.viewport", json!({"width": 480, "height": 320, "placement": "inline"})).unwrap();
    assert_eq!(o.wait("size 480"), "size 480x320 inline yes");
}

#[test]
fn a_plugin_restart_ends_the_session_and_calls_reject_plugin_restarted() {
    let plugin = "dev.goldcoders.restartfixture";
    let mut o = Open::start_with(
        plugin,
        board_perms(),
        r#"
        await rox.call("fx_echo", {});
        for (let i = 0; i < 400; i++) {
          try { await rox.call("fx_echo", {}); } catch (e) { return "ended " + e.code + " " + e.retryable; }
          await new Promise((r) => setTimeout(r, 10));
        }
        return "ended never";
        "#,
        Limits { per_second: 1000, ..Limits::default() },
        |_| {},
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while o.broker.calls.lock().unwrap().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(crate::mini_v1::session::on_plugin_event(&json!({"id": plugin, "generation": 2})), 1);
    assert_eq!(o.wait("ended"), "ended plugin_restarted true");
    assert_eq!(*o.ui.ended.lock().unwrap(), [Ended::Restarted]);
}

#[test]
fn tool_results_resolve_structured_content_or_text() {
    assert_eq!(tool_result(json!({"structuredContent": {"a": 1}, "content": []}), 100).unwrap(), json!({"a": 1}));
    assert_eq!(tool_result(json!({"content": [{"type": "text", "text": "x"}]}), 100).unwrap(), json!("x"));
    let e = tool_result(json!({"isError": true, "content": [{"type": "text", "text": "e".repeat(900)}]}), 10_000).unwrap_err();
    assert_eq!((e.code.as_str(), e.message.len()), ("tool_error", 500));
    assert_eq!(tool_result(json!({"content": [{"type": "text", "text": "x".repeat(200)}]}), 100).unwrap_err().code, "result_too_large");
}

/// Runs `js` (a whole module) on an engine with `limits`; the errors it
/// reports.
fn reported(js: &str, limits: nsh_engine::Limits) -> Vec<(AppErrorKind, String)> {
    let mut config = EngineConfig::new(App::Js(Bundle::source(js.to_owned()).with_policy(Permissions::default().policy())));
    config.limits = limits;
    let (tx, rx) = mpsc::channel();
    let reports = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::start(config, || ApproxTextEngine, Sink(tx, reports.clone())).expect("engine starts");
    let deadline = Instant::now() + Duration::from_secs(10);
    while reports.lock().unwrap().is_empty() && Instant::now() < deadline {
        let _ = rx.recv_timeout(Duration::from_millis(20));
    }
    drop(engine);
    let r = reports.lock().unwrap().clone();
    r
}

#[test]
fn app_08_runtime_errors_and_budget_trips_show_the_specs_messages() {
    let src = "import { Text } from \"nsh\";\nexport default function T() { setTimeout(() => { throw new Error(\"x is undefined\"); }, 1); return <Text>t</Text>; }";
    let js = load::build("t", "Forms", src).unwrap();
    let r = reported(&js, nsh_engine::Limits::default());
    let shown: Vec<_> = r.iter().filter_map(|(k, m)| load::runtime_message("Forms", *k, m)).collect();
    assert_eq!(shown.first().map(String::as_str), Some("Forms stopped: Error: x is undefined."), "{r:?}");

    let spin = "import { Text } from \"nsh\";\nexport default function T() { for (;;) {} }";
    let js = load::build("t", "Forms", spin).unwrap();
    let limits = nsh_engine::Limits { load_budget: Duration::from_millis(200), call_budget: Duration::from_millis(200), ..Default::default() };
    let r = reported(&js, limits);
    let shown: Vec<_> = r.iter().filter_map(|(k, m)| load::runtime_message("Forms", *k, m)).collect();
    assert_eq!(shown.first().map(String::as_str), Some("Forms stopped: it took too long."), "{r:?}");

    let hog = "import { Text } from \"nsh\";\nexport default function T() { const a = []; for (;;) a.push(new Array(100000).fill(1)); }";
    let js = load::build("t", "Forms", hog).unwrap();
    let limits = nsh_engine::Limits { memory_bytes: 16 << 20, ..Default::default() };
    let r = reported(&js, limits);
    let shown: Vec<_> = r.iter().filter_map(|(k, m)| load::runtime_message("Forms", *k, m)).collect();
    assert_eq!(shown.first().map(String::as_str), Some("Forms stopped: it ran out of memory."), "{r:?}");
}
