//! boat.dev against a fake platform (the shared TLS fake, with the test CA) and a fake `ssh` that
//! refuses every login. Nothing here reaches boat, and no login is ever made.

use std::path::{Path, PathBuf};

use super::{prep, ssh_key, BoatProvider};
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

/// boat's listing for computer `comp1`: `first` on the first page, `second` on the page after the
/// cursor (no second page when it's `None`). `bx_cccc2222` is a sandbox boat still has by id; any
/// other is gone. A stop or a delete is answered, so a test can see one was asked for.
fn listing(
    first: &'static str,
    second: Option<&'static str>,
) -> impl Fn(&Seen) -> Answer + Send + Sync + 'static {
    move |req: &Seen| {
        let p = req.path.as_str();
        let page = |ids: &str, next: Option<&str>| {
            let next = next.map_or("null".to_owned(), |n| format!("\"{n}\""));
            ok(
                200,
                &format!(
                    r#"{{"ok":true,"sandboxes":[{ids}],"pageInfo":{{"hasMore":{},"nextCursor":{next}}}}}"#,
                    next != "null"
                ),
            )
        };
        match req.method.as_str() {
            "GET" if p.contains("cursor=c2") => page(second.unwrap_or(""), None),
            "GET" if p.contains("label=buwiz-computer%3Dcomp1") => {
                page(first, second.map(|_| "c2"))
            }
            "GET" if p == "/api/v1/sandboxes/bx_cccc2222" => ok(
                200,
                r#"{"ok":true,"sandbox":{"id":"bx_cccc2222","state":"ready"}}"#,
            ),
            "DELETE" | "POST" => ok(202, "{}"),
            _ => ok(404, r#"{"ok":false,"code":"not_found"}"#),
        }
    }
}

fn sandbox_of(machine: &Machine) -> &str {
    match &machine.reach {
        Reach::Sandbox { id } => id,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn recover_finds_the_sandbox_by_its_label_with_new_secrets_and_deletes_nothing() {
    let fake = serve(listing(
        r#"{"id":"bx_zzzz2222"},{"id":"bx_aaaa2222"}"#,
        None,
    ))
    .await;
    let (ssh, log) = refusing_ssh("recover");
    let p = provider(fake.addr, ssh);
    let spec = spec("recover");
    let old = record(&spec.dir);

    // of two, the first by id, whatever order boat lists them in
    let found = p.recover(&spec, None).await.unwrap().unwrap();
    assert_eq!(sandbox_of(&found), "bx_aaaa2222");
    assert_eq!(found.provider, ProviderKind::Boat);
    assert_eq!(found.dir, spec.dir);
    assert_eq!(found.image.as_deref(), Some("img:1"));
    assert!(found.token.expose().len() >= 24 && found.viewer_password.expose().len() >= 24);
    assert_ne!(found.token.expose(), old.token.expose());
    assert_ne!(found.viewer_password.expose(), old.viewer_password.expose());
    assert_ne!(found.token.expose(), found.viewer_password.expose());

    // the one the record named, when it's among them
    let named = p
        .recover(
            &spec,
            Some(&Reach::Sandbox {
                id: "bx_zzzz2222".to_owned(),
            }),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox_of(&named), "bx_zzzz2222");
    assert_ne!(
        named.token.expose(),
        found.token.expose(),
        "each recover mints its own"
    );

    let got = calls(&fake);
    assert!(got.iter().all(|c| c.starts_with("GET ")), "{got:?}");
    assert!(!log.exists(), "recover logged in");
}

#[tokio::test]
async fn recover_is_none_when_boat_has_nothing_for_the_computer() {
    let fake = serve(listing("", None)).await;
    let (ssh, _) = refusing_ssh("recover-none");
    let p = provider(fake.addr, ssh);
    let spec = spec("recover-none");
    assert!(p.recover(&spec, None).await.unwrap().is_none());
    let gone = Reach::Sandbox {
        id: "bx_dddd2222".to_owned(),
    };
    assert!(p.recover(&spec, Some(&gone)).await.unwrap().is_none());

    // nothing labelled, but the record's own sandbox is still there
    let kept = Reach::Sandbox {
        id: "bx_cccc2222".to_owned(),
    };
    let found = p.recover(&spec, Some(&kept)).await.unwrap().unwrap();
    assert_eq!(sandbox_of(&found), "bx_cccc2222");

    let got = calls(&fake);
    assert!(got.iter().all(|c| c.starts_with("GET ")), "{got:?}");
    assert!(
        got.contains(&"GET /api/v1/sandboxes/bx_dddd2222".to_owned()),
        "{got:?}"
    );
}

#[tokio::test]
async fn recover_reads_every_page_of_the_listing() {
    let fake = serve(listing("", Some(r#"{"id":"bx_bbbb2222"}"#))).await;
    let (ssh, _) = refusing_ssh("recover-pages");
    let found = provider(fake.addr, ssh)
        .recover(&spec("recover-pages"), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox_of(&found), "bx_bbbb2222");
    assert_eq!(
        calls(&fake),
        vec![
            "GET /api/v1/sandboxes?label=buwiz-computer%3Dcomp1&limit=100".to_owned(),
            "GET /api/v1/sandboxes?label=buwiz-computer%3Dcomp1&limit=100&cursor=c2".to_owned(),
        ]
    );
}

#[test]
fn the_box_is_made_as_docker_and_ec2_make_theirs() {
    let s = prep();
    assert!(s.contains(r#"-v "$2-workspace:/workspace""#), "{s}");
    assert!(s.contains(r#"-v "$2-home:/home/box""#), "{s}");
    assert!(
        s.contains("-v /dev/shm/buwiz-start:/run/buwiz-computer"),
        "{s}"
    );
    assert!(s.contains("--user 0 --cap-drop ALL"), "{s}");
    for cap in crate::docker::BOX_CAPS {
        assert!(s.contains(&format!("--cap-add {cap}")), "{cap}: {s}");
    }
    assert_eq!(
        s.matches("--cap-add").count(),
        crate::docker::BOX_CAPS.len(),
        "{s}"
    );
    assert!(
        s.contains("--security-opt no-new-privileges:true") && s.contains("--shm-size 1g"),
        "{s}"
    );
    assert!(
        s.contains("ip_unprivileged_port_start=6906") && s.contains("disable_ipv6=1"),
        "{s}"
    );
    for env in [
        "BOX_TOKEN_FILE=/run/buwiz-computer/token",
        "BOX_VIEWER_PASSWORD_FILE=/run/buwiz-computer/viewer",
        "BOX_DESKTOP=1",
        "BOX_DESKTOP_REQUIRED=1",
        "BOX_CHROME=0",
    ] {
        assert!(s.contains(&format!("-e {env} ")), "{env}: {s}");
    }
    for dropped in [
        "-chrome",
        "chrome-profile",
        "BOX_CDP_PORT",
        "BOX_CHROME=1",
        "BOX_CHROME_PROFILE",
        "256m",
        "BOX_EXEC_BIND",
        "BOX_HOST_BIND",
        "BOX_TOKEN=",
        "BOX_VIEWER_PASSWORD=",
    ] {
        assert!(!s.contains(dropped), "{dropped}: {s}");
    }
    assert!(
        s.contains(
            "-p 127.0.0.1:1337:1337 -p 127.0.0.1:1340:1340 -p 127.0.0.1:6901-6905:6901-6905"
        ),
        "{s}"
    );
    assert_eq!(s.matches(" -p ").count(), 3, "{s}");
}
