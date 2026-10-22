//! Installed real-Pi streaming checks. All HTTP is local, never a live model.
#[allow(dead_code)]
#[path = "../src/agent_transcript.rs"]
mod agent_transcript;
use anyhow::{Context, Result, ensure};
use buckets_core::{AgentBroker, AgentRoot};
use buckets_plugin::{
    AgentMode, AgentPaneContext, AgentRequest, AgentUpdate, PluginOverrides, PluginRegistry,
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
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

const ANSWER: &str = "## Files and boundaries\n\nThe **Rust host** reads `README.md`; the plugin supplies the model.\n\n- UTF-8 stays intact: café, λ, 東京, 🙂.\n- Quotes and escapes: \"quoted\" and \\path.\n\n```rust\nfn answer() -> &'static str {\n    \"hello\"\n}\n```\n\nUse the validated reference below.";
const REASONING: &str = "INTERNAL-REASONING-MUST-NOT-LEAK";

struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    final_sent: Arc<AtomicBool>,
    mode: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn start(file: PathBuf) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}/v1", listener.local_addr()?);
        let stop = Arc::new(AtomicBool::new(false));
        let final_sent = Arc::new(AtomicBool::new(false));
        let mode = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(vec![]));
        let (stopped, finished, mode_in, seen) = (
            stop.clone(),
            final_sent.clone(),
            mode.clone(),
            requests.clone(),
        );
        let worker = thread::spawn(move || {
            let mut turn = 0;
            while !stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_nonblocking(false)
                            .expect("blocking fixture socket");
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                        let result = (|| -> Result<()> {
                            let (path, body, auth) = read_http(&mut stream)?;
                            ensure!(
                                path == "/v1/chat/completions",
                                "unexpected fixture endpoint"
                            );
                            ensure!(!auth, "a keyless fixture received Authorization");
                            seen.lock().unwrap().push(body);
                            turn += 1;
                            write!(
                                stream,
                                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n"
                            )?;
                            if turn == 1 {
                                send(
                                    &mut stream,
                                    json!({"role":"assistant","tool_calls":[{"index":0,"id":"read-fixture","type":"function","function":{"name":"files_read","arguments":json!({"location":file}).to_string()}}]}),
                                    Value::Null,
                                )?;
                                send(&mut stream, json!({}), json!("tool_calls"))?;
                                stream.write_all(b"data: [DONE]\n\n")?;
                                return Ok(());
                            }
                            finished.store(false, Ordering::Release);
                            send(
                                &mut stream,
                                json!({"role":"assistant","reasoning_content":REASONING}),
                                Value::Null,
                            )?;
                            let mut answer = json!({"answer":ANSWER,"citations":[{"location":file,"line":3}],"results":[{"location":file,"name":"invented.txt","line":3}]}).to_string().replace('🙂', "\\ud83d\\ude42");
                            let scenario = mode_in.load(Ordering::Acquire);
                            if scenario == 1 {
                                answer.truncate(answer.len() - 2);
                            }
                            let chars = answer.chars().collect::<Vec<_>>();
                            let mut cursor = 0;
                            let mut chunk = 0;
                            while cursor < chars.len() {
                                let width = [1, 2, 3, 5, 8, 13, 21][chunk % 7];
                                let end = (cursor + width).min(chars.len());
                                let text = chars[cursor..end].iter().collect::<String>();
                                send(&mut stream, json!({"content":text}), Value::Null)?;
                                cursor = end;
                                chunk += 1;
                                thread::sleep(Duration::from_millis(if scenario == 2 {
                                    35
                                } else {
                                    8
                                }));
                            }
                            // This gap is deliberate: first text must reach Rust while
                            // the model HTTP stream is still visibly unfinished.
                            thread::sleep(Duration::from_millis(450));
                            finished.store(true, Ordering::Release);
                            send(&mut stream, json!({}), json!("stop"))?;
                            stream.write_all(b"data: [DONE]\n\n")?;
                            stream.flush()?;
                            Ok(())
                        })();
                        // Client cancellation closes the socket; it is expected here.
                        if let Err(error) = result
                            && !error
                                .chain()
                                .any(|cause| cause.downcast_ref::<std::io::Error>().is_some())
                        {
                            eprintln!("Streaming HTTP fixture: {error:#}");
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            url,
            stop,
            final_sent,
            mode,
            requests,
            worker: Some(worker),
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn send(stream: &mut TcpStream, delta: Value, finish_reason: Value) -> Result<()> {
    write!(
        stream,
        "data: {}\n\n",
        json!({"id":"streaming-fixture","object":"chat.completion.chunk","created":1,"model":"fixture-model","choices":[{"index":0,"delta":delta,"finish_reason":finish_reason}]})
    )?;
    stream.flush()?;
    Ok(())
}
fn read_http(stream: &mut TcpStream) -> Result<(String, Value, bool)> {
    let mut bytes = vec![];
    let header_end = loop {
        let mut chunk = [0; 4096];
        let length = stream.read(&mut chunk)?;
        ensure!(length > 0, "HTTP EOF");
        bytes.extend_from_slice(&chunk[..length]);
        ensure!(bytes.len() < 1_048_576, "fixture request too large");
        if let Some(index) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let path = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_owned();
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        })
        .context("content length missing")?;
    let auth = headers
        .lines()
        .any(|line| line.to_ascii_lowercase().starts_with("authorization:"));
    while bytes.len() < header_end + length {
        let mut chunk = [0; 8192];
        let length = stream.read(&mut chunk)?;
        ensure!(length > 0, "truncated HTTP request");
        bytes.extend_from_slice(&chunk[..length]);
    }
    Ok((
        path,
        serde_json::from_slice(&bytes[header_end..header_end + length])?,
        auth,
    ))
}
fn registry(server: &Server) -> Result<PluginRegistry> {
    let installed = std::env::var_os("BUCKETS_AI_E2E_PLUGIN_DIR")
        .context("Set BUCKETS_AI_E2E_PLUGIN_DIR to standalone installed plugins")?;
    let overrides = BTreeMap::from([(
        "ai".into(),
        serde_json::from_value::<PluginOverrides>(
            json!({"config":{"provider":"compatible","model":"fixture-model","api_key_env":"","base_url":server.url}}),
        )?,
    )]);
    PluginRegistry::discover_with_settings(PathBuf::from(installed), &overrides)
}
fn workspace(root: &Path) -> WorkspaceContext {
    WorkspaceContext {
        workspace_id: "installed-streaming-fixture".into(),
        revision: 1,
        panes: vec![AgentPaneContext {
            id: "files".into(),
            location: root.display().to_string(),
            ..Default::default()
        }],
    }
}
fn request(root: &Path, id: &str, prompt: &str) -> AgentRequest {
    AgentRequest {
        request_id: id.into(),
        mode: AgentMode::Ask,
        prompt: prompt.into(),
        context: workspace(root),
        storage_dir: root.join("state"),
        memory_update: None,
    }
}
fn broker(root: &Path) -> Result<AgentBroker> {
    AgentBroker::new(
        workspace(root),
        vec![AgentRoot {
            location: root.display().to_string(),
            respect_gitignore: true,
            follow_symlinks: false,
        }],
        false,
    )
}
fn assert_snapshots(updates: &[AgentUpdate]) {
    assert!(
        updates.len() >= 2,
        "expected incremental snapshots, not just a final reply"
    );
    let mut last = 0;
    for update in updates {
        assert!(update.sequence > last, "snapshot sequence regressed");
        last = update.sequence;
        assert!(
            ANSWER.starts_with(&update.text),
            "snapshot was raw, corrupted or non-answer text: {:?}",
            update.text
        );
        let wire = serde_json::to_string(update).unwrap();
        assert!(
            !wire.contains(REASONING),
            "reasoning escaped into presentation"
        );
        assert!(
            !wire.contains("\"citations\""),
            "provisional snapshots exposed file actions"
        );
    }
}

#[test]
#[ignore = "requires standalone AI installation and local HTTP"]
fn installed_real_pi_streams_decoded_markdown_before_completion_and_verifies_final_files()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    let file = root.join("README.md");
    fs::write(&file, "# Fixture\n\nRust reads this actual file.\n")?;
    let server = Server::start(file.clone())?;
    let registry = registry(&server)?;
    let broker = broker(&root)?;
    let mut service = registry.start_agent("ai")?;
    let mut updates = vec![];
    let mut visible_before_completion = false;
    let result = service.invoke(
        request(&root, "stream-first", "Read and summarize the fixture"),
        |method, params| broker.handle(method, &params),
        |event| {
            if let Some(update) = event.update {
                if !update.text.is_empty() && !server.final_sent.load(Ordering::Acquire) {
                    visible_before_completion = true;
                }
                updates.push(update);
            }
        },
        &AtomicBool::new(false),
    )?;
    assert!(
        visible_before_completion,
        "first answer text arrived only after HTTP completion"
    );
    assert_snapshots(&updates);
    assert_eq!(updates.last().unwrap().text, ANSWER);
    assert!(
        updates.iter().any(|update| !update.activities.is_empty()),
        "real tool activity was not streamed"
    );
    assert_eq!(result.answer, ANSWER);
    assert_eq!(result.citations[0].location, file.display().to_string());
    assert_eq!(result.results[0].name, "README.md");
    broker.validate_result(&result)?;
    let previous_calls = server.requests.lock().unwrap().len();
    let again = service.invoke(
        request(
            &root,
            "stream-second",
            "Follow up without losing the first question",
        ),
        |method, params| broker.handle(method, &params),
        |_| {},
        &AtomicBool::new(false),
    )?;
    assert_eq!(again.answer, ANSWER);
    assert_eq!(server.requests.lock().unwrap().len(), previous_calls + 1);
    Ok(())
}

#[test]
#[ignore = "requires standalone AI installation and local HTTP"]
fn installed_stream_cancel_retains_partial_snapshot_and_requires_a_new_admission() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    let file = root.join("README.md");
    fs::write(&file, "# Fixture\n\nRead-only content.\n")?;
    let server = Server::start(file)?;
    server.mode.store(2, Ordering::Release);
    let registry = registry(&server)?;
    let broker = broker(&root)?;
    let mut service = registry.start_agent("ai")?;
    let cancel = AtomicBool::new(false);
    let mut updates = vec![];
    let error = service
        .invoke(
            request(&root, "cancelled-stream", "Cancel after text arrives"),
            |method, params| broker.handle(method, &params),
            |event| {
                if let Some(update) = event.update {
                    if update.text.len() > 30 {
                        cancel.store(true, Ordering::Release);
                    }
                    updates.push(update);
                }
            },
            &cancel,
        )
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error:#}");
    assert!(!service.is_running());
    assert_snapshots(&updates);
    assert!(updates.last().unwrap().text.len() > 30);
    assert!(updates.last().unwrap().text.len() < ANSWER.len());
    let calls = server.requests.lock().unwrap().len();
    let error = registry
        .start_agent("ai")?
        .invoke(
            request(&root, "cancelled-stream", "Cancel after text arrives"),
            |method, params| broker.handle(method, &params),
            |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error:#}");
    assert_eq!(
        server.requests.lock().unwrap().len(),
        calls,
        "cancelled work reached the model again"
    );
    server.mode.store(0, Ordering::Release);
    let result = registry.start_agent("ai")?.invoke(
        request(&root, "retry-new-admission", "Retry as a new question"),
        |method, params| broker.handle(method, &params),
        |_| {},
        &AtomicBool::new(false),
    )?;
    assert_eq!(result.answer, ANSWER);
    Ok(())
}

#[test]
#[ignore = "requires standalone AI installation and local HTTP"]
fn installed_stream_partial_text_does_not_become_a_final_result_after_malformed_output()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    let file = root.join("README.md");
    fs::write(&file, "# Fixture\n\nRead-only content.\n")?;
    let server = Server::start(file)?;
    server.mode.store(1, Ordering::Release);
    let registry = registry(&server)?;
    let broker = broker(&root)?;
    let mut updates = vec![];
    let error = registry
        .start_agent("ai")?
        .invoke(
            request(&root, "malformed-final", "Return an interrupted answer"),
            |method, params| broker.handle(method, &params),
            |event| {
                if let Some(update) = event.update {
                    updates.push(update);
                }
            },
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("invalid answer format"),
        "{error:#}"
    );
    assert_snapshots(&updates);
    assert_eq!(updates.last().unwrap().text, ANSWER);
    server.mode.store(0, Ordering::Release);
    let result = registry.start_agent("ai")?.invoke(
        request(&root, "after-malformed", "A fresh retry"),
        |method, params| broker.handle(method, &params),
        |_| {},
        &AtomicBool::new(false),
    )?;
    broker.validate_result(&result)?;
    assert_eq!(result.answer, ANSWER);
    Ok(())
}

#[test]
fn rich_text_reparse_cannot_activate_model_links_images_or_html() {
    use markdown::{ParseOptions, mdast::Node};
    fn inert(node: &Node) {
        assert!(
            !matches!(
                node,
                Node::Link(_)
                    | Node::LinkReference(_)
                    | Node::Image(_)
                    | Node::ImageReference(_)
                    | Node::Html(_)
            ),
            "sanitized Markdown still contains an active renderer node: {node:?}"
        );
        for child in node.children().into_iter().flatten() {
            inert(child);
        }
    }
    for source in [
        "[**nested label** and ![pixel](https://invalid.example/secret)](file:///private/secret)",
        "[reference][key]\n\n[key]: https://invalid.example/token",
        "![reference image][pixel]\n\n[pixel]: https://invalid.example/collect",
        "<https://invalid.example/collect> and <developer@invalid.example>",
        "Bare https://invalid.example/collect?secret=fixture and www.invalid.example",
        "<img src=\"https://invalid.example/collect\" onerror=\"alert(1)\">",
        "<a href=\"file:///private/secret\">open</a>",
        "[escaped \\] bracket](https://invalid.example/collect)",
        "\\![literal](https://invalid.example/collect)",
        "![incomplete](https://invalid.example/collect",
        "&lt;img src=\"https://invalid.example/collect\"&gt;",
        "東京 [🙂](https://invalid.example/collect) café",
    ] {
        for block in agent_transcript::markdown_blocks(source) {
            let reparsed = markdown::to_mdast(&block.markdown, &ParseOptions::gfm()).unwrap();
            inert(&reparsed);
        }
    }
    let source = "```rust\nlet exact = \"[literal](https://invalid.example/)\";\n```";
    let blocks = agent_transcript::markdown_blocks(source);
    assert_eq!(
        blocks[0].code.as_deref(),
        Some("let exact = \"[literal](https://invalid.example/)\";")
    );
}
