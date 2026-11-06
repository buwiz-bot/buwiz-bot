//! A plugin's key across brokers (tests/fixture-plugin.mjs as the plugin):
//! each broker here has a throwaway `HOME` and a file keystore, and two of
//! them share one, as two brokers on the login Keychain do.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const ID: &str = "app.buwiz.keyfx";

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
        let child = Command::new(env!("CARGO_BIN_EXE_buwiz-broker"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", root)
            .env("BUWIZ_KEYSTORE", format!("file:{}", keys.display()))
            .env("BUWIZ_TRUST_FILE", root.join("trust.json"))
            .env("BUWIZ_BROKER_HUB", "1")
            .env("BUWIZ_BROKER_V1_HOST", "1")
            .env("MOCK", "1")
            // a build with the stub sign-in starts only with this set
            .env("BUWIZ_TEST_GOOGLE", "http://127.0.0.1:9")
            .args(["--port", &port.to_string(), "--no-node", "--assets"])
            .arg(root)
            .arg("--data")
            .arg(&data)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let b = Broker { child, port, data };
        b.until(|b| b.call("GET", "/v1/host/plugins", None).0 == 200, "the broker to answer");
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
        let (_, list) = self.call("GET", "/v1/host/plugins", None);
        let list = list.get("plugins").unwrap_or(&list).clone();
        list.as_array().and_then(|l| l.iter().find(|p| p["id"] == ID)).map_or(Value::Null, |p| p["state"].clone())
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
    let seed = buwiz_protocol::keys::b64(&[7; 32]);
    let public = buwiz_protocol::keys::SessionKey::from_seed(&seed).unwrap().public();
    std::fs::write(root.join("publisher.key"), &seed).unwrap();
    let trust = json!({"v": 1, "publishers": {"keyfx-test-01": {"publisher": "app.buwiz", "alg": "ed25519", "public_key": public}},
        "approved_plugins": [{"id": ID, "publisher_key_id": "keyfx-test-01"}]});
    std::fs::write(root.join("trust.json"), trust.to_string()).unwrap();
    let manifest = json!({"protocol": "1.0.0", "id": ID, "short": "keyfx", "version": "1.0.0", "title": "Key Fixture", "description": "A plugin to open.",
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
        .arg(root.join("signed.json"))
        .output()
        .unwrap();
    assert!(signed.status.success(), "{}", String::from_utf8_lossy(&signed.stderr));
    let plugin = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture-plugin.mjs");
    json!({"command": "node", "args": [plugin], "env": {"FX_MANIFEST": root.join("signed.json")}})
}

/// A second install on another database replaces the key in the keystore
/// they share. The plugin of the first opens again once it restarts: it
/// answered 400 and then 503 for good when its record kept the old key.
#[test]
fn a_replaced_key_does_not_fail_the_installed_plugin() {
    let root = std::env::temp_dir().join(format!("buwiz-plugin-keys-{}", std::process::id()));
    let spec = bed(&root);
    let keys = root.join("keys");
    let owners = Broker::start(&root, "owners", &keys);
    owners.install(&spec);
    assert_eq!(owners.open(), 200);
    let before = std::fs::read_to_string(keys.join(format!("{ID}.key"))).unwrap();

    let test = Broker::start(&root, "test", &keys);
    test.install(&spec);
    drop(test);
    assert_ne!(std::fs::read_to_string(keys.join(format!("{ID}.key"))).unwrap(), before, "the install replaced the key");

    drop(owners);
    let owners = Broker::start(&root, "owners", &keys);
    owners.until(|b| b.state() == "running", "the plugin to run");
    assert_eq!(owners.open(), 200, "it opens under the keystore's key");
    assert_eq!(owners.state(), "running");
    drop(owners);
    let _ = std::fs::remove_dir_all(&root);
}
