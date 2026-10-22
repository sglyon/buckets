use buckets_plugin::{AgentMode, AgentRequest, PluginRegistry, WorkspaceContext};
use serde_json::json;
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
fn fixture(timeout: u64) -> (tempfile::TempDir, PluginRegistry) {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("plugin.json"),json!({"id":"agent-fixture","name":"Agent Fixture","protocol_version":1,"executable":"python3","args":["service.py"],"capabilities":["agent"],"timeout_ms":timeout,"config":{}}).to_string()).unwrap();
    fs::write(temp.path().join("service.py"),r#"import json,sys,time,signal
for line in sys.stdin:
 f=json.loads(line)
 if f.get('method')=='agent.cancel': sys.exit(0)
 r=f['params']['request'];p=r.get('prompt','');rid=f['id']
 if p=='permissions':
  import os,stat
  assert stat.S_IMODE(os.stat(r['storage_dir']).st_mode)==0o700
  open(os.path.join(r['storage_dir'],'memory-note.txt'),'w').write('private notes')
 if p=='timeout': time.sleep(10)
 if p=='cancel':
  signal.signal(signal.SIGTERM,signal.SIG_IGN)
  time.sleep(10)
 if p=='oversize':
  print('x'*8388609,flush=True);continue
 if p=='mismatch':
  print(json.dumps({'jsonrpc':'2.0','id':999,'result':{}}),flush=True);continue
 print(json.dumps({'jsonrpc':'2.0','id':'plugin-1','method':'workspace.snapshot','params':{}}),flush=True)
 response=json.loads(sys.stdin.readline())
 assert 'result' in response and response['id']=='plugin-1'
 print(json.dumps({'jsonrpc':'2.0','method':'agent.progress','params':{'request_id':r['request_id'],'message':'Read snapshot'}}),flush=True)
 print(json.dumps({'jsonrpc':'2.0','id':'plugin-2','method':'files.remove','params':{}}),flush=True)
 denied=json.loads(sys.stdin.readline());assert 'error' in denied
 sys.stderr.write('log'*100000);sys.stderr.flush()
 print(json.dumps({'jsonrpc':'2.0','id':rid,'result':{'answer':str(response['result']['revision']),'citations':[],'results':[]}}),flush=True)
"#).unwrap();
    let registry = PluginRegistry::discover(temp.path()).unwrap();
    (temp, registry)
}
fn request(temp: &tempfile::TempDir, id: &str, prompt: &str) -> AgentRequest {
    AgentRequest {
        request_id: id.into(),
        mode: AgentMode::Ask,
        prompt: prompt.into(),
        context: WorkspaceContext {
            workspace_id: "workspace-fixture".into(),
            revision: 7,
            panes: vec![],
        },
        storage_dir: temp.path().join("state"),
        memory_update: None,
    }
}
#[test]
fn duplex_service_stays_alive_and_dispatches_only_read_tools() {
    let (temp, registry) = fixture(3000);
    let mut service = registry.start_agent("agent-fixture").unwrap();
    let cancel = AtomicBool::new(false);
    for id in ["one", "two"] {
        let mut progress = vec![];
        let result = service
            .invoke(
                request(&temp, id, "ask"),
                |method, _| {
                    assert_eq!(method, "workspace.snapshot");
                    Ok(json!({"revision":7}))
                },
                |event| progress.push(event),
                &cancel,
            )
            .unwrap();
        assert_eq!(result.answer, "7");
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].request_id, id);
        assert!(service.is_running());
    }
    service.shutdown();
    assert!(!service.is_running());
}
#[test]
fn second_service_cannot_open_same_workspace_until_first_releases_lock() {
    let (temp, registry) = fixture(3000);
    let mut first = registry.start_agent("agent-fixture").unwrap();
    let cancel = AtomicBool::new(false);
    first
        .invoke(
            request(&temp, "first", "ask"),
            |_, _| Ok(json!({"revision":1})),
            |_| {},
            &cancel,
        )
        .unwrap();
    let mut second = registry.start_agent("agent-fixture").unwrap();
    let error = second
        .invoke(
            request(&temp, "second", "ask"),
            |_, _| Ok(json!({})),
            |_| {},
            &cancel,
        )
        .unwrap_err();
    assert!(error.to_string().contains("already owns"));
    assert!(!second.is_running());
    first.shutdown();
    let mut reopened = registry.start_agent("agent-fixture").unwrap();
    reopened
        .invoke(
            request(&temp, "reopened", "ask"),
            |_, _| Ok(json!({"revision":2})),
            |_| {},
            &cancel,
        )
        .unwrap();
    assert!(temp.path().join("state/.host-service.lock").exists());
}
#[test]
fn malformed_mismatched_and_oversized_frames_poison_and_reap_service() {
    for prompt in ["mismatch", "oversize"] {
        let (temp, registry) = fixture(3000);
        let mut service = registry.start_agent("agent-fixture").unwrap();
        let error = service
            .invoke(
                request(&temp, prompt, prompt),
                |_, _| Ok(json!({})),
                |_| {},
                &AtomicBool::new(false),
            )
            .unwrap_err();
        assert!(!service.is_running());
        assert!(
            error.to_string().contains(if prompt == "mismatch" {
                "does not match"
            } else {
                "exceeds"
            }),
            "{error:#}"
        );
    }
}
#[test]
fn timeout_closes_service_without_marking_a_user_cancellation() {
    let (temp, registry) = fixture(100);
    let mut service = registry.start_agent("agent-fixture").unwrap();
    let error = service
        .invoke(
            request(&temp, "timed-out", "timeout"),
            |_, _| Ok(json!({})),
            |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    assert!(!service.is_running());
    assert!(!temp.path().join("state/host-cancelled-requests").exists());
}
#[test]
fn explicit_cancel_is_durably_marked_before_uncooperative_process_is_killed() {
    let (temp, registry) = fixture(5000);
    let mut service = registry.start_agent("agent-fixture").unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let trigger = cancel.clone();
    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        trigger.store(true, Ordering::Release);
    });
    let error = service
        .invoke(
            request(&temp, "cancel-me", "cancel"),
            |_, _| Ok(json!({})),
            |_| {},
            &cancel,
        )
        .unwrap_err();
    handle.join().unwrap();
    assert!(error.to_string().contains("cancelled"));
    assert!(!service.is_running());
    let markers = fs::read_dir(temp.path().join("state/host-cancelled-requests"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(markers.len(), 1);
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(&markers[0]).unwrap()).unwrap();
    assert_eq!(marker["request_id"], "cancel-me");
}
#[test]
fn cancellation_marker_is_written_while_host_callback_is_still_running() {
    let (temp, registry) = fixture(5000);
    let mut service = registry.start_agent("agent-fixture").unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let trigger = cancel.clone();
    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        trigger.store(true, Ordering::Release);
    });
    let folder = temp.path().join("state/host-cancelled-requests");
    let error = service
        .invoke(
            request(&temp, "cancel-during-read", "ask"),
            |_, _| {
                let deadline = std::time::Instant::now() + Duration::from_secs(1);
                while !folder.exists() && std::time::Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                assert!(
                    folder.exists(),
                    "Cancellation marker must not wait for provider callback return"
                );
                Ok(json!({"revision":7}))
            },
            |_| {},
            &cancel,
        )
        .unwrap_err();
    handle.join().unwrap();
    assert!(error.to_string().contains("cancelled"));
    assert!(!service.is_running());
}

#[test]
#[cfg(unix)]
fn storage_is_private_under_normal_umask() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    // Isolate the process-wide umask from the parallel parent test runner.
    if std::env::var_os("BUCKETS_PRIVATE_DIR_CHILD").is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage_is_private_under_normal_umask",
                "--nocapture",
            ])
            .env("BUCKETS_PRIVATE_DIR_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    unsafe {
        libc::umask(0o022);
    }
    let (temp, registry) = fixture(3000);
    let state = temp.path().join("state");
    fs::create_dir(&state).unwrap();
    assert_eq!(
        fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let mut service = registry.start_agent("agent-fixture").unwrap();
    service
        .invoke(
            request(&temp, "permissions", "permissions"),
            |_, _| Ok(json!({"revision":7})),
            |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(
        fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(fs::metadata(&state).unwrap().uid(), unsafe {
        libc::geteuid()
    });
    assert_eq!(
        fs::read_to_string(state.join("memory-note.txt")).unwrap(),
        "private notes"
    );
    let lock = fs::metadata(state.join(".host-service.lock")).unwrap();
    service.shutdown();
    let mut reopened = registry.start_agent("agent-fixture").unwrap();
    reopened
        .invoke(
            request(&temp, "reopen", "permissions"),
            |_, _| Ok(json!({"revision":7})),
            |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(
        fs::metadata(state.join(".host-service.lock"))
            .unwrap()
            .ino(),
        lock.ino()
    );
}
#[test]
#[cfg(unix)]
fn storage_rejects_symlink_non_directory_and_foreign_owner_leaves() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let (temp, registry) = fixture(3000);
    let target = temp.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    let link = temp.path().join("linked");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let file = temp.path().join("regular-file");
    fs::write(&file, "preserve").unwrap();
    for path in [&link, &file] {
        let mut service = registry.start_agent("agent-fixture").unwrap();
        let mut next = request(&temp, "unsafe-leaf", "ask");
        next.storage_dir = path.clone();
        assert!(
            service
                .invoke(
                    next,
                    |_, _| Ok(json!({"revision":7})),
                    |_| {},
                    &AtomicBool::new(false)
                )
                .is_err()
        );
    }
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(!target.join(".host-service.lock").exists());
    assert_eq!(fs::read_to_string(file).unwrap(), "preserve");
    if fs::metadata("/").unwrap().uid() != unsafe { libc::geteuid() } {
        let mut service = registry.start_agent("agent-fixture").unwrap();
        let mut next = request(&temp, "foreign-owner", "ask");
        next.storage_dir = "/".into();
        let error = service
            .invoke(
                next,
                |_, _| Ok(json!({"revision":7})),
                |_| {},
                &AtomicBool::new(false),
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("owned by the current user"),
            "{error:#}"
        );
    }
}
