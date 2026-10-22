use buckets_plugin::{Capability, IndexRoot, PluginRegistry, SearchMode, SearchRequest};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

fn fixture(code: &str, timeout: u64, capabilities: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("plugin.py"), code).unwrap();
    fs::write(
        dir.path().join("plugin.json"),
        serde_json::to_vec(&json!({
            "id":"fixture", "name":"Fixture", "protocol_version":1,
            "executable":"python3", "args":["plugin.py"],
            "capabilities":capabilities, "permissions":{"read_roots":["."],"network":false},
            "timeout_ms":timeout
        }))
        .unwrap(),
    )
    .unwrap();
    dir
}

fn invoke(dir: &Path) -> anyhow::Result<Value> {
    PluginRegistry::discover(dir)?.invoke_command(
        "fixture",
        "test",
        json!({"message":"quoted '\" $() ` ;"}),
    )
}

#[test]
fn subprocess_round_trip_preserves_arguments() {
    let dir = fixture(
        "import json,sys\nr=json.load(sys.stdin)\nprint(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':r['params']['request']['arguments']}))",
        2000,
        &["commands"],
    );
    assert_eq!(
        invoke(dir.path()).unwrap(),
        json!({"message":"quoted '\" $() ` ;"})
    );
    assert!(
        PluginRegistry::discover(dir.path()).unwrap().plugins()[0].supports(Capability::Commands)
    );
}

#[test]
fn missing_capability_is_rejected_before_execution() {
    let dir = fixture("raise Exception('must not run')", 2000, &["search"]);
    assert!(format!("{:#}", invoke(dir.path()).unwrap_err()).contains("did not declare"));
}

#[test]
fn timeout_is_bounded_even_with_ignored_stdin() {
    let dir = fixture("import time\ntime.sleep(30)", 100, &["commands"]);
    let start = Instant::now();
    assert!(format!("{:#}", invoke(dir.path()).unwrap_err()).contains("timed out"));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn malformed_wrong_id_and_ambiguous_responses_are_rejected() {
    for output in [
        "not json",
        r#"{"jsonrpc":"2.0","id":2,"result":null}"#,
        r#"{"jsonrpc":"2.0","id":1,"result":null,"error":{"code":1,"message":"oops"}}"#,
        "{}\n{}",
    ] {
        let code = format!("print({})", serde_json::to_string(output).unwrap());
        let dir = fixture(&code, 2000, &["commands"]);
        assert!(invoke(dir.path()).is_err(), "accepted {output}");
    }
}

#[test]
fn errors_and_stderr_are_actionable() {
    let dir = fixture(
        "import sys\nprint('missing dependency',file=sys.stderr)\nsys.exit(3)",
        2000,
        &["commands"],
    );
    assert!(format!("{:#}", invoke(dir.path()).unwrap_err()).contains("missing dependency"));
    let dir = fixture(
        "print('{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32000,\"message\":\"model offline\"}}')",
        2000,
        &["commands"],
    );
    assert!(format!("{:#}", invoke(dir.path()).unwrap_err()).contains("model offline"));
}

#[test]
fn excessive_output_is_rejected() {
    let dir = fixture(
        "import sys\nsys.stdout.write('x' * 9000000)",
        2000,
        &["commands"],
    );
    assert!(format!("{:#}", invoke(dir.path()).unwrap_err()).contains("exceeds 8 MiB"));
}

#[test]
fn duplicate_plugin_ids_and_future_versions_are_rejected() {
    let dir = fixture("", 2000, &["commands"]);
    fs::create_dir(dir.path().join("duplicate")).unwrap();
    fs::copy(
        dir.path().join("plugin.json"),
        dir.path().join("duplicate/plugin.json"),
    )
    .unwrap();
    assert!(
        format!("{:#}", PluginRegistry::discover(dir.path()).unwrap_err()).contains("Duplicate")
    );
    fs::remove_file(dir.path().join("duplicate/plugin.json")).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(dir.path().join("plugin.json")).unwrap()).unwrap();
    manifest["protocol_version"] = json!(9);
    fs::write(
        dir.path().join("plugin.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert!(
        format!("{:#}", PluginRegistry::discover(dir.path()).unwrap_err()).contains("protocol 9")
    );
}

#[test]
fn search_roots_and_result_paths_obey_grants() {
    let dir = fixture(
        "import json,sys\nr=json.load(sys.stdin)\np=r['params']['request']['config']['path']\nprint(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':[{'path':p,'score':1.0}]}))",
        2000,
        &["search"],
    );
    let outside = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("inside.txt"), "ok").unwrap();
    fs::write(outside.path().join("outside.txt"), "private").unwrap();
    let registry = PluginRegistry::discover(dir.path()).unwrap();
    let request = |root: &Path, result: &Path| SearchRequest {
        query: "test".into(),
        roots: vec![IndexRoot {
            path: root.to_string_lossy().into_owned(),
            respect_gitignore: true,
            follow_symlinks: false,
        }],
        limit: 10,
        mode: SearchMode::Filename,
        config: json!({"path":result}),
    };
    assert_eq!(
        registry
            .search(
                "fixture",
                request(dir.path(), &dir.path().join("inside.txt"))
            )
            .unwrap()
            .len(),
        1
    );
    assert!(
        format!(
            "{:#}",
            registry
                .search(
                    "fixture",
                    request(outside.path(), &outside.path().join("outside.txt"))
                )
                .unwrap_err()
        )
        .contains("no read grant")
    );
    assert!(
        format!(
            "{:#}",
            registry
                .search(
                    "fixture",
                    request(dir.path(), &outside.path().join("outside.txt"))
                )
                .unwrap_err()
        )
        .contains("outside requested roots")
    );
}

#[test]
fn python_example_searches_real_files_and_respects_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success()
    );
    fs::write(dir.path().join("keep.txt"), "needle\n").unwrap();
    fs::write(dir.path().join("secret.txt"), "needle\n").unwrap();
    fs::write(dir.path().join(".gitignore"), "secret.txt\n").unwrap();
    let example = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/plugins/python-search/search.py");
    fs::copy(example, dir.path().join("search.py")).unwrap();
    fs::write(dir.path().join("plugin.json"), serde_json::to_vec(&json!({"id":"python", "name":"Python", "protocol_version":1,"executable":"python3","args":["search.py"],"capabilities":["search"],"permissions":{"read_roots":["."]}})).unwrap()).unwrap();
    let registry = PluginRegistry::discover(dir.path()).unwrap();
    let request = SearchRequest {
        query: "needle".into(),
        roots: vec![IndexRoot {
            path: dir.path().to_string_lossy().into_owned(),
            respect_gitignore: true,
            follow_symlinks: false,
        }],
        limit: 10,
        mode: SearchMode::Content,
        config: Value::Null,
    };
    let hits = registry.search("python", request).unwrap();
    assert!(
        hits.iter()
            .any(|hit| hit.path.ends_with("keep.txt") && hit.line == Some(1))
    );
    assert!(!hits.iter().any(|hit| hit.path.ends_with("secret.txt")));
}

#[test]
fn local_provider_lists_reads_and_rejects_writes_and_escape() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("readme.txt"), "hello provider").unwrap();
    let example = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/plugins/local-provider/provider.py");
    fs::copy(example, dir.path().join("provider.py")).unwrap();
    fs::write(dir.path().join("plugin.json"), serde_json::to_vec(&json!({"id":"provider", "name":"Provider", "protocol_version":1,"executable":"python3","args":["provider.py"],"capabilities":["provider"],"permissions":{"read_roots":["."]},"config":{"root":"."}})).unwrap()).unwrap();
    let registry = PluginRegistry::discover(dir.path()).unwrap();
    let listing = registry
        .provider_request("provider", "list", json!({"path":"/"}))
        .unwrap();
    assert!(
        listing
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "readme.txt")
    );
    let preview = registry
        .provider_request(
            "provider",
            "read",
            json!({"path":"/readme.txt","max_bytes":5}),
        )
        .unwrap();
    assert_eq!(preview, json!({"text":"hello","truncated":true}));
    for (name, bytes) in [
        ("binary.bin", vec![0, 1, 2]),
        ("invalid.bin", vec![255, 254, 65]),
    ] {
        fs::write(dir.path().join(name), bytes).unwrap();
        assert!(
            registry
                .provider_request(
                    "provider",
                    "read",
                    json!({"path":format!("/{name}"),"max_bytes":100})
                )
                .is_err()
        );
    }
    fs::write(dir.path().join("unicode.txt"), "aéz").unwrap();
    let unicode_preview = registry
        .provider_request(
            "provider",
            "read",
            json!({"path":"/unicode.txt","max_bytes":2}),
        )
        .unwrap();
    assert_eq!(unicode_preview, json!({"text":"a","truncated":true}));
    let full_unicode = registry
        .provider_request(
            "provider",
            "read",
            json!({"path":"/unicode.txt","max_bytes":4}),
        )
        .unwrap();
    assert_eq!(full_unicode, json!({"text":"aéz","truncated":false}));
    assert!(
        format!(
            "{:#}",
            registry
                .provider_request(
                    "provider",
                    "write",
                    json!({"path":"/readme.txt","text":"oops"})
                )
                .unwrap_err()
        )
        .contains("no write permission")
    );
    assert!(
        registry
            .provider_request("provider", "list", json!({"path":"/../"}))
            .is_err()
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        assert!(
            registry
                .provider_request("provider", "list", json!({"path":"/escape"}))
                .is_err()
        );
    }
    assert_eq!(
        fs::read_to_string(dir.path().join("readme.txt")).unwrap(),
        "hello provider"
    );
}

#[test]
#[ignore = "requires the project's .venv with fff-search 0.11 installed"]
fn real_fff_sdk_finds_filename_content_and_respects_gitignore() {
    let project = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        dir.path().join("bucket_example.rs"),
        "fn uniquely_identified_function() {}\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("bucket_secret.rs"),
        "fn uniquely_identified_function() {}\n",
    )
    .unwrap();
    fs::write(dir.path().join(".gitignore"), "bucket_secret.rs\n").unwrap();
    fs::copy(
        project.join("examples/plugins/fff-search/search.py"),
        dir.path().join("search.py"),
    )
    .unwrap();
    fs::write(dir.path().join("plugin.json"), serde_json::to_vec(&json!({"id":"fff", "name":"FFF", "protocol_version":1,"executable":project.join(".venv/bin/python"),"args":["search.py"],"capabilities":["search"],"permissions":{"read_roots":["."]},"timeout_ms":15000})).unwrap()).unwrap();
    let registry = PluginRegistry::discover(dir.path()).unwrap();
    let request = |mode, query: &str| SearchRequest {
        query: query.into(),
        roots: vec![IndexRoot {
            path: dir.path().to_string_lossy().into_owned(),
            respect_gitignore: true,
            follow_symlinks: false,
        }],
        limit: 10,
        mode,
        config: Value::Null,
    };
    let filename = registry
        .search("fff", request(SearchMode::Filename, "buckexamp"))
        .unwrap();
    assert!(
        filename
            .iter()
            .any(|hit| hit.path.ends_with("bucket_example.rs") && hit.score > 0.0)
    );
    assert!(
        !filename
            .iter()
            .any(|hit| hit.path.ends_with("bucket_secret.rs"))
    );
    let content = registry
        .search(
            "fff",
            request(SearchMode::Content, "uniquely_identified_function"),
        )
        .unwrap();
    assert!(content.iter().any(|hit| {
        hit.path.ends_with("bucket_example.rs")
            && hit.line == Some(1)
            && hit
                .snippet
                .as_deref()
                .unwrap()
                .contains("uniquely_identified_function")
    }));
    assert!(
        !content
            .iter()
            .any(|hit| hit.path.ends_with("bucket_secret.rs"))
    );
}
