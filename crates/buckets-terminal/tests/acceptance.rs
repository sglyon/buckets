//! Real PTY acceptance tests. All files, HOME and tool state stay in tempdirs.
#![cfg(unix)]
use alacritty_terminal::term::TermMode;
use buckets_plugin::{CommandResult, PluginRegistry};
use buckets_terminal::{TerminalSession, TerminalSpec, encode_key};
use gpui::{Keystroke, Modifiers};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn spec(root: &Path, program: &str, args: Vec<String>) -> TerminalSpec {
    let root = root.canonicalize().unwrap();
    let mut env = BTreeMap::new();
    for (key, path) in [
        ("HOME", root.clone()),
        ("XDG_CONFIG_HOME", root.join("config")),
        ("XDG_DATA_HOME", root.join("data")),
        ("XDG_STATE_HOME", root.join("state")),
        ("XDG_CACHE_HOME", root.join("cache")),
    ] {
        fs::create_dir_all(&path).unwrap();
        env.insert(key.into(), path.to_string_lossy().into_owned());
    }
    env.insert("GIT_CONFIG_GLOBAL".into(), "/dev/null".into());
    env.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
    env.insert("LC_ALL".into(), "en_US.UTF-8".into());
    TerminalSpec {
        program: program.into(),
        args,
        cwd: root,
        title: "Acceptance PTY".into(),
        env,
    }
}
fn shell(root: &Path, script: &str) -> TerminalSession {
    TerminalSession::spawn(spec(root, "/bin/sh", vec!["-c".into(), script.into()]))
        .expect("real PTY must open; this test must not silently skip PTY failures")
}
fn wait_for(
    session: &TerminalSession,
    description: &str,
    timeout: Duration,
    mut predicate: impl FnMut() -> bool,
) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(15));
    }
    panic!(
        "Timed out waiting for {description}; exited={} status={:?}\n{}",
        session.exited(),
        session.exit_status(),
        session.screen_text()
    );
}
fn screen(session: &TerminalSession, marker: &str) {
    wait_for(session, marker, Duration::from_secs(5), || {
        session.screen_text().contains(marker)
    });
}
fn program(name: &str) -> String {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|root| root.join(name))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("required test executable {name} was not found"))
        .to_string_lossy()
        .into_owned()
}
fn alive(pid: u32) -> bool {
    if unsafe { libc::kill(pid as i32, 0) } != 0 {
        return false;
    }
    // A killed orphan awaiting init's reap cannot perform work and is no leak.
    let state = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .unwrap();
    !String::from_utf8_lossy(&state.stdout)
        .trim()
        .starts_with('Z')
}
struct PidGuard(u32);
impl Drop for PidGuard {
    fn drop(&mut self) {
        if self.0 > 1 {
            unsafe {
                libc::kill(self.0 as i32, libc::SIGKILL);
            }
        }
    }
}
fn child_pid(session: &TerminalSession, marker: &str) -> u32 {
    let mut result = None;
    wait_for(session, "background PID", Duration::from_secs(5), || {
        for (_, suffix) in session
            .screen_text()
            .match_indices(marker)
            .map(|(i, _)| (i, session.screen_text()[i + marker.len()..].to_owned()))
        {
            let digits = suffix
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>();
            if let Ok(pid) = digits.parse::<u32>() {
                result = Some(pid);
                return true;
            }
        }
        false
    });
    result.unwrap()
}

#[test]
fn real_pty_resize_reaches_process_and_exit_is_observable() {
    let root = tempfile::tempdir().unwrap();
    let session = shell(
        root.path(),
        "printf '__READY__\\n'; read answer; stty size; printf '__RESIZED__\\n'; exit 7",
    );
    assert!(session.process_id().is_some_and(|pid| pid > 1));
    screen(&session, "__READY__");
    session.resize(103, 37).unwrap();
    session.write(b"go\r").unwrap();
    screen(&session, "37 103");
    screen(&session, "__RESIZED__");
    wait_for(&session, "exit status", Duration::from_secs(3), || {
        session.exited()
    });
    assert!(
        session
            .exit_status()
            .is_some_and(|status| status.contains('7'))
    );
    assert!(session.write(b"must reject dead session").is_err());
    session.close();
    session.close();
}

#[test]
fn ansi_parser_keeps_wide_unicode_and_ignores_escape_bytes_as_text() {
    let root = tempfile::tempdir().unwrap();
    let session = shell(
        root.path(),
        "printf '\\033[2J\\033[H\\033[38;2;12;34;56m界🙂é\\033[0m\\r\\n__ANSI_DONE__\\n'; sleep 1",
    );
    screen(&session, "__ANSI_DONE__");
    let text = session.screen_text();
    for character in ['界', '🙂', 'é'] {
        assert!(text.contains(character), "missing {character:?}: {text}");
    }
    assert!(!text.contains('\x1b'));
}

#[test]
fn bracketed_paste_preserves_unicode_and_cannot_inject_an_escape_terminator() {
    let root = tempfile::tempdir().unwrap();
    let capture = root.path().join("paste.bin");
    let python = program("python3");
    let code = "import os,sys,tty\ntty.setraw(0)\nos.write(1,b'\\x1b[?2004h__PASTE_READY__')\ndata=b''\nwhile not data.endswith(b'\\x1b[201~'):\n data+=os.read(0,4096)\nopen(sys.argv[1],'wb').write(data)\nos.write(1,b'__PASTE_DONE__')\n";
    let session = TerminalSession::spawn(spec(
        root.path(),
        &python,
        vec![
            "-c".into(),
            code.into(),
            capture.to_string_lossy().into_owned(),
        ],
    ))
    .unwrap();
    screen(&session, "__PASTE_READY__");
    session.paste("hello\né\x1b[201~literal").unwrap();
    screen(&session, "__PASTE_DONE__");
    assert_eq!(
        fs::read(capture).unwrap(),
        "\x1b[200~hello\né[201~literal\x1b[201~".as_bytes()
    );
}

#[test]
fn close_and_drop_terminate_foreground_processes() {
    for explicit in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let session = shell(root.path(), "printf '__ALIVE__\\n'; sleep 120");
        screen(&session, "__ALIVE__");
        let pid = session.process_id().unwrap();
        let _cleanup = PidGuard(pid);
        if explicit {
            session.close();
        }
        drop(session);
        let deadline = Instant::now() + Duration::from_secs(3);
        while alive(pid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !alive(pid),
            "PTY child survived {}",
            if explicit { "close" } else { "drop" }
        );
    }
}

#[test]
fn closing_a_terminal_preserves_unrelated_processes() {
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut unrelated = ChildGuard(Command::new("/bin/sleep").arg("30").spawn().unwrap());
    let root = tempfile::tempdir().unwrap();
    let session = shell(root.path(), "printf '__OWNED_PTY__\\n'; sleep 120");
    screen(&session, "__OWNED_PTY__");
    session.close();
    wait_for(&session, "owned PTY exit", Duration::from_secs(3), || {
        session.exited()
    });
    assert!(
        unrelated.0.try_wait().unwrap().is_none(),
        "PTY cleanup killed an unrelated process"
    );
}

#[test]
fn close_terminates_background_jobs_in_separate_process_groups() {
    let root = tempfile::tempdir().unwrap();
    let script =
        "set -m; sleep 120 & background=$!; printf '__BG_PID=%s__\\n' \"$background\"; wait";
    let session = TerminalSession::spawn(spec(
        root.path(),
        "/bin/bash",
        vec![
            "--noprofile".into(),
            "--norc".into(),
            "-c".into(),
            script.into(),
        ],
    ))
    .unwrap();
    let pid = child_pid(&session, "__BG_PID=");
    let _cleanup = PidGuard(pid);
    assert!(alive(pid));
    session.close();
    let deadline = Instant::now() + Duration::from_secs(3);
    while alive(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(pid), "background job {pid} survived terminal close");
}

#[test]
fn close_cleans_background_jobs_after_the_session_leader_exits() {
    let root = tempfile::tempdir().unwrap();
    let script = "set -m; sleep 120 </dev/null >/dev/null 2>&1 & background=$!; printf '__ORPHAN_PID=%s__\\n' \"$background\"; exit 0";
    let session = TerminalSession::spawn(spec(
        root.path(),
        "/bin/bash",
        vec![
            "--noprofile".into(),
            "--norc".into(),
            "-c".into(),
            script.into(),
        ],
    ))
    .unwrap();
    let pid = child_pid(&session, "__ORPHAN_PID=");
    let _cleanup = PidGuard(pid);
    wait_for(&session, "leader exit", Duration::from_secs(3), || {
        session.exited()
    });
    session.close();
    let deadline = Instant::now() + Duration::from_secs(3);
    while alive(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !alive(pid),
        "orphaned PTY job {pid} survived terminal close"
    );
}

#[test]
fn terminal_key_encoding_keeps_controls_cursor_modes_and_option_meta_distinct() {
    let key = |name: &str, mods: Modifiers, character: Option<&str>| Keystroke {
        key: name.into(),
        modifiers: mods,
        key_char: character.map(str::to_owned),
    };
    let plain = Modifiers::default();
    let ctrl = Modifiers {
        control: true,
        ..plain
    };
    let shift = Modifiers {
        shift: true,
        ..plain
    };
    let alt = Modifiers { alt: true, ..plain };
    for (stroke, wanted) in [
        (key("c", ctrl, None), b"\x03".to_vec()),
        (key("space", ctrl, None), vec![0]),
        (key("escape", plain, None), b"\x1b".to_vec()),
        (key("tab", plain, None), b"\t".to_vec()),
        (key("tab", shift, None), b"\x1b[Z".to_vec()),
        (key("s", alt, Some("ß")), b"\x1bs".to_vec()),
        (key("界", plain, Some("界")), "界".as_bytes().to_vec()),
    ] {
        assert_eq!(
            encode_key(&stroke, TermMode::empty()).unwrap(),
            wanted,
            "{stroke:?}"
        );
    }
    assert_eq!(
        encode_key(&key("up", plain, None), TermMode::APP_CURSOR).unwrap(),
        b"\x1bOA"
    );
    assert_eq!(
        encode_key(&key("up", ctrl, None), TermMode::APP_CURSOR).unwrap(),
        b"\x1b[1;5A"
    );
    assert!(
        encode_key(
            &key(
                "c",
                Modifiers {
                    platform: true,
                    ..plain
                },
                None
            ),
            TermMode::empty()
        )
        .is_none()
    );
}

#[test]
fn typed_plugin_request_reaches_a_real_pty_with_literal_argv_cwd_and_environment() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let cwd = base.join("space 'quote' $(touch INJECTED)");
    fs::create_dir(&cwd).unwrap();
    let capture = base.join("capture.json");
    let probe = base.join("argv probe.py");
    fs::write(&probe,"import os,sys,json\njson.dump(dict(argv=sys.argv[2:],cwd=os.getcwd(),environment=os.environ.get('BUCKETS_LITERAL')),open(sys.argv[1],'w'))\nprint('__ARGV_CAPTURED__',flush=True)\n").unwrap();
    let arguments = vec![
        probe.to_string_lossy().into_owned(),
        capture.to_string_lossy().into_owned(),
        "literal 'quotes' $(touch INJECTED) `whoami`".into(),
        "café 界🙂 with spaces".into(),
    ];
    let request = json!({"program":program("python3"),"args":arguments,"cwd":cwd,"title":"Literal PTY","env":{"BUCKETS_LITERAL":"value 'quoted' $() 界"}});
    let plugin = base.join("plugin");
    fs::create_dir(&plugin).unwrap();
    fs::write(plugin.join("plugin.json"),serde_json::to_vec(&json!({"id":"terminal-fixture","name":"Terminal fixture","protocol_version":1,"executable":"python3","args":["plugin.py"],"capabilities":["commands","terminal"],"permissions":{"terminal":true,"read_roots":[base]}})).unwrap()).unwrap();
    fs::write(plugin.join("plugin.py"),format!("import sys,json\nq=json.load(sys.stdin)\npayload=dict(host_action='terminal',request=json.loads({}))\nprint(json.dumps(dict(jsonrpc='2.0',id=q['id'],result=payload)))\n",serde_json::to_string(&request.to_string()).unwrap())).unwrap();
    let result = PluginRegistry::discover(&plugin)
        .unwrap()
        .invoke_command_result("terminal-fixture", "open", json!({}))
        .unwrap();
    let CommandResult::Terminal(request) = result else {
        panic!("host did not produce typed terminal request")
    };
    let mut options = spec(&base, &request.program, request.args);
    options.cwd = request.cwd;
    options.title = request.title;
    options.env.extend(request.env);
    let session = TerminalSession::spawn(options).unwrap();
    screen(&session, "__ARGV_CAPTURED__");
    let result: serde_json::Value = serde_json::from_slice(&fs::read(capture).unwrap()).unwrap();
    assert_eq!(result["argv"], json!(&arguments[2..]));
    assert_eq!(result["cwd"], json!(cwd));
    assert_eq!(result["environment"], "value 'quoted' $() 界");
    assert!(!cwd.join("INJECTED").exists());
}

#[test]
#[ignore = "requires a real Neovim installation; run explicitly on developer macOS"]
fn real_neovim_edits_and_saves_unicode_in_a_filename_with_spaces() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("café 界 file with spaces.txt");
    let nvim = Path::new("/opt/homebrew/bin/nvim");
    assert!(
        nvim.is_file(),
        "install nvim before this explicitly requested real-tool test"
    );
    let session = TerminalSession::spawn(spec(
        root.path(),
        nvim.to_str().unwrap(),
        vec![
            "--clean".into(),
            "-u".into(),
            "NONE".into(),
            "-i".into(),
            "NONE".into(),
            "-n".into(),
            "--".into(),
            file.to_string_lossy().into_owned(),
        ],
    ))
    .unwrap();
    wait_for(&session, "Neovim startup", Duration::from_secs(5), || {
        session.screen_text().contains("file with spaces.txt")
    });
    let content = "first α line\nsecond 界🙂 café\nthird = literal 'quotes' $(no shell)\n";
    session.write(b"i").unwrap();
    screen(&session, "INSERT");
    // Vim writes the buffer's final EOL itself. Pasting a final newline while
    // inserting into an empty buffer creates an additional empty buffer line.
    session.paste(content.trim_end_matches('\n')).unwrap();
    screen(&session, "first");
    session.write(b"\x1b:wq\r").unwrap();
    wait_for(
        &session,
        "Neovim save and quit",
        Duration::from_secs(8),
        || session.exited(),
    );
    assert_eq!(fs::read_to_string(file).unwrap(), content);
}

#[test]
#[ignore = "requires a real Lazygit installation; run explicitly on developer macOS"]
fn real_lazygit_renders_its_tui_and_restores_the_main_screen_on_quit() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&base)
            .status()
            .unwrap()
            .success()
    );
    fs::write(base.join("untracked.txt"), "fixture").unwrap();
    let config = base.join("lazygit-config");
    fs::create_dir(&config).unwrap();
    fs::write(
        config.join("config.yml"),
        "gui:\n  showWelcomeScreen: false\nupdate:\n  method: never\nconfirmOnQuit: false\n",
    )
    .unwrap();
    let lazygit = Path::new("/opt/homebrew/bin/lazygit");
    assert!(
        lazygit.is_file(),
        "install lazygit before this explicitly requested real-tool test"
    );
    let wrapper = "printf '__MAIN_SCREEN__\\n'; exec \"$1\" --use-config-dir \"$2\"";
    let session = TerminalSession::spawn(spec(
        &base,
        "/bin/sh",
        vec![
            "-c".into(),
            wrapper.into(),
            "fixture".into(),
            lazygit.to_string_lossy().into_owned(),
            config.to_string_lossy().into_owned(),
        ],
    ))
    .unwrap();
    session.resize(120, 35).unwrap();
    wait_for(&session, "Lazygit screen", Duration::from_secs(8), || {
        let screen = session.screen_text();
        screen.to_lowercase().contains("files") && screen.to_lowercase().contains("branches")
    });
    if session.screen_text().contains("get started") {
        session.write(b"\r").unwrap();
        wait_for(
            &session,
            "Lazygit welcome dismissal",
            Duration::from_secs(5),
            || !session.screen_text().contains("get started"),
        );
    }
    assert!(
        !session.screen_text().contains("__MAIN_SCREEN__"),
        "Lazygit did not enter its alternate screen"
    );
    session.write(b"q").unwrap();
    wait_for(&session, "Lazygit quit", Duration::from_secs(5), || {
        session.exited()
    });
    screen(&session, "__MAIN_SCREEN__");
}
