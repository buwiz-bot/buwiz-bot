#![cfg(windows)]

use std::{path::PathBuf, process::Command};

use buwiz_bot::{agent_hub, mini_v1::{attribution::{self, Actor}, document, settle::OpenSettle}};
use nsh_platform_windows::windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, MSG, PM_REMOVE};

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

#[test]
fn home_prefers_home_then_windows_profile() {
    if let Ok(expected) = std::env::var("BUWIZ_WINDOWS_HOME_CASE") {
        let expected = PathBuf::from(expected);
        assert_eq!(buwiz_bot::identity::home(), expected);
        assert_eq!(buwiz_bot::identity::data_dir(), expected.join("Library/Application Support/Buwiz Bot"));
        return;
    }
    for home in [Some(r"C:\buwiz-test-home"), Some(""), None] {
        let profile = r"C:\buwiz-test-profile";
        let mut child = Command::new(std::env::current_exe().unwrap());
        child.args(["--exact", "home_prefers_home_then_windows_profile", "--nocapture"])
            .env_remove("HOME")
            .env_remove("BUWIZ_DATA_DIR")
            .env("USERPROFILE", profile)
            .env("BUWIZ_WINDOWS_HOME_CASE", home.filter(|h| !h.is_empty()).unwrap_or(profile));
        if let Some(home) = home {
            child.env("HOME", home);
        }
        let output = child.output().unwrap();
        assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
}

#[test]
fn desktop_attribution_starts_with_the_person_and_tracks_agent_activity() {
    assert_eq!(attribution::actor(0), Actor::Agent);
    attribution::watch_desktop();
    assert_eq!(attribution::actor(0), Actor::Person);
    assert_eq!(attribution::fresh_press(0), None);
    assert!(attribution::spend_press(0, 1));
    assert!(!attribution::spend_press(0, 1));
    attribution::agent_opened();
    assert_eq!(attribution::actor(0), Actor::Agent);
    assert_eq!(attribution::actor(1), Actor::Agent);
    attribution::agent_call(true);
    attribution::agent_write(true);
    assert_eq!(attribution::agent_calls(), 1);
    attribution::agent_write(false);
    attribution::agent_call(false);
    assert_eq!(attribution::agent_calls(), 0);
    assert_eq!(attribution::actor(0), Actor::Agent);
}

#[test]
fn inline_ready_and_timer_settle_only_once_in_either_order() {
    for first in ["ready", "timer"] {
        let mut hold = OpenSettle::default();
        assert_eq!(hold.current(), None);
        assert!(!hold.settle(0));
        let open = hold.start();
        assert!(hold.settle(open), "{first} reveals the card");
        assert!(!hold.settle(open), "the later callback emits nothing");
        assert_eq!(hold.current(), None);
    }
}

#[test]
fn inline_close_and_reopen_invalidate_old_callbacks() {
    let mut hold = OpenSettle::default();
    let old = hold.start();
    hold.cancel();
    assert!(!hold.settle(old), "destroyed components cannot emit");
    let open = hold.start();
    assert_ne!(open, old);
    assert!(!hold.settle(old), "a queued old timer cannot reveal a new open");
    assert!(hold.settle(open));
    let reopened = hold.start();
    assert!(!hold.settle(open));
    assert!(hold.settle(reopened));
    assert!(!hold.settle(reopened));
}

#[test]
fn session_requests_leave_headroom_for_the_plugins_clock() {
    struct Capture(std::sync::atomic::AtomicI64);
    impl buwiz_bot::mini_v1::host_api::Broker for Capture {
        fn request(&self, method: &str, route: &str, body: Option<serde_json::Value>, _: std::time::Duration) -> Result<serde_json::Value, buwiz_bot::mini_v1::MiniError> {
            assert_eq!((method, route), ("POST", "/v1/host/sessions/open"));
            self.0.store(body.unwrap()["not_after"].as_i64().unwrap(), std::sync::atomic::Ordering::Relaxed);
            Err(buwiz_bot::mini_v1::MiniError::new("captured", "request captured"))
        }
    }
    let broker = Capture(std::sync::atomic::AtomicI64::new(0));
    let before = buwiz_bot::mini_v1::session::now_ms();
    let result = buwiz_bot::mini_v1::session::Session::open(&broker, "app.buwiz.fixture", "card", None, None);
    assert_eq!(result.err().unwrap().code, "captured");
    let after = buwiz_bot::mini_v1::session::now_ms();
    let expiry = broker.0.load(std::sync::atomic::Ordering::Relaxed);
    let lifetime = buwiz_bot::mini_v1::session::MAX_LIFETIME_MS - 60_000;
    assert!((before + lifetime..=after + lifetime).contains(&expiry));
    assert!(expiry <= before + buwiz_bot::mini_v1::session::MAX_LIFETIME_MS - 30_000);
}

#[test]
fn ui_timers_fire_once_and_cancel() {
    if std::env::var_os("BUWIZ_WINDOWS_TIMER_CASE").is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "ui_timers_fire_once_and_cancel", "--nocapture"])
            .env("BUWIZ_WINDOWS_TIMER_CASE", "1")
            .output().unwrap();
        assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    buwiz_bot::mini_v1::native::init();
    let fired = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = fired.clone();
    let _once = buwiz_bot::mini_v1::native::after(std::time::Duration::from_millis(10), move || count.set(count.get() + 1)).unwrap();
    let count = fired.clone();
    let cancelled = buwiz_bot::mini_v1::native::after(std::time::Duration::from_millis(10), move || count.set(count.get() + 100)).unwrap();
    drop(cancelled);
    let until = std::time::Instant::now() + std::time::Duration::from_millis(100);
    loop {
        // SAFETY: this isolated child owns the dispatcher and pumps its UI thread.
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&message);
            }
        }
        if std::time::Instant::now() >= until { break; }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(fired.get(), 1);
}
