#![cfg(windows)]

use buwiz_bot::{agent_hub, mini_v1::{attribution::Actor, document}};

#[test]
fn installed_fonts_are_unavailable_for_every_family_and_style() {
    buwiz_office_kit::fonts::register();
    for family in ["Arial", "", "not-an-installed-font"] {
        for bold in [false, true] {
            for italic in [false, true] {
                assert!(buwiz_office_kit::fonts::installed(family, bold, italic).is_none());
            }
        }
    }
}

#[test]
fn unavailable_agent_cards_always_complete_as_changed() {
    agent_hub::start();
    agent_hub::opened(1);
    for title in ["Allow this change?", "", "\u{202e}hidden"] {
        let once = buwiz_agent::OnceId::parse("press-windows-test").unwrap();
        let card = agent_hub::hold::Card { app: "App".into(), title: title.into(), lines: vec![] };
        let (send, recv) = std::sync::mpsc::channel();
        agent_hub::show_card(1, once.clone(), card, Box::new(move |answer| send.send(answer).unwrap()));
        assert!(matches!(recv.try_recv().unwrap(), agent_hub::hold::Answer::Changed));
        agent_hub::hide_card(once);
    }
    agent_hub::closed(1);
}

#[test]
fn document_opening_fails_closed_without_windows_file_validation() {
    for field in [serde_json::json!({"path": "C:\\plugin\\open-1f\\1\\a.docx"}), serde_json::Value::Null, serde_json::json!({"path": 12})] {
        let result = document::check(true, Actor::Person, Some(std::path::Path::new("C:\\plugin")), &field);
        assert!(matches!(result, Err(document::Refused::Kind)));
    }
    assert!(matches!(document::check(false, Actor::Agent, None, &serde_json::Value::Null), Err(document::Refused::Kind)));
}

#[test]
fn pele_gantt_uses_the_windows_localtime_shim() {
    let _ = buwiz_bot::app_name();
    for millis in [0.0, 1_700_000_000_000.0] {
        let offset = pele::diagrams::gantt::clock::offset_at(millis);
        assert!(offset.is_finite());
        assert!(offset.abs() < 86_400_000.0);
    }
}
