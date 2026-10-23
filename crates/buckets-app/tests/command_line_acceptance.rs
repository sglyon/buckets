#[path = "../src/command_line.rs"]
mod command_line;

use buckets_core::Location;
use command_line::{
    CommandSpec, HistoryStore, Invocation, builtins, completion, parse, resolve_command,
    resolve_location, shell_plan,
};
use serde_json::json;
use std::{fs, path::PathBuf, process::Command};

fn app(input: &str) -> (String, Vec<String>) {
    match parse(input, &Location::local("/active pane")).unwrap() {
        Invocation::App { command, args } => (command, args),
        other => panic!("not an app command: {other:?}"),
    }
}

fn plugin(name: &str, aliases: &[&str]) -> CommandSpec {
    CommandSpec {
        name: name.into(),
        aliases: aliases.iter().map(|s| (*s).into()).collect(),
        usage: format!("{name} [argument]"),
        description: "Offline test contribution".into(),
    }
}

#[test]
fn argument_lexer_preserves_spaces_unicode_empty_values_and_literal_shell_syntax() {
    assert_eq!(
        app(":rename 'café résumé.txt'"),
        ("rename".into(), vec!["café résumé.txt".into()])
    );
    assert_eq!(
        app(r#":cd "folder with space"/nested"#),
        ("cd".into(), vec!["folder with space/nested".into()])
    );
    assert_eq!(
        app(r#":plugin:fixture/echo '' "" escaped\ space '$(touch nope)' "$HOME" "a\b""#).1,
        ["", "", "escaped space", "$(touch nope)", "$HOME", r"a\b"]
    );
    assert_eq!(
        app("  split ../other"),
        ("split".into(), vec!["../other".into()])
    );
    for (input, message) in [
        (":cd 'unfinished", "unmatched quote"),
        (":rename \"unfinished", "unmatched quote"),
        (":cd trailing\\", "incomplete escape"),
        (":", "Enter an application command"),
        (":''", "must not be empty"),
    ] {
        assert!(
            parse(input, &Location::local("/"))
                .unwrap_err()
                .to_string()
                .contains(message),
            "{input}"
        );
    }
    for input in [
        ":cd a\nb",
        ":rename x\ry",
        ":settings\0",
        ":! printf x\nprintf y",
    ] {
        assert!(parse(input, &Location::local("/")).is_err());
    }
}

#[test]
fn path_resolution_keeps_active_provider_identity_and_never_uses_process_cwd() {
    let local = Location::local("/active pane/project");
    assert_eq!(
        resolve_location(&local, "../café folder/./nested").unwrap(),
        Location::local("/active pane/café folder/nested")
    );
    assert_eq!(
        resolve_location(&Location::local("/"), "../../../").unwrap(),
        Location::local("/")
    );
    let ssh = Location::Ssh {
        host: "user@offline".into(),
        path: "/srv/project".into(),
    };
    assert_eq!(
        resolve_location(&ssh, "/etc/config").unwrap(),
        Location::Ssh {
            host: "user@offline".into(),
            path: "/etc/config".into()
        }
    );
    assert_eq!(
        resolve_location(&ssh, "../../../../").unwrap(),
        Location::Ssh {
            host: "user@offline".into(),
            path: "/".into()
        }
    );
    assert_eq!(
        resolve_location(&ssh, "file:///tmp/local%20directory").unwrap(),
        Location::local("/tmp/local directory")
    );
    assert!(resolve_location(&ssh, "~/local-home").is_err());
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        assert_eq!(
            resolve_location(&local, "~").unwrap(),
            Location::local(&home)
        );
        assert_eq!(
            resolve_location(&local, "~/src").unwrap(),
            Location::local(home.join("src"))
        );
    }
    let s3 = Location::S3 {
        bucket: "research".into(),
        prefix: "exports/run".into(),
        endpoint: Some("https://offline.invalid:9000/storage".into()),
    };
    assert_eq!(
        resolve_location(&s3, "../next folder").unwrap(),
        Location::S3 {
            bucket: "research".into(),
            prefix: "exports/next folder".into(),
            endpoint: Some("https://offline.invalid:9000/storage".into())
        }
    );
    let git = Location::Git {
        repo: "/repos/work".into(),
        reference: "refs/heads/feature".into(),
        path: "src/core".into(),
    };
    assert_eq!(
        resolve_location(&git, "../../../../docs").unwrap(),
        Location::Git {
            repo: "/repos/work".into(),
            reference: "refs/heads/feature".into(),
            path: "docs".into()
        }
    );
    let provider = Location::Plugin {
        plugin: "offline".into(),
        path: "/docs/current".into(),
    };
    assert_eq!(
        resolve_location(&provider, "../next").unwrap(),
        Location::Plugin {
            plugin: "offline".into(),
            path: "/docs/next".into()
        }
    );
    for bad in ["bad\npath", "bad\0path", ""] {
        assert!(resolve_location(&local, bad).is_err());
    }
    assert!(resolve_location(&Location::local("relative"), "child").is_err());
}

#[test]
fn registry_preserves_core_names_qualified_plugins_and_explicit_alias_collisions() {
    let mut specs = builtins();
    specs.extend([
        plugin("plugin:first/open", &["nvim", "rename"]),
        plugin("plugin:second/open", &["nvim"]),
        plugin("plugin:native/terminal", &["terminal"]),
    ]);
    assert_eq!(resolve_command("rename", &specs).unwrap().name, "rename");
    assert_eq!(
        resolve_command("terminal", &specs).unwrap().name,
        "plugin:native/terminal"
    );
    assert!(
        resolve_command("nvim", &specs)
            .unwrap_err()
            .to_string()
            .contains("Ambiguous")
    );
    assert_eq!(
        resolve_command("plugin:first/open", &specs).unwrap().name,
        "plugin:first/open"
    );
    assert!(resolve_command("unknown", &specs).is_err());
    assert_eq!(
        completion(":nv", &specs)
            .iter()
            .map(|c| c.label.as_str())
            .collect::<Vec<_>>(),
        ["plugin:first/open", "plugin:second/open"]
    );
    specs.push(plugin("rename", &[]));
    assert!(
        resolve_command("rename", &specs)
            .unwrap_err()
            .to_string()
            .contains("Duplicate")
    );
}

#[test]
fn pure_completion_discovers_unique_plugin_aliases_and_bounds_results_without_execution() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("must not exist");
    let mut specs = builtins();
    specs.push(plugin("plugin:native/nvim", &["nvim"]));
    assert_eq!(completion(":nv", &specs)[0].value, ":nvim ");
    assert_eq!(completion("set", &specs)[0].value, "settings ");
    assert!(completion(":cd \"folder", &specs).is_empty());
    assert!(completion(&format!(":! touch '{}'", marker.display()), &specs).is_empty());
    assert!(!marker.exists());
    specs.extend((0..30).map(|i| plugin(&format!("plugin:fixture/c{i}"), &[])));
    assert_eq!(completion(":", &specs).len(), 12);
    assert!(
        completion(":", &specs)[0]
            .detail
            .contains("split [directory]")
    );
}

#[test]
fn local_shell_runs_exact_quoted_pipeline_in_its_snapshotted_directory() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("folder with space café");
    fs::create_dir(&directory).unwrap();
    let source = r#"  printf '%s\n' 'hello world' | tr a-z A-Z; pwd  "#;
    let invocation = parse(&format!(":!{source}"), &Location::local(&directory)).unwrap();
    let Invocation::Shell {
        source: parsed,
        plan,
    } = invocation
    else {
        panic!("expected shell")
    };
    assert_eq!(parsed, source);
    assert_eq!(plan.args, ["-c", source]);
    assert_eq!(plan.cwd, directory);
    let output = Command::new(&plan.program)
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        output,
        format!(
            "HELLO WORLD\n{}\n",
            directory.canonicalize().unwrap().display()
        )
    );
    let failure = shell_plan(&Location::local(&directory), "printf error >&2; exit 7").unwrap();
    let output = Command::new(&failure.program)
        .args(&failure.args)
        .current_dir(&failure.cwd)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stderr, b"error");
}

#[test]
fn ssh_shell_argv_and_remote_cd_quoting_reject_flags_and_preserve_exact_source_offline() {
    let root = tempfile::tempdir().unwrap();
    let directory = root
        .path()
        .join("café '; touch SHOULD_NOT_EXIST; echo $(touch SHOULD_NOT_EXIST_2) '");
    fs::create_dir(&directory).unwrap();
    let source = "pwd; printf '%s' 'quoted value' # comment";
    let active = Location::Ssh {
        host: "user@offline-host".into(),
        path: directory.to_string_lossy().into(),
    };
    let plan = shell_plan(&active, source).unwrap();
    assert_eq!(&plan.args[..3], ["-t", "--", "user@offline-host"]);
    assert!(plan.args[3].contains(source));
    // Evaluate only the remote command string locally to prove quoting. No SSH
    // process or remote provider is invoked by this acceptance test.
    let output = Command::new("/bin/sh")
        .args(["-c", &plan.args[3]])
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\nquoted value", directory.display())
    );
    assert!(!root.path().join("SHOULD_NOT_EXIST").exists());
    assert!(!root.path().join("SHOULD_NOT_EXIST_2").exists());
    for host in [
        "-oProxyCommand=evil",
        "host\nother",
        "bad host",
        "host\0",
        "",
    ] {
        assert!(
            shell_plan(
                &Location::Ssh {
                    host: host.into(),
                    path: "/tmp".into()
                },
                "pwd"
            )
            .is_err()
        );
    }
    for path in ["/tmp/a\nb", "/tmp/a\0b", "relative"] {
        assert!(
            shell_plan(
                &Location::Ssh {
                    host: "offline".into(),
                    path: path.into()
                },
                "pwd"
            )
            .is_err()
        );
    }
    for location in [
        Location::S3 {
            bucket: "offline".into(),
            prefix: "".into(),
            endpoint: None,
        },
        Location::Git {
            repo: PathBuf::from("/tmp"),
            reference: "HEAD".into(),
            path: "".into(),
        },
        Location::Plugin {
            plugin: "offline".into(),
            path: "/".into(),
        },
    ] {
        assert!(shell_plan(&location, "pwd").is_err());
    }
}

#[test]
fn explicit_history_roundtrips_exact_submissions_and_excludes_empty_drafts() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("command-history.json");
    let mut history = HistoryStore::load(&path).unwrap();
    for empty in ["", "  ", ":", ":!  "] {
        history.record(empty).unwrap();
    }
    assert!(!path.exists());
    let submitted = [
        ":cd 'café folder'",
        r#":! printf '%s\n' 'exact text' | cat  "#,
    ];
    for line in submitted {
        history.record(line).unwrap();
    }
    // Draft edits and Escape are UI-owned; no record call is made for them.
    assert_eq!(HistoryStore::load(&path).unwrap().entries(), submitted);
    history.record(submitted[1]).unwrap();
    assert_eq!(history.entries().len(), 2);
    for i in 0..120 {
        history.record(&format!(":rename file-{i}")).unwrap();
    }
    let reopened = HistoryStore::load(&path).unwrap();
    assert_eq!(reopened.entries().len(), 100);
    assert_eq!(reopened.entries()[0], ":rename file-20");
    assert_eq!(reopened.entries().last().unwrap(), ":rename file-119");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn history_refuses_external_changes_invalid_schema_oversized_and_symlink_sources() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history.json");
    let mut history = HistoryStore::load(&path).unwrap();
    history.record(":settings").unwrap();
    let original = fs::read(&path).unwrap();
    for line in [":!\nmalformed", ":cd\0path", &"x".repeat(4097)] {
        assert!(history.record(line).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    let external = br#"{"version":1,"entries":[":cd /external"]}"#;
    fs::write(&path, external).unwrap();
    assert!(history.record(":rename ours").is_err());
    assert_eq!(fs::read(&path).unwrap(), external);
    assert_eq!(history.entries(), [":settings"]);
    for value in [
        json!({"version":2,"entries":[]}),
        json!({"version":1,"entries":["bad\nline"]}),
        json!({"version":1,"entries":[],"environment":{"secret":"must-not-load"}}),
    ] {
        fs::write(&path, value.to_string()).unwrap();
        assert!(HistoryStore::load(&path).is_err());
    }
    fs::write(&path, vec![b' '; 262_145]).unwrap();
    assert!(HistoryStore::load(&path).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        fs::remove_file(&path).unwrap();
        let target = root.path().join("target.json");
        fs::write(&target, external).unwrap();
        symlink(&target, &path).unwrap();
        assert!(HistoryStore::load(&path).is_err());
        assert_eq!(fs::read(&target).unwrap(), external);
    }
}

#[test]
fn argument_completion_uses_lexer_spans_for_unfinished_quotes_unicode_and_variadic_positions() {
    use command_line::{argument_completion, quote_argument};
    for input in [":cd", ":! echo hi", "", ":cd bad\npath"] {
        assert!(argument_completion(input).is_none());
    }
    let first = argument_completion(":cd ").unwrap();
    assert_eq!(
        (
            first.command.as_str(),
            first.index,
            first.prefix.as_str(),
            first.replace_from
        ),
        ("cd", 0, "", 4)
    );
    let input = ":nvim 'café résumé.txt' \"folder with sp";
    let token = argument_completion(input).unwrap();
    assert_eq!(token.command, "nvim");
    assert_eq!(token.index, 1);
    assert_eq!(token.prefix, "folder with sp");
    assert_eq!(&input[token.replace_from..], "\"folder with sp");
    let candidate = "folder with space/child's café.rs";
    let completed = format!(
        "{}{}",
        &input[..token.replace_from],
        quote_argument(candidate)
    );
    assert_eq!(app(&completed).1, ["café résumé.txt", candidate]);
    assert!(parse(input, &Location::local("/")).is_err());
    let next = argument_completion(":nvim one two ").unwrap();
    assert_eq!(next.index, 2);
    for value in ["", "space value", "café", "a'b", r"a\b", "$(touch nope)"] {
        assert_eq!(
            app(&format!(":rename {}", quote_argument(value))).1,
            [value]
        );
    }
}

#[cfg(unix)]
#[test]
fn history_special_files_are_rejected_before_opening_without_blocking_startup() {
    use std::{sync::mpsc, time::Duration};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history.json");
    assert!(
        Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    let fifo = path.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(HistoryStore::load(&fifo).is_err());
    });
    assert!(
        rx.recv_timeout(Duration::from_secs(1))
            .expect("history startup blocked opening a FIFO")
    );
    fs::remove_file(&path).unwrap();
    let mut store = HistoryStore::load(&path).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(path.with_extension("lock"))
            .status()
            .unwrap()
            .success()
    );
    assert!(store.record(":settings").is_err());
    assert!(!path.exists());
}
