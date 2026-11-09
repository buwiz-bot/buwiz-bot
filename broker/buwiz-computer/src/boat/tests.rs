//! boat.dev against a fake platform (the shared TLS fake, with the test CA) and a fake `ssh` that
//! refuses every login. Nothing here reaches boat, and no login is ever made.

use std::path::{Path, PathBuf};

use super::{ssh_key, BoatProvider};
use crate::cloud::fake::{empty, ok, serve, Answer, Fake, Seen, CA};
use crate::cloud::{Api, Settings};
use crate::{Machine, Power, Problem, Provider, ProviderKind, Reach, Secret, Sleep, Spec};

const SANDBOX: &str = "bx_abcd2345";
/// A host key line that pins (base64 of a well-formed blob), not a real key.
const PIN: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA==";

fn spec(name: &str) -> Spec {
    let dir = std::env::temp_dir().join(format!("boat-test-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    Spec {
        id: "comp1".to_owned(),
        dir,
    }
}

fn record(dir: &Path) -> Machine {
    Machine {
        provider: ProviderKind::Boat,
        reach: Reach::Sandbox {
            id: SANDBOX.to_owned(),
        },
        image: Some("img:1".to_owned()),
        token: Secret::new("tok-boat"),
        viewer_password: Secret::new("pw-boat"),
        dir: dir.to_owned(),
        link: None,
        seen: None,
    }
}

/// A `ssh` that writes its arguments to a log and fails, so no login ever succeeds. It lives in a
/// folder of its own: a computer's folder holds its staged SSH key under `ssh/`.
fn refusing_ssh(name: &str) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("boat-bin-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("ssh.log");
    let program = dir.join("ssh");
    std::fs::write(
        &program,
        format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit 255\n", log.display()),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    (program, log)
}

fn provider(addr: std::net::SocketAddr, ssh: PathBuf) -> BoatProvider {
    BoatProvider::new(Settings::new(
        Some(Secret::new("boat_key")),
        Some("img:1".to_owned()),
    ))
    .with_api(Api::with_test(addr, Some(CA.to_vec())))
    .with_ssh(ssh)
}

/// What boat's API got, as `METHOD path`, in order.
fn calls(fake: &Fake) -> Vec<String> {
    fake.requests()
        .iter()
        .filter(|r| r.host == "boat.dev")
        .map(Seen::call)
        .collect()
}

fn sandbox(state: &str) -> String {
    format!(
        r#"{{"ok":true,"type":"sandbox.info","sandbox":{{"id":"{SANDBOX}","state":"{state}"}}}}"#
    )
}

/// boat's API for a sandbox that is ready, takes a login key, and whose host key is `host_key`.
fn api(host_key: &'static str) -> impl Fn(&Seen) -> Answer + Send + Sync + 'static {
    move |req: &Seen| {
        let path = req.path.as_str();
        let sshkey = format!("/api/v1/sandboxes/{SANDBOX}/sshkey");
        match (req.method.as_str(), path) {
            ("POST", "/api/v1/sandboxes") => ok(
                202,
                &format!(r#"{{"ok":true,"sandbox":{{"id":"{SANDBOX}","state":"provisioning"}}}}"#),
            ),
            ("GET", p) if p == format!("/api/v1/sandboxes/{SANDBOX}") => ok(200, &sandbox("ready")),
            ("POST", p) if p == sshkey => ok(
                200,
                &format!(
                    r#"{{"ok":true,"success":true,"hostKey":"{host_key}","machineIp":"203.0.113.9","sshUser":"user","sshEndpoint":null}}"#
                ),
            ),
            ("DELETE", p) if p == format!("/api/v1/sandboxes/{SANDBOX}") => ok(202, "{}"),
            ("POST", p) if p == format!("/api/v1/sandboxes/{SANDBOX}/stop") => ok(202, "{}"),
            _ => ok(404, r#"{"ok":false,"code":"not_found"}"#),
        }
    }
}

#[test]
fn the_ssh_key_is_the_tokens_own_and_ssh_reads_it() {
    let (first, public) = ssh_key(&Secret::new("tok-a")).unwrap();
    let (_, again) = ssh_key(&Secret::new("tok-a")).unwrap();
    let (_, other) = ssh_key(&Secret::new("tok-b")).unwrap();
    assert_eq!(public, again, "the same token makes the same key");
    assert_ne!(public, other, "another token makes another key");
    assert!(public.starts_with("ssh-ed25519 "));

    // the OpenSSH text is the key the public line names: ssh-keygen reads both and agrees
    let dir = std::env::temp_dir().join("boat-test-keyformat");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("key");
    std::fs::write(&file, first.expose()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let out = std::process::Command::new("/usr/bin/ssh-keygen")
        .args(["-y", "-f"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), public);
}

#[tokio::test]
async fn nothing_is_sent_to_a_host_the_roots_dont_vouch_for() {
    let fake = serve(api(PIN)).await;
    let (ssh, _) = refusing_ssh("untrusted");
    let p = BoatProvider::new(Settings::new(
        Some(Secret::new("boat_key")),
        Some("img:1".to_owned()),
    ))
    .with_api(Api::with_test(fake.addr, None))
    .with_ssh(ssh);
    assert!(p.create(&spec("untrusted")).await.is_err());
    assert!(
        fake.requests().is_empty(),
        "a request was written before the name checked out"
    );
}

#[tokio::test]
async fn a_host_key_that_cannot_be_pinned_stops_before_any_login_and_the_sandbox_goes() {
    let fake = serve(api("not-a-key")).await;
    let (ssh, log) = refusing_ssh("badpin");
    let err = provider(fake.addr, ssh)
        .create(&spec("badpin"))
        .await
        .unwrap_err();
    assert!(matches!(err, Problem::Remote(_)), "{err:?}");
    assert!(
        !log.exists(),
        "ssh was started with a host key it couldn't pin"
    );
    assert_eq!(
        calls(&fake),
        vec![
            "POST /api/v1/sandboxes",
            &format!("GET /api/v1/sandboxes/{SANDBOX}"),
            &format!("POST /api/v1/sandboxes/{SANDBOX}/sshkey"),
            &format!("DELETE /api/v1/sandboxes/{SANDBOX}")
        ]
    );
}

#[tokio::test]
async fn a_login_that_is_refused_deletes_the_sandbox_and_runs_no_setup() {
    let fake = serve(api(PIN)).await;
    let (ssh, log) = refusing_ssh("refused");
    let err = provider(fake.addr, ssh)
        .create(&spec("refused"))
        .await
        .unwrap_err();
    eprintln!("SENT: {}", err.sentence());
    let said = std::fs::read_to_string(&log).unwrap();
    assert!(said.contains("-M") && !said.contains("docker"), "{said}");
    let got = calls(&fake);
    assert_eq!(
        got.last().map(String::as_str),
        Some(format!("DELETE /api/v1/sandboxes/{SANDBOX}").as_str())
    );
    assert!(
        !got.iter().any(|c| c.ends_with("/stop")),
        "a sandbox that never came up is deleted, not stopped"
    );
}

#[tokio::test]
async fn power_reads_boats_state_when_no_link_is_held() {
    for (state, want) in [
        ("ready", Power::Asleep),
        ("archived", Power::Asleep),
        ("provisioning", Power::Waking),
        ("error", Power::Broken("error".to_owned())),
    ] {
        let fake = serve(move |req: &Seen| {
            if req.method == "GET" {
                ok(200, &sandbox(state))
            } else {
                ok(404, "{}")
            }
        })
        .await;
        let (ssh, _) = refusing_ssh("power");
        let machine = record(&spec("power").dir);
        assert_eq!(
            provider(fake.addr, ssh).power(&machine).await.unwrap(),
            want,
            "{state}"
        );
    }
    let fake = serve(|_| ok(404, r#"{"ok":false,"code":"not_found"}"#)).await;
    let (ssh, _) = refusing_ssh("gone");
    assert_eq!(
        provider(fake.addr, ssh)
            .power(&record(&spec("gone").dir))
            .await
            .unwrap(),
        Power::Absent
    );
}

#[tokio::test]
async fn sleep_stops_and_destroy_deletes_with_the_confirmation_header() {
    let fake = serve(api(PIN)).await;
    let (ssh, _) = refusing_ssh("sleep");
    let p = provider(fake.addr, ssh);
    let machine = record(&spec("sleep").dir);
    p.sleep(&machine, Sleep::Deep).await.unwrap();
    p.destroy(&machine).await.unwrap();
    assert_eq!(
        calls(&fake),
        vec![
            format!("POST /api/v1/sandboxes/{SANDBOX}/stop"),
            format!("DELETE /api/v1/sandboxes/{SANDBOX}")
        ]
    );
    let delete = fake
        .requests()
        .into_iter()
        .find(|r| r.method == "DELETE")
        .unwrap();
    assert_eq!(delete.header("x-ascii-confirm-delete"), Some(SANDBOX));
}

#[tokio::test]
async fn a_sandbox_already_gone_is_fine_to_destroy() {
    let fake = serve(|_| empty(404)).await;
    let (ssh, _) = refusing_ssh("destroy404");
    provider(fake.addr, ssh)
        .destroy(&record(&spec("destroy404").dir))
        .await
        .unwrap();
}

#[tokio::test]
async fn sweep_lists_by_label_across_pages_and_deletes_each_sandbox() {
    let fake = serve(|req: &Seen| {
        let p = req.path.as_str();
        match req.method.as_str() {
            "GET" if p.contains("cursor=c2") => ok(200, r#"{"ok":true,"sandboxes":[{"id":"bx_bbbb2222"}],"pageInfo":{"hasMore":false,"nextCursor":null}}"#),
            "GET" if p.contains("label=buwiz-computer%3Dcomp1") => ok(200, r#"{"ok":true,"sandboxes":[{"id":"bx_aaaa2222"}],"pageInfo":{"hasMore":true,"nextCursor":"c2"}}"#),
            "DELETE" => ok(202, "{}"),
            _ => ok(404, "{}"),
        }
    })
    .await;
    let (ssh, _) = refusing_ssh("sweep");
    provider(fake.addr, ssh)
        .sweep(
            "comp1",
            Some(&Reach::Sandbox {
                id: "bx_cccc2222".to_owned(),
            }),
        )
        .await
        .unwrap();
    let got = calls(&fake);
    assert_eq!(
        got,
        vec![
            "GET /api/v1/sandboxes?label=buwiz-computer%3Dcomp1&limit=100".to_owned(),
            "GET /api/v1/sandboxes?label=buwiz-computer%3Dcomp1&limit=100&cursor=c2".to_owned(),
            "DELETE /api/v1/sandboxes/bx_aaaa2222".to_owned(),
            "DELETE /api/v1/sandboxes/bx_bbbb2222".to_owned(),
            "DELETE /api/v1/sandboxes/bx_cccc2222".to_owned(),
        ]
    );
}

#[tokio::test]
async fn statuses_are_sentences_and_the_body_and_key_are_never_shown() {
    let fake = serve(|_| {
        ok(
            401,
            r#"{"ok":false,"code":"unauthorized","message":"boat_key is bad"}"#,
        )
    })
    .await;
    let (ssh, _) = refusing_ssh("401");
    let err = provider(fake.addr, ssh).check().await;
    assert!(err.is_ok(), "check makes no call");
    let err = provider(fake.addr, refusing_ssh("401b").0)
        .create(&spec("401"))
        .await
        .unwrap_err();
    assert!(err.sentence().contains("API key"), "{}", err.sentence());
    assert!(!err.sentence().contains("boat_key"));

    let fake = serve(|_| ok(429, r#"{"ok":false,"code":"rate_limited"}"#)).await;
    let err = provider(fake.addr, refusing_ssh("429").0)
        .create(&spec("429"))
        .await
        .unwrap_err();
    assert!(err.sentence().contains("limiting"), "{}", err.sentence());
}

#[tokio::test]
async fn a_key_is_checked_before_a_call_and_missing_it_is_a_sentence() {
    let p = BoatProvider::new(Settings::new(None, Some("img:1".to_owned())));
    let err = p.check().await.unwrap_err();
    assert!(err.sentence().contains("API key"));
}

#[test]
fn the_label_says_the_regions_are_eu_only() {
    assert!(ProviderKind::Boat.label().contains("EU"));
}

#[tokio::test]
async fn a_check_proves_the_key_with_a_read() {
    let fake = serve(|req: &Seen| match (req.method.as_str(), req.path.as_str(), req.header("authorization")) {
        ("GET", "/api/v1/sandboxes", Some("Bearer boat_key")) => ok(200, r#"{"sandboxes":[]}"#),
        ("GET", "/api/v1/sandboxes", _) => ok(401, "{}"),
        _ => ok(404, "{}"),
    })
    .await;
    let (ssh, _) = refusing_ssh("verify");
    provider(fake.addr, ssh.clone()).verify().await.unwrap();
    let wrong = BoatProvider::new(Settings::new(Some(Secret::new("other")), None)).with_api(Api::with_test(fake.addr, Some(CA.to_vec()))).with_ssh(ssh);
    assert_eq!(wrong.verify().await.unwrap_err().code(), "bad_key");
}
