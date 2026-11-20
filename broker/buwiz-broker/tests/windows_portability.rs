#![cfg(windows)]

use std::path::PathBuf;

use buwiz_broker::helper::keystore::Keystore;
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
