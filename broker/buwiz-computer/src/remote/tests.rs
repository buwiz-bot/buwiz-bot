use super::*;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("buwiz-remote-{name}-{}-{}", std::process::id(), crate::random_hex(4).unwrap()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// An OpenSSH key's text with `cipher` in its cipher field.
fn openssh_key(cipher: &str) -> String {
    let mut raw = b"openssh-key-v1\0".to_vec();
    raw.extend_from_slice(&(cipher.len() as u32).to_be_bytes());
    raw.extend_from_slice(cipher.as_bytes());
    let body = base64::engine::general_purpose::STANDARD.encode(raw);
    format!("-----BEGIN OPENSSH PRIVATE KEY-----\n{body}\n-----END OPENSSH PRIVATE KEY-----\n")
}

fn key_file(d: &Path, cipher: &str) -> PathBuf {
    let path = d.join("id");
    std::fs::write(&path, openssh_key(cipher)).unwrap();
    path
}

const PIN: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA==";

#[test]
fn only_what_is_built_is_a_target_and_each_no_says_why() {
    assert_eq!(target("ssh://me@host.example:2222/").unwrap(), Reach::Ssh { host: "host.example".into(), user: Some("me".into()), port: Some(2222) });
    assert_eq!(target("me@10.0.0.2").unwrap(), Reach::Ssh { host: "10.0.0.2".into(), user: Some("me".into()), port: None });
    assert_eq!(target(" https://box.example.com/ ").unwrap(), Reach::Url { url: "https://box.example.com".into() });
    assert_eq!(target("https://box.example.com:8443").unwrap(), Reach::Url { url: "https://box.example.com:8443".into() });
    for (bad, word) in [
        ("http://box.example.com", "in the open"),
        ("vnc://h", "VNC and RDP"),
        ("rdp://h", "VNC and RDP"),
        ("ftp://h", "ssh:// or https://"),
        ("host.example", "ssh://user@host"),
        ("https://u:p@box.example.com", "isn't a port"),
        ("https://box.example.com/path", "goes by a name"),
        ("https://box.example.com?x=1", "goes by a name"),
        ("https://10.0.0.2", "goes by a name"),
    ] {
        let said = target(bad).unwrap_err().sentence();
        assert!(said.contains(word), "{bad}: {said}");
    }
}

#[test]
fn nothing_typed_can_become_a_flag_to_ssh() {
    for bad in ["ssh://-oProxyCommand=x@host", "ssh://me@-oProxyCommand=x", "ssh://me@ho st", "ssh://me;rm@host", "ssh://me@host$(x)", "-oProxyCommand=x@host"] {
        assert!(target(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_box_needs_both_its_secrets_and_a_plain_host_neither() {
    let d = dir("asked");
    let key = key_file(&d, "none");
    let asked = |token: Option<&str>, password: Option<&str>| Asked { url: "ssh://me@h".into(), token: token.map(Secret::new), viewer_password: password.map(Secret::new), key_file: Some(key.clone()), pin: Some(PIN.into()) };
    assert!(asked(Some("t"), Some("v")).machine(&d).unwrap().has_screen());
    assert!(!asked(None, None).machine(&d).unwrap().has_screen());
    assert!(!asked(Some(" "), Some("")).machine(&d).unwrap().has_screen(), "an empty box is no box");
    assert!(asked(Some("t"), None).machine(&d).is_err());
    assert!(asked(None, Some("v")).machine(&d).is_err());
    let https = Asked { url: "https://box.example.com".into(), ..Asked::default() };
    assert!(https.machine(&d).is_err(), "https is only ever a box");
}

#[test]
fn a_pin_is_checked_before_it_is_kept() {
    let d = dir("pin");
    let key = key_file(&d, "none");
    let ssh = Asked { url: "ssh://me@h".into(), key_file: Some(key), pin: Some("ssh-ed25519 AAAA\n@revoked * ssh-ed25519 AAAA".into()), ..Asked::default() };
    assert!(ssh.machine(&d).is_err());
    let https = Asked { url: "https://a.example".into(), token: Some(Secret::new("t")), viewer_password: Some(Secret::new("v")), pin: Some("x".into()), ..Asked::default() };
    assert!(https.machine(&d).is_err());
}

#[test]
fn a_key_with_a_passphrase_is_refused_and_so_is_what_isnt_a_key() {
    let d = dir("key");
    let locked = key_file(&d, "aes256-ctr");
    assert!(read_key(&locked).unwrap_err().sentence().contains("passphrase"));
    let open = key_file(&d, "none");
    assert!(read_key(&open).is_ok());
    let old = d.join("old");
    std::fs::write(&old, "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\n-----END RSA PRIVATE KEY-----\n").unwrap();
    assert!(read_key(&old).unwrap_err().sentence().contains("passphrase"));
    let junk = d.join("junk");
    std::fs::write(&junk, "not a key").unwrap();
    assert!(read_key(&junk).is_err());
    assert!(read_key(&d).is_err(), "a folder");
    assert!(read_key(&d.join("nothing")).is_err());
    let huge = d.join("huge");
    std::fs::write(&huge, vec![b'x'; 70_000]).unwrap();
    assert!(read_key(&huge).is_err());
}

#[test]
fn free_ports_stay_in_the_range_the_shell_is_kept_off() {
    let ports = free_ports(7).unwrap();
    assert_eq!(ports.len(), 7);
    assert!(ports.iter().all(|p| (PORT_FIRST..=DOCKER_PORT_LAST).contains(p)), "{ports:?}");
    let mut unique = ports.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 7);
}

#[test]
fn a_tool_path_maps_to_the_folder_on_a_plain_host() {
    assert_eq!(relative("/workspace/agents/b/x.txt"), "agents/b/x.txt");
    assert_eq!(relative("agents/b/x.txt"), "agents/b/x.txt");
}

fn fake(d: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = d.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn machine(d: &Path) -> Machine {
    let key = key_file(d, "none");
    Asked { url: "ssh://me@h.example".into(), key_file: Some(key), pin: Some(PIN.into()), ..Asked::default() }.machine(d).unwrap()
}

#[tokio::test]
async fn a_scan_is_only_ever_a_question_for_a_person() {
    let d = dir("scan");
    let scan = fake(&d, "keyscan", "echo 'h ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA=='");
    // `ssh` must not run: no key is trusted yet
    let never = fake(&d, "fake-ssh", &format!("touch {}/ssh-ran", d.display()));
    let provider = RemoteProvider::new().with_programs(never, scan);
    let mut m = machine(&d);
    m.link.as_mut().unwrap().pin = None;
    assert!(matches!(provider.probe(&m).await.unwrap(), Probed::Trust { .. }));
    assert!(!d.join("ssh-ran").exists());
    // and the record can't connect with no pin, however it's asked
    assert!(provider.wake(&m).await.is_err());
    assert!(!d.join("ssh-ran").exists());
}

#[tokio::test]
async fn a_failed_login_leaves_no_key_on_disk_and_no_connection() {
    let d = dir("login");
    let ssh = fake(&d, "fake-ssh", "echo 'Permission denied (publickey).' >&2; exit 255");
    let provider = RemoteProvider::new().with_programs(ssh, PathBuf::from("/nonexistent"));
    let m = machine(&d);
    let said = provider.wake(&m).await.unwrap_err();
    assert_eq!(said.code(), "remote");
    assert!(said.sentence().contains("didn't accept"), "{said}");
    assert!(!d.join("ssh/key").exists(), "the key stayed");
    assert_eq!(provider.power(&m).await.unwrap(), Power::Asleep);
    assert_eq!(std::fs::read_to_string(d.join("ssh/known_hosts")).unwrap(), format!("computer {PIN}\n"));
}

#[tokio::test]
async fn a_wrong_host_key_says_so_and_goes_no_further() {
    let d = dir("hostkey");
    let ssh = fake(&d, "fake-ssh", "echo 'Host key verification failed.' >&2; exit 255");
    let provider = RemoteProvider::new().with_programs(ssh, PathBuf::from("/nonexistent"));
    assert!(provider.wake(&machine(&d)).await.unwrap_err().sentence().contains("isn't the one you trusted"));
}

#[tokio::test]
async fn a_lost_port_is_tried_again_with_others() {
    let d = dir("ports");
    let tries = d.join("tries");
    let body = format!("echo x >> {t}; echo 'bind [127.0.0.1]:47001: Address already in use' >&2; exit 255", t = tries.display());
    let provider = RemoteProvider::new().with_programs(fake(&d, "fake-ssh", &body), PathBuf::from("/nonexistent"));
    let mut m = machine(&d);
    m.link.as_mut().unwrap().boxed = true;
    assert!(provider.wake(&m).await.unwrap_err().sentence().contains("no free ports"));
    assert_eq!(std::fs::read_to_string(&tries).unwrap().lines().count(), 5);
}

#[tokio::test]
async fn asleep_means_every_tool_is_refused_with_no_call_made() {
    let d = dir("asleep");
    let provider = RemoteProvider::new().with_programs(PathBuf::from("/nonexistent"), PathBuf::from("/nonexistent"));
    let m = machine(&d);
    let screen = Screen { display: String::new(), viewer_port: 0, folder: "agents/b".into(), profile: None };
    assert!(matches!(provider.run(&m, &screen, "true", 1000).await, Err(Problem::Asleep)));
    assert!(matches!(provider.screenshot(&m, &screen).await, Err(Problem::Asleep)));
    assert!(matches!(provider.guard(&m).await, Err(Problem::Asleep)));
}

#[tokio::test]
async fn sleeping_and_retiring_never_call_the_far_end() {
    let d = dir("retire");
    let provider = RemoteProvider::new().with_programs(fake(&d, "fake-ssh", &format!("touch {}/ssh-ran", d.display())), PathBuf::from("/nonexistent"));
    let m = machine(&d);
    provider.sleep(&m, Sleep::Deep).await.unwrap();
    provider.destroy(&m).await.unwrap();
    provider.screen_down(&m, "b").await.unwrap();
    assert!(provider.screens(&m).await.unwrap().is_empty(), "a box's other screens aren't ours to clear");
    assert!(!d.join("ssh-ran").exists());
}

#[tokio::test]
async fn the_trait_makes_nothing_and_finds_nothing_lost() {
    let d = dir("make");
    let provider = RemoteProvider::new();
    assert!(provider.create(&Spec { id: "cmp_1".into(), dir: d.clone() }).await.is_err());
    assert_eq!(provider.recover(&Spec { id: "cmp_1".into(), dir: d }, None).await.unwrap(), None);
}

#[tokio::test]
async fn no_outside_check_means_no_and_no_clearing() {
    let d = dir("apart");
    let provider = RemoteProvider::new();
    let m = machine(&d);
    assert!(!provider.users_apart(&m, &["a".to_owned()]).await.unwrap());
    let screen = Screen { display: ":2".into(), viewer_port: 6902, folder: "agents/a".into(), profile: None };
    assert!(matches!(provider.clear_strays(&m, &screen).await, Err(Problem::Unsupported(_))));
}

#[test]
fn what_a_box_says_back_has_this_computers_secrets_taken_out() {
    let d = dir("clean");
    let mut m = machine(&d);
    m.token = Secret::new("tok-123");
    let said: Result<()> = clean(&m, Err(Problem::Guest { status: 500, message: "your token is tok-123".to_owned() }));
    assert_eq!(said.unwrap_err().sentence(), "The computer said: your token is [hidden]");
}
