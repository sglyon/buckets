//! Independent host admission and explicitly installed Rust→TypeScript AI tests.
#[allow(dead_code)]
#[path = "../src/agent_context.rs"]
mod agent_context;

use anyhow::{Context, Result};
use buckets_core::{AgentBroker, AgentRoot};
use buckets_plugin::{
    AgentMode, AgentPaneContext, AgentRequest, AgentResult, PluginOverrides, PluginRegistry,
    WorkspaceContext,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

fn context(root: &Path) -> WorkspaceContext {
    WorkspaceContext {
        workspace_id: "independent-ai-fixture".into(),
        revision: 1,
        panes: vec![AgentPaneContext {
            id: "files".into(),
            location: root.display().to_string(),
            ..Default::default()
        }],
    }
}
fn registry(config: Value) -> Result<PluginRegistry> {
    let directory = std::env::var_os("BUCKETS_AI_E2E_PLUGIN_DIR")
        .context("Set BUCKETS_AI_E2E_PLUGIN_DIR to the installed plugins directory")?;
    let overrides: BTreeMap<String, PluginOverrides> = BTreeMap::from([(
        "ai".into(),
        serde_json::from_value(json!({"config":config}))?,
    )]);
    PluginRegistry::discover_with_settings(PathBuf::from(directory), &overrides)
}
fn manifest_for(root: &Path, config: Value) -> buckets_plugin::PluginManifest {
    serde_json::from_value(json!({"id":"fixture","name":"Fixture","protocol_version":1,"executable":"unused","capabilities":["agent"],"permissions":{"read_roots":[root],"network":false},"config":config})).unwrap()
}
fn empty_context(workspace: &WorkspaceContext) -> WorkspaceContext {
    WorkspaceContext {
        panes: vec![],
        ..workspace.clone()
    }
}
fn brokers(
    registry: &PluginRegistry,
    workspace: WorkspaceContext,
) -> Result<(WorkspaceContext, AgentBroker, Option<AgentBroker>)> {
    let manifest = registry
        .plugins()
        .iter()
        .find(|plugin| plugin.id == "ai")
        .context("AI manifest missing")?;
    let (workspace, reads, searches) = agent_context::admit_context(workspace, manifest)?;
    let read = AgentBroker::new(workspace.clone(), reads, manifest.permissions.network)?;
    let search = searches
        .map(|roots| {
            AgentBroker::new(
                empty_context(&workspace),
                roots,
                manifest.permissions.network,
            )
        })
        .transpose()?;
    Ok((workspace, read, search))
}
fn request(
    id: &str,
    mode: AgentMode,
    workspace: &WorkspaceContext,
    storage: &Path,
) -> AgentRequest {
    AgentRequest {
        request_id: id.into(),
        mode,
        prompt: String::new(),
        context: workspace.clone(),
        storage_dir: storage.into(),
        memory_update: None,
    }
}

#[test]
fn configured_roots_outside_panes_are_admitted_without_expanding_the_manifest_grant() {
    let fixture = tempfile::tempdir().unwrap();
    let panes = fixture.path().join("panes");
    let indexed = fixture.path().join("indexed");
    let unrelated = fixture.path().join("unrelated");
    for path in [&panes, &indexed, &unrelated] {
        fs::create_dir(path).unwrap();
    }
    fs::write(indexed.join("file.txt"), "indexed source").unwrap();
    fs::write(unrelated.join("secret.txt"), "never admitted").unwrap();
    let config = json!({"index_roots":[{"location":indexed,"respect_gitignore":false}]});
    let manifest = manifest_for(fixture.path(), config.clone());
    let (workspace, roots, searches) =
        agent_context::admit_context(context(&panes), &manifest).unwrap();
    let read = AgentBroker::new(workspace.clone(), roots, false).unwrap();
    assert!(
        read.authorize(indexed.join("file.txt").to_str().unwrap())
            .is_ok()
    );
    assert!(
        read.authorize(unrelated.join("secret.txt").to_str().unwrap())
            .is_err()
    );
    let search = AgentBroker::new(empty_context(&workspace), searches.unwrap(), false).unwrap();
    assert!(search.authorize(indexed.to_str().unwrap()).is_ok());
    assert!(search.authorize(panes.to_str().unwrap()).is_err());
    let narrow = manifest_for(&panes, config);
    assert!(agent_context::admit_context(context(&panes), &narrow).is_err());
}

#[test]
fn explicit_ignore_false_is_effective_but_strict_overlaps_and_symlink_escapes_remain_restricted() {
    let fixture = tempfile::tempdir().unwrap();
    let panes = fixture.path().join("panes");
    let indexed = fixture.path().join("indexed");
    let outside = fixture.path().join("outside");
    for path in [&panes, &indexed, &outside] {
        fs::create_dir(path).unwrap();
    }
    fs::write(indexed.join(".gitignore"), "ignored.txt\n").unwrap();
    fs::write(indexed.join("visible.txt"), "visible").unwrap();
    fs::write(indexed.join("ignored.txt"), "ignored").unwrap();
    fs::write(outside.join("secret.txt"), "secret").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.join("secret.txt"), indexed.join("escape.txt")).unwrap();
    let manifest = manifest_for(
        fixture.path(),
        json!({"index_roots":[{"location":indexed,"respect_gitignore":false}]}),
    );
    let (workspace, reads, searches) =
        agent_context::admit_context(context(&panes), &manifest).unwrap();
    let read = AgentBroker::new(workspace.clone(), reads, false).unwrap();
    assert!(
        read.authorize(indexed.join("escape.txt").to_str().unwrap())
            .is_err()
    );
    let roots = searches.unwrap();
    let search = AgentBroker::new(empty_context(&workspace), roots.clone(), false).unwrap();
    let params =
        json!({"location":indexed,"query":"","engine":"builtin","respect_gitignore":false});
    let found = search.handle("files.search", &params).unwrap();
    assert!(
        found["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"] == "ignored.txt")
    );
    let mut strict = roots;
    strict.push(AgentRoot {
        location: indexed.display().to_string(),
        respect_gitignore: true,
        follow_symlinks: false,
    });
    let found = AgentBroker::new(empty_context(&workspace), strict, false)
        .unwrap()
        .handle("files.search", &params)
        .unwrap();
    assert!(
        !found["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"] == "ignored.txt")
    );
    let implicit = manifest_for(
        fixture.path(),
        json!({"index_roots":[{"location":indexed}]}),
    );
    assert!(agent_context::admit_context(context(&panes), &implicit).is_err());
}

#[test]
fn workspace_identity_survives_restoration_and_storage_cannot_escape_via_plugin_name() {
    let fixture = tempfile::tempdir().unwrap();
    let id = agent_context::workspace_identity(fixture.path(), None);
    assert_eq!(
        agent_context::workspace_identity(fixture.path(), Some("stale-session-id")),
        id
    );
    let storage = agent_context::storage_directory(fixture.path(), "../escape", &id);
    assert!(storage.starts_with(fixture.path().join("agents")));
    assert!(!storage.to_string_lossy().contains("../"));
    assert_eq!(agent_context::launch_epoch(), agent_context::launch_epoch());
    use buckets_core::Location;
    let a = fixture.path().join("a");
    let b = fixture.path().join("b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    let a = Location::parse(a.to_str().unwrap()).unwrap();
    let b = Location::parse(b.to_str().unwrap()).unwrap();
    let a_id =
        agent_context::workspace_for_launch(fixture.path(), Some(std::slice::from_ref(&a)), None);
    let b_id =
        agent_context::workspace_for_launch(fixture.path(), Some(std::slice::from_ref(&b)), None);
    assert_ne!(a_id, b_id);
    assert_eq!(
        agent_context::workspace_for_launch(fixture.path(), None, Some(&a_id)),
        a_id
    );
    assert_eq!(
        agent_context::workspace_for_launch(
            fixture.path(),
            Some(std::slice::from_ref(&a)),
            Some(&b_id)
        ),
        a_id
    );
    assert_eq!(
        agent_context::workspace_for_launch(
            fixture.path(),
            Some(&[a.clone(), b.clone(), a.clone()]),
            None
        ),
        agent_context::workspace_for_launch(fixture.path(), Some(&[b, a]), None)
    );
    #[cfg(unix)]
    {
        let work = fixture.path().join("work");
        let outside = fixture.path().join("outside");
        fs::create_dir(&work).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::create_dir(outside.join("nested")).unwrap();
        fs::create_dir(outside.join("work")).unwrap();
        std::os::unix::fs::symlink(outside.join("nested"), fixture.path().join("link")).unwrap();
        let linked =
            Location::parse(fixture.path().join("link/../work").to_str().unwrap()).unwrap();
        let direct = Location::parse(work.to_str().unwrap()).unwrap();
        assert_ne!(
            agent_context::workspace_for_launch(fixture.path(), Some(&[linked]), None),
            agent_context::workspace_for_launch(fixture.path(), Some(&[direct]), None),
            "lexical parent traversal merged distinct symlink-root workspaces"
        );
    }
}

#[test]
#[ignore = "requires explicit standalone plugin installation"]
fn installed_plugin_discovers_commands_and_durable_offline_memory_reopens_and_clears() -> Result<()>
{
    let fixture = tempfile::tempdir()?;
    let registry = registry(json!({"api_key_env":""}))?;
    let commands = registry.commands("ai")?;
    assert_eq!(commands.len(), 4);
    assert!(commands.iter().any(|command| command.id == "ai.ask"));
    let launch = registry.invoke_command("ai", "ai.memory", json!({}))?;
    assert_eq!(launch["host_action"], "agent");
    assert_eq!(launch["request"]["mode"], "memory");
    let workspace = context(fixture.path());
    let storage = fixture.path().join("state");
    let cancelled = AtomicBool::new(false);
    let mut service = registry.start_agent("ai")?;
    let mut write = request("memory-write", AgentMode::Memory, &workspace, &storage);
    write.memory_update = Some("Notes λ\nKeep the importer tests".into());
    let host = |_: &str, _: Value| -> Result<Value> {
        anyhow::bail!("Offline memory must not invoke host or model tools")
    };
    assert_eq!(
        service
            .invoke(write, host, |_| {}, &cancelled)?
            .memory
            .as_deref(),
        Some("Notes λ\nKeep the importer tests")
    );
    service.shutdown();
    let mut reopened = registry.start_agent("ai")?;
    assert_eq!(
        reopened
            .invoke(
                request("memory-read", AgentMode::Memory, &workspace, &storage),
                host,
                |_| {},
                &cancelled
            )?
            .memory
            .as_deref(),
        Some("Notes λ\nKeep the importer tests")
    );
    let mut clear = request("memory-clear", AgentMode::Memory, &workspace, &storage);
    clear.memory_update = Some(String::new());
    reopened.invoke(clear, host, |_| {}, &cancelled)?;
    reopened.shutdown();
    let result = registry.start_agent("ai")?.invoke(
        request("memory-empty", AgentMode::Memory, &workspace, &storage),
        host,
        |_| {},
        &cancelled,
    )?;
    assert_eq!(result.memory.as_deref(), Some(""));
    assert!(storage.join("conversation.sqlite").is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&storage)?.permissions().mode() & 0o777, 0o700);
    }

    Ok(())
}

type HttpRequests = Arc<Mutex<Vec<(String, Value, bool)>>>;
struct HttpFixture {
    base: String,
    seen: HttpRequests,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl HttpFixture {
    fn start(citation: PathBuf) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let base = format!("http://{}", listener.local_addr()?);
        let seen = Arc::new(Mutex::new(vec![]));
        let log = seen.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            let mut turns = 0;
            while !stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                        let response = (|| -> Result<(String, String)> {
                            let (path, body, auth) = read_http(&mut stream)?;
                            log.lock().unwrap().push((path.clone(), body.clone(), auth));
                            if path == "/v1/embeddings" {
                                let input = body["input"]
                                    .as_array()
                                    .context("embedding input not array")?;
                                let data = input
                                    .iter()
                                    .enumerate()
                                    .map(|(index, text)| {
                                        let text = text.as_str().unwrap_or("").to_lowercase();
                                        let vector =
                                            if text == "coffee" || text.contains("hot beverage") {
                                                vec![1., 0., 0.]
                                            } else {
                                                vec![0., 1., 0.]
                                            };
                                        json!({"index":index,"embedding":vector})
                                    })
                                    .collect::<Vec<_>>();
                                return Ok((
                                    "application/json".into(),
                                    json!({"data":data}).to_string(),
                                ));
                            }
                            if path != "/v1/chat/completions" {
                                anyhow::bail!("unexpected fixture path {path}")
                            };
                            turns += 1;
                            let delta = if turns == 1 {
                                json!({"role":"assistant","tool_calls":[{"index":0,"id":"fixture-search","type":"function","function":{"name":"files_search","arguments":json!({"location":citation.parent().unwrap(),"query":"","engine":"builtin"}).to_string()}}]})
                            } else if turns == 2 {
                                json!({"role":"assistant","tool_calls":[{"index":0,"id":"fixture-read","type":"function","function":{"name":"files_read","arguments":json!({"location":citation}).to_string()}}]})
                            } else {
                                json!({"role":"assistant","content":json!({"answer":"Fixture model read the actual file","citations":[{"location":citation,"line":2}],"results":[{"location":citation,"name":"model-invented-name","line":2,"score":0.5}]}).to_string()})
                            };
                            let reason = if turns <= 2 { "tool_calls" } else { "stop" };
                            let mut sse = String::new();
                            for (delta, finish) in
                                [(delta, Value::Null), (json!({}), json!(reason))]
                            {
                                sse.push_str(&format!("data: {}\n\n",json!({"id":"fixture-completion","object":"chat.completion.chunk","created":1,"model":"fixture-model","choices":[{"index":0,"delta":delta,"finish_reason":finish}]})));
                            }
                            sse.push_str("data: [DONE]\n\n");
                            Ok(("text/event-stream".into(), sse))
                        })();
                        let (kind, body) = response.unwrap_or_else(|error| {
                            eprintln!("Local HTTP fixture error: {error:#}");
                            (
                                "application/json".into(),
                                json!({"error":{"message":error.to_string()}}).to_string(),
                            )
                        });
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            base,
            seen,
            stop,
            worker: Some(worker),
        })
    }
    fn embedding_inputs(&self) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _, _)| path == "/v1/embeddings")
            .map(|(_, value, _)| value["input"].as_array().unwrap().len())
            .sum()
    }
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn read_http(stream: &mut TcpStream) -> Result<(String, Value, bool)> {
    let mut bytes = vec![];
    let header_end = loop {
        let mut chunk = [0; 1024];
        let read = stream.read(&mut chunk)?;
        anyhow::ensure!(read > 0, "unexpected HTTP EOF");
        bytes.extend_from_slice(&chunk[..read]);
        anyhow::ensure!(bytes.len() < 1_048_576, "fixture request too large");
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let path = headers
        .lines()
        .next()
        .context("missing request line")?
        .split_whitespace()
        .nth(1)
        .context("missing path")?
        .to_owned();
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        })
        .context("missing content length")?;
    let auth = headers
        .lines()
        .any(|line| line.to_ascii_lowercase().starts_with("authorization:"));
    while bytes.len() < header_end + length {
        let mut chunk = [0; 8192];
        let read = stream.read(&mut chunk)?;
        anyhow::ensure!(read > 0, "truncated request");
        bytes.extend_from_slice(&chunk[..read]);
    }
    Ok((
        path,
        serde_json::from_slice(&bytes[header_end..header_end + length])?,
        auth,
    ))
}

fn invoke(
    service: &mut buckets_plugin::AgentService,
    request: AgentRequest,
    read: &AgentBroker,
    search: Option<&AgentBroker>,
    calls: &mut Vec<String>,
) -> Result<AgentResult> {
    let mode = request.mode;
    let result = service.invoke(
        request,
        |method, params| {
            calls.push(method.to_owned());
            agent_context::dispatch_tool(mode, read, search, method, &params)
        },
        |_| {},
        &AtomicBool::new(false),
    )?;
    read.validate_result(&result)?;
    Ok(result)
}

#[test]
#[ignore = "requires explicit standalone plugin installation and local HTTP"]
fn installed_real_provider_adapter_uses_keyless_local_http_and_verified_host_read_citation()
-> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    let panes = root.join("panes");
    let indexed = root.join("index-only");
    fs::create_dir(&panes)?;
    fs::create_dir(&indexed)?;
    let file = panes.join("space λ.txt");
    fs::write(&file, "first line\nActual broker content on line two\n")?;
    let server = HttpFixture::start(file.clone())?;
    let registry = registry(
        json!({"provider":"compatible","model":"fixture-model","api_key_env":"","base_url":format!("{}/v1",server.base),"index_roots":[{"location":indexed,"respect_gitignore":true}]}),
    )?;
    let (workspace, read, search) = brokers(&registry, context(&panes))?;
    let storage = fixture.path().join("state");
    let mut ask = request("ask-http", AgentMode::Ask, &workspace, &storage);
    ask.prompt = "Read the fixture and cite line two".into();
    let mut service = registry.start_agent("ai")?;
    let mut calls = vec![];
    let result = invoke(
        &mut service,
        ask.clone(),
        &read,
        search.as_ref(),
        &mut calls,
    )
    .with_context(|| {
        format!(
            "Fixture model HTTP requests={}, host callbacks={calls:?}",
            server.seen.lock().unwrap().len()
        )
    })?;
    assert_eq!(result.answer, "Fixture model read the actual file");
    assert_eq!(result.citations[0].location, file.display().to_string());
    assert_eq!(result.citations[0].line, Some(2));
    assert_eq!(
        result.results[0].name, "space λ.txt",
        "model-supplied file names must be replaced with authoritative host names"
    );
    assert!(
        calls
            .iter()
            .filter(|method| method.as_str() == "files.read")
            .count()
            >= 2
    );
    service.shutdown();
    let requests = server.seen.lock().unwrap().len();
    assert_eq!(requests, 3);
    assert!(calls.iter().any(|method| method == "files.search"));
    assert!(server.seen.lock().unwrap().iter().all(|(_, _, auth)| !auth));
    let result = invoke(
        &mut registry.start_agent("ai")?,
        ask.clone(),
        &read,
        search.as_ref(),
        &mut calls,
    )?;
    assert_eq!(result.answer, "Fixture model read the actual file");
    assert_eq!(
        server.seen.lock().unwrap().len(),
        requests,
        "completed durable admission was replayed"
    );
    let mut second = ask.clone();
    second.request_id = "ask-second".into();
    second.prompt = "A second independent question".into();
    invoke(
        &mut registry.start_agent("ai")?,
        second,
        &read,
        search.as_ref(),
        &mut calls,
    )?;
    let after_second = server.seen.lock().unwrap().len();
    assert_eq!(after_second, requests + 1);
    ask.context.revision += 1;
    let error = invoke(
        &mut registry.start_agent("ai")?,
        ask,
        &read,
        search.as_ref(),
        &mut calls,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("older workspace snapshot"), "{error}");
    assert_eq!(
        server.seen.lock().unwrap().len(),
        after_second,
        "old admission with changed snapshot reached model after another admission"
    );
    Ok(())
}

#[test]
#[ignore = "requires explicit standalone plugin installation and local HTTP"]
fn installed_hybrid_search_uses_real_embeddings_cache_ignore_policy_and_removes_stale_files()
-> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    let panes = root.join("panes");
    let indexed = root.join("indexed");
    fs::create_dir(&panes)?;
    fs::create_dir(&indexed)?;
    let semantic = indexed.join("a-beverage.txt");
    let ignored = indexed.join("ignored.txt");
    fs::write(&semantic, "A hot beverage served in a ceramic cup")?;
    fs::write(indexed.join("database.txt"), "database utility")?;
    fs::write(&ignored, "ignored hot beverage")?;
    fs::write(indexed.join(".gitignore"), "ignored.txt\n")?;
    let server = HttpFixture::start(semantic.clone())?;
    let config = json!({"api_key_env":"","embedding_endpoint":format!("{}/v1/embeddings",server.base),"embedding_model":"fixture-vectors","index_roots":[{"location":indexed,"respect_gitignore":true}]});
    let registry = registry(config)?;
    let (workspace, read, search) = brokers(&registry, context(&panes))?;
    let storage = fixture.path().join("state");
    let mut request = request("search-1", AgentMode::Search, &workspace, &storage);
    request.prompt = "coffee".into();
    let mut calls = vec![];
    let mut service = registry.start_agent("ai")?;
    let first = invoke(
        &mut service,
        request.clone(),
        &read,
        search.as_ref(),
        &mut calls,
    )?;
    assert_eq!(first.results[0].location, semantic.display().to_string());
    assert!(
        first
            .results
            .iter()
            .all(|result| result.location != ignored.display().to_string())
    );
    assert!(first.answer.contains("Hybrid semantic/keyword"));
    let inputs = server.embedding_inputs();
    assert_eq!(inputs, 3);
    request.request_id = "search-2".into();
    invoke(
        &mut service,
        request.clone(),
        &read,
        search.as_ref(),
        &mut calls,
    )?;
    assert_eq!(
        server.embedding_inputs(),
        inputs + 1,
        "unchanged file vectors were recomputed"
    );
    let keyword = indexed.join("coffee-database.txt");
    fs::write(&keyword, "coffee database archive")?;
    request.request_id = "search-3".into();
    let fused = invoke(
        &mut service,
        request.clone(),
        &read,
        search.as_ref(),
        &mut calls,
    )?;
    assert_eq!(fused.results[0].location, keyword.display().to_string());
    assert!(
        fused.results[0]
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("keyword rank")
    );
    assert!(
        fused
            .results
            .iter()
            .any(|result| result.location == semantic.display().to_string())
    );
    fs::remove_file(&semantic)?;
    request.request_id = "search-4".into();
    let pruned = invoke(&mut service, request, &read, search.as_ref(), &mut calls)?;
    assert!(
        pruned
            .results
            .iter()
            .all(|result| result.location != semantic.display().to_string())
    );
    assert!(calls.iter().any(|method| method == "files.search"));
    assert!(storage.join("vectors.sqlite").is_file());
    Ok(())
}
