//! The composer's `/` popup through the real composer path on a headless engine: a command
//! typed mid-name (`/uptime` for a plugin's `bir-uptime`) is offered after the commands that
//! start with it, and is never taken by Enter or Tab unless the person selected it.

use std::sync::Arc;

use nsh_core::element::Root;
use nsh_core::platform::HttpRequest;
use nsh_core::prelude::*;
use nsh_engine::{App, EngineConfig, FetchEvents, FetchTask, Network};
use serde_json::json;

use buwiz_office_kit::testing::{Headless, run};

use crate::live::store;
use crate::ui_test_support::{Stage, finish, on, wait_until};

fn in_child(name: &str) -> bool {
    crate::ui_test_support::in_child("slash_menu_ui_tests", name)
}

/// A stand-in broker: the servers' manifest lists none; everything else is `ok`.
struct Broker;

impl Network for Broker {
    fn fetch(&self, request: HttpRequest, events: FetchEvents) -> Box<dyn FetchTask> {
        let path = request.url.strip_prefix(crate::client::backend()).unwrap_or(&request.url).to_owned();
        let answer = match path.as_str() {
            "/mcp/manifest" => json!({"servers": []}),
            _ => json!({"ok": true}),
        };
        finish(events, answer)
    }
}

fn start() -> Headless {
    let mut config = EngineConfig::new(App::Rust(Root::new::<Stage>(|| ())));
    config.viewport = nsh_core::geometry::Size::new(px(1000.0), px(800.0));
    config.network = Some(Arc::new(Broker));
    let mut app = run(config);
    on(&mut app, crate::slash::boot);
    wait_until(&mut app, "the manifest", || store().mcp_loaded.get());
    app
}

/// A plugin whose commands are `bir-uptime` and `home-menu`.
fn install_plugin(app: &mut Headless) {
    on(app, || {
        let p = crate::mini_v1::load::PluginEntry {
            id: "app.buwiz.ebir".to_owned(),
            short: "ebir".to_owned(),
            title: "eBIR".to_owned(),
            enabled: true,
            commands: vec![json!({"name": "bir-uptime", "description": "Is the BIR up"}), json!({"name": "home-menu"})],
            ..Default::default()
        };
        store().v1.entries.set(vec![p]);
    });
}

/// The popup's rows once `typed` is in the box.
fn menu(app: &mut Headless, typed: &'static str) -> Vec<String> {
    on(app, move || crate::composer::draft_changed(typed));
    on(app, || store().pop_rows.get_untracked().into_iter().map(|r| r.label).collect())
}

/// `/uptime` offers the plugin's `bir-uptime`; `/bir` offers it as a start match; one character
/// offers what it always did; start matches come before mid-name ones.
#[test]
fn the_slash_popup_offers_a_command_mid_name_after_the_ones_that_start_with_it() {
    if !in_child("the_slash_popup_offers_a_command_mid_name_after_the_ones_that_start_with_it") {
        return;
    }
    let mut app = start();
    install_plugin(&mut app);
    assert_eq!(menu(&mut app, "/uptime"), ["/bir-uptime"]);
    assert_eq!(menu(&mut app, "/UPTIME"), ["/bir-uptime"], "case is ignored");
    assert_eq!(menu(&mut app, "/bir"), ["/bir-uptime"], "a start match");
    assert_eq!(menu(&mut app, "/me"), ["/messages", "/bir-uptime", "/home-menu"], "the start match first, the rest in their order");
    assert!(menu(&mut app, "/u").is_empty(), "one character: starts only, as before");
    assert_eq!(menu(&mut app, "/s"), ["/status"]);
    assert!(menu(&mut app, "/zz").is_empty());
    assert_eq!(menu(&mut app, "/").len(), 9, "nothing typed: every command");
}

/// Whether the keymap binds `keys` now.
fn binds(app: &mut Headless, keys: &'static str) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.key(keys))).is_ok()
}

/// The draft now.
fn draft(app: &mut Headless) -> String {
    on(app, || store().draft.get_untracked())
}

/// With only mid-name matches, nothing is selected: Enter and Tab are left to the field (Enter
/// sends what was typed, as it did when the popup was empty), and nothing is completed.
#[test]
fn enter_and_tab_leave_a_popup_of_mid_name_matches_alone() {
    if !in_child("enter_and_tab_leave_a_popup_of_mid_name_matches_alone") {
        return;
    }
    let mut app = start();
    install_plugin(&mut app);
    for typed in ["/uptime", "/at"] {
        let rows = menu(&mut app, typed);
        assert!(!rows.is_empty(), "{typed} lists mid-name rows: {rows:?}");
        assert!(!binds(&mut app, "enter"), "{typed}: the popup takes Enter");
        assert!(!binds(&mut app, "tab"), "{typed}: the popup takes Tab");
        assert_eq!(draft(&mut app), typed, "nothing was completed");
    }
    // an empty popup is the same to the keys
    assert!(menu(&mut app, "/zz").is_empty());
    assert!(!binds(&mut app, "enter"));
    // with a start match the first row is selected, as before
    menu(&mut app, "/bir");
    assert!(binds(&mut app, "enter"));
    assert_eq!(draft(&mut app), "/bir-uptime ", "Enter completes a start match");
}

/// The person picks a mid-name row deliberately: an arrow key selects it, then Enter completes
/// the whole command; so does a click (which runs `slash::pick`).
#[test]
fn a_mid_name_row_is_picked_with_the_arrow_keys_or_a_click() {
    if !in_child("a_mid_name_row_is_picked_with_the_arrow_keys_or_a_click") {
        return;
    }
    let mut app = start();
    install_plugin(&mut app);
    assert_eq!(menu(&mut app, "/uptime"), ["/bir-uptime"]);
    assert!(binds(&mut app, "down"), "the arrow keys work on the popup");
    assert!(binds(&mut app, "enter"), "a row is selected now");
    assert_eq!(draft(&mut app), "/bir-uptime ");
    // a click on the row
    assert_eq!(menu(&mut app, "/uptime"), ["/bir-uptime"]);
    let row = on(&mut app, || store().pop_rows.get_untracked().remove(0));
    on(&mut app, move || crate::slash::pick(&row));
    assert_eq!(draft(&mut app), "/bir-uptime ");
    // typing `/uptime` by hand is no command, as before
    assert!(!on(&mut app, || crate::slash::dispatch("/uptime", true)));
}
