//! The trust store (spec 4.1; `v1/trust.mjs`): which publisher keys sign
//! manifests and which plugins may be installed. The file is read on every
//! use, so an edit counts at the next verification; a missing or broken
//! file trusts nothing.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::{from_b64u, get};
use crate::js;

#[derive(Debug, Clone, PartialEq)]
pub struct Publisher {
    pub publisher: String,
    pub public_key: String,
    /// The store marks the key first-party (`"first_party": true`): its
    /// plugins may ship native screens.
    pub first_party: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trust {
    /// key id → publisher key.
    pub publishers: HashMap<String, Publisher>,
    /// `(plugin id, publisher key id)` pairs.
    pub approved: Vec<(String, String)>,
}

impl Trust {
    /// The store in a trust file's shape (`{publishers, approved_plugins}`).
    pub fn from_value(raw: &Value) -> Trust {
        let mut publishers = HashMap::new();
        for (key_id, p) in get(raw, "publishers").and_then(Value::as_object).into_iter().flatten() {
            if p.is_object() && get(p, "alg") == Some(&Value::from("ed25519")) && from_b64u(get(p, "public_key"), 32).is_some() {
                let publisher = get(p, "publisher").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
                let public_key = get(p, "public_key").and_then(Value::as_str).unwrap_or_default().to_owned();
                let first_party = get(p, "first_party") == Some(&Value::Bool(true));
                publishers.insert(key_id.clone(), Publisher { publisher, public_key, first_party });
            }
        }
        let approved = get(raw, "approved_plugins")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| Some((get(a, "id")?.as_str()?.to_owned(), get(a, "publisher_key_id")?.as_str()?.to_owned())))
            .collect();
        Trust { publishers, approved }
    }

    /// The store in `file`; nothing trusted when it cannot be read.
    pub fn load(file: &Path) -> Trust {
        let raw = std::fs::read_to_string(file).map_err(|e| e.to_string()).and_then(|t| buwiz_protocol::jcs::parse_ijson(&t));
        match raw {
            Ok(raw) => Trust::from_value(&raw),
            Err(e) => {
                eprintln!("  v1 trust store {}: {}", file.display(), js::slice_utf16(&e, 200));
                Trust::default()
            }
        }
    }

    /// Whether plugin `id` signed by `key_id` is on the approved list.
    pub fn is_approved(&self, id: &str, key_id: &str) -> bool {
        self.approved.iter().any(|(a, k)| a == id && k == key_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_well_formed_ed25519_keys_and_approvals_are_trusted() {
        let key = buwiz_protocol::keys::b64(&[7; 32]);
        let raw = json!({
            "publishers": {
                "k1": {"alg": "ed25519", "publisher": "app.buwiz", "public_key": key},
                "k2": {"alg": "rsa", "public_key": key},
                "k3": {"alg": "ed25519", "public_key": "short"},
                "k4": {"alg": "ed25519", "public_key": key, "publisher": 0}
            },
            "approved_plugins": [{"id": "dev.a.b", "publisher_key_id": "k1"}, {"id": 5, "publisher_key_id": "k1"}, "x"]
        });
        let t = Trust::from_value(&raw);
        assert_eq!(t.publishers.len(), 2);
        assert_eq!(t.publishers["k1"].publisher, "app.buwiz");
        assert_eq!(t.publishers["k4"].publisher, "");
        assert!(t.is_approved("dev.a.b", "k1") && !t.is_approved("dev.a.b", "k2"));
        assert_eq!(t.approved.len(), 1);
        assert_eq!(Trust::from_value(&json!(null)), Trust::default());
        assert_eq!(Trust::load(Path::new("/nonexistent/trust.json")), Trust::default());
    }
}
