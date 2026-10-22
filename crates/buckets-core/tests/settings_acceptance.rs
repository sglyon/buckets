//! Settings must affect actual Backend provider/search operations, not only a
//! registry created by the settings page. Isolate ambient paths in a child test.
use buckets_core::{Backend, Location, SearchEngine, SearchQuery};
use serde_json::json;
use std::{fs, process::Command};

#[test]
fn backend_provider_and_fff_honor_saved_settings_on_every_dispatch() {
    if let Some(config) = std::env::var_os("BUCKETS_SETTINGS_ACCEPTANCE_CHILD") {
        let config = std::path::PathBuf::from(config);
        let root = config.join("root");
        let settings = config.join("settings.json");
        let marker = config.join("EXECUTED");
        let location = Location::parse("plugin://settings-provider/").unwrap();
        let query = SearchQuery {
            text: "query".into(),
            engine: SearchEngine::Fff,
            ..Default::default()
        };
        fs::write(
            &settings,
            r#"{"plugins":{"settings-provider":{"enabled":false},"fff-search":{"enabled":false}}}"#,
        )
        .unwrap();
        assert!(format!("{:#}", Backend.list(&location).unwrap_err()).contains("disabled"));
        assert!(
            format!(
                "{:#}",
                Backend.search(&Location::local(&root), &query).unwrap_err()
            )
            .contains("disabled")
        );
        assert!(
            !marker.exists(),
            "disabled Backend operation ran plugin process"
        );
        fs::write(&settings,r#"{"plugins":{"settings-provider":{"config":{"label":"saved-provider"}},"fff-search":{"config":{"label":"saved-search"}}}}"#).unwrap();
        assert_eq!(Backend.list(&location).unwrap()[0].name, "saved-provider");
        assert_eq!(
            Backend.search(&Location::local(&root), &query).unwrap()[0].name,
            "saved-search"
        );
        let completed = fs::read(&marker).unwrap();
        fs::write(&settings, "{broken JSON}").unwrap();
        assert!(Backend.list(&location).is_err());
        assert!(Backend.search(&Location::local(&root), &query).is_err());
        assert_eq!(
            fs::read(&marker).unwrap(),
            completed,
            "malformed settings were bypassed by Backend dispatch"
        );
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("root")).unwrap();
    fs::write(temp.path().join("root/saved-search"), "search fixture").unwrap();
    for id in ["settings-provider", "fff-search"] {
        let plugin = temp.path().join("plugins").join(id);
        fs::create_dir_all(&plugin).unwrap();
        fs::write(plugin.join("plugin.json"),serde_json::to_vec(&json!({"id":id,"name":id,"protocol_version":1,"executable":"python3","args":["plugin.py"],"capabilities":["provider","search"],"permissions":{"read_roots":[temp.path()],"network":true},"config":{"label":"manifest-default"}})).unwrap()).unwrap();
        fs::write(plugin.join("plugin.py"),format!(r#"import sys,json
from pathlib import Path
q=json.load(sys.stdin)
with open({},'a') as f:f.write(q['method']+'\n')
c=q['params']['config']
if q['method']=='provider.list': result=[dict(name=c['label'],is_dir=False,size=0)]
elif q['method']=='search': result=[dict(path=str(Path(q['params']['request']['roots'][0]['path'])/c['label']),score=1)]
else:raise RuntimeError(q['method'])
print(json.dumps(dict(jsonrpc='2.0',id=q['id'],result=result)))
"#,serde_json::to_string(&temp.path().join("EXECUTED").to_string_lossy()).unwrap())).unwrap();
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "backend_provider_and_fff_honor_saved_settings_on_every_dispatch",
            "--nocapture",
        ])
        .env("BUCKETS_SETTINGS_ACCEPTANCE_CHILD", temp.path())
        .env("BUCKETS_CONFIG_DIR", temp.path())
        .env("BUCKETS_PLUGIN_DIR", temp.path().join("plugins"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
