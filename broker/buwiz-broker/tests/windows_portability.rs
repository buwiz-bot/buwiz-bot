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
