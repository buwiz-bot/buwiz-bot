//! `_meta["buwiz/account_use"]` through a real call: an app's `buwiz.call`,
//! the signed envelope, the fake broker's result, the open's checks.

use super::*;

const TOOLS: [&str; 4] = ["fx_account_use", "fx_account_fail", "fx_account_read", "fx_echo"];
const USE: &str = "/v1/host/plugins/app.buwiz.fixture/accounts/google/use";

/// An open whose app calls `tool` with `args` as it mounts.
fn open(first_party: bool, agent: bool, tool: &str, args: Value) -> Open {
    let script = format!("return await buwiz.call({tool:?}, {args});");
    Open::start_with(FIXTURE, json!({"tools": TOOLS}), &script, Limits::default(), move |ui| {
        ui.first_party.store(first_party, Ordering::SeqCst);
        ui.agent.store(agent, Ordering::SeqCst);
    })
}

fn work() -> Value {
    json!({"field": {"account": "google", "label": "work"}})
}

fn switched(o: &Open) -> Vec<(String, Value)> {
    o.broker.switched.lock().unwrap().clone()
}

#[test]
fn the_persons_press_in_a_first_party_app_switches_and_the_app_gets_only_its_answer() {
    let mut o = open(true, false, "fx_account_use", work());
    assert_eq!(o.wait("{"), r#"{"switched":true}"#);
    assert_eq!(switched(&o), [(USE.to_owned(), json!({"label": "work"}))]);
    assert_eq!(o.broker.calls.lock().unwrap()[0].0["actor"], "person");
}

#[test]
fn the_ais_press_on_a_person_only_tool_is_refused_before_anything_is_sent() {
    let mut o = open(true, true, "fx_account_use", work());
    let said = o.wait("threw");
    assert!(said.starts_with("threw permission_denied: Only a person can do this in Fixture"), "{said}");
    assert!(o.broker.calls.lock().unwrap().is_empty());
    assert!(switched(&o).is_empty());
}

/// A plugin's own word got the call past the press check, as a read: the
/// switch is still the person's.
#[test]
fn a_call_of_the_ais_that_got_through_switches_nothing() {
    let mut o = open(true, true, "fx_account_read", work());
    let said = o.wait("threw");
    assert!(said.starts_with("threw permission_denied: Only a person can do this in Fixture"), "{said}");
    assert_eq!(o.broker.calls.lock().unwrap()[0].0["actor"], "agent");
    assert!(switched(&o).is_empty());
}

#[test]
fn a_third_party_plugins_field_switches_nothing() {
    let mut o = open(false, false, "fx_account_use", work());
    assert_eq!(o.wait("threw"), "threw permission_denied: Only a first-party plugin can switch accounts.");
    assert!(switched(&o).is_empty());
}

#[test]
fn a_field_that_names_no_account_of_the_grammar_is_refused() {
    let fields = [json!({"account": "google"}), json!({"label": "work"}), json!("work"), json!({"account": "google", "label": "a b"}), json!({"account": "../x", "label": "work"})];
    for field in fields {
        let mut o = open(true, false, "fx_account_use", json!({"field": field}));
        assert!(o.wait("threw").starts_with("threw invalid_args"), "{field}");
        assert!(switched(&o).is_empty(), "{field}");
    }
}

#[test]
fn a_failed_tool_switches_nothing() {
    let mut o = open(true, false, "fx_account_fail", work());
    assert!(o.wait("threw").starts_with("threw tool_error"));
    assert!(switched(&o).is_empty());
}

#[test]
fn an_older_plugin_with_no_field_changes_nothing() {
    let mut o = open(true, false, "fx_echo", json!({"a": 1}));
    assert_eq!(o.wait("{"), r#"{"a":1}"#);
    assert!(switched(&o).is_empty());
}

/// The broker's sentence names labels, and what an app shows can reach the
/// model: only the code gets through.
#[test]
fn a_refused_switch_says_its_code_and_no_label() {
    let mut o = open(true, false, "fx_account_use", json!({"field": {"account": "google", "label": "gone"}}));
    assert_eq!(o.wait("threw"), "threw needs_signin: Buwiz Bot couldn't switch to that account.");
}

#[test]
fn a_result_after_the_screen_closed_switches_nothing() {
    let mut o = open(true, false, "fx_account_use", json!({"field": {"account": "google", "label": "work"}, "ms": 300}));
    let deadline = Instant::now() + Duration::from_secs(60);
    while o.broker.running.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the call never reached the broker");
        std::thread::yield_now();
    }
    o.ctx.close();
    o.wait("threw");
    assert!(switched(&o).is_empty());
}
