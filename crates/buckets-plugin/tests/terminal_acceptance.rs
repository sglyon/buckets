//! Independent host-action boundary checks. These do not launch an editor.
use buckets_plugin::{CommandResult, PluginRegistry};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn fixture(root: &Path, response: Value) -> (tempfile::TempDir, PluginRegistry) {
    let plugin = tempfile::tempdir().unwrap();
    fs::write(plugin.path().join("plugin.json"),serde_json::to_vec(&json!({"id":"terminal-acceptance","name":"Terminal acceptance","protocol_version":1,"executable":"python3","args":["plugin.py"],"capabilities":["commands","terminal"],"permissions":{"terminal":true,"read_roots":[root]}})).unwrap()).unwrap();
    fs::write(
        plugin.path().join("result.json"),
        serde_json::to_vec(&response).unwrap(),
    )
    .unwrap();
    fs::write(plugin.path().join("plugin.py"),"import json,sys\nq=json.load(sys.stdin)\nprint(json.dumps(dict(jsonrpc='2.0',id=q['id'],result=json.load(open('result.json')))))\n").unwrap();
    let registry = PluginRegistry::discover(plugin.path()).unwrap();
    (plugin, registry)
}
fn request(root: &Path) -> Value {
    json!({"host_action":"terminal","request":{"program":"/bin/sh","args":["literal 'quotes' $()"],"cwd":root,"title":"Acceptance","env":{"BUCKETS_LITERAL":"café 界"}}})
}

#[test]
fn terminal_shaped_legacy_values_do_not_invoke_host_actions() {
    let root = tempfile::tempdir().unwrap();
    let legacy =
        json!({"program":"/bin/sh","args":["-l"],"cwd":root.path(),"message":"ordinary JSON"});
    let (_plugin, registry) = fixture(root.path(), legacy.clone());
    let result = registry
        .invoke_command_result("terminal-acceptance", "legacy", json!({}))
        .unwrap();
    let CommandResult::Value(value) = result else {
        panic!("host interpreted legacy JSON as a terminal request")
    };
    assert_eq!(value, legacy);
}

#[test]
fn original_command_api_cannot_bypass_terminal_host_action_validation() {
    let root = tempfile::tempdir().unwrap();
    for invalid in [
        json!({"host_action":"arbitrary-host-command","request":{}}),
        json!({"host_action":"terminal","request":null}),
        json!({"host_action":false,"request":{}}),
    ] {
        let (_plugin, registry) = fixture(root.path(), invalid);
        assert!(
            registry
                .invoke_command("terminal-acceptance", "legacy", json!({}))
                .is_err()
        );
        assert!(
            registry
                .invoke_command_result("terminal-acceptance", "typed", json!({}))
                .is_err()
        );
    }
}

#[cfg(unix)]
#[test]
fn terminal_cwd_grant_resolves_symlinks_before_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let alias = root.path().join("escape");
    std::os::unix::fs::symlink(outside.path(), &alias).unwrap();
    let mut value = request(root.path());
    value["request"]["cwd"] = json!(alias);
    let (_plugin, registry) = fixture(root.path(), value);
    let error = registry
        .invoke_command_result("terminal-acceptance", "open", json!({}))
        .unwrap_err();
    assert!(format!("{error:#}").contains("no read grant"), "{error:#}");
}

#[test]
fn terminal_environment_and_argument_limits_reject_ambiguous_or_oversized_input() {
    let root = tempfile::tempdir().unwrap();
    for (field, value) in [
        ("args", json!(vec!["x"; 1025])),
        ("env", json!({"9INVALID":"x"})),
        ("env", json!({"VALID":"nul\u{0000}value"})),
        (
            "env",
            json!({"FIRST":"a".repeat(32768),"SECOND":"b".repeat(32768),"THIRD":"c".repeat(32768),"FOURTH":"d".repeat(32768),"FIFTH":"e".repeat(32768)}),
        ),
    ] {
        let mut invalid = request(root.path());
        invalid["request"][field] = value;
        let (_plugin, registry) = fixture(root.path(), invalid);
        assert!(
            registry
                .invoke_command_result("terminal-acceptance", "open", json!({}))
                .is_err(),
            "accepted invalid {field}"
        );
    }
}
