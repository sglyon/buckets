use buckets_plugin::{
    AccountAction, AccountRequest, AgentMode, AgentRequest, CredentialBroker, PluginRegistry,
    WorkspaceContext,
};
use serde_json::{Value, json};
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

fn acquire(broker: &mut CredentialBroker) -> Value {
    broker
        .dispatch("credentials.acquire", json!({"key":"chatgpt"}))
        .unwrap()
}
#[test]
fn credential_lease_serializes_refresh_and_atomic_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("accounts");
    let mut first = CredentialBroker::new(&root, "ai").unwrap();
    let mut second = CredentialBroker::new(&root, "ai").unwrap();
    assert_eq!(
        first
            .dispatch("credentials.read", json!({"key":"chatgpt"}))
            .unwrap(),
        json!({})
    );
    let lease = acquire(&mut first);
    assert!(
        second
            .dispatch("credentials.acquire", json!({"key":"chatgpt"}))
            .unwrap_err()
            .to_string()
            .contains("busy")
    );
    first
        .dispatch(
            "credentials.commit",
            json!({"lease":lease["lease"],"value":{"access":"fixture","revision":1}}),
        )
        .unwrap();
    let lease = acquire(&mut second);
    assert_eq!(lease["value"]["revision"], 1);
    second
        .dispatch("credentials.commit", json!({"lease":lease["lease"]}))
        .unwrap();
    assert_eq!(
        first
            .dispatch("credentials.read", json!({"key":"chatgpt"}))
            .unwrap()["value"]["revision"],
        1
    );
    let lease = acquire(&mut first);
    first
        .dispatch(
            "credentials.commit",
            json!({"lease":lease["lease"],"value":null}),
        )
        .unwrap();
    assert_eq!(
        second
            .dispatch("credentials.read", json!({"key":"chatgpt"}))
            .unwrap(),
        json!({})
    );
}
#[test]
fn dropped_broker_releases_lease_and_namespaces_are_isolated() {
    let temp = tempfile::tempdir().unwrap();
    let mut first = CredentialBroker::new(temp.path(), "first").unwrap();
    let mut other = CredentialBroker::new(temp.path(), "other").unwrap();
    let lease = acquire(&mut first);
    let other_lease = acquire(&mut other);
    other
        .dispatch(
            "credentials.commit",
            json!({"lease":other_lease["lease"],"value":{"refresh":"other-fixture"}}),
        )
        .unwrap();
    assert_eq!(
        first
            .dispatch("credentials.read", json!({"key":"chatgpt"}))
            .unwrap(),
        json!({})
    );
    assert!(
        other
            .dispatch(
                "credentials.commit",
                json!({"lease":lease["lease"],"value":{}})
            )
            .is_err()
    );
    drop(first);
    let mut reopened = CredentialBroker::new(temp.path(), "first").unwrap();
    acquire(&mut reopened);
}
#[test]
fn vault_rejects_path_keys_bad_leases_and_oversized_records_without_echoing_data() {
    let temp = tempfile::tempdir().unwrap();
    assert!(CredentialBroker::new(temp.path(), "../ai").is_err());
    let mut broker = CredentialBroker::new(temp.path(), "ai").unwrap();
    for key in ["../other", "/tmp/record", "", "a/b", "a.json", "a\0b"] {
        assert!(
            broker
                .dispatch("credentials.read", json!({"key":key}))
                .is_err()
        );
        assert!(
            broker
                .dispatch("credentials.acquire", json!({"key":key}))
                .is_err()
        );
    }
    assert!(
        broker
            .dispatch(
                "credentials.commit",
                json!({"lease":"unknown","value":"secret-fixture"})
            )
            .is_err()
    );
    let lease = acquire(&mut broker);
    let error = broker
        .dispatch(
            "credentials.commit",
            json!({"lease":lease["lease"],"value":"secret-fixture".repeat(30000)}),
        )
        .unwrap_err();
    assert!(!error.to_string().contains("secret-fixture"));
    // A failed commit releases its lease, preserving the last durable value.
    acquire(&mut broker);
}

#[test]
fn credential_lock_child() {
    let Some(root) = std::env::var_os("BUCKETS_CREDENTIAL_TEST_ROOT") else {
        return;
    };
    let mut broker = CredentialBroker::new(root, "ai").unwrap();
    let result = broker.dispatch("credentials.acquire", json!({"key":"chatgpt"}));
    if std::env::var_os("BUCKETS_CREDENTIAL_EXPECT_BUSY").is_some() {
        assert!(result.unwrap_err().to_string().contains("busy"));
    } else {
        assert_eq!(result.unwrap()["value"]["revision"], 7);
    }
}
#[test]
fn credential_leases_hold_across_processes_and_release_after_commit() {
    let temp = tempfile::tempdir().unwrap();
    let mut broker = CredentialBroker::new(temp.path(), "ai").unwrap();
    let lease = acquire(&mut broker);
    let executable = std::env::current_exe().unwrap();
    let blocked = std::process::Command::new(&executable)
        .args(["--exact", "credential_lock_child"])
        .env("BUCKETS_CREDENTIAL_TEST_ROOT", temp.path())
        .env("BUCKETS_CREDENTIAL_EXPECT_BUSY", "1")
        .output()
        .unwrap();
    assert!(
        blocked.status.success(),
        "{}",
        String::from_utf8_lossy(&blocked.stderr)
    );
    broker
        .dispatch(
            "credentials.commit",
            json!({"lease":lease["lease"],"value":{"revision":7}}),
        )
        .unwrap();
    let released = std::process::Command::new(executable)
        .args(["--exact", "credential_lock_child"])
        .env("BUCKETS_CREDENTIAL_TEST_ROOT", temp.path())
        .env_remove("BUCKETS_CREDENTIAL_EXPECT_BUSY")
        .output()
        .unwrap();
    assert!(
        released.status.success(),
        "{}",
        String::from_utf8_lossy(&released.stderr)
    );
}
#[cfg(unix)]
#[test]
fn vault_files_are_private_and_reject_links_or_nonregular_files() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("accounts");
    let mut broker = CredentialBroker::new(&root, "ai").unwrap();
    let lease = acquire(&mut broker);
    broker
        .dispatch(
            "credentials.commit",
            json!({"lease":lease["lease"],"value":{"access":"fixture"}}),
        )
        .unwrap();
    for path in [&root, &root.join("ai")] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    for name in ["chatgpt.json", "chatgpt.lock"] {
        assert_eq!(
            fs::metadata(root.join("ai").join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let outside = temp.path().join("outside");
    fs::write(&outside, "private fixture").unwrap();
    fs::remove_file(root.join("ai/chatgpt.json")).unwrap();
    symlink(&outside, root.join("ai/chatgpt.json")).unwrap();
    assert!(
        broker
            .dispatch("credentials.read", json!({"key":"chatgpt"}))
            .is_err()
    );
    let lease = broker
        .dispatch("credentials.acquire", json!({"key":"other"}))
        .unwrap();
    symlink(&outside, root.join("ai/other.json")).unwrap();
    assert!(
        broker
            .dispatch(
                "credentials.commit",
                json!({"lease":lease["lease"],"value":{}})
            )
            .is_err()
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), "private fixture");
    symlink(&outside, root.join("ai/linked.lock")).unwrap();
    assert!(
        broker
            .dispatch("credentials.acquire", json!({"key":"linked"}))
            .is_err()
    );
    fs::hard_link(&outside, root.join("ai/hard.json")).unwrap();
    assert!(
        broker
            .dispatch("credentials.read", json!({"key":"hard"}))
            .is_err()
    );
    fs::create_dir(root.join("ai/directory.json")).unwrap();
    assert!(
        broker
            .dispatch("credentials.read", json!({"key":"directory"}))
            .is_err()
    );
    let alias = temp.path().join("alias");
    symlink(&root, &alias).unwrap();
    assert!(CredentialBroker::new(&alias, "ai").is_err());
    symlink(root.join("ai"), root.join("linked-plugin")).unwrap();
    assert!(CredentialBroker::new(&root, "linked-plugin").is_err());
}

fn fixture(
    capabilities: &[&str],
    network: bool,
    timeout: u64,
) -> (tempfile::TempDir, PluginRegistry) {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("plugin.json"), json!({"id":"accounts-fixture","name":"Accounts Fixture","protocol_version":1,"executable":"python3","args":["service.py"],"capabilities":capabilities,"permissions":{"network":network},"timeout_ms":timeout,"config":{}}).to_string()).unwrap();
    fs::write(temp.path().join("service.py"),r#"import json,sys,time,signal
for line in sys.stdin:
 f=json.loads(line);r=f['params']['request'];rid=f['id'];a=r['action'];account=r.get('account_id')
 if account=='timeout': time.sleep(10)
 if account=='cancel':
  signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(10)
 if account=='mismatch':
  print(json.dumps({'jsonrpc':'2.0','id':999,'result':{}}),flush=True);continue
 print(json.dumps({'jsonrpc':'2.0','id':'vault','method':'credentials.read','params':{'key':'chatgpt'}}),flush=True)
 response=json.loads(sys.stdin.readline());assert 'result' in response
 print(json.dumps({'jsonrpc':'2.0','id':'url','method':'browser.open','params':{'url':'https://auth.openai.com/api/accounts/authorize?state=fixture'}}),flush=True)
 opened=json.loads(sys.stdin.readline())
 assert ('result' in opened)==(a=='sign_in' and account!='no-network')
 print(json.dumps({'jsonrpc':'2.0','id':'forbidden','method':'files.read','params':{}}),flush=True)
 assert 'error' in json.loads(sys.stdin.readline())
 print(json.dumps({'jsonrpc':'2.0','method':'accounts.progress','params':{'message':'Checking connection'}}),flush=True)
 print(json.dumps({'jsonrpc':'2.0','id':rid,'result':{'accounts':[{'id':'account-one','label':'Fixture','connected':True,'plan_enabled':True}],'active_account_id':'account-one','plan_enabled':True,'using_plan':True,'models':[{'id':'fixture-model','name':'Fixture model'}],'usage_url':'https://chatgpt.com/settings/usage'}}),flush=True)
"#).unwrap();
    let registry = PluginRegistry::discover(temp.path()).unwrap();
    (temp, registry)
}
#[test]
fn accounts_are_opt_in_and_browser_requires_explicit_sign_in_and_network() {
    let (_temp, registry) = fixture(&["agent"], true, 3000);
    assert!(registry.start_accounts("accounts-fixture").is_err());
    for (network, action, id) in [
        (true, AccountAction::Status, None),
        (true, AccountAction::SignIn, None),
        (false, AccountAction::SignIn, Some("no-network")),
    ] {
        let (_temp, registry) = fixture(&["accounts"], network, 3000);
        let mut service = registry.start_accounts("accounts-fixture").unwrap();
        let mut urls = 0;
        let mut events = vec![];
        let state = service
            .invoke(
                AccountRequest {
                    action,
                    account_id: id.map(str::to_owned),
                },
                |method, _| match method {
                    "credentials.read" => Ok(json!({})),
                    "browser.open" => {
                        urls += 1;
                        Ok(json!({}))
                    }
                    _ => panic!("Unexpected callback"),
                },
                |event| events.push(event),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(
            urls,
            usize::from(network && action == AccountAction::SignIn)
        );
        assert_eq!(events.len(), 1);
        assert!(state.using_plan);
        assert_eq!(state.models[0].id, "fixture-model");
        assert!(service.is_running());
    }
}
#[test]
fn accounts_reject_bad_frames_timeouts_and_cancellation_and_reap_process() {
    for id in ["mismatch", "timeout", "cancel"] {
        let (_temp, registry) =
            fixture(&["accounts"], true, if id == "timeout" { 60 } else { 3000 });
        let mut service = registry.start_accounts("accounts-fixture").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = cancel.clone();
        let thread = if id == "cancel" {
            Some(thread::spawn(move || {
                thread::sleep(Duration::from_millis(80));
                trigger.store(true, Ordering::Release)
            }))
        } else {
            None
        };
        let error = service
            .invoke(
                AccountRequest {
                    action: AccountAction::Status,
                    account_id: Some(id.into()),
                },
                |_, _| Ok(json!({})),
                |_| {},
                &cancel,
            )
            .unwrap_err();
        if let Some(thread) = thread {
            thread.join().unwrap();
        }
        assert!(!service.is_running());
        assert!(
            error.to_string().contains(match id {
                "mismatch" => "does not match",
                "timeout" => "timed out",
                _ => "cancelled",
            }),
            "{error:#}"
        );
    }
}

#[test]
fn agent_credentials_require_accounts_grant_and_browser_is_never_available_to_agent() {
    for accounts in [false, true] {
        let capabilities = if accounts {
            vec!["agent", "accounts"]
        } else {
            vec!["agent"]
        };
        let (temp, registry) = fixture(&capabilities, true, 3000);
        fs::write(temp.path().join("service.py"),format!(r#"import json,sys
for line in sys.stdin:
 f=json.loads(line);r=f['params']['request'];rid=f['id']
 print(json.dumps({{'jsonrpc':'2.0','id':'credential','method':'credentials.read','params':{{'key':'chatgpt'}}}}),flush=True)
 assert ('result' in json.loads(sys.stdin.readline()))=={granted}
 print(json.dumps({{'jsonrpc':'2.0','id':'url','method':'browser.open','params':{{'url':'https://auth.openai.com/api/accounts/authorize'}}}}),flush=True)
 assert 'error' in json.loads(sys.stdin.readline())
 print(json.dumps({{'jsonrpc':'2.0','id':rid,'result':{{'answer':'Fixture complete','citations':[],'results':[]}}}}),flush=True)
"#, granted=if accounts {"True"} else {"False"})).unwrap();
        let mut called = 0;
        let mut service = registry.start_agent("accounts-fixture").unwrap();
        let result = service
            .invoke(
                AgentRequest {
                    request_id: "fixture".into(),
                    mode: AgentMode::Ask,
                    prompt: "fixture".into(),
                    context: WorkspaceContext {
                        workspace_id: "fixture".into(),
                        ..Default::default()
                    },
                    storage_dir: temp.path().join("state"),
                    memory_update: None,
                },
                |method, _| {
                    assert_eq!(method, "credentials.read");
                    called += 1;
                    Ok(json!({}))
                },
                |_| {},
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(called, usize::from(accounts));
        assert_eq!(result.answer, "Fixture complete");
    }
}
