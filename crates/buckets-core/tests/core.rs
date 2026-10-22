use buckets_core::{Backend, Location, Pane, SearchEngine, SearchQuery, Workspace};
use std::fs;

#[test]
fn locations_roundtrip_unusual_names_and_roots() {
    for input in [
        "/tmp/a space/#name",
        "ssh://dev@host/home/dev/a%20b",
        "s3://my-bucket/a%20b?endpoint=https%3A%2F%2Fexample.test",
        "git:///tmp/repo?ref=HEAD&path=src%2Flib",
    ] {
        let location = Location::parse(input).unwrap();
        assert_eq!(Location::parse(&location.display()).unwrap(), location);
    }
    assert!(Location::parse("ssh://host/").unwrap().parent().is_none());
    assert!(Location::parse("s3://bucket/").unwrap().parent().is_none());
    assert!(Location::parse("git:///tmp/repo?path=../secret").is_err());
    assert!(Location::parse("ssh://user:password@host/").is_err());
    assert!(Location::parse("/tmp").unwrap().join("../escape").is_err());
}
#[test]
fn copy_preserves_source_and_refuses_collisions() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    fs::write(&a, "source").unwrap();
    fs::write(&b, "destination").unwrap();
    assert!(
        Backend
            .copy(&Location::local(&a), &Location::local(&b))
            .is_err()
    );
    assert_eq!(fs::read_to_string(&b).unwrap(), "destination");
    fs::remove_file(&b).unwrap();
    Backend
        .copy(&Location::local(&a), &Location::local(&b))
        .unwrap();
    assert_eq!(fs::read_to_string(&a).unwrap(), "source");
    assert_eq!(fs::read_to_string(&b).unwrap(), "source");
}
#[test]
fn copy_and_move_refuse_descendants_including_symlinked_parent() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    fs::create_dir_all(src.join("nested")).unwrap();
    assert!(
        Backend
            .copy(
                &Location::local(&src),
                &Location::local(src.join("nested/copy"))
            )
            .is_err()
    );
    assert!(
        Backend
            .move_entry(
                &Location::local(&src),
                &Location::local(src.join("nested/move"))
            )
            .is_err()
    );
    #[cfg(unix)]
    {
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(src.join("nested"), &alias).unwrap();
        assert!(
            Backend
                .copy(&Location::local(&src), &Location::local(alias.join("copy")))
                .is_err()
        );
    }
    assert!(src.exists());
}
#[test]
fn symlinks_are_copied_without_following_them() {
    #[cfg(unix)]
    {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("file"), "ok").unwrap();
        std::os::unix::fs::symlink(".", src.join("loop")).unwrap();
        std::os::unix::fs::symlink("missing", src.join("dangling")).unwrap();
        let dest = temp.path().join("copy");
        Backend
            .copy(&Location::local(&src), &Location::local(&dest))
            .unwrap();
        assert_eq!(
            fs::read_link(dest.join("loop")).unwrap(),
            std::path::PathBuf::from(".")
        );
        assert_eq!(
            fs::read_link(dest.join("dangling")).unwrap(),
            std::path::PathBuf::from("missing")
        );
    }
}
#[test]
fn rename_is_no_replace_and_duplicate_picks_unique_name() {
    let temp = tempfile::tempdir().unwrap();
    let a = Location::local(temp.path().join("notes.md"));
    Backend.create_file(&a).unwrap();
    let one = Backend.duplicate(&a).unwrap();
    let two = Backend.duplicate(&a).unwrap();
    assert_eq!(one.name(), "notes copy.md");
    assert_eq!(two.name(), "notes copy 2.md");
    assert!(Backend.rename(&a, "notes copy.md").is_err());
    assert!(a.local_path().unwrap().exists());
    let renamed = Backend.rename(&a, "renamed.md").unwrap();
    assert!(renamed.local_path().unwrap().exists());
    assert!(!a.local_path().unwrap().exists());
}
#[test]
fn preview_is_bounded_and_detects_binary() {
    let temp = tempfile::tempdir().unwrap();
    let p = temp.path().join("file");
    fs::write(&p, "0123456789").unwrap();
    let preview = Backend.preview(&Location::local(&p), 4).unwrap();
    assert_eq!(preview.text, "0123");
    assert!(preview.truncated);
    fs::write(&p, [0, 1, 2, 3]).unwrap();
    assert!(Backend.preview(&Location::local(&p), 100).unwrap().binary);
}
#[test]
fn failed_navigation_keeps_pane_and_history_intact() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("file"), "").unwrap();
    let mut pane = Pane::new(Location::local(temp.path()));
    pane.load(&Backend).unwrap();
    assert!(
        pane.navigate(&Backend, Location::local(temp.path().join("missing")))
            .is_err()
    );
    assert_eq!(pane.location, Location::local(temp.path()));
    assert_eq!(pane.entries.len(), 1);
    assert!(pane.history.is_empty());
}
#[test]
fn search_respects_gitignore_and_does_not_follow_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join(".gitignore"), "ignored/\n").unwrap();
    fs::create_dir(temp.path().join("ignored")).unwrap();
    fs::write(temp.path().join("ignored/needle"), "").unwrap();
    fs::write(temp.path().join("needle.txt"), "").unwrap();
    let mut query = SearchQuery {
        text: "needle".into(),
        engine: SearchEngine::Builtin,
        ..Default::default()
    };
    let root = Location::local(temp.path());
    assert_eq!(Backend.search(&root, &query).unwrap().len(), 1);
    query.respect_gitignore = false;
    assert_eq!(Backend.search(&root, &query).unwrap().len(), 2);
    query.max_results = 1;
    assert_eq!(Backend.search(&root, &query).unwrap().len(), 1);
}
#[test]
fn pane_selection_survives_refresh_and_workspace_never_loses_last_pane() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("b"), "").unwrap();
    let mut pane = Pane::new(Location::local(temp.path()));
    pane.load(&Backend).unwrap();
    pane.toggle_selected();
    fs::write(temp.path().join("a"), "").unwrap();
    pane.load(&Backend).unwrap();
    assert_eq!(pane.selected_entries()[0].name, "b");
    let mut w = Workspace::new(Location::local(temp.path()));
    w.close_active().unwrap();
    assert!(w.close_active().is_err());
    w.cycle(-1);
    assert_eq!(w.active, 0);
}
#[test]
fn git_snapshot_listing_and_preview_are_real() {
    use std::process::Command;
    let temp = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(temp.path())
                .args(args)
                .status()
                .unwrap()
                .success()
        )
    };
    run(&["init", "-q"]);
    fs::write(temp.path().join("snapshot.txt"), "committed content").unwrap();
    run(&["add", "snapshot.txt"]);
    run(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "-qm",
        "fixture",
    ]);
    fs::write(temp.path().join("snapshot.txt"), "working content").unwrap();
    let location = Location::Git {
        repo: temp.path().into(),
        reference: "HEAD".into(),
        path: String::new(),
    };
    let entries = Backend.list(&location).unwrap();
    assert_eq!(entries[0].name, "snapshot.txt");
    assert_eq!(
        Backend.preview(&entries[0].location, 100).unwrap().text,
        "committed content"
    );
    assert!(Backend.create_file(&location.join("new").unwrap()).is_err());
}

// Environment-sensitive behavior is exercised in child processes so parallel tests never
// change HOME, plugin discovery, or user trash directories in the parent test process.
#[test]
fn isolated_provider_and_trash_child() {
    let Ok(case) = std::env::var("BUCKETS_CORE_TEST_CASE") else {
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os("BUCKETS_CORE_TEST_ROOT").unwrap());
    match case.as_str() {
        "plugin" => {
            let location = Location::parse("plugin://fixture/").unwrap();
            let entries = Backend.list(&location).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].name, "example.txt");
            assert_eq!(
                Backend.preview(&entries[0].location, 100).unwrap().text,
                "plugin text"
            );
            assert!(
                Backend
                    .create_file(&location.join("new.txt").unwrap())
                    .is_err()
            );
        }
        "binary" => {
            let source = Location::parse("plugin://binary/source.bin").unwrap();
            let local = Location::local(root.join("downloaded.bin"));
            Backend.copy(&source, &local).unwrap();
            assert_eq!(
                fs::read(local.local_path().unwrap()).unwrap(),
                [0, 255, 128, 3]
            );
            let uploaded = Location::parse("plugin://binary/uploaded.bin").unwrap();
            Backend.copy(&local, &uploaded).unwrap();
            assert_eq!(
                fs::read(root.join("plugins/binary/uploaded.bin")).unwrap(),
                [0, 255, 128, 3]
            );
            assert!(Backend.copy(&local, &uploaded).is_err());
        }
        "trash" => {
            let source = root.join("delete-me.txt");
            fs::write(&source, "recoverable").unwrap();
            Backend.trash(&Location::local(&source)).unwrap();
            assert!(!source.exists());
            #[cfg(target_os = "macos")]
            let destination = root.join(".Trash/delete-me.txt");
            #[cfg(not(target_os = "macos"))]
            let destination = root.join(".local/share/Trash/files/delete-me.txt");
            assert_eq!(fs::read_to_string(&destination).unwrap(), "recoverable");
            assert!(Backend.trash(&Location::local(&root)).is_err());
            assert!(Backend.trash(&Location::local(&destination)).is_err());
        }
        _ => panic!("Unknown child fixture"),
    }
}
fn child_case(case: &str, root: &std::path::Path) {
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "isolated_provider_and_trash_child",
            "--nocapture",
        ])
        .env("BUCKETS_CORE_TEST_CASE", case)
        .env("BUCKETS_CORE_TEST_ROOT", root)
        .env("HOME", root)
        .env("BUCKETS_PLUGIN_DIR", root.join("plugins"))
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "Child fixture failed: {} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn trash_is_recoverable_and_refuses_home_or_retrash() {
    let temp = tempfile::tempdir().unwrap();
    child_case("trash", temp.path());
}
#[test]
fn external_plugin_provider_is_used_by_real_backend() {
    let temp = tempfile::tempdir().unwrap();
    let plugin = temp.path().join("plugins/fixture");
    fs::create_dir_all(&plugin).unwrap();
    fs::write(plugin.join("plugin.json"),serde_json::to_vec(&serde_json::json!({"id":"fixture","name":"Fixture provider","protocol_version":1,"executable":"python3","args":["provider.py"],"capabilities":["provider"],"permissions":{"read_roots":[temp.path()],"write":false},"timeout_ms":2000})).unwrap()).unwrap();
    fs::write(
        plugin.join("provider.py"),
        r#"import sys,json
q=json.loads(sys.stdin.readline())
if q['method']=='provider.list': result=[dict(name='example.txt',is_dir=False,size=11)]
elif q['method']=='provider.read': result=dict(text='plugin text')
else: raise RuntimeError('unexpected method')
print(json.dumps(dict(jsonrpc='2.0',id=q['id'],result=result)))
"#,
    )
    .unwrap();
    child_case("plugin", temp.path());
}

#[test]
fn installed_fd_and_rg_use_literal_queries_and_ignore_rules() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join(".gitignore"), "ignored/\n").unwrap();
    fs::create_dir(temp.path().join("ignored")).unwrap();
    fs::write(temp.path().join("a[1].txt"), "literal [needle] here\n").unwrap();
    fs::write(
        temp.path().join("ignored/a[1].txt"),
        "literal [needle] hidden\n",
    )
    .unwrap();
    for (program, engine, text) in [
        ("fd", SearchEngine::Fd, "[1]"),
        ("rg", SearchEngine::Rg, "[needle]"),
    ] {
        if std::process::Command::new(program)
            .arg("--version")
            .output()
            .is_err()
        {
            continue;
        }
        let query = SearchQuery {
            text: text.into(),
            engine,
            ..Default::default()
        };
        let hits = Backend
            .search(&Location::local(temp.path()), &query)
            .unwrap();
        assert_eq!(
            hits.len(),
            1,
            "{program} must follow .gitignore outside Git repositories and interpret literals"
        );
        assert_eq!(hits[0].name, "a[1].txt");
        if engine == SearchEngine::Rg {
            assert_eq!(hits[0].line, Some(1));
        }
    }
}

#[test]
fn binary_plugin_provider_transfers_arbitrary_bytes_without_loss() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("plugins/binary");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("source.bin"), [0, 255, 128, 3]).unwrap();
    fs::write(directory.join("plugin.json"),serde_json::to_vec(&serde_json::json!({"id":"binary","name":"Binary filesystem fixture","protocol_version":1,"executable":"python3","args":["provider.py"],"capabilities":["provider"],"permissions":{"read_roots":[temp.path()],"write":true},"timeout_ms":2000})).unwrap()).unwrap();
    fs::write(directory.join("provider.py"),r#"import sys,json,base64
from pathlib import Path
root=Path(__file__).parent
q=json.loads(sys.stdin.readline());r=q['params']['request'];method=q['method']
if method=='provider.list':result=[dict(name=p.name,is_dir=False,size=p.stat().st_size) for p in root.glob('*.bin')]
elif method=='provider.read':
 data=(root/r['path'].lstrip('/')).read_bytes();limit=r['max_bytes'];result=dict(data_base64=base64.b64encode(data[:limit]).decode(),truncated=len(data)>limit)
elif method=='provider.write':
 data=base64.b64decode(r['data_base64'])
 with open(root/r['path'].lstrip('/'),'xb') as file:file.write(data)
 result={}
else:raise RuntimeError(method)
print(json.dumps(dict(jsonrpc='2.0',id=q['id'],result=result)))
"#).unwrap();
    child_case("binary", temp.path());
}
