//! App fingerprints and manifest signatures (spec 6.2, 4.2;
//! `v1/signing.mjs` and `v1/sign-manifest.mjs`). A manifest signed here is
//! byte for byte the one Node's `sign-manifest.mjs` writes.

use std::os::unix::fs::PermissionsExt as _;

use serde_json::{json, Value};

use super::{get, signed_bytes};
use crate::js;

/// The Keychain service publisher keys live under (account = key id).
pub const PUBLISHER_SERVICE: &str = "dev.goldcoders.publisher";

/// `sha256:<hex>` of an app source's bytes.
pub fn fingerprint_of(bytes: &[u8]) -> String {
    format!("sha256:{}", buwiz_protocol::keys::sha256_hex(bytes))
}

/// A signed copy of manifest `m`: each `(app id, source bytes)` fills that
/// app's fingerprint first; the key id is the manifest's publisher.key_id.
pub fn sign_manifest(m: &Value, seed: &str, sources: &[(String, Vec<u8>)]) -> Result<Value, String> {
    let mut out = m.clone();
    let o = out.as_object_mut().ok_or("the manifest must be a JSON object")?;
    o.shift_remove("signature");
    for (id, bytes) in sources {
        let app = o.get_mut("apps").and_then(Value::as_array_mut).and_then(|apps| apps.iter_mut().find(|a| get(a, "id").and_then(Value::as_str) == Some(id)));
        let Some(Value::Object(app)) = app else { return Err(format!("the manifest has no app {id}")) };
        app.insert("fingerprint".to_owned(), Value::String(fingerprint_of(bytes)));
    }
    let key_id = o.get("publisher").and_then(|p| get(p, "key_id")).filter(|k| js::truthy(k)).cloned().ok_or("the manifest has no publisher.key_id")?;
    let bytes = signed_bytes(&out)?;
    let key = buwiz_protocol::keys::SessionKey::from_seed(seed).map_err(|_| "a seed is 32 bytes of base64url".to_owned())?;
    let value = key.sign_bytes(&bytes);
    out.as_object_mut().expect("an object above").insert("signature".to_owned(), json!({"alg": "ed25519", "key_id": key_id, "value": value}));
    Ok(out)
}

/// A copy of plugin.json `p` with `p[block].signature`: ed25519 by
/// `p[block].publisher.key_id` over JCS of all of `p` without that
/// signature (§7.3); `v1::package::check_signature` verifies it.
pub fn sign_block(p: &Value, block: &str, seed: &str) -> Result<Value, String> {
    let mut out = p.clone();
    let b = out.get_mut(block).and_then(Value::as_object_mut).ok_or_else(|| format!("plugin.json has no {block} block"))?;
    b.shift_remove("signature");
    let key_id = b.get("publisher").and_then(|p| get(p, "key_id")).filter(|k| js::truthy(k)).cloned().ok_or_else(|| format!("plugin.json has no {block}.publisher.key_id"))?;
    let bytes = signed_bytes(&out)?;
    let key = buwiz_protocol::keys::SessionKey::from_seed(seed).map_err(|_| "a seed is 32 bytes of base64url".to_owned())?;
    let value = key.sign_bytes(&bytes);
    out[block].as_object_mut().expect("an object above").insert("signature".to_owned(), json!({"alg": "ed25519", "key_id": key_id, "value": value}));
    Ok(out)
}

const USAGE: &str = "usage: buwiz-broker sign-manifest <manifest.json> (--key-file <path> | --keychain <key_id>) [--app <id>=<file>]... [--out <file>]\n       buwiz-broker sign-manifest <plugin.json> --block buwiz (--key-file <path> | --keychain <key_id>) [--out <file>]";

/// `buwiz-broker sign-manifest`, as `node server/v1/sign-manifest.mjs`:
///
/// ```text
/// buwiz-broker sign-manifest manifest.json --keychain gc-2026-01 \
///     --app forms=dist/forms.tsx [--out signed.json]
/// buwiz-broker sign-manifest manifest.json --key-file test.key ...
/// ```
///
/// `--block buwiz` signs a package's plugin.json instead: the key is
/// the block's `publisher.key_id`, the signature goes in the block.
///
/// The key comes from the login Keychain (service
/// `dev.goldcoders.publisher`, account = key id) through `/usr/bin/security`,
/// or from a 0600 file holding the base64url seed (test keys). The seed is
/// never printed; the signed manifest goes to `--out` or stdout.
pub fn cli(args: &[String]) -> Result<(), String> {
    let (mut file, mut out, mut key_file, mut keychain, mut block) = (None, None, None, None, None);
    let mut apps = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--key-file" => key_file = it.next().cloned(),
            "--keychain" => keychain = it.next().cloned(),
            "--out" => out = it.next().cloned(),
            "--block" => block = it.next().cloned(),
            "--app" => {
                let spec = it.next().cloned().unwrap_or_default();
                let (id, path) = spec.split_once('=').ok_or_else(|| format!("--app {spec}: expected <id>=<file>"))?;
                apps.push((id.to_owned(), std::fs::read(path).map_err(|e| format!("{path}: {e}"))?));
            }
            _ => file = Some(a.clone()),
        }
    }
    let Some(file) = file.filter(|_| key_file.is_some() != keychain.is_some()) else { return Err(USAGE.to_owned()) };
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{file}: {e}"))?;
    let m = buwiz_protocol::jcs::parse_ijson(&text)?;
    if !m.is_object() {
        return Err("the manifest must be a JSON object".to_owned());
    }
    if block.as_ref().is_some_and(|b| b.is_empty() || !apps.is_empty()) {
        return Err(USAGE.to_owned());
    }
    let signer = match &block {
        Some(b) => m.get(b.as_str()).ok_or_else(|| format!("{file} has no {b} block"))?,
        None => &m,
    };
    let key_id = signer.get("publisher").and_then(|p| get(p, "key_id"));
    if let Some(kc) = &keychain {
        if key_id.and_then(Value::as_str) != Some(kc.as_str()) {
            let at = block.as_ref().map(|b| format!("{b}.")).unwrap_or_default();
            return Err(format!("the manifest's {at}publisher.key_id is {}, not {kc}", super::tpl(key_id)));
        }
    }
    let seed = match (&key_file, &keychain) {
        (Some(path), _) => read_key_file(path)?,
        (_, Some(kc)) => crate::helper::keystore::security(&["find-generic-password", "-s", PUBLISHER_SERVICE, "-a", kc, "-w"])
            .map(|s| s.trim().to_owned())
            .ok_or_else(|| format!("no publisher key {kc} in the login Keychain"))?,
        _ => unreachable!("one of them, checked above"),
    };
    let signed = match &block {
        Some(b) => sign_block(&m, b, &seed)?,
        None => sign_manifest(&m, &seed, &apps)?,
    };
    let signed = format!("{}\n", js::stringify_pretty(&signed));
    match out {
        Some(path) => std::fs::write(&path, signed).map_err(|e| format!("{path}: {e}")),
        None => {
            use std::io::Write as _;
            std::io::stdout().write_all(signed.as_bytes()).map_err(|e| e.to_string())
        }
    }
}

/// A key file must be the owner's alone, as ssh insists for private keys.
fn read_key_file(path: &str) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(format!("{path} is readable by others; chmod 600 it"));
    }
    std::fs::read_to_string(path).map(|s| s.trim().to_owned()).map_err(|e| format!("{path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_fills_fingerprints_and_appends_the_signature() {
        let seed = buwiz_protocol::keys::b64(&[5; 32]);
        let m = json!({"publisher": {"key_id": "k"}, "signature": {"old": 1}, "apps": [{"id": "a", "fingerprint": "x", "z": 1}]});
        let s = sign_manifest(&m, &seed, &[("a".to_owned(), b"src".to_vec())]).unwrap();
        let keys: Vec<&String> = s.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["publisher", "apps", "signature"]);
        let app_keys: Vec<&String> = s["apps"][0].as_object().unwrap().keys().collect();
        assert_eq!(app_keys, ["id", "fingerprint", "z"], "a filled fingerprint keeps its place");
        assert_eq!(s["apps"][0]["fingerprint"], fingerprint_of(b"src"));
        assert_eq!(s["signature"]["key_id"], "k");
        assert_eq!(sign_manifest(&m, &seed, &[("b".to_owned(), vec![])]).unwrap_err(), "the manifest has no app b");
        assert_eq!(sign_manifest(&json!({"publisher": {}}), &seed, &[]).unwrap_err(), "the manifest has no publisher.key_id");
        assert!(sign_manifest(&json!({"publisher": {"key_id": "k"}, "w": 1.5}), &seed, &[]).unwrap_err().contains("safe integers only"));
    }

    #[test]
    fn a_block_signature_covers_the_whole_plugin_json_and_sits_in_the_block() {
        let seed = buwiz_protocol::keys::b64(&[5; 32]);
        let p = json!({"name": "x", "mcpServers": {"s": {"command": "c"}}, "buwiz": {"id": "a", "publisher": {"key_id": "k"}, "signature": {"old": 1}}});
        let s = sign_block(&p, "buwiz", &seed).unwrap();
        let keys: Vec<&String> = s["buwiz"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["id", "publisher", "signature"]);
        assert_eq!(s["buwiz"]["signature"]["key_id"], "k");
        let public = buwiz_protocol::keys::SessionKey::from_seed(&seed).unwrap().public();
        let bytes = signed_bytes(&super::super::package::unsigned(&s)).unwrap();
        assert!(super::super::ed_verify(&public, &bytes, s["buwiz"]["signature"]["value"].as_str().unwrap()));
        let mut changed = s.clone();
        changed["mcpServers"]["s"]["command"] = json!("evil");
        let bytes = signed_bytes(&super::super::package::unsigned(&changed)).unwrap();
        assert!(!super::super::ed_verify(&public, &bytes, s["buwiz"]["signature"]["value"].as_str().unwrap()));
        assert_eq!(sign_block(&json!({"name": "x"}), "buwiz", &seed).unwrap_err(), "plugin.json has no buwiz block");
        assert_eq!(sign_block(&json!({"buwiz": {}}), "buwiz", &seed).unwrap_err(), "plugin.json has no buwiz.publisher.key_id");
    }

    #[test]
    fn the_cli_signs_a_block_with_a_key_file() {
        let dir = std::env::temp_dir().join(format!("rox-sign-block-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (key, input, out) = (dir.join("k.key"), dir.join("plugin.json"), dir.join("signed.json"));
        std::fs::write(&key, buwiz_protocol::keys::b64(&[5; 32])).unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&input, r#"{"name":"x","buwiz":{"publisher":{"key_id":"k"}}}"#).unwrap();
        let s = |p: &std::path::Path| p.display().to_string();
        cli(&[s(&input), "--block".into(), "buwiz".into(), "--key-file".into(), s(&key), "--out".into(), s(&out)]).unwrap();
        let signed: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(signed, sign_block(&json!({"name": "x", "buwiz": {"publisher": {"key_id": "k"}}}), "buwiz", &buwiz_protocol::keys::b64(&[5; 32])).unwrap());
        assert!(cli(&[s(&input), "--block".into(), "buwiz".into(), "--app".into(), format!("a={}", s(&key)), "--key-file".into(), s(&key)]).unwrap_err().starts_with("usage"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
