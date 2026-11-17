//! The limits a host enforces on a manifest (spec section 24), against the table the plugin
//! developer kit documents in its manifest guide (`docs/manifest.md`, "Limits"): a plugin author
//! designs to that table, so the broker's numbers must be those.

use buwiz_broker::v1::manifest::limits_json;
use serde_json::json;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;

#[test]
fn the_brokers_limits_are_the_documented_ones() {
    let documented = json!({
        "manifestBytes": 256 * KIB, "pluginId": 255, "version": 32, "title": 60, "description": 300, "publisherName": 80,
        "commands": 32, "scopes": 16, "routines": 8, "apps": 16,
        "commandDescription": 200, "commandArgs": 120, "scopeDescription": 120, "scopeHint": 1500,
        "routineName": 80, "routineDescription": 300, "routinePrompt": 4000, "everyS": [60, 86400],
        "reads": 4, "readsTools": 8, "writes": 8,
        "permTools": 64, "openApps": 16, "openExternal": 8, "attach": 16, "pickExtensions": 16,
        "pickMaxBytes": 50 * MIB, "storageQuota": 10 * MIB, "storageDefault": MIB, "previewMaxBytes": 8 * MIB,
        "propsBytes": 8 * KIB, "propsDepth": 8, "propsArray": 100, "schemaProperties": 16, "schemaMaxLength": 512,
        "inlineWidth": [240, 960], "inlineHeight": [96, 800],
    });
    let held = limits_json();
    for (key, want) in documented.as_object().unwrap() {
        assert_eq!(&held[key], want, "LIMITS.{key}");
    }
}
