//! The limits the broker enforces on a manifest (spec section 24) are these numbers, the ones
//! the protocol guide lists in its "Limits" table: a plugin author designs to that table.
//!
//! The table's home is the developer kit's repository, so this test holds a copy of its numbers
//! and cannot read the guide itself; checking the guide's own table against them is that
//! repository's. The check goes both ways: every limit the broker enforces is in the copy, and
//! every row of the copy is a limit the broker has, so a limit added to the broker without the
//! table fails here.

use buwiz_broker::v1::manifest::limits_json;
use serde_json::json;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;

#[test]
fn the_brokers_limits_are_exactly_the_numbers_the_protocol_guide_lists() {
    let listed = json!({
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
    let (listed, held) = (listed.as_object().unwrap(), held.as_object().unwrap());
    for key in held.keys() {
        assert!(listed.contains_key(key), "the broker enforces LIMITS.{key}, which the table does not list");
    }
    for (key, want) in listed {
        assert_eq!(held.get(key), Some(want), "LIMITS.{key}");
    }
}
