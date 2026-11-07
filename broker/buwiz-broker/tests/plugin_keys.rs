//! A plugin's key across brokers (tests/fixture-plugin.mjs as the plugin):
//! each broker here has a throwaway `HOME` and a file keystore, and two of
//! them share one, as two brokers on the login Keychain do.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const ID: &str = "app.buwiz.keyfx";

/// A broker built with the stub sign-in starts only with this set, and one
/// built without it refuses to: `cargo test` and `--all-features` both run here.
fn stub_sign_in(c: &mut Command) {
    if cfg!(feature = "test-google") {
        c.env("BUWIZ_TEST_GOOGLE", "http://127.0.0.1:9");
    }
}

/// `path` to take a broker's stderr, so a test that fails can say why.
fn file_for(path: &Path) -> std::fs::File {
    std::fs::File::create(path).unwrap()
}

/// What a broker wrote to `path`.
fn said(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

struct Broker {
    child: Child,
    port: u16,
    data: PathBuf,
}

impl Drop for Broker {
    fn drop(&mut self) {
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let _ = self.child.wait();
    }
}

impl Broker {
    fn start(root: &Path, data: &str, keys: &Path) -> Broker {
        let port = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let data = root.join(data);
        std::fs::create_dir_all(&data).unwrap();
        let log = root.join(format!("broker-{port}.log"));
        let mut broker = Command::new(env!("CARGO_BIN_EXE_buwiz-broker"));
        broker.env_clear();
        stub_sign_in(&mut broker);
        let child = broker
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", root)
            .env("BUWIZ_KEYSTORE", format!("file:{}", keys.display()))
            .env("BUWIZ_TRUST_FILE", root.join("trust.json"))
            .env("BUWIZ_BROKER_HUB", "1")
            .env("BUWIZ_BROKER_V1_HOST", "1")
            .env("MOCK", "1")
            .args(["--port", &port.to_string(), "--no-node", "--assets"])
            .arg(root)
            .arg("--data")
            .arg(&data)
            .stdout(std::process::Stdio::null())
            .stderr(file_for(&log))
            .spawn()
            .unwrap();
        let mut b = Broker { child, port, data };
        let start = Instant::now();
        while b.call("GET", "/v1/host/plugins", None).0 != 200 {
            let gone = b.child.try_wait().unwrap();
            assert!(gone.is_none() && start.elapsed() < Duration::from_secs(20), "the broker on {port} never answered ({gone:?}):\n{}", said(&log));
            std::thread::sleep(Duration::from_millis(50));
        }
        b
    }

    fn until(&self, ok: impl Fn(&Broker) -> bool, what: &str) {
        let start = Instant::now();
        while !ok(self) {
            assert!(start.elapsed() < Duration::from_secs(20), "waited for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let Ok(token) = std::fs::read_to_string(self.data.join("host-token")) else { return (0, Value::Null) };
        let Ok(mut s) = TcpStream::connect(("127.0.0.1", self.port)) else { return (0, Value::Null) };
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let body = body.map(Value::to_string).unwrap_or_default();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nx-buwiz-host: {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
            token.trim(),
            body.len()
        );
        s.write_all(head.as_bytes()).unwrap();
        s.write_all(body.as_bytes()).unwrap();
        let mut raw = String::new();
        let _ = s.read_to_string(&mut raw);
        let (head, rest) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
        (head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0), serde_json::from_str(rest).unwrap_or(Value::Null))
    }

    fn install(&self, spec: &Value) {
        let (_, inspected) = self.call("POST", "/v1/host/plugins/inspect", Some(spec));
        let mut body = spec.clone();
        body["manifest_sha256"] = inspected["manifest_sha256"].clone();
        let (status, answer) = self.call("POST", "/v1/host/plugins/install", Some(&body));
        assert_eq!((status, &answer["state"]), (200, &json!("running")), "{inspected} {answer}");
    }

    fn state(&self) -> Value {
        self.listed()["state"].clone()
    }

    fn listed(&self) -> Value {
        let (_, list) = self.call("GET", "/v1/host/plugins", None);
        let list = list.get("plugins").unwrap_or(&list).clone();
        list.as_array().and_then(|l| l.iter().find(|p| p["id"] == ID)).cloned().unwrap_or(Value::Null)
    }

    fn code(&self) -> Value {
        self.listed()["code"].clone()
    }

    /// The key `v1_plugins` records.
    fn record(&self) -> String {
        let db = rusqlite::Connection::open(self.data.join("generated/app.db")).unwrap();
        db.query_row("SELECT plugin_pub FROM v1_plugins WHERE id=?", [ID], |r| r.get(0)).unwrap()
    }

    /// Opens the fixture's app: the answer's status.
    fn open(&self) -> u16 {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let open_id = buwiz_protocol::keys::b64(&buwiz_protocol::keys::random::<16>());
        let ks_pub = buwiz_protocol::keys::b64(&buwiz_protocol::keys::random::<32>());
        let perms_sha256 = buwiz_protocol::keys::sha256_hex(b"{}");
        let body = json!({"plugin": ID, "app": "home", "open_id": open_id, "ks_pub": ks_pub, "perms_sha256": perms_sha256, "not_after": now + 3_600_000});
        self.call("POST", "/v1/host/sessions/open", Some(&body)).0
    }
}

/// A folder with a publisher, a trust file and the fixture's signed
/// manifest: the plugin's spawn spec.
fn bed(root: &Path) -> Value {
    let _ = std::fs::remove_dir_all(root);
    std::fs::create_dir_all(root).unwrap();
    signed(root, "1.0.0", "signed.json")
}

/// The fixture at `version`, signed into `out`: its spawn spec.
fn signed(root: &Path, version: &str, out: &str) -> Value {
    let seed = buwiz_protocol::keys::b64(&[7; 32]);
    let public = buwiz_protocol::keys::SessionKey::from_seed(&seed).unwrap().public();
    // a key file is the owner's alone, whatever the umask: sign-manifest checks
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(root.join("publisher.key")).unwrap().write_all(seed.as_bytes()).unwrap();
    let trust = json!({"v": 1, "publishers": {"keyfx-test-01": {"publisher": "app.buwiz", "alg": "ed25519", "public_key": public}},
        "approved_plugins": [{"id": ID, "publisher_key_id": "keyfx-test-01"}]});
    std::fs::write(root.join("trust.json"), trust.to_string()).unwrap();
    let manifest = json!({"protocol": "1.0.0", "id": ID, "short": "keyfx", "version": version, "title": "Key Fixture", "description": "A plugin to open.",
        "publisher": {"id": "app.buwiz", "key_id": "keyfx-test-01", "name": "Fixture"},
        "apps": [{"id": "home", "title": "Home", "description": "An app to open.", "entry": "buwiz-app://home", "mime": "text/x-nsh-tsx", "sdk": "1",
            "fingerprint": format!("sha256:{}", "0".repeat(64)), "placement": ["full"], "permissions": {}}]});
    std::fs::write(root.join("manifest.json"), manifest.to_string()).unwrap();
    std::fs::write(root.join("home.tsx"), "export default function App() { return null; }\n").unwrap();
    let signed = Command::new(env!("CARGO_BIN_EXE_buwiz-broker"))
        .arg("sign-manifest")
        .arg(root.join("manifest.json"))
        .arg("--key-file")
        .arg(root.join("publisher.key"))
        .arg("--app")
        .arg(format!("home={}", root.join("home.tsx").display()))
        .arg("--out")
        .arg(root.join(out))
        .output()
        .unwrap();
    assert!(signed.status.success(), "{}", String::from_utf8_lossy(&signed.stderr));
    let plugin = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture-plugin.mjs");
    json!({"command": "node", "args": [plugin], "env": {"FX_MANIFEST": root.join(out)}})
}

/// A second install on another database replaces the key in the keystore
/// they share. The record pins the old one: the first broker's plugin stops
/// with a reason, an enable (an agent's) changes nothing, and only a
/// person's Retry takes the new key.
#[test]
fn a_replaced_key_stops_the_plugin_until_a_persons_retry() {
    let root = std::env::temp_dir().join(format!("buwiz-plugin-keys-{}", std::process::id()));
    let spec = bed(&root);
    let keys = root.join("keys");
    let owners = Broker::start(&root, "owners", &keys);
    owners.install(&spec);
    assert_eq!(owners.open(), 200);
    let pinned = owners.record();
    let before = std::fs::read_to_string(keys.join(format!("{ID}.key"))).unwrap();

    let test = Broker::start(&root, "test", &keys);
    test.install(&spec);
    drop(test);
    assert_ne!(std::fs::read_to_string(keys.join(format!("{ID}.key"))).unwrap(), before, "the install replaced the key");

    drop(owners);
    let owners = Broker::start(&root, "owners", &keys);
    owners.until(|b| b.state() == "failed", "the plugin to stop");
    assert_eq!(owners.code(), "plugin_key_changed");
    assert_eq!(owners.record(), pinned, "a spawn rewrites nothing");
    assert_eq!(owners.open(), 503);

    let (_, enabled) = owners.call("POST", &format!("/v1/host/plugins/{ID}/enable"), Some(&json!({})));
    assert_eq!(enabled["code"], "plugin_key_changed", "an enable takes no key: {enabled}");
    assert_eq!(owners.record(), pinned);

    // a restart doesn't take it either
    drop(owners);
    let owners = Broker::start(&root, "owners", &keys);
    owners.until(|b| b.state() == "failed", "the plugin to stop again");
    assert_eq!(owners.record(), pinned);

    let (_, retried) = owners.call("POST", &format!("/v1/host/plugins/{ID}/enable"), Some(&json!({"adopt_key": true})));
    assert_eq!(retried["state"], "running", "{retried}");
    assert_ne!(owners.record(), pinned);
    assert_eq!(owners.open(), 200, "it opens under the keystore's key");

    drop(owners);
    let owners = Broker::start(&root, "owners", &keys);
    owners.until(|b| b.state() == "running", "the plugin to run after a restart");
    assert_eq!(owners.open(), 200, "the key stays taken");
    drop(owners);
    let _ = std::fs::remove_dir_all(&root);
}

/// An update holds the pin too: one under a replaced key is refused and
/// stops the plugin, so Retry is on offer, and goes through after it.
#[test]
fn an_update_under_a_replaced_key_waits_for_retry() {
    let root = std::env::temp_dir().join(format!("buwiz-plugin-update-{}", std::process::id()));
    let spec = bed(&root);
    let newer = signed(&root, "1.0.1", "newer.json");
    let keys = root.join("keys");
    let b = Broker::start(&root, "data", &keys);
    b.install(&spec);
    let pinned = b.record();
    std::fs::write(keys.join(format!("{ID}.key")), buwiz_protocol::keys::b64(&[3; 32])).unwrap();

    let (_, inspected) = b.call("POST", "/v1/host/plugins/inspect", Some(&newer));
    let mut body = newer.clone();
    body["manifest_sha256"] = inspected["manifest_sha256"].clone();
    let (status, refused) = b.call("POST", "/v1/host/plugins/install", Some(&body));
    assert_eq!((status, &refused["code"]), (400, &json!("plugin_key_changed")), "{refused}");
    assert_eq!(b.record(), pinned);
    assert_eq!(b.listed()["version"], "1.0.0");
    assert_eq!((b.state(), b.code()), (json!("failed"), json!("plugin_key_changed")));

    let (_, retried) = b.call("POST", &format!("/v1/host/plugins/{ID}/enable"), Some(&json!({"adopt_key": true})));
    assert_eq!(retried["state"], "running", "{retried}");
    b.install(&newer);
    assert_eq!(b.listed()["version"], "1.0.1");
    drop(b);
    let _ = std::fs::remove_dir_all(&root);
}

/// Retry writes the pin only once the plugin has passed its checks.
#[test]
fn a_retry_that_fails_the_gate_leaves_the_pin() {
    let root = std::env::temp_dir().join(format!("buwiz-plugin-gate-{}", std::process::id()));
    let spec = bed(&root);
    signed(&root, "1.0.1", "newer.json");
    let keys = root.join("keys");
    let b = Broker::start(&root, "data", &keys);
    b.install(&spec);
    let pinned = b.record();
    drop(b);
    std::fs::write(keys.join(format!("{ID}.key")), buwiz_protocol::keys::b64(&[3; 32])).unwrap();
    let good = std::fs::read(root.join("signed.json")).unwrap();
    std::fs::copy(root.join("newer.json"), root.join("signed.json")).unwrap();

    let b = Broker::start(&root, "data", &keys);
    b.until(|b| b.code() == "plugin_key_changed", "the plugin to stop");
    let (_, retried) = b.call("POST", &format!("/v1/host/plugins/{ID}/enable"), Some(&json!({"adopt_key": true})));
    assert_ne!(retried["state"], "running", "{retried}");
    assert_eq!(b.record(), pinned, "the served manifest is not the installed one: {retried}");

    // the plugin put right: the stop comes back, and Retry goes through
    std::fs::write(root.join("signed.json"), good).unwrap();
    let (_, enabled) = b.call("POST", &format!("/v1/host/plugins/{ID}/enable"), Some(&json!({})));
    assert_eq!(enabled["code"], "plugin_key_changed", "{enabled}");
    let (_, retried) = b.call("POST", &format!("/v1/host/plugins/{ID}/enable"), Some(&json!({"adopt_key": true})));
    assert_eq!(retried["state"], "running", "{retried}");
    assert_ne!(b.record(), pinned);
    drop(b);
    let _ = std::fs::remove_dir_all(&root);
}

/// `adopt_key` takes a key only from a plugin stopped for that reason, and
/// a keystore item that is no key fails closed, Retry or not.
#[test]
fn what_is_no_key_fails_closed() {
    let root = std::env::temp_dir().join(format!("buwiz-plugin-nokey-{}", std::process::id()));
    let spec = bed(&root);
    let keys = root.join("keys");
    let b = Broker::start(&root, "data", &keys);
    b.install(&spec);
    let pinned = b.record();
    drop(b);
    std::fs::write(keys.join(format!("{ID}.key")), "not a seed").unwrap();

    let b = Broker::start(&root, "data", &keys);
    b.until(|b| b.state() == "failed", "the plugin to stop");
    assert_eq!(b.code(), "plugin_key_invalid");
    let (_, retried) = b.call("POST", &format!("/v1/host/plugins/{ID}/enable"), Some(&json!({"adopt_key": true})));
    assert_eq!(retried["code"], "plugin_key_invalid", "{retried}");
    assert!(retried["message"].as_str().unwrap().contains("Uninstall the plugin, then install it again"), "{retried}");
    assert_eq!(b.record(), pinned);

    // an update can't write a record from it
    let newer = signed(&root, "1.0.1", "newer.json");
    let (_, inspected) = b.call("POST", "/v1/host/plugins/inspect", Some(&newer));
    let mut body = newer.clone();
    body["manifest_sha256"] = inspected["manifest_sha256"].clone();
    assert_eq!(b.call("POST", "/v1/host/plugins/install", Some(&body)).1["code"], "plugin_key_invalid");
    assert_eq!(b.record(), pinned);

    // what the message says works
    assert_eq!(b.call("POST", &format!("/v1/host/plugins/{ID}/uninstall"), Some(&json!({}))).0, 200);
    b.install(&spec);
    assert_eq!(b.open(), 200);
    drop(b);
    let _ = std::fs::remove_dir_all(&root);
}

/// The broker with nothing but `env`, on a throwaway `HOME`.
fn bare(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_buwiz-broker"));
    c.env_clear().env("PATH", std::env::var_os("PATH").unwrap()).env("HOME", root).envs(env.iter().copied()).args(args);
    stub_sign_in(&mut c);
    c
}

/// `keystore-check` with no `--data` goes by the same rule as a broker.
#[test]
fn keystore_check_keeps_to_no_keychain() {
    let root = std::env::temp_dir().join(format!("buwiz-plugin-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let out = bare(&root, &["keystore-check", ID], &[("BUWIZ_NO_KEYCHAIN", "1")]).output().unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("no key in the file"), "{said}");
    let _ = std::fs::remove_dir_all(&root);
}

/// `helper` with a bare `--db` name and no `--data` has no folder for its
/// keys: it refuses, and the working folder stays empty.
#[test]
fn a_helper_with_a_bare_db_name_needs_its_data_dir() {
    let root = std::env::temp_dir().join(format!("buwiz-plugin-bare-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let port = free_port();
    let log = std::env::temp_dir().join(format!("buwiz-plugin-bare-{}.log", std::process::id()));
    let mut helper = bare(&root, &["helper", "--db", "app.db", "--port", &port], &[("BUWIZ_LINE_TOKEN", "t")])
        .current_dir(&root)
        .stdout(std::process::Stdio::null())
        .stderr(file_for(&log))
        .spawn()
        .unwrap();
    let start = Instant::now();
    let status = loop {
        match helper.try_wait().unwrap() {
            Some(status) => break Some(status),
            None if start.elapsed() > Duration::from_secs(20) => break None,
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    let _ = helper.kill();
    let _ = helper.wait();
    assert_eq!(status.and_then(|s| s.code()), Some(2), "it refuses: {}", said(&log));
    assert!(said(&log).contains("needs --data"), "for this reason: {}", said(&log));
    let _ = std::fs::remove_file(&log);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "nothing lands in the working folder");
    let _ = std::fs::remove_dir_all(&root);
}

/// Whether `broker`, listening on `port`, took the Keychain: it leaves the
/// marker of its one-time key move beside the database, and with no plugin
/// installed that move reads nothing from the Keychain.
fn took_the_keychain(db: &Path, port: &str, mut broker: Command) -> bool {
    let log = db.with_file_name(format!("broker-{port}.log"));
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    let mut child = broker.stdout(std::process::Stdio::null()).stderr(file_for(&log)).spawn().unwrap();
    let start = Instant::now();
    // an answer, not a connection: it binds its port before its plugin host
    while !answers(port) {
        let gone = child.try_wait().unwrap();
        assert!(gone.is_none() && start.elapsed() < Duration::from_secs(20), "the broker on {port} never answered ({gone:?}):\n{}", said(&log));
        std::thread::sleep(Duration::from_millis(50));
    }
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let _ = child.wait();
    db.with_file_name(buwiz_broker::helper::keystore::MOVED_MARKER).exists()
}

/// Whether something on `port` answers a request.
fn answers(port: &str) -> bool {
    let Ok(mut s) = TcpStream::connect(format!("127.0.0.1:{port}")) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    s.read(&mut [0; 1]).is_ok_and(|n| n > 0)
}

fn free_port() -> String {
    TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port().to_string()
}

/// The real brokers tests/bundle.rs and tests/hub.rs start name no
/// `BUWIZ_KEYSTORE`: on a data dir of theirs, nothing goes near the Keychain.
#[test]
fn a_broker_on_its_own_data_dir_stays_off_the_keychain() {
    let root = std::env::temp_dir().join(format!("buwiz-plugin-own-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let port = free_port();
    let args = ["--port", &port, "--no-node", "--assets", root.to_str().unwrap(), "--data", data.to_str().unwrap()];
    let db = data.join("generated/app.db");
    assert!(!took_the_keychain(&db, &port, bare(&root, &args, &[("BUWIZ_BROKER_HUB", "1"), ("MOCK", "1")])));
    assert!(db.exists(), "its plugin host ran");
    let _ = std::fs::remove_dir_all(&root);
}

/// `helper --db` takes any file, so one that merely sits at the owner's
/// path says nothing about whose broker this is: only `--data` does.
#[test]
fn a_helper_goes_by_its_data_dir_not_the_shape_of_its_db_path() {
    let root = std::env::temp_dir().join(format!("buwiz-plugin-helper-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let owners = buwiz_protocol::broker_dir::new_dir(&root);
    let db = owners.join("generated/app.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let env = [("BUWIZ_LINE_TOKEN", "t")];
    let port = free_port();
    let args = ["helper", "--db", db.to_str().unwrap(), "--port", &port];
    assert!(!took_the_keychain(&db, &port, bare(&root, &args, &env)));
    let _ = std::fs::remove_dir_all(&root);
}

fn helper(root: &Path, cwd: &Path, args: &[&str]) -> Child {
    let mut c = Command::new(env!("CARGO_BIN_EXE_buwiz-broker"));
    c.env_clear();
    stub_sign_in(&mut c);
    c.env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", root)
        .env("BUWIZ_LINE_TOKEN", "test")
        .env("BUWIZ_NO_KEYCHAIN", "1")
        .env("BUWIZ_KEYSTORE", format!("file:{}", root.join("keys").display()))
        .env("BUWIZ_TRUST_FILE", root.join("trust.json"))
        .arg("helper")
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(file_for(&root.join("helper.log")))
        .spawn()
        .unwrap()
}

/// Keys never land in the working folder: a `--db` that names no folder is
/// refused, with what to pass instead, and passing it works.
#[test]
fn a_helper_db_that_names_no_folder_needs_data() {
    let root = std::env::temp_dir().join(format!("buwiz-helper-db-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (cwd, data) = (root.join("cwd"), root.join("data"));
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    for db in ["app.db", "./app.db", "././app.db"] {
        let mut child = helper(&root, &cwd, &["--db", db]);
        let start = Instant::now();
        let code = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status.code();
            }
            if start.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{db}: the helper started");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(code, Some(2), "{db}");
        let log = said(&root.join("helper.log"));
        assert!(log.contains("needs --data DIR"), "{db}: {log}");
        assert_eq!(std::fs::read_dir(&cwd).unwrap().count(), 0, "{db}: it wrote to the working folder");
    }
    let mut child = helper(&root, &cwd, &["--db", "./app.db", "--data", data.to_str().unwrap()]);
    let mut first = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(child.stdout.take().unwrap()), &mut first).unwrap();
    let _ = child.kill();
    let _ = child.wait();
    assert!(first.starts_with("buwiz-broker helper on"), "{first:?}: {}", said(&root.join("helper.log")));
    let _ = std::fs::remove_dir_all(&root);
}
