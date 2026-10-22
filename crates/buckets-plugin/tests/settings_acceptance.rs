//! Independent checks of saved overrides at the subprocess boundary.
use buckets_plugin::{
    CommandResult, DecorationRequest, IndexRoot, PluginOverrides, PluginRegistry, SearchMode,
    SearchRequest,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

fn manifest(root: &Path) -> Value {
    json!({"id":"settings-probe","name":"Settings probe","protocol_version":1,"executable":"python3","args":["plugin.py"],"capabilities":["commands","search","decorations","provider"],"permissions":{"read_roots":[root],"network":true,"write":true},"config":{"nested":{"retained":true,"replace":"old"},"theme":"dark"}})
}

fn overrides(value: Value) -> BTreeMap<String, PluginOverrides> {
    serde_json::from_value(value).unwrap()
}

#[test]
fn disabled_plugin_rejects_every_dispatch_before_executing_its_process() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("plugin.json"),
        serde_json::to_vec(&manifest(dir.path())).unwrap(),
    )
    .unwrap();
    fs::write(
        dir.path().join("plugin.py"),
        "from pathlib import Path\nPath('EXECUTED').write_text('unexpected')\n",
    )
    .unwrap();
    let registry = PluginRegistry::discover_with_settings(
        dir.path(),
        &overrides(json!({"settings-probe":{"enabled":false}})),
    )
    .unwrap();
    assert_eq!(
        registry.plugins().len(),
        1,
        "disabled plugin must remain configurable"
    );
    assert!(!registry.is_enabled("settings-probe"));
    let results = [
        registry.commands("settings-probe").map(|_| ()),
        registry
            .invoke_command("settings-probe", "open", json!({}))
            .map(|_| ()),
        registry
            .invoke_command_result("settings-probe", "open", json!({}))
            .map(|_| ()),
        registry
            .search(
                "settings-probe",
                SearchRequest {
                    query: "x".into(),
                    roots: vec![IndexRoot {
                        path: dir.path().to_string_lossy().into_owned(),
                        respect_gitignore: true,
                        follow_symlinks: false,
                    }],
                    limit: 1,
                    mode: SearchMode::Filename,
                    config: json!({}),
                },
            )
            .map(|_| ()),
        registry
            .decorate(
                "settings-probe",
                DecorationRequest {
                    location: dir.path().to_string_lossy().into_owned(),
                    entries: vec![],
                },
            )
            .map(|_| ()),
        registry
            .provider_request("settings-probe", "list", json!({"path":"/"}))
            .map(|_| ()),
    ];
    for result in results {
        let error = result.expect_err("disabled plugin dispatched a request");
        assert!(format!("{error:#}").contains("disabled"), "{error:#}");
    }
    assert!(!dir.path().join("EXECUTED").exists());
}

#[test]
fn saved_nested_overrides_reach_process_and_leave_manifest_bytes_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = serde_json::to_vec_pretty(&manifest(dir.path())).unwrap();
    fs::write(dir.path().join("plugin.json"), &bytes).unwrap();
    fs::write(dir.path().join("plugin.py"), "import json,sys\nr=json.load(sys.stdin)\nprint(json.dumps(dict(jsonrpc='2.0',id=r['id'],result=r['params']['config'])))\n").unwrap();
    let saved = dir.path().join("settings.json");
    fs::write(&saved, r#"{"settings-probe":{"config":{"nested":{"replace":"saved","extra":"界"},"new_advanced":[1,2]}}}"#).unwrap();
    let settings =
        serde_json::from_slice::<BTreeMap<String, PluginOverrides>>(&fs::read(saved).unwrap())
            .unwrap();
    let registry = PluginRegistry::discover_with_settings(dir.path(), &settings).unwrap();
    let actual = registry
        .invoke_command("settings-probe", "echo", json!({}))
        .unwrap();
    assert_eq!(
        actual,
        json!({"nested":{"retained":true,"replace":"saved","extra":"界"},"theme":"dark","new_advanced":[1,2]})
    );
    assert_eq!(fs::read(dir.path().join("plugin.json")).unwrap(), bytes);
    assert_eq!(
        registry.plugin("settings-probe").unwrap().default_config,
        manifest(dir.path())["config"]
    );
}

#[test]
fn declared_invalid_values_are_rejected_without_execution() {
    let dir = tempfile::tempdir().unwrap();
    let mut value = manifest(dir.path());
    value["config"] = json!({"target":"embedded","workers":2,"enabled":true,"program":"nvim"});
    value["settings_fields"] = json!([
        {"key":"target","title":"Target","kind":"select","options":["embedded","external"]},
        {"key":"workers","title":"Workers","kind":"number","min":1,"max":8},
        {"key":"enabled","title":"Enabled","kind":"bool"},
        {"key":"program","title":"Program","kind":"text"}
    ]);
    fs::write(
        dir.path().join("plugin.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    fs::write(
        dir.path().join("plugin.py"),
        "from pathlib import Path\nPath('EXECUTED').touch()\n",
    )
    .unwrap();
    for config in [
        json!({"target":"unrecognized"}),
        json!({"workers":0}),
        json!({"workers":9}),
        json!({"enabled":"yes"}),
        json!({"program":"bad\u{0}name"}),
        json!(["not object"]),
    ] {
        assert!(
            PluginRegistry::discover_with_settings(
                dir.path(),
                &overrides(json!({"settings-probe":{"config":config}}))
            )
            .is_err()
        );
    }
    assert!(!dir.path().join("EXECUTED").exists());
}

#[test]
fn removing_overrides_reenables_plugin_and_restores_nested_manifest_defaults() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("plugin.json"),
        serde_json::to_vec(&manifest(dir.path())).unwrap(),
    )
    .unwrap();
    fs::write(dir.path().join("plugin.py"),"import json,sys\nr=json.load(sys.stdin)\nprint(json.dumps(dict(jsonrpc='2.0',id=r['id'],result=r['params']['config'])))\n").unwrap();
    let original = PluginRegistry::discover(dir.path()).unwrap();
    let changed=original.with_settings(&overrides(json!({"settings-probe":{"enabled":false,"config":{"nested":{"replace":"changed"},"override_only":true}}}))).unwrap();
    assert!(!changed.is_enabled("settings-probe"));
    let reset = changed.with_settings(&BTreeMap::new()).unwrap();
    assert!(reset.is_enabled("settings-probe"));
    assert_eq!(
        reset
            .invoke_command("settings-probe", "echo", json!({}))
            .unwrap(),
        manifest(dir.path())["config"]
    );
    assert_eq!(
        original.plugin("settings-probe").unwrap().config,
        manifest(dir.path())["config"],
        "reconfiguration mutated the source catalog"
    );
}

#[test]
fn native_tools_uses_saved_editor_executable_without_rewriting_distribution() {
    let dir = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/native-tools");
    fs::copy(source.join("plugin.py"), dir.path().join("plugin.py")).unwrap();
    let mut value: Value =
        serde_json::from_slice(&fs::read(source.join("plugin.json")).unwrap()).unwrap();
    value["permissions"]["read_roots"] = json!([root.path()]);
    let bytes = serde_json::to_vec(&value).unwrap();
    fs::write(dir.path().join("plugin.json"), &bytes).unwrap();
    let settings = overrides(
        json!({"native-tools":{"config":{"terminal_target":"embedded","nvim":"/bin/echo"}}}),
    );
    let registry = PluginRegistry::discover_with_settings(dir.path(), &settings).unwrap();
    let selected = root.path().join("literal ' quote 界.txt");
    fs::write(&selected, "fixture").unwrap();
    let result = registry
        .invoke_command_result(
            "native-tools",
            "open_nvim",
            json!({"location":root.path(),"selected":[selected]}),
        )
        .unwrap();
    let CommandResult::Terminal(request) = result else {
        panic!("saved embedded target did not return a terminal action")
    };
    assert_eq!(request.program, "/bin/echo");
    assert_eq!(
        request.args,
        vec!["--".to_string(), selected.to_string_lossy().into_owned()]
    );
    assert_eq!(fs::read(dir.path().join("plugin.json")).unwrap(), bytes);
}
