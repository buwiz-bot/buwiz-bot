use std::cell::RefCell;
use std::collections::HashMap;

use super::*;

const OUTLINE: &str = "M50 10 C75 10 90 30 90 55 C90 80 72 92 50 92 C28 92 10 80 10 55 C10 30 25 10 50 10 Z";

fn fox() -> Value {
    json!({"name": "Fox", "shape": "custom", "color": "#E8772E", "outline": OUTLINE})
}

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

/// A link's files, and what was asked of them.
#[derive(Default)]
struct Site {
    files: HashMap<String, Vec<u8>>,
    asked: RefCell<Vec<(String, usize)>>,
}

impl Site {
    fn with(files: &[(&str, Value)]) -> Self {
        let files = files.iter().map(|(k, v)| ((*k).to_owned(), v.to_string().into_bytes())).collect();
        Site { files, ..Site::default() }
    }

    async fn get(&self, u: Url, max: usize) -> Result<Vec<u8>, String> {
        self.asked.borrow_mut().push((u.to_string(), max));
        self.files.get(u.as_str()).cloned().ok_or_else(|| "no packs at that link".to_owned())
    }
}

const ROOT: &str = "https://example.com/a/";

fn index(packs: Value) -> Value {
    json!({"version": 1, "packs": packs})
}

fn two_packs() -> Site {
    let cat = json!({"name": "Cat", "shape": "custom", "color": "#222222", "outline": OUTLINE});
    Site::with(&[
        (
            "https://example.com/a/index.json",
            index(json!([
                {"id": "animals", "title": "Animals", "file": "packs/animals.json", "groups": ["Originals"]},
                {"id": "pets", "file": "packs/pets.json"},
            ])),
        ),
        ("https://example.com/a/packs/animals.json", json!({"designs": [fox()]})),
        ("https://example.com/a/packs/pets.json", json!({"designs": [cat]})),
    ])
}

async fn read_site(site: &Site, pick: &str) -> Result<Value, String> {
    read(&format!("{ROOT}index.json"), pick, |u, m| site.get(u, m)).await
}

fn names(v: &Value) -> Vec<&str> {
    v["designs"].as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect()
}

#[test]
fn a_link_means_its_index() {
    let raw = |p: &str| format!("https://raw.githubusercontent.com/{p}/index.json");
    assert_eq!(index_url("").unwrap().as_str(), raw("buwiz-bot/avatars/HEAD"));
    assert_eq!(index_url("https://github.com/me/mine").unwrap().as_str(), raw("me/mine/HEAD"));
    assert_eq!(index_url("https://github.com/me/mine.git").unwrap().as_str(), raw("me/mine/HEAD"));
    assert_eq!(index_url("https://github.com/me/mine/tree/dev").unwrap().as_str(), raw("me/mine/dev"));
    assert_eq!(index_url("https://example.com/p/index.json").unwrap().as_str(), "https://example.com/p/index.json");
}

#[test]
fn a_link_that_could_reach_something_private_is_refused() {
    for bad in [
        "http://example.com/index.json",
        "file:///etc/passwd",
        "https://localhost/index.json",
        "https://127.0.0.1/index.json",
        "https://[::1]/index.json",
        "https://169.254.169.254/index.json",
        "https://192.168.1.1/index.json",
        "https://user:pw@example.com/index.json",
        "https://example.com:8787/index.json",
        "https://printer.local/index.json",
        "https://example.com/page",
        "https://github.com/just-an-owner",
        "not a link",
    ] {
        assert!(index_url(bad).is_err(), "{bad}");
    }
}

#[test]
fn only_public_addresses_are_public() {
    for bad in [
        "127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.0.1", "169.254.169.254", "0.0.0.0", "100.64.0.1", "198.18.0.1",
        "192.0.0.1", "224.0.0.1", "240.0.0.1", "255.255.255.255", "::1", "::", "fc00::1", "fd12::1", "fe80::1", "ff02::1",
        "2001:db8::1", "::ffff:127.0.0.1", "::ffff:10.0.0.1",
    ] {
        assert!(!public_ip(bad.parse().unwrap()), "{bad}");
    }
    for good in ["1.1.1.1", "185.199.108.133", "2606:50c0:8000::154", "::ffff:8.8.8.8"] {
        assert!(public_ip(good.parse().unwrap()), "{good}");
    }
}

#[test]
fn a_pack_file_stays_beside_its_index() {
    let at = url("https://example.com/a/index.json");
    assert_eq!(pack_url(&at, "packs/x.json").unwrap().as_str(), "https://example.com/a/packs/x.json");
    for bad in [
        "../x.json",
        "packs/../../x.json",
        "./x.json",
        "/x.json",
        "//evil.example/x.json",
        "https://evil.example/x.json",
        "x.txt",
        "x.json?y=1",
        "packs\\x.json",
    ] {
        assert_eq!(pack_url(&at, bad), None, "{bad}");
    }
}

#[test]
fn a_bad_index_is_refused() {
    let at = url("https://example.com/a/index.json");
    assert!(parse_index(b"<html>", &at).is_err());
    assert!(parse_index(json!({"version": 2, "packs": []}).to_string().as_bytes(), &at).is_err());
    assert!(parse_index(json!({"version": 1}).to_string().as_bytes(), &at).is_err());
    let many: Vec<Value> = (0..=PACKS_MAX).map(|i| json!({"id": format!("p{i}"), "file": "p.json"})).collect();
    assert!(parse_index(index(json!(many)).to_string().as_bytes(), &at).is_err());
}

#[test]
fn an_index_entry_with_a_bad_file_is_skipped() {
    let at = url("https://example.com/a/index.json");
    let v = index(json!([{"id": "good", "file": "g.json"}, {"id": "bad", "file": "../g.json"}, {"file": "n.json"}]));
    let packs = parse_index(v.to_string().as_bytes(), &at).unwrap();
    assert_eq!(packs.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["good"]);
    assert_eq!(packs[0].title, "good", "the title falls back to the id");
}

#[test]
fn a_design_keeps_only_what_a_design_has() {
    let mut d = fox();
    d["image"] = "https://evil.example/track.png".into();
    d["onload"] = "rm -rf /".into();
    d["why"] = "a\u{7}b".into();
    let clean = clean_design(&d).unwrap();
    let mut keys: Vec<&str> = clean.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["color", "name", "outline", "shape", "why"]);
    assert_eq!(clean["why"], "ab");
}

#[test]
fn an_unsound_design_is_refused() {
    let named = |mut d: Value, k: &str, v: Value| {
        d[k] = v;
        d
    };
    assert!(clean_design(&named(fox(), "name", "".into())).is_err());
    assert!(clean_design(&named(fox(), "name", 3.into())).is_err());
    assert!(clean_design(&named(fox(), "color", "red".into())).is_err());
    assert!(clean_design(&named(fox(), "outline", "M0 0".into())).is_err());
    assert!(clean_design(&named(fox(), "outline", "x".repeat(avatar_shape::OUTLINE_MAX + 1).into())).is_err());
    assert!(clean_design(&named(fox(), "shape", "virus".into())).is_err());
    assert!(clean_design(&named(fox(), "solids", json!([{"kind": "bomb"}]))).is_err());
}

#[test]
fn long_text_is_cut() {
    let d = clean_design(&json!({"name": "n".repeat(500), "why": "w".repeat(5000), "shape": "cloud", "color": "#FFFFFF"})).unwrap();
    assert_eq!(d["name"].as_str().unwrap().chars().count(), NAME_MAX);
    assert_eq!(d["why"].as_str().unwrap().chars().count(), WHY_MAX);
}

#[test]
fn too_many_designs_stop_the_read() {
    let mut found = Found::default();
    let list: Vec<Value> = (0..=DESIGNS_MAX).map(|_| fox()).collect();
    assert!(parse_pack(json!({"designs": list}).to_string().as_bytes(), &mut found).is_err());
    assert_eq!(found.designs.len(), DESIGNS_MAX);
}

#[tokio::test]
async fn all_reads_every_pack_within_the_caps() {
    let site = two_packs();
    assert_eq!(names(&read_site(&site, "").await.unwrap()), ["Fox", "Cat"]);
    assert_eq!(names(&read_site(&site, "ALL").await.unwrap()), ["Fox", "Cat"]);
    let asked = site.asked.borrow();
    assert_eq!(asked[0], (format!("{ROOT}index.json"), INDEX_MAX));
    assert_eq!(asked[1], (format!("{ROOT}packs/animals.json"), PACK_MAX));
}

#[tokio::test]
async fn a_pick_names_a_pack_a_group_or_a_design() {
    let site = two_packs();
    assert_eq!(names(&read_site(&site, "animals").await.unwrap()), ["Fox"]);
    assert_eq!(names(&read_site(&site, "originals").await.unwrap()), ["Fox"]);
    assert_eq!(names(&read_site(&site, "cat").await.unwrap()), ["Cat"]);
    assert_eq!(read_site(&site, "dragon").await.unwrap_err(), "nothing called “dragon” at that link");
}

#[tokio::test]
async fn one_bad_pack_leaves_the_others() {
    let mut site = two_packs();
    site.files.insert(format!("{ROOT}packs/pets.json"), b"{not json".to_vec());
    let v = read_site(&site, "").await.unwrap();
    assert_eq!(names(&v), ["Fox"]);
    assert_eq!(v["refused"][0]["name"], "pets");
}

#[tokio::test]
async fn a_refused_design_says_why_and_the_rest_come() {
    let site = Site::with(&[
        ("https://example.com/a/index.json", index(json!([{"id": "p", "file": "p.json"}]))),
        ("https://example.com/a/p.json", json!({"designs": [fox(), {"name": "Bad", "shape": "custom", "color": "red"}]})),
    ]);
    let v = read_site(&site, "").await.unwrap();
    assert_eq!(names(&v), ["Fox"]);
    assert_eq!(v["refused"][0]["name"], "Bad");
    assert!(v["refused"][0]["why"].as_str().unwrap().contains("color"));
}

#[tokio::test]
async fn a_list_reads_only_the_index() {
    let site = two_packs();
    let v = list(&format!("{ROOT}index.json"), |u, m| site.get(u, m)).await.unwrap();
    assert_eq!(v["packs"][0], json!({"id": "animals", "title": "Animals", "groups": ["Originals"]}));
    assert_eq!(site.asked.borrow().len(), 1);
}

#[tokio::test]
async fn a_missing_index_says_so() {
    let site = Site::default();
    assert_eq!(read_site(&site, "").await.unwrap_err(), "no packs at that link");
}

#[tokio::test]
async fn this_mac_is_never_read() {
    // a name that resolves here gets as far as the look at its address
    assert_eq!(get_public(url("https://localhost.localdomain.invalid/x.json"), 10).await.unwrap_err(), "couldn't read that link");
    assert_eq!(get_public(url("http://127.0.0.1:8787/state/x"), 10).await.unwrap_err(), "a pack link must be https");
    assert_eq!(get_public(url("https://127.0.0.1/x"), 10).await.unwrap_err(), "a pack link must name a public host");
}
