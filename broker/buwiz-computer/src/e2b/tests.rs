//! The E2B provider against a fake platform that speaks TLS with a test CA: its names are
//! `*.e2b.app`, its certificate is one the tests' root vouches for, and the box behind its relays
//! answers as a box. Nothing here reaches E2B.

use super::E2bProvider;
use crate::cloud::fake::{empty, ok, serve, Answer, Seen, CA};
use crate::cloud::{Api, Settings};
use crate::{Machine, Power, Problem, Provider, ProviderKind, Reach, Secret, Sleep, Spec, Woke};
use hyper::Method;
use std::net::SocketAddr;
use std::time::Duration;

const CREATED: &str = r#"{"sandboxID":"sbx-1","envdAccessToken":"envd-1","templateID":"tpl","clientID":"c","envdVersion":"0.2.0"}"#;

/// The platform as it answers a sandbox that exists, and the box behind its names.
fn platform(req: &Seen) -> Answer {
    let api = req.host == "api.e2b.app";
    let envd = req.host == "sandbox.e2b.app";
    match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/v2/sandboxes") if api => ok(201, CREATED),
        ("POST", p) if envd && p.starts_with("/files?") => ok(200, "[]"),
        ("POST", "/sandboxes/sbx-1/pause") if api => empty(204),
        ("POST", "/v2/sandboxes/sbx-1/connect") if api => ok(201, "{}"),
        ("GET", "/sandboxes/sbx-1") if api => ok(200, r#"{"state":"paused"}"#),
        ("DELETE", "/sandboxes/sbx-1") if api => empty(204),
        ("GET", "/v1/info") => ok(200, r#"{"capabilities":{"viewer":{"kind":"voxl"}}}"#),
        ("GET", "/v1/ready") => ok(200, "{}"),
        ("GET", "/v1/screens") => ok(200, "[]"),
        _ => ok(404, "{}"),
    }
}

fn provider(addr: SocketAddr) -> E2bProvider {
    E2bProvider::new(Settings::new(
        Some(Secret::new("key-1")),
        Some("tpl".to_owned()),
    ))
    .with_api(Api::with_test(addr, Some(CA.to_vec())))
}

fn spec(name: &str) -> Spec {
    Spec {
        id: "c1".to_owned(),
        dir: std::env::temp_dir().join(format!("e2b-test-{name}")),
    }
}

/// A saved record for sandbox `id`, as create would have left it.
fn machine(id: &str, spec: &Spec) -> Machine {
    Machine {
        provider: ProviderKind::E2b,
        reach: Reach::Sandbox { id: id.to_owned() },
        image: Some("tpl".to_owned()),
        token: Secret::new("tok-e2b"),
        viewer_password: Secret::new("pw-e2b"),
        dir: spec.dir.clone(),
        link: None,
        seen: None,
    }
}

#[tokio::test]
async fn the_lifecycle_is_one_sandbox_and_its_secrets_go_in_in_order() {
    let fake = serve(platform).await;
    let p = provider(fake.addr);
    let made = p.create(&spec("lifecycle")).await.unwrap();
    assert_eq!(
        made.reach,
        Reach::Sandbox {
            id: "sbx-1".to_owned()
        }
    );
    assert_eq!(p.power(&made).await.unwrap(), Power::Asleep);
    assert_eq!(p.wake(&made).await.unwrap(), Woke::Resumed);
    assert_eq!(p.power(&made).await.unwrap(), Power::Awake);
    p.sleep(&made, Sleep::Light).await.unwrap();
    assert_eq!(p.power(&made).await.unwrap(), Power::Asleep);
    assert_eq!(p.wake(&made).await.unwrap(), Woke::Resumed);
    p.destroy(&made).await.unwrap();

    assert_eq!(
        fake.api_calls(),
        vec![
            "POST /v2/sandboxes",
            "POST /sandboxes/sbx-1/pause",
            "GET /sandboxes/sbx-1",
            "POST /v2/sandboxes/sbx-1/connect",
            "POST /sandboxes/sbx-1/pause",
            "GET /sandboxes/sbx-1",
            "POST /v2/sandboxes/sbx-1/connect",
            "DELETE /sandboxes/sbx-1",
        ]
    );
    let requests = fake.requests();
    // the key goes only to the platform's API; the envd token only to the sandbox's files
    assert!(requests
        .iter()
        .filter(|r| r.header("x-api-key").is_some())
        .all(|r| r.host == "api.e2b.app" && r.header("x-api-key") == Some("key-1")));
    let uploads: Vec<&Seen> = requests
        .iter()
        .filter(|r| r.host == "sandbox.e2b.app")
        .collect();
    let paths: Vec<&str> = uploads.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "/files?path=/run/buwiz/token&username=box",
            "/files?path=/run/buwiz/viewer&username=box",
            "/files?path=/run/buwiz/go&username=box"
        ]
    );
    assert!(uploads
        .iter()
        .all(|r| r.header("x-access-token") == Some("envd-1")
            && r.header("e2b-sandbox-id") == Some("sbx-1")
            && r.header("e2b-sandbox-port") == Some("49983")));
    assert_eq!(uploads[0].body, made.token.expose().as_bytes());
    assert_eq!(uploads[1].body, made.viewer_password.expose().as_bytes());
    assert!(uploads[2].body.is_empty(), "go is a trigger, not a secret");
    // the box got its bearer, and only after the handshake that checked its name
    let box_calls: Vec<&Seen> = requests.iter().filter(|r| r.path == "/v1/info").collect();
    assert!(!box_calls.is_empty());
    assert!(box_calls
        .iter()
        .all(|r| r.header("authorization") == Some(&format!("Bearer {}", made.token.expose()))));
}

#[tokio::test]
async fn a_missing_key_or_template_is_a_sentence_and_no_call_is_made() {
    let no_key = E2bProvider::new(Settings::new(None, Some("tpl".to_owned())));
    let sentence = no_key.check().await.unwrap_err().sentence();
    assert!(sentence.contains("API key"), "{sentence}");
    let no_template = E2bProvider::new(Settings::new(Some(Secret::new("key-1")), None));
    let sentence = no_template.check().await.unwrap_err().sentence();
    assert!(sentence.contains("template"), "{sentence}");
    assert!(no_key.create(&spec("nokey")).await.is_err());
}

#[tokio::test]
async fn nothing_is_sent_to_a_host_the_roots_dont_vouch_for() {
    // the fake's certificate is not in the public roots, and this provider trusts no extra root
    let fake = serve(platform).await;
    let p = E2bProvider::new(Settings::new(
        Some(Secret::new("key-1")),
        Some("tpl".to_owned()),
    ))
    .with_api(Api::with_test(fake.addr, None));
    let err = p.create(&spec("untrusted")).await.unwrap_err();
    assert!(matches!(err, Problem::Failed(_)), "{err:?}");
    assert!(!err.sentence().contains("key-1"));
    assert!(
        fake.requests().is_empty(),
        "a request was written before the name checked out"
    );
}

#[tokio::test]
async fn power_reads_the_platform_when_no_link_is_held() {
    for (status, body, want) in [
        (200, r#"{"state":"running"}"#, Power::Asleep),
        (200, r#"{"state":"paused"}"#, Power::Asleep),
        (404, "{}", Power::Absent),
        (
            200,
            r#"{"state":"killing"}"#,
            Power::Broken("killing".to_owned()),
        ),
    ] {
        let fake = serve(move |_| ok(status, body)).await;
        let p = provider(fake.addr);
        let record = machine("sbx-1", &spec("power"));
        assert_eq!(p.power(&record).await.unwrap(), want, "{status} {body}");
    }
}

#[tokio::test]
async fn a_failed_create_kills_the_sandbox_and_says_why_without_the_key() {
    // the files won't take the secrets: the sandbox is killed, and no sentence says the key
    let fake = serve(|req| {
        if req.host == "sandbox.e2b.app" {
            ok(500, "{}")
        } else {
            platform(req)
        }
    })
    .await;
    let err = provider(fake.addr)
        .create(&spec("failed"))
        .await
        .unwrap_err();
    assert!(
        err.sentence().contains("so the computer was stopped"),
        "{}",
        err.sentence()
    );
    assert!(!err.sentence().contains("key-1") && !err.sentence().contains("envd-1"));
    assert_eq!(
        fake.api_calls(),
        vec!["POST /v2/sandboxes", "DELETE /sandboxes/sbx-1"]
    );
}

#[tokio::test]
async fn a_status_is_a_sentence_and_a_missing_template_names_itself() {
    let fake = serve(|req| {
        if req.path == "/v2/sandboxes" {
            ok(404, "{}")
        } else {
            platform(req)
        }
    })
    .await;
    let err = provider(fake.addr)
        .create(&spec("template"))
        .await
        .unwrap_err();
    assert!(
        err.sentence().contains("no template named tpl"),
        "{}",
        err.sentence()
    );

    let fake = serve(|req| {
        if req.path == "/v2/sandboxes" {
            ok(401, r#"{"message":"key-1 is bad"}"#)
        } else {
            platform(req)
        }
    })
    .await;
    let err = provider(fake.addr).create(&spec("auth")).await.unwrap_err();
    assert!(
        err.sentence().contains("refused this broker's key"),
        "{}",
        err.sentence()
    );
    assert!(
        !err.sentence().contains("key-1"),
        "the platform's body is never shown"
    );
}

#[tokio::test]
async fn a_sandbox_that_is_gone_is_gone_on_wake_and_fine_to_destroy() {
    let fake = serve(|req| {
        if req.path == "/v2/sandboxes/sbx-1/connect" {
            ok(404, "{}")
        } else {
            platform(req)
        }
    })
    .await;
    let p = provider(fake.addr);
    let record = machine("sbx-1", &spec("gone"));
    assert_eq!(p.wake(&record).await.unwrap_err(), Problem::Gone);

    let fake = serve(|req| {
        if req.method == "DELETE" {
            empty(404)
        } else {
            platform(req)
        }
    })
    .await;
    provider(fake.addr)
        .destroy(&machine("sbx-1", &spec("gone2")))
        .await
        .unwrap();
}

#[tokio::test]
async fn sweep_stops_every_sandbox_the_computer_names_and_recover_makes_none() {
    let fake = serve(|req| {
        let api = req.host == "api.e2b.app";
        match (req.method.as_str(), req.path.as_str()) {
            ("GET", p) if api && p.contains("nextToken=tok2") => {
                ok(200, r#"[{"sandboxID":"sbx-b"}]"#)
            }
            ("GET", p) if api && p.contains("metadata=buwiz-computer%3Dc1") => (
                200,
                vec![("X-Next-Token".to_owned(), "tok2".to_owned())],
                br#"[{"sandboxID":"sbx-a"}]"#.to_vec(),
            ),
            ("DELETE", _) if api => empty(204),
            _ => ok(404, "{}"),
        }
    })
    .await;
    let p = provider(fake.addr);
    p.sweep(
        "c1",
        Some(&Reach::Sandbox {
            id: "sbx-c".to_owned(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        fake.api_calls(),
        vec![
            "GET /v2/sandboxes?state=running,paused&limit=100&metadata=buwiz-computer%3Dc1",
            "GET /v2/sandboxes?state=running,paused&limit=100&metadata=buwiz-computer%3Dc1&nextToken=tok2",
            "DELETE /sandboxes/sbx-a",
            "DELETE /sandboxes/sbx-b",
            "DELETE /sandboxes/sbx-c",
        ]
    );

    let fake = serve(platform).await;
    assert!(provider(fake.addr)
        .recover(&spec("recover"), None)
        .await
        .unwrap()
        .is_none());
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn answers_are_limited_in_size_and_time_and_redirects_are_errors() {
    let fake = serve(|req| match req.path.as_str() {
        "/big" => (200, vec![], vec![b'x'; 2 * 1024 * 1024]),
        "/redirect" => (
            302,
            vec![("Location".to_owned(), "https://evil.example/".to_owned())],
            vec![],
        ),
        "/hold" => empty(0),
        _ => ok(200, "{}"),
    })
    .await;
    let api = Api::with_test(fake.addr, Some(CA.to_vec()));
    let patience = Duration::from_secs(3);
    assert!(api
        .call("api.e2b.app", Method::GET, "/big", &[], None, patience)
        .await
        .is_err());
    let redirect = api
        .call("api.e2b.app", Method::GET, "/redirect", &[], None, patience)
        .await
        .unwrap_err();
    assert!(
        redirect.sentence().contains("redirect"),
        "{}",
        redirect.sentence()
    );
    let slow = api
        .call(
            "api.e2b.app",
            Method::GET,
            "/hold",
            &[],
            None,
            Duration::from_millis(300),
        )
        .await
        .unwrap_err();
    assert!(slow.sentence().contains("too long"), "{}", slow.sentence());
    assert!(api
        .call("api.e2b.app", Method::GET, "/small", &[], None, patience)
        .await
        .is_ok());
}

/// The platform for a Check: this team's sandboxes for the right key, and its templates.
fn checked(req: &Seen) -> Answer {
    let good = req.header("x-api-key") == Some("key-1");
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/sandboxes") if good => ok(200, "[]"),
        ("GET", "/sandboxes") => ok(401, r#"{"message":"bad key"}"#),
        ("GET", "/templates") => ok(200, r#"[{"templateID":"t-9","aliases":["tpl"]}]"#),
        _ => ok(404, "{}"),
    }
}

#[tokio::test]
async fn a_check_proves_the_key_and_finds_the_template_by_its_name() {
    let fake = serve(checked).await;
    provider(fake.addr).verify().await.unwrap();
    let wrong = E2bProvider::new(Settings::new(Some(Secret::new("key-2")), Some("tpl".to_owned()))).with_api(Api::with_test(fake.addr, Some(CA.to_vec())));
    assert_eq!(wrong.verify().await.unwrap_err().code(), "bad_key");
    let missing = E2bProvider::new(Settings::new(Some(Secret::new("key-1")), Some("nope".to_owned()))).with_api(Api::with_test(fake.addr, Some(CA.to_vec())));
    let said = missing.verify().await.unwrap_err();
    assert_eq!(said.code(), "no_template");
    assert!(!said.sentence().contains("key-1"));
}

#[tokio::test]
async fn new_settings_reach_a_running_provider() {
    let shared = crate::Shared::new(Settings::default());
    let p = E2bProvider::shared(shared.clone());
    assert_eq!(p.check().await.unwrap_err().code(), "no_key");
    shared.set(Settings::new(Some(Secret::new("key-1")), Some("tpl".to_owned())));
    p.check().await.unwrap();
}

#[test]
fn a_lost_computer_is_never_offered_keep_and_the_sentence_says_why() {
    let p = E2bProvider::new(Settings::new(None, Some("tpl".to_owned())));
    let why = p.cant_keep_lost().unwrap();
    assert!(why.contains("new secrets") && why.contains("deleted"), "{why}");
}
