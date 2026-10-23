use buckets_plugin::{
    CommandContext, CommandResult, DecorationRequest, DecorationTone, PluginRegistry,
};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

fn example(
    name: &str,
    root: &Path,
    config: serde_json::Value,
) -> (tempfile::TempDir, PluginRegistry) {
    let dir = tempfile::tempdir().unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/plugins")
        .join(name);
    fs::copy(source.join("plugin.py"), dir.path().join("plugin.py")).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(source.join("plugin.json")).unwrap()).unwrap();
    manifest["permissions"]["read_roots"] = json!([root]);
    manifest["config"] = config;
    fs::write(
        dir.path().join("plugin.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let registry = PluginRegistry::discover(dir.path()).unwrap();
    (dir, registry)
}

fn git(root: &Path, args: &[&str]) {
    let result = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn git_decorations_cover_staged_unstaged_renamed_untracked_ignored_and_directories() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    git(&root, &["init", "-q"]);
    fs::create_dir(root.join("nested")).unwrap();
    for name in [
        "staged.txt",
        "unstaged.txt",
        "rename old\nfile.txt",
        "nested/child.txt",
    ] {
        fs::write(root.join(name), "original\n").unwrap();
    }
    fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
    git(&root, &["add", "."]);
    git(
        &root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "fixture",
        ],
    );
    fs::write(root.join("staged.txt"), "staged\n").unwrap();
    git(&root, &["add", "staged.txt"]);
    fs::write(root.join("unstaged.txt"), "unstaged\n").unwrap();
    git(
        &root,
        &["mv", "rename old\nfile.txt", "rename 'new' file.txt"],
    );
    fs::write(root.join("nested/child.txt"), "changed\n").unwrap();
    fs::write(root.join("untracked.txt"), "new\n").unwrap();
    fs::write(root.join("ignored.txt"), "hidden\n").unwrap();
    let (_plugin, registry) = example("git-status", &root, json!({"show_ignored":true}));
    let names = [
        "staged.txt",
        "unstaged.txt",
        "rename 'new' file.txt",
        "nested",
        "untracked.txt",
        "ignored.txt",
    ];
    let request = DecorationRequest {
        location: root.to_string_lossy().into_owned(),
        entries: names
            .iter()
            .map(|name| root.join(name).to_string_lossy().into_owned())
            .collect(),
    };
    let decorations = registry.decorate("git-status", request).unwrap();
    assert_eq!(decorations.len(), names.len());
    let by_name = |name: &str| {
        decorations
            .iter()
            .find(|d| d.path == root.join(name).to_string_lossy())
            .unwrap()
    };
    assert_eq!(by_name("staged.txt").tone, DecorationTone::Success);
    assert!(by_name("staged.txt").tooltip.contains("staged modified"));
    assert_eq!(by_name("unstaged.txt").tone, DecorationTone::Warning);
    assert!(by_name("rename 'new' file.txt").tooltip.contains("renamed"));
    assert_eq!(by_name("nested").badge, "M");
    assert_eq!(by_name("untracked.txt").badge, "?");
    assert_eq!(by_name("ignored.txt").tone, DecorationTone::Neutral);
    let outside = tempfile::tempdir().unwrap();
    assert!(
        registry
            .decorate(
                "git-status",
                DecorationRequest {
                    location: outside.path().to_string_lossy().into_owned(),
                    entries: vec![]
                }
            )
            .unwrap()
            .is_empty()
    );
}

#[test]
fn decoration_host_rejects_unrequested_paths_and_oversized_badges() {
    for result in [
        json!([{"path":"/not-requested","badge":"M","tooltip":"bad"}]),
        json!([{"path":"/requested","badge":"0123456789","tooltip":"too wide"}]),
    ] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("plugin.py"), format!("import json,sys\nr=json.load(sys.stdin)\nprint(json.dumps({{'jsonrpc':'2.0','id':r['id'],'result':json.loads({})}}))",serde_json::to_string(&result.to_string()).unwrap())).unwrap();
        fs::write(dir.path().join("plugin.json"), serde_json::to_vec(&json!({"id":"fixture","name":"Fixture","protocol_version":1,"executable":"python3","args":["plugin.py"],"capabilities":["decorations"]})).unwrap()).unwrap();
        let registry = PluginRegistry::discover(dir.path()).unwrap();
        assert!(
            registry
                .decorate(
                    "fixture",
                    DecorationRequest {
                        location: "/".into(),
                        entries: vec!["/requested".into()]
                    }
                )
                .is_err()
        );
    }
}

#[cfg(unix)]
#[test]
fn native_tools_launch_with_literal_argv_cwd_and_private_self_removing_script() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let cwd = root.join("space 'quote' $(touch INJECTED)");
    fs::create_dir(&cwd).unwrap();
    let selected = cwd.join("file 'quote' $(touch INJECTED) `whoami`.txt");
    fs::write(&selected, "data").unwrap();
    let capture = root.join("capture.json");
    let tool = root.join("fake nvim");
    let tool_code = format!(
        "#!/usr/bin/env python3\nimport json,os,sys\nwith open({},'w') as f: json.dump({{'cwd':os.getcwd(),'argv':sys.argv[1:]}},f)\n",
        serde_json::to_string(&capture.to_string_lossy()).unwrap()
    );
    fs::write(&tool, tool_code).unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
    let launcher = root.join("fake-terminal.py");
    let script_capture = root.join("script.json");
    fs::write(&launcher,"import json,os,subprocess,sys\np=sys.argv[1]\nwith open(sys.argv[2],'w') as f: json.dump({'path':p,'mode':os.stat(p).st_mode & 0o777},f)\nsubprocess.run([p],check=True)\n").unwrap();
    let (_plugin, registry) = example(
        "native-tools",
        &root,
        json!({"terminal_target":"external","nvim":tool,"lazygit":tool,"shell":tool,"terminal_argv":["python3",launcher,"{script}",script_capture]}),
    );
    let commands = registry.commands("native-tools").unwrap();
    assert_eq!(commands.len(), 6);
    assert_eq!(commands[0].shortcut.as_deref(), Some("ctrl-e"));
    registry
        .invoke_command(
            "native-tools",
            "open_nvim",
            json!({"location":cwd,"selected":[selected]}),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let result: serde_json::Value = loop {
        if let Ok(bytes) = fs::read(&capture)
            && let Ok(result) = serde_json::from_slice(&bytes)
        {
            break result;
        }
        assert!(
            Instant::now() < deadline,
            "Fake terminal did not finish writing argv capture"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(result["cwd"], json!(cwd));
    assert_eq!(result["argv"], json!(["--", selected]));
    let script: serde_json::Value =
        serde_json::from_slice(&fs::read(script_capture).unwrap()).unwrap();
    assert_eq!(script["mode"], 0o700);
    assert!(!PathBuf::from(script["path"].as_str().unwrap()).exists());
    assert!(!cwd.join("INJECTED").exists());
    assert!(
        registry
            .invoke_command(
                "native-tools",
                "open_nvim",
                json!({"location":"ssh://host/path","selected":[]})
            )
            .is_err()
    );
}

#[test]
fn native_tools_report_missing_executable_before_launching_terminal() {
    let root = tempfile::tempdir().unwrap();
    let (_plugin, registry) = example(
        "native-tools",
        root.path(),
        json!({"nvim":"buckets-no-such-nvim-executable"}),
    );
    let error = registry
        .invoke_command(
            "native-tools",
            "open_nvim",
            json!({"location":root.path(),"selected":[]}),
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("Neovim is unavailable"));
}

#[cfg(unix)]
#[test]
fn native_tools_request_host_owned_terminal_with_literal_selection_and_no_launch() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let selected = directory.join("a 'quote' $(touch SHOULD_NOT_EXIST).txt");
    fs::write(&selected, "content").unwrap();
    let fake_tool = directory.join("fake-editor");
    fs::write(&fake_tool, "#!/bin/sh\ntouch SHOULD_NOT_EXIST\n").unwrap();
    fs::set_permissions(&fake_tool, fs::Permissions::from_mode(0o700)).unwrap();
    let (_plugin, registry) = example(
        "native-tools",
        &directory,
        json!({"nvim":fake_tool,"lazygit":fake_tool,"shell":"/bin/sh"}),
    );
    let result = registry
        .invoke_command_result(
            "native-tools",
            "open_nvim",
            json!({"location":directory,"selected":[selected]}),
        )
        .unwrap();
    let CommandResult::Terminal(request) = result else {
        panic!("expected terminal launch request")
    };
    assert_eq!(request.program, fake_tool.to_string_lossy());
    assert_eq!(request.cwd, directory);
    assert_eq!(
        request.args,
        vec!["--".to_owned(), selected.to_string_lossy().into_owned()]
    );
    assert_eq!(request.title, "Neovim");
    assert!(
        !directory.join("SHOULD_NOT_EXIST").exists(),
        "plugin ran the editor instead of requesting a host-owned terminal"
    );
    let result = registry
        .invoke_command_result(
            "native-tools",
            "open_terminal",
            json!({"location":directory,"selected":[]}),
        )
        .unwrap();
    let CommandResult::Terminal(request) = result else {
        panic!("expected shell request")
    };
    assert_eq!(request.program, "/bin/sh");
    assert_eq!(request.args, vec!["-l"]);
}

#[test]
fn terminal_request_requires_capability_permission_and_valid_bounded_argv() {
    let root = tempfile::tempdir().unwrap();
    let request = json!({"program":"/bin/sh","args":["literal $() 'quotes'"],"cwd":root.path(),"title":"Shell","env":{"BUCKETS_FIXTURE":"yes"}});
    let result = json!({"host_action":"terminal","request":request});
    let (plugin, _) = example("native-tools", root.path(), json!({}));
    fs::write(plugin.path().join("plugin.py"),format!("import json,sys\nr=json.load(sys.stdin)\nprint(json.dumps({{'jsonrpc':'2.0','id':r['id'],'result':json.loads({})}}))",serde_json::to_string(&result.to_string()).unwrap())).unwrap();
    let manifest_path = plugin.path().join("plugin.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    let invoke = || {
        PluginRegistry::discover(plugin.path())
            .unwrap()
            .invoke_command_result("native-tools", "test", json!({}))
    };
    assert!(matches!(invoke().unwrap(), CommandResult::Terminal(_)));
    manifest["permissions"]["terminal"] = json!(false);
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(format!("{:#}", invoke().unwrap_err()).contains("no terminal permission"));
    manifest["permissions"]["terminal"] = json!(true);
    manifest["capabilities"] = json!(["commands"]);
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(format!("{:#}", invoke().unwrap_err()).contains("Terminal capability"));
    manifest["capabilities"] = json!(["commands", "terminal"]);
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    for (field, value) in [
        ("program", json!("relative-program")),
        ("args", json!(["nul\u{0000}argument"])),
        ("cwd", json!("/does-not-exist/buckets-test")),
        ("title", json!("bad\nterminal title")),
        ("env", json!({"INVALID=NAME":"x"})),
    ] {
        let mut invalid = result.clone();
        invalid["request"][field] = value;
        fs::write(plugin.path().join("plugin.py"),format!("import json,sys\nr=json.load(sys.stdin)\nprint(json.dumps({{'jsonrpc':'2.0','id':r['id'],'result':json.loads({})}}))",serde_json::to_string(&invalid.to_string()).unwrap())).unwrap();
        assert!(invoke().is_err(), "accepted invalid {field}");
    }
}

#[test]
fn native_tools_command_line_paths_override_selection_without_shell_interpolation() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let child = directory.join("space 'quote' $(touch NO)");
    fs::create_dir(&child).unwrap();
    let selected = directory.join("selected.txt");
    fs::write(&selected, "selected").unwrap();
    let (_plugin, registry) = example(
        "native-tools",
        &directory,
        json!({"nvim":"/bin/echo","lazygit":"/bin/echo","shell":"/bin/sh"}),
    );
    let commands = registry.commands("native-tools").unwrap();
    let command = |id: &str| commands.iter().find(|command| command.id == id).unwrap();
    assert_eq!(
        command("open_nvim").command_line.as_ref().unwrap().name,
        "nvim"
    );
    let context = || CommandContext {
        location: directory.to_string_lossy().into_owned(),
        selected: vec![selected.to_string_lossy().into_owned()],
    };
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let argument = "new 'file' $(touch NO).rs";
    let CommandResult::Terminal(request) = registry
        .invoke_command_line_result(
            "native-tools",
            command("open_nvim"),
            context(),
            vec![argument.into()],
            &cancelled,
        )
        .unwrap()
    else {
        panic!("terminal expected")
    };
    assert_eq!(
        request.args,
        vec![
            "--".to_string(),
            directory.join(argument).to_string_lossy().into_owned()
        ]
    );
    assert!(!directory.join("NO").exists());
    let new_uri_path = directory.join("new URI file.rs");
    let new_uri = format!(
        "file://{}",
        new_uri_path.to_string_lossy().replace(' ', "%20")
    );
    let CommandResult::Terminal(request) = registry
        .invoke_command_line_result(
            "native-tools",
            command("open_nvim"),
            context(),
            vec![new_uri],
            &cancelled,
        )
        .unwrap()
    else {
        panic!("terminal expected")
    };
    assert_eq!(
        request.args,
        vec![
            "--".to_string(),
            new_uri_path.to_string_lossy().into_owned()
        ]
    );
    let CommandResult::Terminal(request) = registry
        .invoke_command_line_result(
            "native-tools",
            command("open_terminal"),
            context(),
            vec![child.file_name().unwrap().to_string_lossy().into_owned()],
            &cancelled,
        )
        .unwrap()
    else {
        panic!("terminal expected")
    };
    assert_eq!(request.cwd, child);
    assert_eq!(request.args, ["-l"]);
    let mut remote = context();
    remote.location = "ssh://offline-host/srv".into();
    assert!(
        registry
            .invoke_command_line_result(
                "native-tools",
                command("open_nvim"),
                remote,
                vec![],
                &cancelled
            )
            .is_err()
    );
}
