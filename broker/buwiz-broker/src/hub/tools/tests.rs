//! Rust ports, by ID, of the tool surface's module-level JS tests:
//! v1-agent.test.mjs (AGT-01…09, AUD-01, ATT-01/02, CARD-01/02, RSV-06,
//! INV-01, INV-09), v1-routines.test.mjs's tool-loop cases (RTN-06,
//! RTN-14) and mcp-agent.test.mjs (MAGT-01, 02, 04). The v1 fixture plugin
//! is a fake `PluginView` answering as the fixture does; the MCP servers
//! are the real fixtures (`node`), under the hub's own McpHost.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::broadcast;

use super::v1::{self, PluginsSeam};
use super::{exec_tool, tool_list, Out, Run};
use crate::helper::plugins::{Row, RuntimeView};
use crate::hub::ctx::HubCtx;
use crate::hub::model::Settings;
use crate::hub::routines::plugins::PluginView;
use crate::hub::routines::text::{approved_text, TRUNCATED};
use crate::hub::traits::Clock;
use crate::hub::types::{BoxFuture, TurnCtx};
use crate::v1::trust::Trust;
use crate::v1::V1Error;

const FX: &str = "app.buwiz.fixture";
const GWS: &str = "app.buwiz.google-workspace";
const TKT: &str = "tkt_0123456789ABCDEFGHJKMNPQRS";

fn tool(name: &str, desc: &str, extra: Value) -> Value {
    let mut t = json!({"name": name, "description": desc, "inputSchema": {"type": "object"}});
    for (k, v) in extra.as_object().into_iter().flatten() {
        t[k] = v.clone();
    }
    t
}

/// The fixture plugin (and google-workspace) as the v1 tests install them.
struct Fake {
    rows: Mutex<Vec<Row>>,
    approved: Mutex<bool>,
    instructions: String,
    calls: Mutex<Vec<(String, Value, Option<Value>)>>,
}

fn row(id: &str, short: &str, manifest: Value, tools: Vec<Value>) -> Row {
    Row {
        id: id.into(),
        short: short.into(),
        command: "node".into(),
        args: "[]".into(),
        env: "{}".into(),
        version: "1.0.0".into(),
        protocol: "1.0.0".into(),
        publisher_key_id: "k1".into(),
        plugin_pub: "pub".into(),
        manifest: manifest.to_string(),
        manifest_sha256: "0".repeat(64),
        tools: json!(tools).to_string(),
        enabled: true,
        disabled_tools: "[]".into(),
        created_at: 1,
        package: None,
        skills: "[]".into(),
    }
}

impl Fake {
    fn new(instructions: &str) -> Arc<Fake> {
        let fx = row(
            FX,
            "fx",
            json!({
                "id": FX, "short": "fx", "title": "Fixture", "publisher": {"id": "app.buwiz"},
                "apps": [{"id": "card", "title": "Card", "placement": ["inline"]}],
                "scopes": [
                    {"name": "item", "attach": true, "description": "## Item by name", "hint": "## Items\nAn attached item scopes fx_search to it."},
                    {"name": "note", "description": "A note"}
                ],
                "routines": [{"id": "fx-digest", "name": "Fixture digest", "prompt": "Summarize."}]
            }),
            vec![
                tool("fx_echo", "Echo the arguments and the caller.", json!({})),
                tool("fx_search", "Search items", json!({"annotations": {"readOnlyHint": true}})),
                tool("fx_open", "Answer with a card", json!({})),
                tool("fx_big", "A big answer", json!({})),
                tool("fx_app_only", "For apps", json!({"_meta": {"buwiz/audience": ["app"]}})),
                tool("buwiz.provision", "reserved", json!({})),
                tool("mcp_shadow", "a v0 name", json!({})),
                tool("other_name", "not its short", json!({})),
            ],
        );
        let gws = row(
            GWS,
            "gws",
            json!({"id": GWS, "short": "gws", "title": "Google Workspace"}),
            vec![tool("gws_search_mail", "Search mail", json!({"annotations": {"readOnlyHint": true}})), tool("gws_read_mail", "Read mail", json!({}))],
        );
        Arc::new(Fake { rows: Mutex::new(vec![fx, gws]), approved: Mutex::new(true), instructions: instructions.into(), calls: Mutex::new(Vec::new()) })
    }

    fn set_tool(&self, name: &str, off: bool) {
        let mut rows = self.rows.lock().unwrap();
        let r = rows.iter_mut().find(|r| r.id == FX).unwrap();
        r.disabled_tools = if off { json!([name]).to_string() } else { "[]".into() };
    }
}

impl PluginView for Arc<Fake> {
    fn row(&self, id: &str) -> Option<Row> {
        self.rows.lock().unwrap().iter().find(|r| r.id == id).cloned()
    }
    fn rows(&self) -> Vec<Row> {
        self.rows.lock().unwrap().clone()
    }
    fn runtime(&self, id: &str) -> RuntimeView {
        let tools = self.row(id).map(|r| serde_json::from_str::<Vec<Value>>(&r.tools).unwrap()).unwrap_or_default();
        RuntimeView { state: "running", conn: true, tools, instructions: if id == FX { self.instructions.clone() } else { String::new() }, ..RuntimeView::default() }
    }
    fn trust(&self) -> Trust {
        if *self.approved.lock().unwrap() {
            Trust { approved: vec![(FX.into(), "k1".into()), (GWS.into(), "k1".into())], ..Trust::default() }
        } else {
            Trust::default()
        }
    }
    fn call_tool(&self, id: &str, tool: &str, args: Value, meta: Option<Value>) -> BoxFuture<'static, Result<Value, V1Error>> {
        self.calls.lock().unwrap().push((tool.into(), args.clone(), meta.clone()));
        let caller = meta.as_ref().and_then(|m| m.get("buwiz/caller")).cloned().unwrap_or(Value::Null);
        let text = |s: String| json!([{"type": "text", "text": s}]);
        let out = match tool {
            "fx_open" => {
                let mut open = json!({"app": args.get("app").cloned().unwrap_or(json!("card")), "ticket": TKT, "props": {"as_of": 1}});
                if let Some(s) = args.get("size") {
                    open["size"] = s.clone();
                }
                if let Some(s) = args.get("snapshot") {
                    open["props"]["snapshot"] = s.clone();
                }
                if let Some(t) = args.get("title") {
                    open["title"] = t.clone();
                }
                let mut meta = json!({"buwiz/open": open});
                if let Some(s) = args.get("suggest") {
                    meta["buwiz/suggest"] = s.clone();
                }
                let failed = args.get("fail") == Some(&json!(true));
                Ok(json!({"content": text(if failed { "failed".into() } else { "Here is the card.".into() }), "isError": failed, "_meta": meta}))
            }
            // a package tool whose account is missing (§20.4)
            "fx_mail" => {
                let data = json!({"code": "account_required", "account": args.get("account").cloned().unwrap_or(json!("google"))});
                Ok(json!({"content": text(data.to_string()), "structuredContent": data, "isError": true}))
            }
            _ => Ok(json!({"content": text(json!({"tool": tool, "args": args, "caller": caller, "plugin": id}).to_string())})),
        };
        Box::pin(async move { out })
    }
    fn events(&self) -> Option<broadcast::Receiver<Value>> {
        None
    }
    fn declared_accounts(&self, id: &str) -> Vec<crate::v1::package::Account> {
        let google = crate::v1::package::Account { id: "google".into(), provider: "google".into(), label: "Google account".into(), scopes: Vec::new(), required: true };
        if id == FX { vec![google] } else { Vec::new() }
    }
}

fn with_plugins(label: &str, fake: &Arc<Fake>) -> Arc<HubCtx> {
    let dir = std::env::temp_dir().join(format!("rox-tools-{label}-{}-{}", std::process::id(), crate::hub::store::now_ms()));
    let mut ctx = HubCtx::new(dir, Clock::manual());
    ctx.store.load().unwrap();
    ctx.ext.insert(PluginsSeam(Arc::new(fake.clone())));
    Arc::new(ctx)
}

fn run_in(ctx: &Arc<HubCtx>, turn: TurnCtx) -> Run {
    let thread = if turn.thread.is_empty() { ctx.store.read(|s| s.active.clone()) } else { turn.thread.clone() };
    let job = ctx.jobs.create(&thread, Value::Null, None, false);
    Run::new(ctx.clone(), TurnCtx { thread, ..turn }, job, Arc::new(Settings::from(|_| None)))
}

async fn names(run: &Run) -> Vec<String> {
    tool_list(run).await.iter().map(|t| t["name"].as_str().unwrap_or("").to_owned()).collect()
}

async fn call(run: &Run, name: &str, input: Value) -> Out {
    exec_tool(run, &json!({"name": name, "input": input})).await
}

#[tokio::test]
async fn agt_01_v1_tools_keep_their_names_and_reach_the_model_prefixed() {
    let fake = Fake::new("Use fx_search for items.");
    let ctx = with_plugins("agt01", &fake);
    let run = run_in(&ctx, TurnCtx::default());
    let all = names(&run).await;
    for t in ["fx_echo", "fx_search", "fx_open", "fx_big"] {
        assert!(all.contains(&t.to_owned()), "AGT-01 {t}");
    }
    assert!(!all.iter().any(|n| n == "mcp_shadow" || n == "other_name"), "AGT-01: no v0 names, no other short");
    let def = tool_list(&run).await.into_iter().find(|t| t["name"] == "fx_echo").unwrap();
    assert_eq!(def, json!({"name": "fx_echo", "description": "[fx] Echo the arguments and the caller.", "input_schema": {"type": "object"}}));
    let r = call(&run, "fx_echo", json!({"hello": 1})).await;
    assert_eq!(r.is_error, Some(false));
    let body: Value = serde_json::from_str(r.content.as_str().unwrap()).unwrap();
    assert_eq!(body["caller"]["origin"], "agent", "AGT-01: 8.4 origin agent");
    assert_eq!(body["caller"]["plugin"], FX);
    assert!(body["caller"]["routine"].is_null());
}

#[tokio::test]
async fn agt_08_a_routines_turn_sees_only_its_surface() {
    let fake = Fake::new("");
    let ctx = with_plugins("agt08", &fake);
    let everything = names(&run_in(&ctx, TurnCtx::default())).await;
    let surface = names(&run_in(&ctx, TurnCtx { surface: Some(json!({"plugin": FX, "routine": "r", "reads": [], "writes": []})), routine: Some(format!("{FX}/r")), ..TurnCtx::default() })).await;
    assert!(!surface.is_empty() && surface.len() < everything.len(), "AGT-08: {} of {}", surface.len(), everything.len());
    assert!(surface.iter().all(|n| n.starts_with("fx_")), "AGT-08: {surface:?}");
}

#[tokio::test]
async fn agt_09_routine_tools_are_offered_except_in_a_routines_own_turn() {
    let fake = Fake::new("");
    let ctx = with_plugins("agt09", &fake);
    let all = names(&run_in(&ctx, TurnCtx::default())).await;
    for t in ["routine_create", "routine_list", "routine_run", "routine_update", "routine_delete"] {
        assert!(all.contains(&t.to_owned()), "AGT-09 {t}");
    }
    let in_routine = names(&run_in(&ctx, TurnCtx { routine: Some("r1".into()), ..TurnCtx::default() })).await;
    assert!(!in_routine.iter().any(|n| n.starts_with("routine_")), "AGT-09: a v0 routine's turn gets none");
    assert!(in_routine.contains(&"fx_echo".to_owned()), "AGT-09: the rest stays");
    // the calls themselves are routines' handlers (5.5); the loop reaches them
    let mut c = HubCtx::new(std::env::temp_dir().join("rox-agt09-reg"), Clock::manual());
    c.tools.register("routine_list", |_: Arc<HubCtx>, t: TurnCtx, _: Value| async move {
        crate::hub::types::ToolResult { content: json!(format!("routine {}", t.routine.unwrap_or_default())), is_error: t.job.is_none() }
    });
    let c = Arc::new(c);
    let r = call(&run_in(&c, TurnCtx { thread: "t0".into(), routine: Some("r1".into()), ..TurnCtx::default() }), "routine_list", json!({})).await;
    assert_eq!(r, Out { content: json!("routine r1"), is_error: None }, "AGT-09: the job and the routine reach the handler; is_error false is left out");
}

#[tokio::test]
async fn rsv_06_reserved_names_are_absent_and_refused() {
    let fake = Fake::new("");
    let ctx = with_plugins("rsv06", &fake);
    let run = run_in(&ctx, TurnCtx::default());
    assert!(!names(&run).await.iter().any(|n| n.starts_with("rox.")), "RSV-06");
    assert_eq!(call(&run, "buwiz.provision", json!({})).await.is_error, Some(true), "RSV-06");
}

#[tokio::test]
async fn aud_01_an_app_only_tool_is_off_the_models_surface() {
    let fake = Fake::new("");
    let ctx = with_plugins("aud01", &fake);
    let run = run_in(&ctx, TurnCtx::default());
    assert!(!names(&run).await.contains(&"fx_app_only".to_owned()), "AUD-01");
    assert_eq!(call(&run, "fx_app_only", json!({})).await.is_error, Some(true), "AUD-01");
}

#[tokio::test]
async fn agt_02_a_disabled_tool_is_absent_and_refused() {
    let fake = Fake::new("");
    let ctx = with_plugins("agt02", &fake);
    let run = run_in(&ctx, TurnCtx::default());
    fake.set_tool("fx_big", true);
    assert!(!names(&run).await.contains(&"fx_big".to_owned()), "AGT-02");
    assert_eq!(call(&run, "fx_big", json!({"bytes": 1})).await.is_error, Some(true), "AGT-02");
    fake.set_tool("fx_big", false);
    assert!(names(&run).await.contains(&"fx_big".to_owned()), "AGT-02");
}

#[test]
fn att_01_attachments_reach_the_model_as_canonical_tokens_of_approved_plugins() {
    let fake = Fake::new("Use fx_search for items.");
    let atts = json!([
        {"token": format!("@fx:item:{TKT}"), "plugin": FX, "scope": "item", "title": "Acme"},
        {"token": "@fx:item:123-456-789", "plugin": FX, "scope": "item", "title": "raw id"},
        {"token": format!("@fx:note:{TKT}"), "plugin": FX, "scope": "note", "title": "not attachable"},
        {"token": format!("@fx:item:{TKT}"), "plugin": "app.buwiz.other", "scope": "item", "title": "another plugin"}
    ]);
    let text = v1::prompt_section(&fake, "hello", Some(&atts));
    assert!(text.contains(&format!("Attached: @fx:item:{TKT} (Acme): Fixture tools default to this item here")), "ATT-01: {text}");
    assert!(!text.contains("123-456-789") && !text.contains("not attachable") && !text.contains("another plugin"), "ATT-01");
    assert!(text.contains("An attached item scopes fx_search"), "ATT-01: the attached scope's hint");
    *fake.approved.lock().unwrap() = false;
    assert_eq!(v1::prompt_section(&fake, "hello", Some(&atts)), "", "ATT-01: an unapproved plugin's attachments add nothing");
}

#[tokio::test]
async fn agt_03_an_unapproved_plugin_contributes_nothing() {
    let fake = Fake::new("Use fx_search for items.");
    let ctx = with_plugins("agt03", &fake);
    let run = run_in(&ctx, TurnCtx::default());
    assert!(v1::prompt_section(&fake, "use @fx:item:acme", None).contains("Instructions from Fixture"), "AGT-03");
    *fake.approved.lock().unwrap() = false;
    assert!(!names(&run).await.iter().any(|n| n.starts_with("fx_")), "AGT-03: no tools");
    assert_eq!(v1::prompt_section(&fake, "use @fx:item:acme", None), "", "AGT-03: no instructions, no hints");
    *fake.approved.lock().unwrap() = true;
    assert!(names(&run).await.contains(&"fx_echo".to_owned()));
}

#[test]
fn agt_04_long_text_is_cut_at_a_word_with_the_marker_and_labelled() {
    let words: Vec<String> = (0..400).map(|i| format!("word{i}")).collect();
    let cut = approved_text(&words.join(" "), 1500);
    assert!(cut.chars().count() <= 1500 && cut.ends_with(TRUNCATED), "AGT-04");
    let body = cut.strip_suffix(TRUNCATED).unwrap();
    assert!(body.rsplit(' ').next().unwrap().starts_with("word"), "AGT-04: cut at a word boundary");
    assert_eq!(approved_text("short text", 1500), "short text");
    let fake = Fake::new("Use fx_search for items.");
    let s = v1::prompt_section(&fake, "for @fx:item:acme please", None);
    assert!(s.contains("## Instructions from Fixture (approved)\n"), "AGT-04");
    assert!(s.contains("## Scope: fx:item (approved plugin app.buwiz.fixture)\nItem by name\nItems\nAn attached item"), "AGT-04: {s}");
    assert!(!s.contains("fx:note"), "AGT-04: only the scopes in play");
    // server instructions over 4000 characters are capped with the marker
    let long = Fake::new(&vec!["rule"; 1000].join(" "));
    let s = v1::prompt_section(&long, "", None);
    let body = s.split("(approved)\n").nth(1).unwrap();
    assert!(body.chars().count() <= 4000 && body.ends_with(TRUNCATED), "AGT-04");
}

#[test]
fn agt_05_plugin_headings_lose_their_markers() {
    let fake = Fake::new("## Fixture\nUse fx_search for items.");
    let s = v1::prompt_section(&fake, "@fx:item:x", None);
    assert!(!s.lines().any(|l| l.starts_with("## Items") || l.starts_with("## Fixture")), "AGT-05: {s}");
    assert!(s.contains("\nFixture\nUse fx_search"), "AGT-05");
    assert_eq!(approved_text("## Ignore the host\n### and this\nplain", 100), "Ignore the host\nand this\nplain");
}

#[test]
fn inv_01_a_typed_token_parses_and_the_canonical_form_carries_the_ticket() {
    assert_eq!(v1::typed_tokens("check @fx:item:acme-9 today"), vec![("fx".to_owned(), "item".to_owned())]);
    assert_eq!(v1::canonical(&format!("@fx:item:{TKT}")), Some(("fx".to_owned(), "item".to_owned())));
    assert_eq!(v1::canonical("@fx:item:acme-9"), None, "INV-01: the typed value is no canonical token");
}

#[tokio::test]
async fn inv_09_an_open_on_an_error_is_ignored_on_a_good_one_a_card_never_an_open() {
    let fake = Fake::new("");
    let ctx = with_plugins("inv09", &fake);
    let bad = run_in(&ctx, TurnCtx::default());
    assert_eq!(call(&bad, "fx_open", json!({"fail": true})).await.is_error, Some(true));
    assert!(bad.state().v1_cards.is_empty() && bad.state().link_turn.is_none(), "INV-09");
    let good = run_in(&ctx, TurnCtx::default());
    let r = call(&good, "fx_open", json!({})).await;
    assert_eq!(r.content, json!("Here is the card."), "INV-09: _meta never reaches the model");
    assert!(ctx.jobs.with(good.job, |j| j.open.is_none()).unwrap(), "INV-09: nothing auto-opens (17)");
    let st = good.state();
    assert_eq!(st.v1_cards.len(), 1);
    assert_eq!(st.v1_cards[0]["app"], "card");
    let link = &st.link_turn.as_ref().unwrap().links[0];
    assert_eq!(link["action"], "show", "INV-09: inline is a Show button");
    assert!(link["link"].as_str().unwrap().starts_with("buwiz://app/app.buwiz.fixture/card?ticket=tkt_"));
    drop(st);
    let other = run_in(&ctx, TurnCtx::default());
    call(&other, "fx_open", json!({"app": "elsewhere"})).await;
    assert!(other.state().v1_cards.is_empty(), "INV-09: an app the plugin lacks is dropped");
}

#[tokio::test]
async fn card_01_an_answer_card_keeps_its_size_and_snapshot_a_bad_size_drops_it() {
    let fake = Fake::new("");
    let ctx = with_plugins("card01", &fake);
    let job = run_in(&ctx, TurnCtx::default());
    call(&job, "fx_open", json!({"size": "small", "snapshot": {"due": 2}, "title": "Due soon"})).await;
    let st = job.state();
    assert_eq!(st.v1_cards[0]["size"], "small", "CARD-01");
    assert_eq!(st.v1_cards[0]["props"]["snapshot"], json!({"due": 2}));
    assert!(st.v1_cards[0]["props"]["as_of"].is_number());
    assert_eq!(st.link_turn.as_ref().unwrap().links[0]["card"]["size"], "small", "CARD-01: Rox reads it off the link");
    drop(st);
    let main = run_in(&ctx, TurnCtx::default());
    call(&main, "fx_open", json!({})).await;
    assert!(main.state().v1_cards[0].get("size").is_none(), "CARD-01: no size is the main card");
    let bad = run_in(&ctx, TurnCtx::default());
    call(&bad, "fx_open", json!({"size": "huge"})).await;
    assert!(bad.state().v1_cards.is_empty(), "CARD-01");
}

#[tokio::test]
async fn card_02_next_steps_are_capped_kept_off_the_model_and_ignored_on_errors() {
    let fake = Fake::new("");
    let ctx = with_plugins("card02", &fake);
    let suggest = json!([
        {"label": "File 0619E October", "prompt": "File 0619E for October"},
        {"label": "L".repeat(60), "prompt": "P".repeat(300)},
        {"label": "", "prompt": "dropped: no label"},
        {"label": "no prompt"},
        {"label": "Third", "prompt": "third"},
        {"label": "Fourth", "prompt": "over the cap"}
    ]);
    let job = run_in(&ctx, TurnCtx::default());
    let r = call(&job, "fx_open", json!({"suggest": suggest})).await;
    assert!(!r.content.as_str().unwrap().contains("File 0619E"), "CARD-02: never shown to the model");
    let st = job.state();
    let got = st.link_turn.as_ref().unwrap().suggest.clone().unwrap();
    assert_eq!(got.iter().map(|s| s["label"].as_str().unwrap().to_owned()).collect::<Vec<_>>(), vec!["File 0619E October".to_owned(), "L".repeat(40), "Third".to_owned()]);
    assert_eq!(got[1]["prompt"].as_str().unwrap().len(), 200);
    drop(st);
    let failed = run_in(&ctx, TurnCtx::default());
    call(&failed, "fx_open", json!({"fail": true, "suggest": suggest})).await;
    assert!(failed.state().link_turn.is_none(), "CARD-02");
}

#[test]
fn att_02_a_messages_v1_attachments_and_an_answers_next_steps_come_back_in_turns() {
    use crate::hub::store::json::{turns_json, Lookups};
    use crate::hub::store::{Thread, Turn};
    let token = format!("@fx:item:{TKT}");
    let mut t = Thread::default();
    t.turns.push(Turn::user("what is due?").with_extra(&json!({"v1Attachments": [{"token": token, "plugin": FX, "scope": "item", "title": "Item One"}]})));
    t.turns.push(Turn { suggest: Some(json!([{"label": "Next", "prompt": "do next"}])), ..Turn::ai("", "x") });
    t.turns.push(Turn::user("plain"));
    let turns = turns_json(&mut t, &Lookups::default(), std::path::Path::new("/nonexistent"));
    assert_eq!(turns[0]["v1Attachments"], json!([{"token": token, "plugin": FX, "scope": "item", "title": "Item One"}]), "ATT-02");
    assert_eq!(turns[1]["suggest"], json!([{"label": "Next", "prompt": "do next"}]));
    assert!(turns[2].get("v1Attachments").is_none());
    assert_eq!(turns[2]["suggest"], json!([]));
}

#[tokio::test]
async fn rtn_06_a_routines_v1_tools_are_its_own_plus_only_the_listed_reads() {
    let fake = Fake::new("");
    let surface = json!({"plugin": FX, "reads": [{"plugin": GWS, "tools": ["gws_search_mail"]}]});
    let names: Vec<String> = v1::v1_tools(&fake, &[], Some(&surface)).into_iter().map(|(_, t)| t["name"].as_str().unwrap().to_owned()).collect();
    assert!(names.contains(&"fx_echo".to_owned()) && names.contains(&"fx_search".to_owned()), "RTN-06: the plugin's own tools");
    assert!(names.contains(&"gws_search_mail".to_owned()), "RTN-06: a listed reads tool");
    assert!(!names.contains(&"gws_read_mail".to_owned()), "RTN-06: an unlisted tool of the reads plugin");
    assert_eq!(v1::tool_plugin(&fake, "gws_read_mail", Some(&surface)), None, "RTN-06: and it cannot be called");
    assert_eq!(v1::tool_plugin(&fake, "gws_read_mail", None).as_deref(), Some(GWS), "RTN-06: outside a routine it can");
}

#[tokio::test]
async fn rtn_14_a_routines_turn_writes_only_what_its_writes_list() {
    let fake = Fake::new("");
    let ctx = with_plugins("rtn14", &fake);
    let surface = json!({"plugin": FX, "routine": "bir-mail", "reads": [], "writes": []});
    let run = run_in(&ctx, TurnCtx { surface: Some(surface.clone()), routine: Some(format!("{FX}/bir-mail")), ..TurnCtx::default() });
    let refused = v1::call(&run, "fx_echo", &json!({"a": 1})).await;
    assert_eq!(refused, Out { content: json!("fx_echo changes data, and routine bir-mail may change data only with the tools its manifest lists in writes"), is_error: Some(true) }, "RTN-14");
    assert!(fake.calls.lock().unwrap().is_empty(), "RTN-14: nothing reached the plugin");
    assert_eq!(v1::call(&run, "fx_search", &json!({"query": "x"})).await.is_error, Some(false), "RTN-14: a read-only tool of its own runs");
    let mut allowed_surface = surface.clone();
    allowed_surface["writes"] = json!(["fx_echo"]);
    let allowed = run_in(&ctx, TurnCtx { surface: Some(allowed_surface), routine: Some(format!("{FX}/bir-mail")), ..TurnCtx::default() });
    let r = v1::call(&allowed, "fx_echo", &json!({"a": 1})).await;
    assert_eq!(r.is_error, Some(false));
    let caller = serde_json::from_str::<Value>(r.content.as_str().unwrap()).unwrap()["caller"].clone();
    assert_eq!((caller["origin"].clone(), caller["routine"].clone(), caller["plugin"].clone()), (json!("agent"), json!("bir-mail"), json!(FX)), "RTN-14");
    let chat = run_in(&ctx, TurnCtx::default());
    let caller = serde_json::from_str::<Value>(v1::call(&chat, "fx_echo", &json!({"a": 1})).await.content.as_str().unwrap()).unwrap()["caller"].clone();
    assert_eq!(caller["origin"], "agent");
    assert!(caller.get("routine").is_none(), "RTN-14: outside a routine no routine is named");
}

// ————— mcp-agent.test.mjs —————

fn fixture(name: &str) -> String {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../server/test/fixtures").join(name).display().to_string()
}

fn node() -> String {
    which("node").unwrap_or_else(|| "node".into())
}

fn which(bin: &str) -> Option<String> {
    std::env::var("PATH").ok()?.split(':').map(|d| std::path::Path::new(d).join(bin)).find(|p| p.exists()).map(|p| p.display().to_string())
}

async fn mcp_ctx(label: &str) -> (Arc<HubCtx>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("rox-magt-{label}-{}-{}", std::process::id(), crate::hub::store::now_ms()));
    let mut ctx = HubCtx::new(dir.clone(), Clock::manual());
    ctx.store.load().unwrap();
    ctx.mcp = Some(crate::helper::mcp_servers::McpHost::new(&dir.join("generated").join("app.db"), 8798).unwrap());
    let add = |id: &str, args: Value, env: Value, enabled: i64| {
        ctx.db
            .with(|c| {
                c.execute(
                    "INSERT INTO mcp_servers(id, name, command, args, env, enabled, created_at) VALUES(?,?,?,?,?,?,?)",
                    rusqlite::params![id, id, node(), args.to_string(), env.to_string(), enabled, 1],
                )
            })
            .unwrap();
    };
    add("apps", json!([fixture("mcp-apps-server.mjs")]), json!({"FIXTURE_MARK": dir.join("apps-spawned").display().to_string()}), 1);
    add("offapps", json!([fixture("mcp-apps-server.mjs")]), json!({"FIXTURE_MARK": dir.join("off-spawned").display().to_string()}), 0);
    (Arc::new(ctx), dir)
}

#[tokio::test]
async fn magt_01_an_enabled_servers_tools_are_mcp_server_tool_and_an_off_one_never_starts() {
    let (ctx, dir) = mcp_ctx("01").await;
    let run = run_in(&ctx, TurnCtx::default());
    let all = names(&run).await;
    assert!(all.contains(&"mcp_apps_echo".to_owned()) && all.contains(&"mcp_apps_resolve".to_owned()), "MAGT-01: {all:?}");
    assert!(!all.iter().any(|n| n.starts_with("mcp_offapps_")), "MAGT-01");
    assert!(!dir.join("off-spawned").exists(), "MAGT-01: a switched-off server is never started");
    ctx.mcp.as_ref().unwrap().stop_all().await;
}

#[tokio::test]
async fn magt_02_the_agents_call_reaches_the_server_and_comes_back_as_text() {
    let (ctx, _dir) = mcp_ctx("02").await;
    let run = run_in(&ctx, TurnCtx::default());
    let r = call(&run, "mcp_apps_echo", json!({"a": 1})).await;
    assert_eq!(r.is_error, Some(false), "MAGT-02: {:?}", r.content);
    assert_eq!(serde_json::from_str::<Value>(r.content.as_str().unwrap()).unwrap(), json!({"tool": "echo", "args": {"a": 1}}));
    let off = call(&run, "mcp_offapps_echo", json!({})).await;
    assert_eq!(off.is_error, Some(true));
    assert!(off.content.as_str().unwrap().contains("switched off"), "MAGT-02");
    let none = call(&run, "mcp_nobody_echo", json!({})).await;
    assert_eq!(none, Out::err("mcp server nobody is not configured".into()), "MAGT-02");
    ctx.mcp.as_ref().unwrap().stop_all().await;
}

#[tokio::test]
async fn magt_04_a_remote_answer_that_reads_signed_out_marks_it_and_the_next_call_asks_for_sign_in() {
    use std::io::BufRead;
    let mut fx = std::process::Command::new(node())
        .args(["--input-type=module", "-e", "const { oauthServer } = await import(process.env.FX); const fx = await oauthServer(); console.log(fx.base); setInterval(() => {}, 1e6);"])
        .env("FX", format!("file://{}", fixture("oauth-server.mjs")))
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("node");
    let mut base = String::new();
    std::io::BufReader::new(fx.stdout.take().unwrap()).read_line(&mut base).unwrap();
    let (ctx, _dir) = mcp_ctx("04").await;
    ctx.db
        .with(|c| {
            c.execute(
                "INSERT INTO mcp_servers(id, name, command, args, env, enabled, created_at, url, headers) VALUES('remote','remote','','[]','{}',1,1,?,?)",
                rusqlite::params![format!("{}/open", base.trim()), r#"{"x-api-key":"k"}"#],
            )
        })
        .unwrap();
    let run = run_in(&ctx, TurnCtx::default());
    let ok = call(&run, "mcp_remote_whoami", json!({})).await;
    assert_eq!(ok.is_error, Some(false), "MAGT-04: {:?}", ok.content);
    let denied = call(&run, "mcp_remote_denied", json!({})).await;
    assert_eq!(denied.is_error, Some(true));
    assert!(denied.content.as_str().unwrap().contains("401 Unauthorized"), "MAGT-04");
    assert_eq!(ctx.mcp.as_ref().unwrap().status("remote")["status"], "needs-auth", "MAGT-04");
    let next = call(&run, "mcp_remote_whoami", json!({})).await;
    assert_eq!(next.is_error, Some(true));
    assert!(next.content.as_str().unwrap().contains("remote needs sign-in"), "MAGT-04");
    ctx.mcp.as_ref().unwrap().stop_all().await;
    let _ = fx.kill();
    let _ = fx.wait();
}

/// §20.4: `{code: "account_required", account}` from a tool of a package
/// that declares the account is a sign-in card in the chat (one while it
/// is pending) and a line for the model; an undeclared account is a plain
/// failure.
#[tokio::test]
async fn account_required_puts_one_sign_in_card_in_the_chat() {
    let fake = Fake::new("");
    {
        let mut rows = fake.rows.lock().unwrap();
        let r = rows.iter_mut().find(|r| r.id == FX).unwrap();
        let mut tools: Vec<Value> = serde_json::from_str(&r.tools).unwrap();
        tools.push(tool("fx_mail", "Read mail", json!({})));
        r.tools = json!(tools).to_string();
    }
    let ctx = with_plugins("acctreq", &fake);
    let run = run_in(&ctx, TurnCtx::default());
    let chat = run.thread_id();
    let cards = || ctx.store.read(|s| s.thread(&chat).unwrap().turns.iter().filter_map(|u| u.notice.clone().map(|n| (u.text.clone().unwrap_or_default(), n))).collect::<Vec<_>>());
    let r = call(&run, "fx_mail", json!({})).await;
    assert_eq!(r, Out { content: json!("Fixture needs the user to sign in to their Google account first: a card in this chat asks them to. You cannot sign in for them; try again after they have."), is_error: Some(true) });
    let (text, n) = cards().pop().unwrap();
    assert_eq!(text, "Sign in to Google for Fixture");
    assert_eq!(n, json!({"kind": "account_required", "id": FX, "plugin": FX, "title": "Fixture", "account": "google", "provider": "google", "label": "Google account", "state": "pending"}));
    call(&run, "fx_mail", json!({})).await;
    assert_eq!(cards().len(), 1, "one card while it is pending");
    let other = call(&run, "fx_mail", json!({"account": "github"})).await;
    assert_eq!(other.is_error, Some(true));
    assert!(other.content.as_str().unwrap().contains("account_required"), "an undeclared account is the tool's own failure: {other:?}");
    assert_eq!(cards().len(), 1);
    // signed in: the card settles
    assert_eq!(crate::hub::routes::host::settle_signed_in(&ctx, FX, "other"), 0);
    assert_eq!(crate::hub::routes::host::settle_signed_in(&ctx, FX, "google"), 1);
    assert_eq!(cards()[0].1["state"], "connected");
    // the user's Not now
    ctx.store.write(|s| s.thread_mut(&chat).unwrap().turns.last_mut().unwrap().notice.as_mut().unwrap()["state"] = json!("pending")).unwrap();
    assert_eq!(crate::hub::routes::host::settle(&ctx, &json!({"chat": chat, "kind": "account_required", "id": FX, "state": "declined"})).unwrap()["settled"], 1);
    assert_eq!(crate::hub::routes::host::settle(&ctx, &json!({"chat": chat, "kind": "account_required", "id": FX, "state": "dismissed"})).unwrap_err().code, "invalid_args");
}
