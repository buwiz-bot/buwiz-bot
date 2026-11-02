//! `buwiz-broker manifest <op>`: the manifest checks on JSON in, JSON out,
//! one request on stdin. `server/test/v1-rust.mjs` runs every v1-manifest
//! case through it beside Node's `v1/manifest.mjs`; a plugin author can
//! use `verify` to see what Rox will say about a manifest.
//!
//! | op | stdin | `value` |
//! |---|---|---|
//! | `verify` | `{text, trust, host?}` (`trust` as in trust.json) | `{manifest, sha256}` |
//! | `tools` | `{manifest, tools}` | `null` |
//! | `props` | `{schema, props}` | `null` |
//! | `review` | `{manifest, titles?}` (plugin id → title) | the review |
//! | `added` | `{prev, next}` | lines |
//! | `semver` | `{a, b}` | -1, 0, 1 or `null` |
//! | `limits` | `{}` | section 24 |
//! | `package` | `{text, trust, host?, platform?}` (a package's plugin.json) | `{id, short, version, server, accounts, files, binary}` |
//!
//! The answer is `{ok: true, value}`, or a refusal's wire form.

use serde_json::{json, Value};

use super::manifest::{self, HOST_PROTOCOL};
use super::trust::Trust;
use super::{get, tpl, V1Error};
use crate::js;

/// The answer to `op` on request `input`; `None` for an unknown op.
pub fn run(op: &str, input: &Value) -> Option<Value> {
    let empty = json!({});
    let arg = |k: &str| get(input, k).unwrap_or(&Value::Null);
    let answer: Result<Value, V1Error> = match op {
        "verify" => {
            let host = get(input, "host").and_then(Value::as_str).unwrap_or(HOST_PROTOCOL);
            let text = arg("text").as_str().unwrap_or_default();
            manifest::verify(text, &Trust::from_value(get(input, "trust").unwrap_or(&empty)), host).map(|v| json!({"manifest": v.manifest, "sha256": v.sha256}))
        }
        "tools" => manifest::check_tools(arg("manifest"), arg("tools").as_array().map(Vec::as_slice).unwrap_or_default()).map(|()| Value::Null),
        "props" => manifest::validate_props(get(input, "schema"), arg("props")).map(|()| Value::Null),
        "review" => {
            let titles = get(input, "titles");
            let title_of = |id: Option<&Value>| match (titles, id.and_then(Value::as_str)) {
                (Some(t), Some(id)) => get(t, id).map(js::string_of).unwrap_or_else(|| id.to_owned()),
                _ => tpl(id),
            };
            Ok(manifest::review(arg("manifest"), &title_of))
        }
        "added" => Ok(json!(manifest::added_permissions(arg("prev"), arg("next")))),
        "semver" => Ok(json!(manifest::compare_semver(arg("a").as_str().unwrap_or_default(), arg("b").as_str().unwrap_or_default()))),
        "limits" => Ok(manifest::limits_json()),
        "package" => {
            let host = get(input, "host").and_then(Value::as_str).unwrap_or(HOST_PROTOCOL);
            let platform = get(input, "platform").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(crate::helper::release::platform);
            let text = arg("text").as_str().unwrap_or_default();
            super::package::verify(text, &Trust::from_value(get(input, "trust").unwrap_or(&empty)), host, &platform).map(|p| {
                let file = |f: &super::package::File| json!({"path": f.path, "asset": f.asset, "sha256": f.sha256});
                json!({
                    "id": p.id, "short": p.short, "version": p.version, "server": p.server,
                    "accounts": p.accounts.iter().map(super::package::Account::json).collect::<Vec<_>>(),
                    "files": p.files.iter().map(file).collect::<Vec<_>>(), "binary": p.binary.as_ref().map(file),
                })
            })
        }
        _ => return None,
    };
    Some(match answer {
        Ok(value) => json!({"ok": true, "value": value}),
        Err(e) => e.body(),
    })
}
