use buckets_core::{AgentBroker, AgentRoot, Location};
use buckets_plugin::{AgentCitation, AgentFileResult, AgentResult, WorkspaceContext};
use serde_json::json;
use std::fs;
fn broker(root: &str) -> AgentBroker {
    AgentBroker::new(
        WorkspaceContext {
            workspace_id: "fixture".into(),
            revision: 1,
            panes: vec![],
        },
        vec![AgentRoot {
            location: root.into(),
            respect_gitignore: true,
            follow_symlinks: false,
        }],
        true,
    )
    .unwrap()
}
#[test]
fn read_tools_are_bounded_and_ignore_policy_cannot_be_relaxed() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path();
    fs::write(p.join("a.txt"), "abcdef").unwrap();
    fs::write(p.join(".gitignore"), "ignored.txt\n").unwrap();
    fs::write(p.join("ignored.txt"), "private index material").unwrap();
    let b = broker(p.to_str().unwrap());
    let read = b
        .handle(
            "files.read",
            &json!({"location":p.join("a.txt"),"max_bytes":3}),
        )
        .unwrap();
    assert_eq!(read["text"], "abc");
    assert_eq!(read["truncated"], true);
    let hits = b
        .handle(
            "files.search",
            &json!({"location":p,"query":"","respect_gitignore":false}),
        )
        .unwrap();
    assert!(
        hits["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["name"] == "a.txt")
    );
    assert!(
        !hits["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["name"] == "ignored.txt")
    );
    // Explicit known-file reads remain possible; indexing uses filtered search.
    assert!(
        b.handle("files.read", &json!({"location":p.join("ignored.txt")}))
            .is_ok()
    );
    assert!(
        b.handle(
            "files.read",
            &json!({"location":p.join("a.txt"),"max_bytes":262145})
        )
        .is_err()
    );
    assert!(
        b.handle("files.remove", &json!({"location":p.join("a.txt")}))
            .is_err()
    );
    assert!(
        b.handle("files.list", &json!({"location":p,"limit":1001}))
            .is_err()
    );
}
#[test]
fn provider_identity_and_path_component_boundaries_are_preserved() {
    let ssh = broker("ssh://example/project");
    assert!(ssh.authorize("ssh://example/project/a.txt").is_ok());
    for location in [
        "ssh://other/project/a.txt",
        "ssh://example/project-other/a.txt",
        "ssh://example/project/%2e%2e/secret",
    ] {
        assert!(ssh.authorize(location).is_err(), "{location}");
    }
    let s3 = broker("s3://bucket/prefix?endpoint=https%3A%2F%2Fstorage.example");
    assert!(
        s3.authorize("s3://bucket/prefix/a?endpoint=https%3A%2F%2Fstorage.example")
            .is_ok()
    );
    assert!(
        s3.authorize("s3://bucket/prefix/a?endpoint=https%3A%2F%2Fother.example")
            .is_err()
    );
    assert!(
        s3.authorize("s3://bucket/prefix-other/a?endpoint=https%3A%2F%2Fstorage.example")
            .is_err()
    );
    let no_network = AgentBroker::new(
        WorkspaceContext {
            workspace_id: "fixture".into(),
            ..Default::default()
        },
        vec![AgentRoot {
            location: "ssh://example/project".into(),
            respect_gitignore: true,
            follow_symlinks: false,
        }],
        false,
    )
    .unwrap();
    assert!(no_network.authorize("ssh://example/project/a").is_err());
}
#[test]
fn local_symlink_escape_and_fabricated_references_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(root.path().join("good"), "content").unwrap();
    fs::write(outside.path().join("secret"), "secret").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
    let b = broker(root.path().to_str().unwrap());
    #[cfg(unix)]
    assert!(
        b.handle(
            "files.read",
            &json!({"location":root.path().join("escape/secret")})
        )
        .is_err()
    );
    let mut result = AgentResult {
        answer: "Answer".into(),
        citations: vec![AgentCitation {
            location: root.path().join("good").to_string_lossy().into(),
            line: Some(1),
            label: None,
        }],
        ..Default::default()
    };
    b.validate_result(&result).unwrap();
    result.citations[0].line = Some(0);
    assert!(b.validate_result(&result).is_err());
    result.citations.clear();
    result.results.push(AgentFileResult {
        location: outside.path().join("secret").to_string_lossy().into(),
        name: "secret".into(),
        ..Default::default()
    });
    assert!(b.validate_result(&result).is_err());
    result.results[0].location = Location::local(root.path().join("good")).display();
    result.results[0].score = f64::NAN;
    assert!(b.validate_result(&result).is_err());
}
#[test]
fn revision_snapshot_is_the_supplied_invocation_context() {
    let b = AgentBroker::new(
        WorkspaceContext {
            workspace_id: "stable-workspace".into(),
            revision: 42,
            panes: vec![],
        },
        vec![],
        false,
    )
    .unwrap();
    let snapshot = b.handle("workspace.snapshot", &json!({})).unwrap();
    assert_eq!(snapshot["workspace_id"], "stable-workspace");
    assert_eq!(snapshot["revision"], 42);
}
#[test]
fn overlapping_roots_use_strict_ignore_policy_and_implicit_symlinks_are_filtered() {
    use buckets_plugin::{AgentEntry, AgentPaneContext};
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let child = temp.path().join("child");
    fs::create_dir(&child).unwrap();
    fs::write(child.join(".gitignore"), "ignored\n").unwrap();
    fs::write(child.join("ignored"), "exclude").unwrap();
    fs::write(child.join("good"), "include").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), child.join("escape")).unwrap();
    let escape = child.join("escape").to_string_lossy().into_owned();
    let context = WorkspaceContext {
        workspace_id: "snapshot".into(),
        revision: 1,
        panes: vec![AgentPaneContext {
            id: "pane".into(),
            location: child.to_string_lossy().into(),
            cursor: Some(escape.clone()),
            entries: vec![AgentEntry {
                location: escape.clone(),
                name: "escape".into(),
                is_dir: true,
            }],
            ..Default::default()
        }],
    };
    let roots = vec![
        AgentRoot {
            location: temp.path().to_string_lossy().into(),
            respect_gitignore: false,
            follow_symlinks: false,
        },
        AgentRoot {
            location: child.to_string_lossy().into(),
            respect_gitignore: true,
            follow_symlinks: false,
        },
    ];
    let b = AgentBroker::new(context.clone(), roots.clone(), false).unwrap();
    let snapshot = b.handle("workspace.snapshot", &json!({})).unwrap();
    assert!(snapshot["panes"][0]["cursor"].is_null());
    assert_eq!(snapshot["panes"][0]["entries"], json!([]));
    let results = b
        .handle(
            "files.search",
            &json!({"location":child,"query":"","respect_gitignore":false}),
        )
        .unwrap();
    assert!(
        !results["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "ignored")
    );
    let mut selected = context;
    selected.panes[0].selected.push(escape);
    assert!(AgentBroker::new(selected, roots, false).is_err());
}
#[test]
#[cfg(unix)]
fn git_repository_grants_canonicalize_symlinked_repo_identity() {
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real");
    let alias = temp.path().join("alias");
    fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let location = Location::Git {
        repo: alias,
        reference: "HEAD".into(),
        path: "src".into(),
    };
    let b = broker(&location.display());
    assert!(
        b.authorize(
            &Location::Git {
                repo: real.clone(),
                reference: "HEAD".into(),
                path: "src/main.rs".into()
            }
            .display()
        )
        .is_ok()
    );
    assert!(
        b.authorize(
            &Location::Git {
                repo: real,
                reference: "other".into(),
                path: "src/main.rs".into()
            }
            .display()
        )
        .is_err()
    );
}
#[test]
fn s3_endpoint_credentials_never_enter_agent_context() {
    for endpoint in [
        "https://user:secret@storage.example",
        "https://storage.example?token=secret",
        "https://storage.example#secret",
    ] {
        let location = Location::S3 {
            bucket: "bucket".into(),
            prefix: "prefix".into(),
            endpoint: Some(endpoint.into()),
        };
        let result = AgentBroker::new(
            WorkspaceContext {
                workspace_id: "credential-boundary".into(),
                ..Default::default()
            },
            vec![AgentRoot {
                location: location.display(),
                respect_gitignore: true,
                follow_symlinks: false,
            }],
            true,
        );
        assert!(result.is_err(), "accepted {endpoint}");
    }
}

#[test]
fn file_only_index_candidates_are_counted_after_directory_filtering() {
    let temp = tempfile::tempdir().unwrap();
    for n in 0..160 {
        fs::create_dir(temp.path().join(format!("a-directory-{n:03}"))).unwrap();
    }
    let nested = temp.path().join("z-files");
    fs::create_dir(&nested).unwrap();
    for name in ["alpha.txt", "beta.txt", "gamma.txt"] {
        fs::write(nested.join(name), name).unwrap();
    }
    fs::write(temp.path().join(".gitignore"), "ignored.txt\n").unwrap();
    fs::write(nested.join("ignored.txt"), "not indexed").unwrap();
    let b = broker(temp.path().to_str().unwrap());
    let response=b.handle("files.search",&json!({"location":temp.path(),"query":"","engine":"builtin","limit":3,"files_only":true})).unwrap();
    let results = response["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    assert!(
        results
            .iter()
            .all(|hit| hit["is_dir"] == false && hit["name"] != "ignored.txt")
    );
    let names = results
        .iter()
        .map(|hit| hit["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["alpha.txt", "beta.txt", "gamma.txt"]);
    // Exercise the exact provider-side Python search used over SSH, using a
    // local fixture rather than claiming live SSH authentication coverage.
    let query = json!({"engine":"builtin","text":"","hidden":false,"respect_gitignore":true,"max_results":3,"files_only":true});
    let output = std::process::Command::new("python3")
        .arg(format!("{}/src/ssh_search.py", env!("CARGO_MANIFEST_DIR")))
        .arg(temp.path())
        .arg(query.to_string())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let remote: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(remote.len(), 3);
    assert!(remote.iter().all(
        |hit| std::path::Path::new(hit["path"].as_str().unwrap()).is_file()
            && hit["name"] != "ignored.txt"
    ));
}
#[test]
fn file_result_labels_and_kinds_must_match_authorized_targets() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("real.rs");
    fs::write(&file, "content").unwrap();
    let directory = temp.path().join("folder");
    fs::create_dir(&directory).unwrap();
    let b = broker(temp.path().to_str().unwrap());
    let read = b.handle("files.read", &json!({"location":file})).unwrap();
    assert_eq!(read["name"], "real.rs");
    let mut result = AgentResult {
        results: vec![AgentFileResult {
            location: file.to_string_lossy().into(),
            name: "README.md".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(
        b.validate_result(&result)
            .unwrap_err()
            .to_string()
            .contains("name does not match")
    );
    result.results[0].name = "real.rs".into();
    b.validate_result(&result).unwrap();
    result.results[0].is_dir = true;
    assert!(b.validate_result(&result).is_err());
    result.results[0].is_dir = false;
    result.results[0].location = directory.to_string_lossy().into();
    result.results[0].name = "folder".into();
    assert!(
        b.validate_result(&result)
            .unwrap_err()
            .to_string()
            .contains("not a regular file")
    );
}
