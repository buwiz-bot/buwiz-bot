//! The manifest verifier against pinned answers, in
//! `tests/pins/v1_manifest/cases.json`.
//!
//! A case is small: the fixture `tests/fixtures/v1/manifest.json` (with
//! `publisher.key_id` set to `test-key`), a list of edits, and the pinned
//! answer. The test applies the edits, signs the result with a fixed
//! key (32 bytes of 3; the board and card fingerprints filled), and asks `buwiz-broker manifest <op>` (here
//! `v1::cli::run`, the same entry) the question.
//!
//! The case fields, in the order they apply:
//! - `edit`: a list of `["set", path, value]`, `["del", path]`,
//!   `["push", path, value]`, `["extend", path, values]`, `["reshort", short]`
//!   (renames the `fx_` tools to the new short everywhere). A path is dotted;
//!   a number indexes an array. A value may be a macro (its first key starts
//!   with `$`): `{"$rep": [text, n, prefix?]}` the text n times,
//!   `{"$many": [n, template, from?]}` an array (`{i}` in a template string is
//!   the index), `{"$map": [n, key, value]}` an object, `{"$deep": d}` d nested
//!   objects, `{"$id": 255|256}` a plugin id at the limit and over,
//!   `{"$from": path, "with": {...}}` a copy of a part of the manifest with
//!   some keys replaced.
//! - `size`: pad `x_pad` so the signed text is exactly that many bytes.
//! - `key: "other"`: sign with another key. `raw`: this text instead.
//! - `post`: `["post-set", path, v]` / `["post-del", path]` on the signed
//!   object (a change after signing); `layout: "reversed3"` writes the keys in
//!   reverse order, indented by 3; `replace: [from, to]` changes the text.
//! - `trust`: `default` (the fixture and the two long plugin ids approved) or
//!   `first` (the key is first-party).
//!
//! `expect` is the pinned answer: `{ok: true, sha256}` for a manifest it
//! verified (the hash is of the canonical form, so it also proves the
//! signature bytes), `{ok: true, value}` for the other ops, `{ok: false,
//! code, message}` for a refusal, `{signError}` for a manifest that could not
//! be signed. For the cases refused as not I-JSON, only the code and the
//! prefix `manifest: is not I-JSON: ` are pinned (parsers word the rest
//! differently); this broker's own full message for each is pinned apart, in
//! `v1_manifest/parse_messages.json`. To change a case on purpose, edit the pin; `BUWIZ_REFRESH_PINS=1`
//! rewrites the whole file from the Rust answers and fails (review `git diff`).

#[path = "../../test-support/pins.rs"]
mod pins;

use std::path::{Path, PathBuf};

use buwiz_broker::js;
use buwiz_broker::v1::cli;
use buwiz_broker::v1::signing::sign_manifest;
use serde_json::{json, Map, Value};

const PARSE: &str = "manifest: is not I-JSON: ";
const PIN: &str = "v1_manifest/cases";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1")
}

fn seed(of: u8) -> String {
    buwiz_protocol::keys::b64(&[of; 32])
}

// ── macros ──────────────────────────────────────────────────────────────

fn subst(v: &Value, i: usize) -> Value {
    match v {
        Value::String(s) => Value::String(s.replace("{i}", &i.to_string())),
        Value::Array(a) => Value::Array(a.iter().map(|x| subst(x, i)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), subst(x, i))).collect()),
        other => other.clone(),
    }
}

fn long_id(n: u64) -> String {
    let (a, b, c) = ("a".repeat(63), "b".repeat(63), "c".repeat(63));
    match n {
        255 => format!("{a}.{b}.{c}.{}", "d".repeat(63)),
        256 => format!("{a}.{b}.{c}.{}.e", "d".repeat(62)),
        _ => panic!("$id {n}"),
    }
}

fn deep(d: u64) -> Value {
    if d == 1 {
        json!({})
    } else {
        json!({"a": deep(d - 1)})
    }
}

fn at<'a>(m: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(m, |x, k| match x {
        Value::Array(a) => &a[k.parse::<usize>().unwrap()],
        _ => &x[k],
    })
}

fn at_mut<'a>(m: &'a mut Value, path: &str) -> &'a mut Value {
    path.split('.').fold(m, |x, k| match x {
        Value::Array(a) => &mut a[k.parse::<usize>().unwrap()],
        _ => x.get_mut(k).unwrap_or_else(|| panic!("no {k} in {path}")),
    })
}

fn num(v: &Value) -> usize {
    v.as_u64().expect("a number") as usize
}

fn expand(v: &Value, m: &Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.iter().map(|x| expand(x, m)).collect()),
        Value::Object(o) => match o.keys().next().map(String::as_str) {
            Some("$rep") => {
                let a = v["$rep"].as_array().unwrap();
                let prefix = a.get(2).and_then(Value::as_str).unwrap_or("");
                Value::String(format!("{prefix}{}", a[0].as_str().unwrap().repeat(num(&a[1]))))
            }
            Some("$many") => {
                let a = v["$many"].as_array().unwrap();
                let from = a.get(2).map_or(0, num);
                let tmpl = expand(&a[1], m);
                Value::Array((0..num(&a[0])).map(|k| subst(&tmpl, k + from)).collect())
            }
            Some("$map") => {
                let a = v["$map"].as_array().unwrap();
                let val = expand(&a[2], m);
                Value::Object((0..num(&a[0])).map(|k| (a[1].as_str().unwrap().replace("{i}", &k.to_string()), subst(&val, k))).collect())
            }
            Some("$deep") => deep(v["$deep"].as_u64().unwrap()),
            Some("$id") => Value::String(long_id(v["$id"].as_u64().unwrap())),
            Some("$from") => {
                let mut copy = at(m, v["$from"].as_str().unwrap()).clone();
                for (k, x) in expand(&v["with"], m).as_object().unwrap() {
                    copy[k] = x.clone();
                }
                copy
            }
            _ => Value::Object(o.iter().map(|(k, x)| (k.clone(), expand(x, m))).collect()),
        },
        other => other.clone(),
    }
}

fn apply(m: &mut Value, edit: &Value) {
    let e = edit.as_array().unwrap();
    let (op, a) = (e[0].as_str().unwrap(), &e[1]);
    let parent = |path: &str| -> (String, String) {
        let (head, last) = path.rsplit_once('.').unwrap_or(("", path));
        (head.to_owned(), last.to_owned())
    };
    match op {
        "set" | "del" => {
            let (head, last) = parent(a.as_str().unwrap());
            let value = (op == "set").then(|| expand(&e[2], m));
            let target = if head.is_empty() { &mut *m } else { at_mut(m, &head) };
            match (target, value) {
                (Value::Array(arr), Some(v)) => arr[last.parse::<usize>().unwrap()] = v,
                (t, Some(v)) => drop(t.as_object_mut().unwrap().insert(last, v)),
                (t, None) => drop(t.as_object_mut().unwrap().shift_remove(&last)),
            }
        }
        "push" => {
            let v = expand(&e[2], m);
            at_mut(m, a.as_str().unwrap()).as_array_mut().unwrap().push(v);
        }
        "extend" => {
            let v = expand(&e[2], m);
            at_mut(m, a.as_str().unwrap()).as_array_mut().unwrap().extend(v.as_array().unwrap().iter().cloned());
        }
        "reshort" => {
            let short = expand(a, m);
            let text = js::stringify(m).replace("\"fx_", &format!("\"{}_", short.as_str().unwrap()));
            let mut renamed: Value = serde_json::from_str(&text).unwrap();
            renamed["short"] = short;
            *m = renamed;
        }
        _ => panic!("edit {op}"),
    }
}

// ── the manifest of a case ──────────────────────────────────────────────

fn signed(case: &Value) -> Result<Value, String> {
    let mut m: Value = serde_json::from_str(&std::fs::read_to_string(fixtures().join("manifest.json")).unwrap()).unwrap();
    m["publisher"]["key_id"] = json!("test-key");
    for e in case["edit"].as_array().into_iter().flatten() {
        apply(&mut m, e);
    }
    let sources: Vec<(String, Vec<u8>)> = ["board", "card"]
        .into_iter()
        .filter(|id| m["apps"].as_array().is_some_and(|a| a.iter().any(|x| x["id"] == *id)))
        .map(|id| (id.to_owned(), std::fs::read(fixtures().join(format!("{id}.tsx"))).unwrap()))
        .collect();
    let seed = seed(if case["key"] == "other" { 4 } else { 3 });
    if let Some(size) = case["size"].as_u64() {
        m["x_pad"] = json!("");
        let base = js::stringify(&sign_manifest(&m, &seed, &sources)?).len();
        assert!(size as usize >= base, "{}: size {size} is under the {base} bytes of the unpadded manifest", case["id"]);
        m["x_pad"] = json!("x".repeat(size as usize - base));
    }
    sign_manifest(&m, &seed, &sources)
}

fn text_of(case: &Value) -> Result<String, String> {
    if let Some(raw) = case["raw"].as_str() {
        return Ok(raw.to_owned());
    }
    let mut m = signed(case)?;
    for p in case["post"].as_array().into_iter().flatten() {
        let path = p[1].as_str().unwrap();
        let (head, last) = path.rsplit_once('.').unwrap_or(("", path));
        let target = if head.is_empty() { &mut m } else { at_mut(&mut m, head) };
        match p[0].as_str().unwrap() {
            "post-set" => drop(target.as_object_mut().unwrap().insert(last.to_owned(), p[2].clone())),
            _ => drop(target.as_object_mut().unwrap().shift_remove(last)),
        }
    }
    if case["layout"] == "reversed3" {
        let reversed: Map<String, Value> = m.as_object().unwrap().iter().rev().map(|(k, v)| (k.clone(), v.clone())).collect();
        return Ok(js::stringify_indent(&Value::Object(reversed), 3));
    }
    let mut text = js::stringify(&m);
    if let Some(r) = case["replace"].as_array() {
        text = text.replacen(r[0].as_str().unwrap(), r[1].as_str().unwrap(), 1);
    }
    Ok(text)
}

fn trust(name: &str) -> Value {
    let public = buwiz_protocol::keys::SessionKey::from_seed(&seed(3)).unwrap().public();
    let mut publisher = json!({"publisher": "app.buwiz", "alg": "ed25519", "public_key": public});
    let ids = match name {
        "first" => {
            publisher["first_party"] = json!(true);
            vec!["app.buwiz.fixture".to_owned()]
        }
        _ => vec!["app.buwiz.fixture".to_owned(), long_id(255), long_id(256)],
    };
    json!({"v": 1, "publishers": {"test-key": publisher}, "approved_plugins": ids.iter().map(|id| json!({"id": id, "publisher_key_id": "test-key"})).collect::<Vec<_>>()})
}

/// An answer of `cli::run` as the pin keeps it.
fn pinned_form(answer: Value, op: &str) -> Value {
    if answer["ok"] == true {
        return match op {
            "verify" => json!({"ok": true, "sha256": answer["value"]["sha256"]}),
            _ => json!({"ok": true, "value": answer["value"]}),
        };
    }
    let message = answer["message"].as_str().unwrap();
    let message = if message.starts_with(PARSE) { PARSE } else { message };
    let mut out = json!({"ok": false, "code": answer["code"], "message": message});
    if answer["retryable"] == true {
        out["retryable"] = json!(true);
    }
    out
}

/// This broker's own full message for a case refused as not I-JSON.
fn parse_message(case: &Value) -> String {
    let text = text_of(case).unwrap();
    let a = cli::run("verify", &json!({"text": text, "trust": trust("default")})).unwrap();
    a["message"].as_str().unwrap().to_owned()
}

fn answer(case: &Value) -> Value {
    let op = case["op"].as_str().unwrap();
    let ask = |op: &str, input: Value| cli::run(op, &input).unwrap_or_else(|| panic!("no op {op}"));
    match op {
        "verify" => match text_of(case) {
            Err(e) => json!({"signError": e}),
            Ok(text) => {
                let t = trust(case["trust"].as_str().unwrap_or("default"));
                pinned_form(ask("verify", json!({"text": text, "trust": t})), op)
            }
        },
        "tools" => {
            let t = trust("default");
            let verified = ask("verify", json!({"text": js::stringify(&signed(case).unwrap()), "trust": t}));
            assert_eq!(verified["ok"], true, "{}: the manifest to check tools against: {verified}", case["id"]);
            pinned_form(ask("tools", json!({"manifest": verified["value"]["manifest"], "tools": case["tools"]})), op)
        }
        "props" => {
            let empty = json!({});
            pinned_form(ask("props", json!({"schema": expand(&case["schema"], &empty), "props": expand(&case["props"], &empty)})), op)
        }
        "review" => {
            let manifest = if case["manifest"].is_null() { signed(case).unwrap() } else { case["manifest"].clone() };
            pinned_form(ask("review", json!({"manifest": manifest, "titles": case["titles"]})), op)
        }
        "added" => pinned_form(ask("added", json!({"prev": case["prev"], "next": case["next"]})), op),
        "semver" => pinned_form(ask("semver", json!({"a": case["a"], "b": case["b"]})), op),
        _ => panic!("op {op}"),
    }
}

/// The pin file as it is written: one line a case.
fn compact(doc: &Value) -> String {
    let lines = |v: &Value| v.as_array().unwrap().iter().map(|c| format!("    {}", js::stringify(c))).collect::<Vec<_>>().join(",\n");
    let other = doc["grammar_other"].as_object().unwrap().iter().map(|(k, v)| format!("    {}: {}", js::stringify(&json!(k)), js::stringify(v))).collect::<Vec<_>>().join(",\n");
    format!(
        "{{\n  \"about\": {},\n  \"cases\": [\n{}\n  ],\n  \"grammar\": [\n{}\n  ],\n  \"grammar_other\": {{\n{other}\n  }}\n}}\n",
        js::stringify(&doc["about"]),
        lines(&doc["cases"]),
        lines(&doc["grammar"])
    )
}

#[test]
fn every_manifest_case_answers_as_node_did() {
    let pinned = pins::load(PIN);
    let mut ours = pinned.clone();
    let mut differ = Vec::new();
    for case in ours["cases"].as_array_mut().unwrap() {
        let got = answer(case);
        if got != case["expect"] {
            differ.push(format!("{}\n    node: {}\n    here: {}", case["id"], case["expect"], got));
        }
        case["expect"] = got;
    }
    // the two parsers word a parse error differently: for those refusals only the code and the
    // prefix are compared with the pin, and the full wording of this broker is pinned on its own
    let parse: Map<String, Value> = ours["cases"].as_array().unwrap().iter().filter(|c| c["expect"]["message"] == PARSE).map(|c| (c["id"].as_str().unwrap().to_owned(), json!(parse_message(c)))).collect();
    assert!(!parse.is_empty());
    assert!(ours["cases"].as_array().unwrap().len() >= 200, "the pin holds {} cases", ours["cases"].as_array().unwrap().len());
    pins::check("v1_manifest/parse_messages", &Value::Object(parse));
    if pins::refreshing() {
        if differ.is_empty() {
            return;
        }
        std::fs::write(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pins").join(format!("{PIN}.json")), compact(&ours)).unwrap();
        pins::rewritten(PIN, "json");
    }
    assert!(differ.is_empty(), "{} manifest case(s) answer differently from the pinned answer:\n  {}", differ.len(), differ.join("\n  "));
}

#[test]
fn the_pin_is_compact_and_covers_the_old_test() {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pins").join(format!("{PIN}.json"))).unwrap();
    assert!(text.len() < 100_000, "the pin is {} bytes", text.len());
    assert_eq!(compact(&serde_json::from_str(&text).unwrap()), text, "the pin is written one case a line (BUWIZ_REFRESH_PINS=1 writes it so)");
    let pin: Value = serde_json::from_str(&text).unwrap();
    let ids: Vec<&str> = pin["cases"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids.iter().collect::<std::collections::HashSet<_>>().len(), ids.len(), "case ids are unique");
    // every test of the old file has cases here
    for tag in ["MAN-01", "MAN-02", "MAN-03", "MAN-04", "MAN-05", "MAN-06", "MAN-07", "MAN-08", "MAN-10", "MAN-11", "MAN-12", "MAN-13", "MAN-14", "MAN-21", "LCY-09"] {
        assert!(ids.iter().any(|i| i.starts_with(tag)), "no case for {tag}");
    }
}

// MAN-07: the grammar of section 3, a valid and an invalid value each
#[test]
fn the_grammars_accept_and_refuse_as_node_did() {
    let pin = pins::load(PIN);
    for g in pin["grammar"].as_array().unwrap() {
        let kind = g["kind"].as_str().unwrap();
        for v in g["values"].as_array().unwrap() {
            let (value, node) = (v[0].as_str().unwrap(), v[1].as_bool().unwrap());
            // the host never checks this one as a name, only as a token it rewrites
            let ours = match kind {
                "canonicalToken" => buwiz_protocol::tokens::canonical(value).is_some(),
                _ => buwiz_broker::v1::is_v1(kind, Some(&json!(value))),
            };
            assert_eq!(ours, node, "{kind} {value}");
        }
    }
    let other = &pin["grammar_other"];
    for t in other["typed"].as_array().unwrap() {
        let found: Vec<Value> = buwiz_protocol::tokens::typed_tokens(t[0].as_str().unwrap()).iter().map(|x| json!({"short": x.short, "scope": x.scope, "value": x.value})).collect();
        assert_eq!(Value::Array(found), t[1], "typed tokens of {}", t[0]);
    }
    for s in other["slash"].as_array().unwrap() {
        let got = buwiz_protocol::tokens::slash(s[0].as_str().unwrap()).map_or(Value::Null, |x| json!([x.short.unwrap_or(""), x.name, x.args]));
        assert_eq!(got, s[1], "slash {}", s[0]);
    }
    assert_eq!(buwiz_broker::v1::is_v1("reservedTool", Some(&json!("rox.provision"))), other["rox_provision_is_a_reserved_tool"].as_bool().unwrap());
    let (short, scope, ticket) = buwiz_protocol::tokens::canonical("@fx:item:tkt_0123456789ABCDEFGHJKMNPQRS").unwrap();
    assert_eq!(json!({"short": short, "scope": scope, "ticket": ticket}), other["canonical"]);
}

// the sign-manifest command: a 0600 key file, --app fingerprints, and the bytes written
#[test]
fn sign_manifest_writes_the_bytes_node_wrote() {
    use std::os::unix::fs::PermissionsExt;
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }
    let dir = std::env::temp_dir().join(format!("buwiz-sign-pin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = Cleanup(dir.clone());
    let mut m: Value = serde_json::from_str(&std::fs::read_to_string(fixtures().join("manifest.json")).unwrap()).unwrap();
    m["publisher"]["key_id"] = json!("test-key");
    // what a serializer could get wrong: integer-like keys, -0, 1e3, U+2028, control characters
    m["x_extra"] = buwiz_protocol::jcs::parse_ijson("{\"b\":\"\\u0007 é \u{2028}\",\"10\":[-0,1e3],\"2\":{}}").unwrap();
    let (template, key, out) = (dir.join("template.json"), dir.join("key"), dir.join("signed.json"));
    std::fs::write(&template, js::stringify(&m)).unwrap();
    std::fs::write(&key, seed(3)).unwrap();
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_buwiz-broker"))
            .arg("sign-manifest")
            .arg(&template)
            .args(["--key-file"])
            .arg(&key)
            .args(["--app", &format!("board={}", fixtures().join("board.tsx").display()), "--app", &format!("card={}", fixtures().join("card.tsx").display()), "--out"])
            .arg(&out)
            .output()
            .unwrap()
    };
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    let refused = run();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("readable by others"), "{}", String::from_utf8_lossy(&refused.stderr));
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let done = run();
    assert!(done.status.success(), "{}", String::from_utf8_lossy(&done.stderr));
    let written = std::fs::read_to_string(&out).unwrap();
    assert!(!written.contains(&seed(3)), "the seed is not in the output");
    pins::check_text("v1_manifest/signed", &written);
    // and it verifies, with the card's own fingerprint
    let signed: Value = serde_json::from_str(&written).unwrap();
    let digest = buwiz_protocol::keys::sha256_hex(&std::fs::read(fixtures().join("card.tsx")).unwrap());
    assert_eq!(signed["apps"][1]["fingerprint"], format!("sha256:{digest}"));
    let verified = cli::run("verify", &json!({"text": written, "trust": trust("default")})).unwrap();
    assert_eq!(verified["ok"], true, "{verified}");
}
