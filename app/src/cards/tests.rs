//! The wire parser and the reply rules against nativechat's recorded
//! frames (`fixtures/wire`). Skipped when that checkout is not there.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::approval::{self, ApprovalSpec, Resolution};
use super::choice::{self, FormSpec};
use super::plugin_needs::{PluginNeedKind, PluginNeedsSpec};
use super::transport::{self, Sent, UserFormReply};
use super::{Kind, key_of, kind_of};

const WIRE: &str = "/Volumes/goldcoders/OSS/nativechat/fixtures/wire";

/// Every JSON file under `WIRE/<dir>`, or `None` when the fixtures are
/// not on this machine.
fn fixtures(dir: &str) -> Option<Vec<(PathBuf, Value)>> {
    let root = Path::new(WIRE).join(dir);
    if !root.is_dir() {
        eprintln!("skipped: no {}", root.display());
        return None;
    }
    let mut out: Vec<(PathBuf, Value)> = std::fs::read_dir(&root)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            let v = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            (p, v)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Some(out)
}

fn fixture(path: &str) -> Option<Value> {
    let p = Path::new(WIRE).join(path);
    let raw = std::fs::read_to_string(&p).ok().or_else(|| {
        eprintln!("skipped: no {}", p.display());
        None
    })?;
    Some(serde_json::from_str(&raw).unwrap())
}

#[test]
fn every_awaiting_frame_is_an_approval_or_a_user_form() {
    let Some(frames) = fixtures("agui/custom/run-awaiting-approval") else { return };
    assert!(!frames.is_empty());
    for (path, v) in frames {
        let reason = v["reason"].as_str().unwrap_or("exec-consent");
        let kind = kind_of(&v);
        if reason == "user-form" {
            assert_eq!(kind, Some(Kind::UserForm), "{}", path.display());
            assert!(v["entryId"].is_string() && key_of(&v, Kind::UserForm) == v["entryId"].as_str().unwrap());
        } else {
            assert_eq!(kind, Some(Kind::Approval), "{}", path.display());
            let spec = ApprovalSpec::from_wire(&v).expect("an approval");
            assert_eq!(spec.call_id, v["callId"].as_str().unwrap());
            assert_eq!(spec.run_id, v["runId"].as_str().unwrap());
            assert_eq!(key_of(&v, Kind::Approval), spec.call_id);
        }
    }
}

#[test]
fn a_routine_delete_is_titled_by_the_servers_question() {
    let Some(v) = fixture("agui/custom/run-awaiting-approval/a_delete_asks_first_naming_the_routine_as_stored.json")
    else {
        return;
    };
    let spec = ApprovalSpec::from_wire(&v).unwrap();
    assert_eq!(spec.reason, "policy-approval");
    assert!(!spec.is_review_an_action() && !spec.runs_on_this_mac());
    assert_eq!(
        approval::approval_title(&spec, "Hex", false),
        "Delete the routine \"Weekly report\"? It stops for good."
    );
    assert!(approval::card_lines(&spec, false).is_empty());
    assert_eq!(approval::arguments_box(&spec), None);
    assert_eq!(spec.outcome("Hex", Resolution::DenyOnce), "Hex was not allowed to run commands on its computer.");
}

#[test]
fn the_tunnel_card_reviews_and_keeps_the_computers_choice() {
    let Some(v) = fixture("agui/custom/run-awaiting-approval/bypass_skips_the_card_and_a_deny_does_not_consent_the_run.json")
    else {
        return;
    };
    let spec = ApprovalSpec::from_wire(&v).unwrap();
    assert!(spec.is_review_an_action() && spec.is_egress_tunnel());
    assert_eq!(approval::approval_title(&spec, "Hex", true), "Review an action");
    assert_eq!(
        approval::card_lines(&spec, true),
        vec![approval::EGRESS_TUNNEL_ASK_REASON.to_owned(), "Click at (120, 40) on the agent's own screen".to_owned()]
    );
    assert_eq!(spec.egress_mode(Resolution::Always), Some("bypass"));
    assert_eq!(spec.egress_mode(Resolution::Never), Some("never"));
    assert_eq!(spec.egress_mode(Resolution::AllowOnce), None);
    assert_eq!(spec.outcome("Hex", Resolution::Always), "Hex may use your network from its computer.");
}

#[test]
fn a_policy_shell_card_shows_its_command() {
    let Some(v) =
        fixture("agui/custom/run-awaiting-approval/a_card_approved_resume_keeps_the_ceiling_it_started_under.json")
    else {
        return;
    };
    let spec = ApprovalSpec::from_wire(&v).unwrap();
    assert_eq!(spec.tool, "shell");
    assert_eq!(approval::approval_title(&spec, "Hex", false), "Allow Hex to run a command on its computer?");
    if spec.summary.is_empty() {
        assert_eq!(approval::arguments_box(&spec), Some(spec.command.clone()));
    }
}

#[test]
fn a_local_shell_card_keeps_a_rule_for_its_command() {
    let spec = ApprovalSpec::from_wire(&json!({
        "type": "CUSTOM", "name": "run-awaiting-approval", "runId": "run_1", "callId": "call_1",
        "tool": "user_machine_shell", "arguments": {"command": "ls -la"}
    }))
    .unwrap();
    assert_eq!(spec.reason, "exec-consent");
    assert!(spec.runs_on_this_mac() && !spec.is_review_an_action());
    assert_eq!(spec.command, "ls -la");
    assert_eq!(approval::arguments_box(&spec).as_deref(), Some("ls -la"));
    assert_eq!(
        approval::approval_title(&spec, "Hex", false),
        "Allow Hex to run this command on your local computer?"
    );
    assert_eq!(spec.outcome("Hex", Resolution::Always), "Hex can run this command on your computer without asking.");
    assert_eq!(
        transport::rule_body("allow", &spec.command),
        json!({"machineId": transport::machine_id(), "kind": "allow", "pattern": "ls -la"})
    );
    assert_eq!(transport::answer_body("call_1", true), json!({"call_id": "call_1", "approved": true}));
}

#[test]
fn the_plugin_needs_frame_lists_an_install_and_a_choice() {
    let Some(frames) = fixtures("agui/custom/opengrok.pluginNeeds") else { return };
    for (path, v) in frames {
        assert_eq!(kind_of(&v), Some(Kind::PluginNeeds), "{}", path.display());
        let spec = PluginNeedsSpec::from_wire(&v).expect("needs");
        assert!(spec.send_again);
        assert_eq!(spec.needs[0].kind, PluginNeedKind::Install);
        assert_eq!(spec.needs[0].plugin, "nope");
        let PluginNeedKind::Choose(accounts) = &spec.needs[1].kind else { panic!("{:?}", spec.needs[1]) };
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].1, "Demo");
        assert_eq!(accounts[0].2, "token");
    }
    let unknown = json!({"type": "CUSTOM", "name": "opengrok.pluginNeeds", "value": {"needs": [{"plugin": "x", "need": "telepathy"}]}});
    assert_eq!(PluginNeedsSpec::from_wire(&unknown), None);
}

#[test]
fn the_offer_to_save_a_login_is_its_own_card() {
    let Some(frames) = fixtures("agui/custom/credential.offer_save") else { return };
    for (path, v) in frames {
        assert_eq!(kind_of(&v), Some(Kind::SaveLogin), "{}", path.display());
        let key = key_of(&v, Kind::SaveLogin);
        assert_eq!(key, format!("save-login-{}", v["formEntryId"].as_str().unwrap()));
        assert!(!v.to_string().contains("password"), "the frame never carries one");
    }
}

#[test]
fn a_replayed_user_form_is_a_settled_user_form_card() {
    let Some(frames) = fixtures("agui/custom/user-form") else { return };
    assert!(!frames.is_empty());
    for (path, v) in frames {
        assert_eq!(kind_of(&v), Some(Kind::UserForm), "{}", path.display());
        assert_eq!(key_of(&v, Kind::UserForm), v["entryId"].as_str().unwrap(), "{}", path.display());
        // The envelope alone (the CUSTOM's `value`) reads the same.
        assert_eq!(kind_of(&v["value"]), Some(Kind::UserForm), "{}", path.display());
        assert_eq!(key_of(&v["value"], Kind::UserForm), v["entryId"].as_str().unwrap());
    }
}

#[test]
fn frames_that_paint_nothing_are_not_cards() {
    for dir in ["agui/custom/run-timing", "agui/custom/pending-user-message", "agui/TEXT_MESSAGE_CONTENT", "agui/TOOL_CALL_START"]
    {
        let Some(frames) = fixtures(dir) else { return };
        for (path, v) in frames {
            assert_eq!(kind_of(&v), None, "{}", path.display());
        }
    }
}

/// The `form` frontend tool (no recorded frame: nativechat declares it
/// client-side, `agui_tools`).
#[test]
fn a_form_tool_call_is_a_choice_card_answered_in_its_own_words() {
    let call = json!({"tool": "form", "toolCallId": "tc_1", "arguments": {
        "title": "Trip", "prompt": "Pick one",
        "fields": [{"id": "where", "label": "Where", "options": ["Lisbon", "Oslo"]}]
    }});
    assert_eq!(kind_of(&call), Some(Kind::Choice));
    assert_eq!(key_of(&call, Kind::Choice), "tc_1");
    let Some(super::Card::Choice(spec)) = super::Card::from_wire(&call) else { panic!() };
    assert!(spec.answers_on_pick());
    let picks = [("where".to_owned(), "Oslo".to_owned())].into();
    assert_eq!(spec.answer_text(&picks), "Trip\nWhere: Oslo");

    // Arguments as JSON text, several questions: chips and a button.
    let text = json!({"toolCallName": "form", "arguments":
        "{\"fields\":[{\"label\":\"Size\",\"options\":[\"S\",\"M\"]},{\"label\":\"Color\",\"options\":[{\"label\":\"Red\"}]}]}"});
    let Some(super::Card::Choice(spec)) = super::Card::from_wire(&text) else { panic!() };
    assert!(!spec.answers_on_pick());
    assert_eq!(spec.fields[1].id, "field-1");
    assert_eq!(spec.fields[1].options, vec!["Red"]);
    assert_eq!(spec.submit, "Send");

    // A CUSTOM `form` widget, and the inline object.
    let custom = json!({"type": "CUSTOM", "name": "form", "value": {"fields": [{"label": "A", "options": ["x"]}]}});
    assert_eq!(kind_of(&custom), Some(Kind::Choice));
    let inline = json!({"component": "form", "fields": [{"label": "A", "options": ["x"]}]});
    assert_eq!(kind_of(&inline), Some(Kind::Choice));
    assert!(FormSpec::from_value(&json!({"fields": []})).is_none());
}

#[test]
fn a_choices_letter_is_its_key() {
    assert_eq!(choice::choice_letter(0), Some('A'));
    assert_eq!(choice::choice_letter(25), Some('Z'));
    assert_eq!(choice::choice_letter(26), None);
    assert_eq!(choice::choice_index("a"), Some(0));
    assert_eq!(choice::choice_index("C"), Some(2));
    assert_eq!(choice::choice_index("ab"), None);
    assert_eq!(choice::choice_index("1"), None);
}

#[test]
fn user_form_replies_are_read_as_nativechat_reads_them() {
    let Some(replies) = fixtures("rest/POST__ag-ui_user-form_submit") else { return };
    for (path, f) in replies {
        let (status, body) = (f["status"].as_u64().unwrap() as u16, &f["body"]);
        let reply = transport::user_form_action_from_http(status, body);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        match status {
            403 => assert!(
                matches!(&reply, UserFormReply::Refused(s) if s.starts_with("This computer is shared")),
                "{name}: {reply:?}"
            ),
            404 => assert_eq!(reply, UserFormReply::MissingEntry, "{name}"),
            200 if body.get("formResolution").is_some() => {
                assert!(matches!(reply, UserFormReply::Settled(_)), "{name}: {reply:?}")
            }
            200 => assert!(
                matches!(reply, UserFormReply::AlreadyAnswered | UserFormReply::Empty | UserFormReply::Settled(_)),
                "{name}: {reply:?}"
            ),
            _ => assert!(matches!(&reply, UserFormReply::Failed(s) if !s.is_empty()), "{name}: {reply:?}"),
        }
    }
    let Some(dismissed) = fixtures("rest/POST__ag-ui_user-form_dismiss") else { return };
    for (path, f) in dismissed {
        if f["body"] == json!({"alreadyAnswered": true}) {
            let reply = transport::user_form_action_from_http(200, &f["body"]);
            assert_eq!(reply, UserFormReply::AlreadyAnswered, "{}", path.display());
        }
    }
    assert_eq!(transport::user_form_action_from_http(404, &json!("Not found")), UserFormReply::MissingRoute);
    assert_eq!(transport::user_form_action_from_http(200, &Value::Null), UserFormReply::Empty);
}

#[test]
fn rule_and_policy_refusals_are_the_servers_words() {
    let Some(rule) = fixture("rest/POST__local-exec_policy_rule/422-the_desktop_enrols_sets_its_policy_and_reads_it_back.json")
    else {
        return;
    };
    let Sent::Refused(why) = transport::classify(422, &rule["body"]) else { panic!() };
    assert!(why.starts_with("an allow rule must be one plain command"));
    assert!(transport::plain_reason(&why).is_some_and(|r| r.ends_with('.')));
    let Some(egress) =
        fixture("rest/PUT__coworkers__coworker_id__computer_egress-policy/403-an_org_shared_box_takes_its_choice_from_the_admin_only.json")
    else {
        return;
    };
    assert_eq!(
        transport::classify(403, &egress["body"]),
        Sent::Refused("only the organization's admin may do that".to_owned())
    );
    let Some(answer) =
        fixture("rest/POST__ag-ui_runs__run_id__answer/200-a_card_approved_resume_keeps_the_ceiling_it_started_under.json")
    else {
        return;
    };
    assert!(matches!(transport::classify(200, &answer["body"]), Sent::Done(_)));
    // Today's server has no such route.
    assert_eq!(transport::classify(404, &json!({"error": "not found"})), Sent::Offline);
}

#[test]
fn a_submit_body_carries_a_saved_login_and_a_dismiss_its_mode() {
    let mut values = serde_json::Map::new();
    values.insert("email".into(), json!("ada@example.com"));
    assert_eq!(
        transport::submit_body("e_1", "cw_1", values.clone(), Some("sl_1"), Some("ada")),
        json!({"entryId": "e_1", "agentId": "cw_1", "values": {"email": "ada@example.com"},
               "savedLogin": true, "savedLoginId": "sl_1", "username": "ada"})
    );
    assert_eq!(
        transport::submit_body("e_1", "cw_1", values, None, None),
        json!({"entryId": "e_1", "agentId": "cw_1", "values": {"email": "ada@example.com"}})
    );
    assert_eq!(
        transport::dismiss_body("e_1", "cw_1", "escalated"),
        json!({"entryId": "e_1", "agentId": "cw_1", "mode": "escalated"})
    );
}
