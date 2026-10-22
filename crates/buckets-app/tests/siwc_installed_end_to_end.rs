//! Independent installed Rust→TypeScript SIWC fixtures. No live account/network.
use anyhow::{Context, Result, bail, ensure};
use buckets_plugin::{
    AccountAction, AccountRequest, AccountState, AgentMode, AgentPaneContext, AgentRequest,
    AgentResult, CredentialBroker, PluginRegistry, WorkspaceContext,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    registry: PluginRegistry,
    browser: Arc<Mutex<Vec<String>>>,
}
impl Fixture {
    fn new() -> Result<Self> {
        let installed = PathBuf::from(std::env::var_os("BUCKETS_AI_E2E_PLUGIN_DIR").context(
            "Set BUCKETS_AI_E2E_PLUGIN_DIR to an explicitly installed plugins directory",
        )?);
        let directory = tempfile::tempdir()?;
        let root = directory.path().canonicalize()?;
        let plugin = root.join("plugins/ai");
        fs::create_dir_all(&plugin)?;
        let log = root.join("network.jsonl");
        let control = root.join("control.json");
        fs::write(&control, "{}")?;
        let loader = root.join("preload.mjs");
        let network = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/siwc-network.mjs");
        fs::write(
            &loader,
            format!(
                "globalThis.__bucketsSiwcFixture = {};\nawait import({});\n",
                json!({"log":log,"control":control}),
                serde_json::to_string(url::Url::from_file_path(network).unwrap().as_str())?
            ),
        )?;
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(installed.join("ai/plugin.json"))?)?;
        manifest["args"] = json!(["--import", loader, installed.join("ai/dist/plugin.js")]);
        manifest["timeout_ms"] = json!(30000);
        ensure!(
            manifest["config"]["provider"] == "chatgpt",
            "fresh installation must default to ChatGPT"
        );
        ensure!(
            manifest["capabilities"]
                .as_array()
                .is_some_and(|values| values.contains(&json!("accounts"))),
            "installed plugin lacks account capability"
        );
        fs::write(
            plugin.join("plugin.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        let registry = PluginRegistry::discover(root.join("plugins"))?;
        Ok(Self {
            _directory: directory,
            root,
            registry,
            browser: Arc::new(Mutex::new(vec![])),
        })
    }
    fn control(&self, value: Value) -> Result<()> {
        fs::write(self.root.join("control.json"), serde_json::to_vec(&value)?)?;
        Ok(())
    }
    fn events(&self) -> Result<Vec<Value>> {
        let text = fs::read_to_string(self.root.join("network.jsonl")).unwrap_or_default();
        text.lines()
            .map(|line| Ok(serde_json::from_str(line)?))
            .collect()
    }
    fn vault(&self) -> Result<CredentialBroker> {
        CredentialBroker::new(self.root.join("accounts"), "ai")
    }
    fn account(
        &self,
        action: AccountAction,
        account_id: Option<&str>,
        browser_mode: &str,
    ) -> Result<AccountState> {
        let mut service = self.registry.start_accounts("ai")?;
        let mut vault = self.vault()?;
        let cancel = AtomicBool::new(false);
        let state = service.invoke(
            AccountRequest {
                action,
                account_id: account_id.map(str::to_owned),
            },
            |method, params| {
                if method == "browser.open" {
                    let value = params["url"]
                        .as_str()
                        .context("missing authorization URL")?;
                    self.browser.lock().unwrap().push(value.into());
                    simulated_browser(value, browser_mode)?;
                    Ok(json!({"opened":true}))
                } else {
                    vault.dispatch(method, params)
                }
            },
            |_| {},
            &cancel,
        )?;
        assert_public(&serde_json::to_string(&state)?);
        service.shutdown();
        Ok(state)
    }
    fn expire(&self) -> Result<()> {
        let mut vault = self.vault()?;
        let lease = vault.dispatch("credentials.acquire", json!({"key":"chatgpt"}))?;
        let mut value = lease["value"].clone();
        let id = value["active_account_id"]
            .as_str()
            .context("no active fixture account")?
            .to_owned();
        let account = value["accounts"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|account| account["id"] == id)
            .unwrap();
        account["tokens"]["expires_at"] = json!(1);
        vault.dispatch(
            "credentials.commit",
            json!({"lease":lease["lease"],"value":value}),
        )?;
        Ok(())
    }
    fn ask(&self, id: &str, prompt: &str) -> Result<(AgentResult, usize)> {
        let mut service = self.registry.start_agent("ai")?;
        let mut vault = self.vault()?;
        let mut snapshots = 0;
        let workspace = WorkspaceContext {
            workspace_id: "siwc-installed-fixture".into(),
            revision: 1,
            panes: vec![AgentPaneContext {
                id: "files".into(),
                location: self.root.display().to_string(),
                ..Default::default()
            }],
        };
        let request = AgentRequest {
            request_id: id.into(),
            mode: AgentMode::Ask,
            prompt: prompt.into(),
            context: workspace.clone(),
            storage_dir: self.root.join("workspace"),
            memory_update: None,
        };
        let result = service.invoke(
            request,
            |method, params| {
                if method.starts_with("credentials.") {
                    vault.dispatch(method, params)
                } else if method == "workspace.snapshot" {
                    snapshots += 1;
                    Ok(serde_json::to_value(&workspace)?)
                } else {
                    bail!("Unexpected model host method {method}")
                }
            },
            |_| {},
            &AtomicBool::new(false),
        )?;
        service.shutdown();
        assert_public(&serde_json::to_string(&result)?);
        Ok((result, snapshots))
    }
    fn memory(&self, text: Option<&str>) -> Result<String> {
        let mut service = self.registry.start_agent("ai")?;
        let request = AgentRequest {
            request_id: format!("notes-{}", text.is_some()),
            mode: AgentMode::Memory,
            prompt: String::new(),
            context: WorkspaceContext {
                workspace_id: "siwc-installed-fixture".into(),
                revision: 1,
                panes: vec![],
            },
            storage_dir: self.root.join("workspace"),
            memory_update: text.map(str::to_owned),
        };
        let result = service.invoke(
            request,
            |_, _| bail!("Offline notes must not access credentials or network"),
            |_| {},
            &AtomicBool::new(false),
        )?;
        Ok(result.memory.unwrap())
    }
    fn assert_checkpoints_private(&self) -> Result<()> {
        if !self.root.join("workspace").exists() {
            return Ok(());
        }
        for path in fs::read_dir(self.root.join("workspace"))? {
            let path = path?.path();
            if path.is_file() {
                assert_public(&String::from_utf8_lossy(&fs::read(path)?));
            }
        }
        for event in self.events()? {
            ensure!(
                event.get("failure").is_none(),
                "fixture rejected serialized provider contract: {event}"
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [self.root.join("accounts"), self.root.join("accounts/ai")] {
                assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o700);
            }
            assert_eq!(
                fs::metadata(self.root.join("accounts/ai/chatgpt.json"))?
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        Ok(())
    }
}
fn assert_public(text: &str) {
    for secret in [
        "fixture-access-",
        "fixture-refresh-",
        "RAW-FIXTURE-SECRET",
        "ambient-api-key-must-never-be-used",
    ] {
        assert!(
            !text.contains(secret),
            "credential/diagnostic material escaped private auth storage"
        );
    }
}
fn callback(url: &url::Url) -> Result<u16> {
    ensure!(
        url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.path() == "/auth/callback",
        "callback must remain loopback"
    );
    let mut stream =
        TcpStream::connect(("127.0.0.1", url.port().context("missing callback port")?))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let target = format!("{}?{}", url.path(), url.query().unwrap_or(""));
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        url.port().unwrap()
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response
        .split_whitespace()
        .nth(1)
        .context("missing callback response")?
        .parse()?)
}
fn simulated_browser(value: &str, mode: &str) -> Result<()> {
    let auth = url::Url::parse(value)?;
    ensure!(
        auth.origin().ascii_serialization() == "https://auth.openai.com"
            && auth.path() == "/api/accounts/authorize",
        "sign-in used an unsupported endpoint"
    );
    let params = auth
        .query_pairs()
        .collect::<std::collections::BTreeMap<_, _>>();
    for key in ["state", "nonce", "code_challenge", "ext_agent_host_id"] {
        ensure!(
            params.get(key).is_some_and(|value| !value.is_empty()),
            "missing authorization binding"
        );
    }
    ensure!(
        params["code_challenge_method"] == "S256"
            && params["resource"] == "https://api.openai.com/v1",
        "wrong authorization contract"
    );
    let mut redirect = url::Url::parse(&params["redirect_uri"])?;
    if mode == "wrong_state_then_a" {
        let mut wrong = redirect.clone();
        wrong
            .query_pairs_mut()
            .append_pair("state", "wrong-state")
            .append_pair("code", "wrong-code")
            .append_pair("client_id", "oaiapp_fixture_a");
        ensure!(callback(&wrong)? == 400, "invalid state was accepted");
    }
    redirect
        .query_pairs_mut()
        .append_pair("state", &params["state"]);
    if mode == "deny" {
        redirect
            .query_pairs_mut()
            .append_pair("error", "access_denied");
        ensure!(callback(&redirect)? == 400, "decline not acknowledged");
        return Ok(());
    }
    let issued = if params["client_id"] == "dynamic_agent_client" {
        ensure!(
            params
                .get("agent_name_hint")
                .is_some_and(|value| value == "Buckets"),
            "wrong application identity"
        );
        if mode == "b" {
            "oaiapp_fixture_b"
        } else {
            "oaiapp_fixture_a"
        }
    } else {
        ensure!(
            !params.contains_key("agent_name_hint"),
            "reauthorization repeated registration metadata"
        );
        &params["client_id"]
    };
    let code = json!({"client":issued,"nonce":params["nonce"],"challenge":params["code_challenge"],"redirect":params["redirect_uri"]}).to_string();
    redirect
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("client_id", issued);
    ensure!(
        callback(&redirect)? == 200,
        "valid authorization callback failed"
    );
    Ok(())
}

#[test]
#[ignore = "requires explicit standalone plugin installation"]
fn installed_chatgpt_sign_in_harness_and_account_isolation() -> Result<()> {
    let fixture = Fixture::new()?;
    let status = fixture.account(AccountAction::Status, None, "")?;
    assert!(status.sign_in_required && status.accounts.is_empty());
    assert!(
        fixture
            .ask("unsigned", "no API fallback")
            .unwrap_err()
            .to_string()
            .contains("Continue with ChatGPT")
    );
    assert!(fixture.events()?.is_empty());
    assert_eq!(
        fixture.memory(Some("local workspace note"))?,
        "local workspace note"
    );
    let connected = fixture.account(AccountAction::SignIn, None, "wrong_state_then_a")?;
    assert!(connected.using_plan && connected.first_connection && connected.plan_enabled);
    assert_eq!(
        connected
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["fixture-model-a", "fixture-model-b"]
    );
    let a = connected.active_account_id.clone().unwrap();
    let reopened = fixture.account(AccountAction::Status, None, "")?;
    assert_eq!(reopened.active_account_id.as_deref(), Some(a.as_str()));
    let reauthorized = fixture.account(AccountAction::SignIn, Some(&a), "a")?;
    assert!(!reauthorized.first_connection);
    let urls = fixture.browser.lock().unwrap().clone();
    let first = url::Url::parse(&urls[0])?;
    let second = url::Url::parse(&urls[1])?;
    let query = |url: &url::Url, key: &str| {
        url.query_pairs()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.into_owned())
    };
    assert_eq!(
        query(&first, "ext_agent_host_id"),
        query(&second, "ext_agent_host_id")
    );
    assert_ne!(query(&first, "nonce"), query(&second, "nonce"));
    assert_eq!(
        query(&second, "client_id").as_deref(),
        Some("oaiapp_fixture_a")
    );
    let (answer, snapshots) = fixture.ask("question-a", "PRIVATE-PROMPT-FOR-ACCOUNT-A")?;
    assert_eq!(answer.answer, "private-answer-oaiapp_fixture_a");
    assert_eq!(
        snapshots, 1,
        "namespaced model tools must reach the actual Rust callback"
    );
    let b = fixture.account(AccountAction::SignIn, None, "b")?;
    assert_ne!(b.active_account_id.as_deref(), Some(a.as_str()));
    assert_eq!(
        b.accounts.len(),
        2,
        "same email must not merge registrations"
    );
    let (answer, _) = fixture.ask("question-b", "question for account B")?;
    assert_eq!(answer.answer, "private-answer-oaiapp_fixture_b");
    let before = fixture
        .events()?
        .iter()
        .filter(|event| event["kind"] == "responses")
        .count();
    assert!(
        fixture
            .ask("question-a", "PRIVATE-PROMPT-FOR-ACCOUNT-A")
            .unwrap_err()
            .to_string()
            .contains("older workspace snapshot")
    );
    assert_eq!(
        fixture
            .events()?
            .iter()
            .filter(|event| event["kind"] == "responses")
            .count(),
        before
    );
    for event in fixture
        .events()?
        .iter()
        .filter(|event| event["kind"] == "responses" && event["client"] == "oaiapp_fixture_b")
    {
        let serialized = event["body"].to_string();
        assert!(
            !serialized.contains("PRIVATE-PROMPT-FOR-ACCOUNT-A")
                && !serialized.contains("private-answer-oaiapp_fixture_a"),
            "account A transcript was sent to B"
        );
    }
    assert_eq!(fixture.memory(None)?, "local workspace note");
    fixture.control(json!({"responsesFailure":"subscription_sharing_usage_limit_exceeded"}))?;
    let error = fixture
        .ask("plan-limit", "question")
        .unwrap_err()
        .to_string();
    assert!(error.contains("usage limit"), "{error}");
    assert_public(&error);
    fixture.control(json!({"endWithoutCompleted":true}))?;
    let error = fixture
        .ask("interrupted", "question")
        .unwrap_err()
        .to_string();
    assert!(error.contains("did not complete"), "{error}");
    assert_public(&error);
    fixture.control(json!({"revocationFailure":true}))?;
    let disconnected = fixture.account(AccountAction::SignOut, None, "")?;
    assert!(!disconnected.using_plan && disconnected.sign_in_required);
    assert!(
        disconnected
            .message
            .unwrap()
            .contains("could not be confirmed")
    );
    assert!(
        fixture
            .ask("after-signout", "no fallback")
            .unwrap_err()
            .to_string()
            .contains("Continue with ChatGPT")
    );
    fixture.assert_checkpoints_private()?;
    Ok(())
}

#[test]
#[ignore = "requires explicit standalone plugin installation"]
fn installed_chatgpt_refresh_is_serialized_and_terminal_failure_retains_registration() -> Result<()>
{
    let fixture = Arc::new(Fixture::new()?);
    let initial = fixture.account(AccountAction::SignIn, None, "a")?;
    fixture.expire()?;
    let workers = (0..2)
        .map(|_| {
            let fixture = fixture.clone();
            thread::spawn(move || fixture.account(AccountAction::Models, None, ""))
        })
        .collect::<Vec<_>>();
    for worker in workers {
        assert!(worker.join().unwrap()?.plan_enabled);
    }
    let events = fixture.events()?;
    assert_eq!(
        events
            .iter()
            .filter(|event| event["kind"] == "token" && event["grant"] == "refresh_token")
            .count(),
        1,
        "simultaneous processes refreshed a rotating token twice"
    );
    let mut vault = fixture.vault()?;
    let record = vault.dispatch("credentials.read", json!({"key":"chatgpt"}))?;
    assert!(
        record["value"]["accounts"][0]["tokens"]["refresh"]
            .as_str()
            .unwrap()
            .ends_with("-rotated")
    );
    fixture.expire()?;
    let expired = vault.dispatch("credentials.read", json!({"key":"chatgpt"}))?;
    fixture.control(json!({"missingRefresh":true}))?;
    let error = fixture
        .account(AccountAction::Models, None, "")
        .unwrap_err()
        .to_string();
    assert!(error.contains("could not renew"), "{error}");
    assert_public(&error);
    let preserved = vault.dispatch("credentials.read", json!({"key":"chatgpt"}))?;
    assert_eq!(
        preserved, expired,
        "a malformed refresh must preserve the prior token set for recovery"
    );
    fixture.control(json!({"refreshFailure":"invalid_grant"}))?;
    let error = fixture
        .account(AccountAction::Models, None, "")
        .unwrap_err()
        .to_string();
    assert!(error.contains("session expired"), "{error}");
    assert_public(&error);
    let status = fixture.account(AccountAction::Status, None, "")?;
    assert!(!status.accounts[0].connected && !status.plan_enabled);
    assert_eq!(status.active_account_id, initial.active_account_id);
    let record = vault.dispatch("credentials.read", json!({"key":"chatgpt"}))?;
    assert_eq!(
        record["value"]["accounts"][0]["client_id"],
        "oaiapp_fixture_a"
    );
    assert!(record["value"]["accounts"][0].get("tokens").is_none());
    fixture.assert_checkpoints_private()?;
    Ok(())
}

#[test]
#[ignore = "requires explicit standalone plugin installation"]
fn installed_chatgpt_rejects_invalid_identity_decline_and_cancel() -> Result<()> {
    for invalid in ["badNonce", "badAudience", "badSignature"] {
        let fixture = Fixture::new()?;
        fixture.control(json!({(invalid):true}))?;
        let error = fixture
            .account(AccountAction::SignIn, None, "a")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("identity could not be verified"),
            "{invalid}: {error}"
        );
        assert_public(&error);
        let status = fixture.account(AccountAction::Status, None, "")?;
        assert!(!status.using_plan && status.accounts.iter().all(|account| !account.connected));
        assert!(
            fixture
                .events()?
                .iter()
                .all(|event| event["kind"] != "responses")
        );
    }
    let limited = Fixture::new()?;
    limited.control(json!({"noPlan":true}))?;
    let identity_only = limited.account(AccountAction::SignIn, None, "a")?;
    assert!(identity_only.accounts[0].connected && identity_only.sign_in_required);
    assert!(!identity_only.plan_enabled && !identity_only.first_connection);
    assert!(
        limited
            .ask("no-plan-grant", "question")
            .unwrap_err()
            .to_string()
            .contains("Continue with ChatGPT")
    );
    limited.control(json!({}))?;
    let enabled = limited.account(
        AccountAction::SignIn,
        identity_only.active_account_id.as_deref(),
        "a",
    )?;
    assert!(enabled.using_plan && enabled.first_connection);
    let last_url = limited.browser.lock().unwrap().last().unwrap().clone();
    assert!(
        url::Url::parse(&last_url)?
            .query_pairs()
            .any(|(key, value)| key == "prompt" && value == "consent")
    );
    let fixture = Fixture::new()?;
    let error = fixture
        .account(AccountAction::SignIn, None, "deny")
        .unwrap_err()
        .to_string();
    assert!(error.contains("declined"), "{error}");
    let mut service = fixture.registry.start_accounts("ai")?;
    let mut vault = fixture.vault()?;
    let cancel = Arc::new(AtomicBool::new(false));
    let cancelled = cancel.clone();
    let result = service.invoke(
        AccountRequest {
            action: AccountAction::SignIn,
            account_id: None,
        },
        |method, params| {
            if method == "browser.open" {
                cancelled.store(true, Ordering::Release);
                Ok(json!({"opened":false}))
            } else {
                vault.dispatch(method, params)
            }
        },
        |_| {},
        &cancel,
    );
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert!(!service.is_running());
    assert!(
        fixture
            .account(AccountAction::Status, None, "")?
            .accounts
            .iter()
            .all(|account| !account.connected)
    );
    Ok(())
}
