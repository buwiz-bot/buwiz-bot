#![cfg(windows)]

use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::{Request, service::service_fn};
use hyper_util::rt::TokioIo;
use buwiz_broker::helper::keystore::Keystore;
use buwiz_broker::leaf::{self, Leaf};
use buwiz_broker::office::workspace::Workspace;
use buwiz_computer::{provider_for, Link, Machine, Network, Provider, ProviderKind, Reach, RemoteProvider, Secret, Spec};
use buwiz_computer::remote::{self, ssh, Asked};

struct Folder(PathBuf);

impl Folder {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("buwiz-windows-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_ssh_unavailable<T>(result: buwiz_computer::Result<T>) {
    let problem = result.err().expect("SSH computers must be unavailable");
    assert_eq!(problem.code(), "ssh_unavailable");
    assert_eq!(problem.sentence(), "SSH computers aren't available on Windows yet");
}

fn ssh_machine(dir: PathBuf) -> Machine {
    Machine {
        provider: ProviderKind::Remote,
        reach: Reach::Ssh { host: "host.example".into(), user: Some("person".into()), port: None },
        image: None,
        token: Secret::new(""),
        viewer_password: Secret::new(""),
        dir,
        link: Some(Link { boxed: false, pin: None, key: None }),
        egress: None,
        seen: None,
    }
}

#[tokio::test]
async fn ssh_remote_add_and_existing_computers_are_unavailable() {
    let dir = Folder::new("ssh-remote");
    let computer = dir.0.join("computer");
    let asked = Asked { url: "ssh://person@host.example".into(), key_file: Some(dir.0.join("missing-key")), ..Asked::default() };
    assert_ssh_unavailable(asked.machine(&computer));
    let provider = RemoteProvider::new().with_programs(dir.0.join("missing-ssh"), dir.0.join("missing-keyscan"));
    let mut machine = ssh_machine(computer.clone());
    for link in [None, machine.link.clone()] {
        machine.link = link;
        assert_ssh_unavailable(provider.trial(&machine).await);
        assert_ssh_unavailable(provider.probe(&machine).await);
        assert_ssh_unavailable(provider.power(&machine).await);
        assert_ssh_unavailable(provider.wake(&machine).await);
        assert_ssh_unavailable(provider.read_file(&machine, "example.txt").await);
    }
    assert!(!computer.exists());
}

#[tokio::test]
async fn ssh_primitives_refuse_before_spawning_or_staging_keys() {
    let dir = Folder::new("ssh-programs");
    let program = dir.0.join("missing-program");
    let computer = dir.0.join("computer");
    let at = ssh::At { host: "host.example".into(), user: None, port: None };
    assert_ssh_unavailable(ssh::scan(&program, &at.host, at.port).await);
    match ssh::Master::open(&program, &computer, &at, "", None, &[]).await {
        Err(ssh::Failed::Said(problem)) => assert_ssh_unavailable::<()>(Err(problem)),
        _ => panic!("SSH must be refused before opening a master"),
    }
    assert!(!computer.exists());
}

#[tokio::test]
async fn ssh_cloud_providers_refuse_add_and_existing_computers() {
    let dir = Folder::new("ssh-cloud");
    let spec = Spec { id: "windows-example".into(), dir: dir.0.join("computer"), network: Network::Plain };
    for kind in [ProviderKind::Ec2, ProviderKind::Boat] {
        let provider = provider_for(kind);
        assert_ssh_unavailable(provider.check().await);
        assert_ssh_unavailable(provider.verify().await);
        assert_ssh_unavailable(provider.create(&spec).await);
        let mut machine = ssh_machine(spec.dir.clone());
        machine.provider = kind;
        machine.reach = match kind {
            ProviderKind::Ec2 => Reach::Instance { id: "i-example".into(), region: "ap-southeast-1".into() },
            _ => Reach::Sandbox { id: "example".into() },
        };
        assert_ssh_unavailable(provider.power(&machine).await);
        assert_ssh_unavailable(provider.wake(&machine).await);
        assert_ssh_unavailable(provider.read_file(&machine, "example.txt").await);
        assert_ssh_unavailable(provider.destroy(&machine).await);
    }
    assert!(!spec.dir.exists());
}

#[tokio::test]
async fn https_remote_computers_remain_available() {
    let dir = Folder::new("https-remote");
    let provider = RemoteProvider::new();
    provider.check().await.unwrap();
    let asked = Asked {
        url: "https://host.example".into(),
        token: Some(Secret::new("synthetic-token")),
        viewer_password: Some(Secret::new("synthetic-password")),
        ..Asked::default()
    };
    let machine = asked.machine(&dir.0).unwrap();
    assert!(matches!(machine.reach, Reach::Url { .. }));
    assert_eq!(provider.power(&machine).await.unwrap(), buwiz_computer::Power::Asleep);
    assert!(remote::target("ssh://-bad-host").is_err());
}

#[test]
fn file_keystore_round_trips_and_rejects_invalid_names() {
    let dir = Folder::new("keys");
    let store = Keystore::File(dir.0.join("keys"));
    let id = "com.example.windows";
    assert_eq!(store.get(id).unwrap(), None);
    let seed = buwiz_protocol::keys::b64(&[7; 32]);
    store.put(id, &seed).unwrap();
    assert_eq!(store.get(id).unwrap(), Some(seed));
    assert!(store.put("../escape", "invalid").is_err());
    store.remove(id).unwrap();
    assert_eq!(store.get(id).unwrap(), None);
}

#[test]
fn default_keystore_is_file_even_for_the_owners_folder() {
    let home = std::path::Path::new(r"C:\buwiz-test-home");
    for data in [buwiz_protocol::broker_dir::new_dir(home), buwiz_protocol::broker_dir::old_dir(home)] {
        assert_eq!(Keystore::for_data("", false, &data, home).unwrap(), Keystore::File(data.join("keys")));
        assert_eq!(Keystore::for_data("keychain", false, &data, home).unwrap(), Keystore::Keychain);
        assert_eq!(Keystore::for_data("file:C:\\custom-keys", false, &data, home).unwrap(), Keystore::File(r"C:\custom-keys".into()));
        assert!(Keystore::for_data("invalid", false, &data, home).is_err());
    }
}

#[test]
fn workspace_creates_empty_files_without_overwriting_or_escaping() {
    let dir = Folder::new("office");
    let mut workspace = Workspace::at(&dir.0).unwrap();
    assert!(workspace.files(None, None).unwrap()["files"].as_array().unwrap().is_empty());
    workspace.create("empty.docx", b"").unwrap();
    assert_eq!(std::fs::read(dir.0.join("empty.docx")).unwrap(), b"");
    assert!(workspace.create("empty.docx", b"replacement").is_err());
    assert!(workspace.create("../outside.docx", b"escape").is_err());
    assert!(workspace.open("missing.docx").is_err());
    assert!(buwiz_broker::hub::library::appmeta::mtime_ms(&dir.0.join("empty.docx")).unwrap() > 0.0);
}

#[test]
fn tool_paths_keep_drive_letters_and_semicolon_separators() {
    let path = r"C:\Windows\System32;C:\Program Files\nodejs";
    let home = r"C:\Users\Tester";
    let extended = buwiz_mcp::tool_path(path, home);
    let paths: Vec<PathBuf> = std::env::split_paths(&extended).collect();
    assert_eq!(paths[0], PathBuf::from(r"C:\Windows\System32"));
    assert_eq!(paths[1], PathBuf::from(r"C:\Program Files\nodejs"));
    assert!(paths.contains(&PathBuf::from(home).join(".cargo/bin")));
    assert_eq!(buwiz_mcp::tool_path("", ""), "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_routes_reject_windows_paths_after_url_decoding() {
    let dir = Folder::new("file-routes");
    for folder in ["assets", "attachments", "avatars", "files"] {
        std::fs::create_dir_all(dir.0.join("generated").join(folder)).unwrap();
        std::fs::write(dir.0.join("generated").join(folder).join("ok.png"), b"image").unwrap();
    }
    std::fs::write(dir.0.join("generated/outside.png"), b"synthetic canary").unwrap();
    let leaf = Arc::new(Leaf::new(dir.0.clone(), Vec::new()));
    let (client, server) = tokio::io::duplex(4096);
    let serving = tokio::spawn(async move {
        hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(server), service_fn(move |req: Request<hyper::body::Incoming>| {
            let leaf = leaf.clone();
            async move {
                let route = leaf::route(req.method(), req.uri().path()).unwrap();
                Ok::<_, std::convert::Infallible>(leaf.serve(route, req).await)
            }
        })).await.unwrap();
    });
    let (mut send, connection) = hyper::client::conn::http1::handshake(TokioIo::new(client)).await.unwrap();
    let connected = tokio::spawn(connection);
    for prefix in ["/asset/generated/assets/", "/files/attachments/", "/files/avatars/", "/file/raw/", "/file/preview/"] {
        for (path, expected) in [("ok.png", 200), ("missing.png", 404), ("%GG", 500), ("..%5Coutside.png", 404), ("C%3Aoutside.png", 404), ("%5C%5Cserver%5Cshare", 404), ("ok.png%3Astream", 404)] {
            let url = format!("{prefix}{path}");
            let req = Request::builder().uri(&url).body(Empty::<Bytes>::new()).unwrap();
            let response = send.send_request(req).await.unwrap();
            let status = response.status().as_u16();
            response.into_body().collect().await.unwrap();
            assert_eq!(status, expected, "{url}");
        }
    }
    drop(send);
    connected.await.unwrap().unwrap();
    serving.await.unwrap();
}
