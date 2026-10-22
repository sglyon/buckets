//! Opt-in duplex services. Ordinary one-shot plugins remain unchanged.
use super::{
    AccountAction, AccountEvent, AccountRequest, AccountState, Capability, MAX_REQUEST_BYTES,
    MAX_RESPONSE_BYTES, PROTOCOL_VERSION, PluginRegistry, terminate,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    #[default]
    Ask,
    Search,
    Memory,
    Index,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentLaunchRequest {
    pub mode: AgentMode,
    #[serde(default)]
    pub prompt: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentEntry {
    pub location: String,
    pub name: String,
    #[serde(default)]
    pub is_dir: bool,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentPaneContext {
    pub id: String,
    pub location: String,
    #[serde(default)]
    pub selected: Vec<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub entries: Vec<AgentEntry>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceContext {
    pub workspace_id: String,
    pub revision: u64,
    pub panes: Vec<AgentPaneContext>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    pub request_id: String,
    pub mode: AgentMode,
    #[serde(default)]
    pub prompt: String,
    pub context: WorkspaceContext,
    pub storage_dir: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_update: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentCitation {
    pub location: String,
    #[serde(default)]
    pub line: Option<u64>,
    #[serde(default)]
    pub label: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentFileResult {
    pub location: String,
    pub name: String,
    #[serde(default)]
    pub is_dir: bool,
    #[serde(default)]
    pub line: Option<u64>,
    #[serde(default)]
    pub snippet: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub score: f64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentResult {
    #[serde(default)]
    pub answer: String,
    #[serde(default)]
    pub citations: Vec<AgentCitation>,
    #[serde(default)]
    pub results: Vec<AgentFileResult>,
    #[serde(default)]
    pub memory: Option<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentEvent {
    pub request_id: String,
    #[serde(default)]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<AgentUpdate>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhase {
    Waiting,
    Reading,
    Responding,
    Verifying,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActivityState {
    Running,
    Complete,
    Error,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AgentActivity {
    pub id: String,
    pub title: String,
    pub state: AgentActivityState,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentUpdate {
    pub sequence: u64,
    pub text: String,
    pub phase: AgentPhase,
    pub activities: Vec<AgentActivity>,
}
impl AgentUpdate {
    fn validate(&self, previous_sequence: u64) -> Result<()> {
        ensure!(
            self.sequence > previous_sequence,
            "Agent stream sequence did not advance"
        );
        ensure!(self.text.len() <= 65536, "Agent stream text exceeds 64 KiB");
        ensure!(
            self.activities.len() <= 32,
            "Agent stream has too many activities"
        );
        let mut ids = std::collections::BTreeSet::new();
        for activity in &self.activities {
            ensure!(
                !activity.id.is_empty()
                    && activity.id.len() <= 128
                    && !activity.id.chars().any(char::is_control),
                "Invalid agent activity identifier"
            );
            ensure!(
                activity.title.len() <= 512 && !activity.title.chars().any(char::is_control),
                "Invalid agent activity title"
            );
            ensure!(
                ids.insert(&activity.id),
                "Duplicate agent activity identifier"
            );
        }
        Ok(())
    }
}

/// One serial duplex connection. Use off the UI thread; dropping it terminates
/// and reaps its entire process group. A poisoned connection is not reusable.
pub struct AgentService {
    storage: Option<(PathBuf, std::fs::File)>,
    child: Option<Child>,
    writer: Option<SyncSender<Vec<u8>>>,
    reader: Receiver<Result<(Value, usize)>>,
    output_open: std::sync::Arc<AtomicBool>,
    config: Value,
    timeout: Duration,
    next_id: u64,
    pub(super) credentials_allowed: bool,
    network_allowed: bool,
}
impl PluginRegistry {
    pub fn start_agent(&self, id: &str) -> Result<AgentService> {
        self.start_duplex(id, Capability::Agent)
    }
    pub(super) fn start_duplex(&self, id: &str, capability: Capability) -> Result<AgentService> {
        let plugin = self.require(id, capability)?;
        let executable = if plugin.executable.contains(std::path::MAIN_SEPARATOR) {
            plugin.directory.join(&plugin.executable)
        } else {
            PathBuf::from(&plugin.executable)
        };
        let mut command = Command::new(executable);
        command
            .args(&plugin.args)
            .current_dir(&plugin.directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().context("Could not start agent plugin")?;
        let mut stdin = child.stdin.take().context("Agent stdin unavailable")?;
        let stdout = child.stdout.take().context("Agent stdout unavailable")?;
        let mut stderr = child.stderr.take().context("Agent stderr unavailable")?;
        let (tx, writes) = mpsc::sync_channel::<Vec<u8>>(4);
        thread::spawn(move || {
            while let Ok(bytes) = writes.recv() {
                if stdin.write_all(&bytes).and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        });
        let (lines, reader) = mpsc::sync_channel(16);
        let output_open = std::sync::Arc::new(AtomicBool::new(true));
        let reader_open = output_open.clone();
        thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                let result = (&mut stdout)
                    .take((MAX_RESPONSE_BYTES + 2) as u64)
                    .read_until(b'\n', &mut line)
                    .context("Read agent response")
                    .and_then(|n| {
                        ensure!(n != 0, "Agent plugin closed its output");
                        ensure!(
                            line.len() <= MAX_RESPONSE_BYTES,
                            "Agent frame exceeds 8 MiB"
                        );
                        ensure!(
                            line.last() == Some(&b'\n'),
                            "Agent frame is not newline terminated"
                        );
                        serde_json::from_slice::<Value>(&line)
                            .context("Agent returned malformed JSON")
                            .map(|frame| (frame, line.len()))
                    });
                let failed = result.is_err();
                if failed {
                    reader_open.store(false, Ordering::Release);
                }
                if lines.send(result).is_err() || failed {
                    break;
                }
            }
        });
        // Always drain stderr without retaining unbounded output or blocking the
        // plugin when a log pipe fills. Diagnostics belong in progress/errors.
        thread::spawn(move || {
            let mut sink = [0; 4096];
            while let Ok(n) = stderr.read(&mut sink) {
                if n == 0 {
                    break;
                }
            }
        });
        Ok(AgentService {
            storage: None,
            child: Some(child),
            writer: Some(tx),
            reader,
            output_open,
            config: plugin.config.clone(),
            timeout: Duration::from_millis(plugin.timeout_ms),
            next_id: 1,
            credentials_allowed: plugin.supports(Capability::Accounts),
            network_allowed: plugin.permissions.network,
        })
    }
}
impl AgentService {
    pub(super) fn invoke_accounts(
        &mut self,
        request: AccountRequest,
        mut host: impl FnMut(&str, Value) -> Result<Value>,
        mut progress: impl FnMut(AccountEvent),
        cancel: &AtomicBool,
    ) -> Result<AccountState> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .context("Account request sequence exhausted")?;
        let deadline = Instant::now() + self.timeout.min(Duration::from_secs(300));
        let pid = self
            .child
            .as_ref()
            .context("Account service is closed")?
            .id();
        let writer = self
            .writer
            .as_ref()
            .context("Account input is closed")?
            .clone();
        let stopped = AtomicBool::new(false);
        let (result, stopped_for) = thread::scope(|scope| {
            let watchdog = scope.spawn(|| {
                loop {
                    let reason = if cancel.load(Ordering::Acquire) {
                        Some("cancelled")
                    } else if Instant::now() >= deadline {
                        Some("timed out")
                    } else {
                        None
                    };
                    if let Some(reason) = reason {
                        if let Ok(mut bytes) = serde_json::to_vec(
                            &json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":id}}),
                        ) {
                            bytes.push(b'\n');
                            let _ = writer.try_send(bytes);
                        }
                        #[cfg(unix)]
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGTERM);
                        }
                        thread::sleep(Duration::from_millis(100));
                        #[cfg(unix)]
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                        return Some(reason);
                    }
                    if stopped.load(Ordering::Acquire) {
                        return None;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            });
            let result =
                self.invoke_accounts_inner(id, deadline, request, &mut host, &mut progress, cancel);
            stopped.store(true, Ordering::Release);
            (result, watchdog.join().ok().flatten())
        });
        if let Some(reason) = stopped_for {
            self.shutdown();
            bail!("Account request {reason}");
        }
        if result.is_err() {
            self.shutdown();
        }
        result
    }

    fn invoke_accounts_inner(
        &mut self,
        id: u64,
        deadline: Instant,
        request: AccountRequest,
        host: &mut impl FnMut(&str, Value) -> Result<Value>,
        progress: &mut impl FnMut(AccountEvent),
        cancel: &AtomicBool,
    ) -> Result<AccountState> {
        ensure!(self.is_running(), "Account service is not running");
        ensure!(
            self.credentials_allowed,
            "Plugin did not declare accounts capability"
        );
        request.validate()?;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"accounts.invoke","params":{"protocol_version":PROTOCOL_VERSION,"config":self.config,"request":request}}), deadline, cancel)?;
        let mut frames = 0;
        loop {
            ensure!(!cancel.load(Ordering::Acquire), "Account request cancelled");
            ensure!(Instant::now() < deadline, "Account request timed out");
            let (frame, frame_bytes) = match self.reader.recv_timeout(Duration::from_millis(20)) {
                Ok(frame) => frame?,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => bail!("Account plugin output closed"),
            };
            frames += 1;
            ensure!(frames <= 2048, "Account service exceeded frame budget");
            ensure!(
                frame["jsonrpc"] == "2.0",
                "Account frame has invalid JSON-RPC version"
            );
            if let Some(method) = frame.get("method").and_then(Value::as_str) {
                ensure!(
                    frame.get("result").is_none() && frame.get("error").is_none(),
                    "Account request contains response fields"
                );
                if method == "accounts.progress" {
                    ensure!(
                        frame.get("id").is_none(),
                        "Account progress must be a notification"
                    );
                    let event: AccountEvent = serde_json::from_value(frame["params"].clone())
                        .context("Invalid account progress")?;
                    ensure!(event.message.len() <= 4096, "Account progress is too long");
                    progress(event);
                    continue;
                }
                ensure!(
                    frame_bytes <= MAX_REQUEST_BYTES,
                    "Account callback exceeds 1 MiB"
                );
                let callback_id = frame
                    .get("id")
                    .and_then(Value::as_str)
                    .context("Account callback requires a string id")?;
                ensure!(
                    !callback_id.is_empty() && callback_id.len() <= 128,
                    "Invalid account callback id"
                );
                let allowed = matches!(
                    method,
                    "credentials.read"
                        | "credentials.acquire"
                        | "credentials.commit"
                        | "credentials.release"
                ) || (method == "browser.open"
                    && request.action == AccountAction::SignIn
                    && self.network_allowed);
                ensure!(!cancel.load(Ordering::Acquire), "Account request cancelled");
                let response = if allowed {
                    host(
                        method,
                        frame.get("params").cloned().unwrap_or_else(|| json!({})),
                    )
                } else {
                    Err(anyhow::anyhow!("Account host operation is not permitted"))
                };
                let response = match response {
                    Ok(result) => json!({"jsonrpc":"2.0","id":callback_id,"result":result}),
                    Err(error) => {
                        json!({"jsonrpc":"2.0","id":callback_id,"error":{"code":-32000,"message":format!("{error:#}")}})
                    }
                };
                self.send(response, deadline, cancel)?;
                continue;
            }
            ensure!(
                frame["id"] == id,
                "Account response id does not match request"
            );
            ensure!(
                frame.get("result").is_some() != frame.get("error").is_some(),
                "Account response needs exactly one result or error"
            );
            if let Some(error) = frame.get("error") {
                bail!(
                    "Account plugin: {}",
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Account operation failed")
                );
            }
            let state: AccountState = serde_json::from_value(frame["result"].clone())
                .context("Invalid account result")?;
            state.validate()?;
            return Ok(state);
        }
    }
    pub fn is_running(&mut self) -> bool {
        // Do not reap here: teardown must signal the owned process group while
        // its PID is still reserved, then wait. EOF/protocol failure marks the
        // connection unavailable without creating a recycled-PID kill window.
        self.child.is_some() && self.output_open.load(Ordering::Acquire)
    }
    pub fn shutdown(&mut self) {
        self.writer.take();
        if let Some(mut child) = self.child.take() {
            terminate(&mut child);
        }
        self.storage.take();
    }
    fn send(&self, value: Value, deadline: Instant, cancel: &AtomicBool) -> Result<()> {
        let mut bytes = serde_json::to_vec(&value)?;
        ensure!(
            bytes.len() <= MAX_REQUEST_BYTES,
            "Agent host frame exceeds 1 MiB"
        );
        bytes.push(b'\n');
        let writer = self.writer.as_ref().context("Agent service is closed")?;
        loop {
            ensure!(!cancel.load(Ordering::Acquire), "Agent request cancelled");
            ensure!(Instant::now() < deadline, "Agent request timed out");
            match writer.try_send(bytes) {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Disconnected(_)) => bail!("Agent plugin input closed"),
                Err(mpsc::TrySendError::Full(returned)) => {
                    bytes = returned;
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }
    pub fn invoke(
        &mut self,
        request: AgentRequest,
        mut host: impl FnMut(&str, Value) -> Result<Value>,
        mut progress: impl FnMut(AgentEvent),
        cancel: &AtomicBool,
    ) -> Result<AgentResult> {
        let prepared = (|| -> Result<()> {
            ensure!(self.is_running(), "Agent service is not running");
            self.acquire_storage(&request.storage_dir)
        })();
        if let Err(error) = prepared {
            self.shutdown();
            return Err(error);
        }
        let request_id = request.request_id.clone();
        let storage = request.storage_dir.clone();
        let pid = self.child.as_ref().context("Agent service is closed")?.id();
        let writer = self
            .writer
            .as_ref()
            .context("Agent input is closed")?
            .clone();
        let stopped = AtomicBool::new(false);
        let deadline = Instant::now() + self.timeout;
        // The child is not reaped inside invoke_inner. Joining this watchdog
        // before teardown keeps its process-group ID reserved through the grace
        // period, even when a provider callback is still blocked.
        let (result, watchdog) = thread::scope(|scope| {
            let watcher = scope.spawn(|| -> Result<Option<&str>> {
                loop {
                    if cancel.load(Ordering::Acquire) {
                        let marker = write_cancel_marker(&storage, &request_id);
                        graceful_cancel(pid, &writer, &request_id);
                        marker.context("Could not durably record explicit agent cancellation")?;
                        return Ok(Some("cancelled"));
                    }
                    if Instant::now() >= deadline {
                        #[cfg(unix)]
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                        return Ok(Some("timed out"));
                    }
                    if stopped.load(Ordering::Acquire) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(None)
            });
            let result = self.invoke_inner(request, &mut host, &mut progress, cancel);
            stopped.store(true, Ordering::Release);
            let watchdog = watcher
                .join()
                .map_err(|_| anyhow::anyhow!("Agent cancellation watchdog panicked"))
                .and_then(|result| result);
            (result, watchdog)
        });
        match watchdog {
            Ok(Some(reason)) => {
                self.shutdown();
                bail!("Agent request {reason}");
            }
            Err(error) => {
                self.shutdown();
                return Err(error);
            }
            Ok(None) => {}
        }
        if result.is_err() {
            self.shutdown();
        }
        result
    }
    fn acquire_storage(&mut self, directory: &std::path::Path) -> Result<()> {
        ensure!(
            directory.is_absolute(),
            "Agent storage directory must be absolute"
        );
        std::fs::create_dir_all(directory).context("Create private agent storage directory")?;
        // Inspect the leaf through a no-follow directory handle before changing
        // permissions or allowing the child to create its database and notes.
        let mut directory_options = std::fs::OpenOptions::new();
        directory_options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            directory_options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        }
        let directory_handle = directory_options
            .open(directory)
            .context("Agent storage must be a non-symlink directory owned by the current user")?;
        let metadata = directory_handle.metadata()?;
        ensure!(metadata.is_dir(), "Agent storage must be a directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            ensure!(
                metadata.uid() == unsafe { libc::geteuid() },
                "Agent storage must be owned by the current user"
            );
            directory_handle
                .set_permissions(std::fs::Permissions::from_mode(0o700))
                .context("Make agent storage private (0700)")?;
        }
        #[cfg(not(unix))]
        ensure!(
            !std::fs::symlink_metadata(directory)?
                .file_type()
                .is_symlink(),
            "Agent storage must not be a symbolic link"
        );
        let directory = directory.canonicalize()?;
        if let Some((current, _)) = &self.storage {
            ensure!(
                *current == directory,
                "Agent service cannot change workspace storage"
            );
            return Ok(());
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
        }
        let lock = options
            .open(directory.join(".host-service.lock"))
            .context("Open agent storage lock")?;
        lock.try_lock().map_err(|error| {
            anyhow::anyhow!("Workspace agent service already owns this storage: {error}")
        })?;
        self.storage = Some((directory, lock));
        Ok(())
    }

    fn invoke_inner(
        &mut self,
        request: AgentRequest,
        host: &mut impl FnMut(&str, Value) -> Result<Value>,
        progress: &mut impl FnMut(AgentEvent),
        cancel: &AtomicBool,
    ) -> Result<AgentResult> {
        ensure!(
            !request.request_id.is_empty() && request.request_id.len() <= 128,
            "Invalid agent request id"
        );
        ensure!(request.prompt.len() <= 16384, "Agent prompt exceeds 16 KiB");
        ensure!(
            request
                .memory_update
                .as_ref()
                .is_none_or(|v| v.len() <= 65536),
            "Workspace memory exceeds 64 KiB"
        );
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .context("Agent request sequence exhausted")?;
        let deadline = Instant::now() + self.timeout;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"agent.invoke","params":{"protocol_version":PROTOCOL_VERSION,"config":self.config,"request":request}}),deadline,cancel)?;
        let mut frames = 0;
        let mut stream_sequence = 0;
        let mut inbound_bytes = 0usize;
        loop {
            ensure!(!cancel.load(Ordering::Acquire), "Agent request cancelled");
            ensure!(Instant::now() < deadline, "Agent request timed out");
            let (frame, frame_bytes) = match self.reader.recv_timeout(Duration::from_millis(20)) {
                Ok(frame) => frame?,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => bail!("Agent plugin output closed"),
            };
            frames += 1;
            // A five-minute request may deliver coalesced 25 Hz snapshots.
            // Bound both frame count and cumulative bytes without retaining them.
            ensure!(frames <= 8192, "Agent exceeded frame budget");
            // Count every inbound wire frame exactly once, including callback
            // requests and the final response. Preserve whitespace/newline size
            // from the reader instead of charging only normalized JSON data.
            inbound_bytes = inbound_bytes.saturating_add(frame_bytes);
            ensure!(
                inbound_bytes <= 256 * 1024 * 1024,
                "Agent exceeded invocation byte budget"
            );
            ensure!(
                frame["jsonrpc"] == "2.0",
                "Agent frame has invalid JSON-RPC version"
            );
            if let Some(method) = frame.get("method").and_then(Value::as_str) {
                ensure!(
                    frame.get("result").is_none() && frame.get("error").is_none(),
                    "Agent request contains response fields"
                );
                if method == "agent.progress" {
                    ensure!(frame.get("id").is_none(), "Progress must be a notification");
                    let event: AgentEvent = serde_json::from_value(frame["params"].clone())
                        .context("Invalid agent progress")?;
                    ensure!(
                        event.request_id == request.request_id && event.message.len() <= 4096,
                        "Invalid agent progress context"
                    );
                    if let Some(update) = &event.update {
                        update.validate(stream_sequence)?;
                        stream_sequence = update.sequence;
                    }
                    progress(event);
                    continue;
                }
                ensure!(
                    frame_bytes <= MAX_REQUEST_BYTES,
                    "Agent host callback exceeds 1 MiB"
                );
                let callback_id = frame
                    .get("id")
                    .and_then(Value::as_str)
                    .context("Host callback requires a string id")?;
                ensure!(
                    !callback_id.is_empty() && callback_id.len() <= 128,
                    "Invalid callback id"
                );
                let response = match method {
                    "workspace.snapshot" | "files.list" | "files.read" | "files.search" => host(
                        method,
                        frame.get("params").cloned().unwrap_or_else(|| json!({})),
                    ),
                    "credentials.read"
                    | "credentials.acquire"
                    | "credentials.commit"
                    | "credentials.release"
                        if self.credentials_allowed =>
                    {
                        host(
                            method,
                            frame.get("params").cloned().unwrap_or_else(|| json!({})),
                        )
                    }
                    _ => Err(anyhow::anyhow!("Unknown host method {method}")),
                };
                let response = match response {
                    Ok(result) => json!({"jsonrpc":"2.0","id":callback_id,"result":result}),
                    Err(error) => {
                        json!({"jsonrpc":"2.0","id":callback_id,"error":{"code":-32000,"message":format!("{error:#}")}})
                    }
                };
                self.send(response, deadline, cancel)?;
                continue;
            }
            ensure!(
                frame["id"] == id,
                "Agent response id does not match request"
            );
            ensure!(
                frame.get("result").is_some() != frame.get("error").is_some(),
                "Agent response needs exactly one result or error"
            );
            if let Some(error) = frame.get("error") {
                bail!(
                    "Agent plugin: {}",
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                );
            }
            return serde_json::from_value(frame["result"].clone()).context("Invalid agent result");
        }
    }
}
impl Drop for AgentService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn write_cancel_marker(directory: &std::path::Path, request_id: &str) -> Result<()> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    request_id.hash(&mut hash);
    let folder = directory.join("host-cancelled-requests");
    std::fs::create_dir_all(&folder)?;
    let path = folder.join(format!("{:016x}.json", hash.finish()));
    let temporary = folder.join(format!(
        ".{:016x}-{}.tmp",
        hash.finish(),
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| -> Result<()> {
        file.write_all(&serde_json::to_vec(&json!({"request_id":request_id}))?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        std::fs::File::open(&folder)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn graceful_cancel(pid: u32, writer: &SyncSender<Vec<u8>>, request_id: &str) {
    if let Ok(mut bytes) = serde_json::to_vec(
        &json!({"jsonrpc":"2.0","method":"agent.cancel","params":{"request_id":request_id}}),
    ) {
        bytes.push(b'\n');
        let _ = writer.try_send(bytes);
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGTERM);
    }
    thread::sleep(Duration::from_secs(2));
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}
