//! A small, versioned JSON-RPC plugin host. Plugins run as separate processes and
//! may be written in any language. The host is not an operating-system sandbox:
//! install only trusted executables. Manifest permissions constrain host dispatch.

use anyhow::{Context, Result, bail, ensure};
mod agent;
pub use agent::*;
mod accounts;
pub use accounts::*;
mod commands;
pub use commands::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_REQUEST_BYTES: usize = 1_048_576;
pub const MAX_RESPONSE_BYTES: usize = 8_388_608;
const MAX_STDERR_BYTES: usize = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Agent,
    Accounts,
    Search,
    Provider,
    Commands,
    Decorations,
    Terminal,
}

/// These are host-dispatch grants, not an OS sandbox. A plugin can access its
/// ambient process permissions; never install an untrusted executable.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Permissions {
    #[serde(default)]
    pub read_roots: Vec<PathBuf>,
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub terminal: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginOverrides {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "empty_object")]
    pub config: Value,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}
fn empty_object() -> Value {
    json!({})
}
impl Default for PluginOverrides {
    fn default() -> Self {
        Self {
            enabled: None,
            config: empty_object(),
            extra: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKind {
    Bool,
    Text,
    Number,
    Select,
}

/// Optional UI hints for top-level plugin configuration keys. Unknown keys and
/// nested advanced configuration remain supported without a schema engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingField {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub kind: SettingKind,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub protocol_version: u32,
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub config: Value,
    #[serde(default)]
    pub settings_fields: Vec<SettingField>,
    #[serde(skip)]
    pub default_config: Value,
    #[serde(skip)]
    pub enabled: bool,
    #[serde(skip)]
    pub directory: PathBuf,
}

fn default_timeout() -> u64 {
    10_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRoot {
    pub path: String,
    #[serde(default = "yes")]
    pub respect_gitignore: bool,
    #[serde(default)]
    pub follow_symlinks: bool,
}
fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Filename,
    Content,
    Semantic,
    Hybrid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub roots: Vec<IndexRoot>,
    pub limit: usize,
    #[serde(default)]
    pub mode: SearchMode,
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub path: String,
    pub score: f64,
    #[serde(default)]
    pub line: Option<u64>,
    #[serde(default)]
    pub snippet: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandDescriptor {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub shortcut: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_line: Option<CommandLineSpec>,
}

/// A host-owned PTY request. Each argument stays an argument; the host must not
/// convert this structure to a shell command string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalLaunchRequest {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub title: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Legacy plugins may return arbitrary JSON. Only an explicit host_action
/// envelope requests terminal behavior and receives additional host checks.
#[derive(Debug, Clone)]
pub enum CommandResult {
    Value(Value),
    Terminal(TerminalLaunchRequest),
    Agent(AgentLaunchRequest),
}

/// Generic row metadata: filesystem strings remain owned by the application and
/// plugins cannot decorate a path the application did not explicitly request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecorationRequest {
    pub location: String,
    pub entries: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecorationTone {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decoration {
    pub path: String,
    pub badge: String,
    pub tooltip: String,
    #[serde(default)]
    pub tone: DecorationTone,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    pub params: Value,
}

/// Error returned by a Rust SDK handler. Standard JSON-RPC codes are accepted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct PluginRegistry {
    plugins: Vec<PluginManifest>,
}

impl PluginManifest {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let metadata = fs::metadata(path)
            .with_context(|| format!("Read plugin manifest {}", path.display()))?;
        ensure!(
            metadata.len() <= MAX_REQUEST_BYTES as u64,
            "Plugin manifest is too large"
        );
        let mut manifest: Self = serde_json::from_slice(&fs::read(path)?)
            .with_context(|| format!("Invalid plugin manifest {}", path.display()))?;
        ensure!(
            !manifest.id.is_empty()
                && manifest
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')),
            "Plugin id must contain only letters, numbers, '.', '-' or '_'"
        );
        ensure!(
            manifest.protocol_version == PROTOCOL_VERSION,
            "Plugin {} requires protocol {}, host supports {}",
            manifest.id,
            manifest.protocol_version,
            PROTOCOL_VERSION
        );
        ensure!(
            !manifest.executable.is_empty(),
            "Plugin executable is empty"
        );
        ensure!(
            (1..=300_000).contains(&manifest.timeout_ms),
            "Plugin timeout must be 1..300000 ms"
        );
        manifest.directory = path.parent().unwrap_or(Path::new(".")).canonicalize()?;
        for root in &mut manifest.permissions.read_roots {
            if root.is_relative() {
                *root = manifest.directory.join(&*root);
            }
            *root = root.canonicalize().with_context(|| {
                format!("Plugin permission root {} does not exist", root.display())
            })?;
        }
        manifest.enabled = true;
        manifest.default_config = manifest.config.clone();
        manifest.validate_settings()?;
        Ok(manifest)
    }

    pub fn supports(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    pub fn validate_settings(&self) -> Result<()> {
        ensure!(
            self.settings_fields.len() <= 128,
            "Plugin {} exposes more than 128 settings",
            self.id
        );
        let mut seen = BTreeSet::new();
        for field in &self.settings_fields {
            ensure!(
                !field.key.is_empty()
                    && field.key.len() <= 128
                    && field
                        .key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
                    && seen.insert(&field.key),
                "Plugin {} has an invalid or duplicate setting key",
                self.id
            );
            ensure!(
                !field.title.is_empty()
                    && field.title.chars().count() <= 128
                    && field.description.chars().count() <= 1024,
                "Plugin {} has invalid setting labels",
                self.id
            );
            ensure!(
                field.min.is_none_or(f64::is_finite)
                    && field.max.is_none_or(f64::is_finite)
                    && !matches!((field.min,field.max),(Some(a),Some(b)) if a>b),
                "Plugin {} setting {} has invalid bounds",
                self.id,
                field.key
            );
            if field.kind == SettingKind::Select {
                ensure!(
                    !field.options.is_empty()
                        && field.options.len() <= 128
                        && field
                            .options
                            .iter()
                            .all(|option| option.len() <= 4096 && !option.contains('\0')),
                    "Plugin {} setting {} has invalid select options",
                    self.id,
                    field.key
                );
            }
            let Some(value) = self.config.get(&field.key) else {
                continue;
            };
            let valid = match field.kind {
                SettingKind::Bool => value.is_boolean(),
                SettingKind::Text => value
                    .as_str()
                    .is_some_and(|text| text.len() <= 32768 && !text.contains('\0')),
                SettingKind::Number => value.as_f64().is_some_and(|n| {
                    n.is_finite()
                        && field.min.is_none_or(|min| n >= min)
                        && field.max.is_none_or(|max| n <= max)
                }),
                SettingKind::Select => value
                    .as_str()
                    .is_some_and(|text| field.options.iter().any(|option| option == text)),
            };
            ensure!(
                valid,
                "Plugin {} setting {} has an invalid value",
                self.id,
                field.key
            );
        }
        Ok(())
    }
}

impl PluginRegistry {
    pub fn discover(directory: impl AsRef<Path>) -> Result<Self> {
        Self::discover_with_settings(directory, &BTreeMap::new())
    }

    pub fn discover_with_settings_file(
        directory: impl AsRef<Path>,
        settings_path: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::discover_with_settings(directory, &read_plugin_overrides(settings_path)?)
    }

    pub fn discover_with_settings(
        directory: impl AsRef<Path>,
        overrides: &BTreeMap<String, PluginOverrides>,
    ) -> Result<Self> {
        let directory = directory.as_ref();
        if !directory.exists() {
            return Ok(Self::default());
        }
        let mut paths = Vec::new();
        if directory.join("plugin.json").is_file() {
            paths.push(directory.join("plugin.json"));
        }
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() && entry.path().join("plugin.json").is_file() {
                paths.push(entry.path().join("plugin.json"));
            }
        }
        paths.sort();
        let mut by_id = BTreeMap::new();
        for path in paths {
            let mut manifest = PluginManifest::load(path)?;
            if let Some(settings) = overrides.get(&manifest.id) {
                ensure!(
                    settings.config.is_object(),
                    "Plugin {} config overrides must be an object",
                    manifest.id
                );
                manifest.enabled = settings.enabled.unwrap_or(true);
                if !settings.config.is_null() {
                    merge_config(&mut manifest.config, &settings.config);
                }
                manifest.validate_settings()?;
            }
            ensure!(
                !by_id.contains_key(&manifest.id),
                "Duplicate plugin id {}",
                manifest.id
            );
            by_id.insert(manifest.id.clone(), manifest);
        }
        Ok(Self {
            plugins: by_id.into_values().collect(),
        })
    }

    pub fn plugins(&self) -> &[PluginManifest] {
        &self.plugins
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.plugins
            .iter()
            .find(|plugin| plugin.id == id)
            .is_some_and(|plugin| plugin.enabled)
    }

    /// Reconfigure an already discovered catalog without starting any process.
    /// Each call starts from manifest defaults, so removing an override resets it.
    pub fn with_settings(&self, overrides: &BTreeMap<String, PluginOverrides>) -> Result<Self> {
        let mut registry = self.clone();
        for plugin in &mut registry.plugins {
            plugin.config = plugin.default_config.clone();
            plugin.enabled = true;
            if let Some(settings) = overrides.get(&plugin.id) {
                ensure!(
                    settings.config.is_object(),
                    "Plugin {} config overrides must be an object",
                    plugin.id
                );
                plugin.enabled = settings.enabled.unwrap_or(true);
                if !settings.config.is_null() {
                    merge_config(&mut plugin.config, &settings.config);
                }
            }
            plugin.validate_settings()?;
        }
        Ok(registry)
    }

    pub fn plugin(&self, id: &str) -> Result<&PluginManifest> {
        self.plugins
            .iter()
            .find(|p| p.id == id)
            .with_context(|| format!("Plugin {id} is not installed"))
    }

    pub fn search(&self, id: &str, mut request: SearchRequest) -> Result<Vec<SearchResult>> {
        let plugin = self.require(id, Capability::Search)?;
        ensure!(
            request.limit > 0 && request.limit <= 10_000,
            "Search limit must be 1..10000"
        );
        ensure!(
            !request.roots.is_empty(),
            "Search requires at least one root"
        );
        for root in &mut request.roots {
            root.path = authorized_path(plugin, &root.path)?
                .to_string_lossy()
                .into_owned();
        }
        let result = self.call(plugin, "search", serde_json::to_value(&request)?)?;
        let mut hits: Vec<SearchResult> =
            serde_json::from_value(result).context("Plugin returned invalid search results")?;
        ensure!(
            hits.len() <= request.limit,
            "Plugin exceeded requested result limit"
        );
        for hit in &mut hits {
            ensure!(hit.score.is_finite(), "Plugin returned non-finite score");
            ensure!(hit.line != Some(0), "Plugin result line numbers start at 1");
            let path = Path::new(&hit.path)
                .canonicalize()
                .with_context(|| format!("Plugin result does not exist: {}", hit.path))?;
            ensure!(
                request.roots.iter().any(|r| path.starts_with(&r.path)),
                "Plugin returned a result outside requested roots: {}",
                hit.path
            );
            hit.path = path.to_string_lossy().into_owned();
        }
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(a.path.cmp(&b.path))
                .then(a.line.cmp(&b.line))
        });
        Ok(hits)
    }

    pub fn commands(&self, id: &str) -> Result<Vec<CommandDescriptor>> {
        let plugin = self.require(id, Capability::Commands)?;
        let commands: Vec<CommandDescriptor> =
            serde_json::from_value(self.call(plugin, "commands.list", Value::Null)?)
                .context("Plugin returned invalid command descriptors")?;
        ensure!(
            commands.len() <= 256,
            "Plugin returned more than 256 commands"
        );
        let mut seen = BTreeSet::new();
        let mut cli_names = BTreeSet::new();
        for command in &commands {
            command.validate()?;
            ensure!(
                seen.insert(&command.id),
                "Duplicate plugin command id {}",
                command.id
            );
            if let Some(spec) = &command.command_line {
                for name in std::iter::once(&spec.name).chain(&spec.aliases) {
                    ensure!(
                        cli_names.insert(name),
                        "Duplicate command-line name or alias {name}"
                    );
                }
            }
        }
        Ok(commands)
    }

    pub fn decorate(&self, id: &str, request: DecorationRequest) -> Result<Vec<Decoration>> {
        let plugin = self.require(id, Capability::Decorations)?;
        ensure!(
            request.entries.len() <= 10_000,
            "Decoration request exceeds 10000 entries"
        );
        let requested: BTreeSet<_> = request.entries.iter().cloned().collect();
        let result = self.call(plugin, "decorations", serde_json::to_value(request)?)?;
        let mut decorations: Vec<Decoration> =
            serde_json::from_value(result).context("Plugin returned invalid row decorations")?;
        ensure!(
            decorations.len() <= requested.len(),
            "Plugin returned too many decorations"
        );
        let mut seen = BTreeSet::new();
        for decoration in &decorations {
            ensure!(
                requested.contains(&decoration.path),
                "Plugin decorated an unrequested path: {}",
                decoration.path
            );
            ensure!(
                seen.insert(decoration.path.clone()),
                "Plugin returned duplicate decorations for {}",
                decoration.path
            );
            ensure!(
                !decoration.badge.is_empty() && decoration.badge.chars().count() <= 8,
                "Decoration badges must contain 1..8 characters"
            );
            ensure!(
                decoration.tooltip.chars().count() <= 512,
                "Decoration tooltip exceeds 512 characters"
            );
            ensure!(
                !decoration.badge.chars().any(char::is_control)
                    && !decoration
                        .tooltip
                        .chars()
                        .any(|c| c.is_control() && c != '\n'),
                "Decoration contains invalid control characters"
            );
        }
        decorations.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(decorations)
    }

    pub fn invoke_command(&self, id: &str, command: &str, arguments: Value) -> Result<Value> {
        let plugin = self.require(id, Capability::Commands)?;
        let value = self.call(
            plugin,
            "commands.invoke",
            json!({"command": command, "arguments": arguments}),
        )?;
        self.command_result(plugin, value.clone())?;
        Ok(value)
    }

    pub fn invoke_command_result(
        &self,
        id: &str,
        command: &str,
        arguments: Value,
    ) -> Result<CommandResult> {
        let plugin = self.require(id, Capability::Commands)?;
        let value = self.call(
            plugin,
            "commands.invoke",
            json!({"command":command,"arguments":arguments}),
        )?;
        self.command_result(plugin, value)
    }

    /// Cancellation stops this invocation process, but cannot undo plugin-owned
    /// effects already performed. Callers must also reject stale UI completions.
    pub fn invoke_command_result_cancellable(
        &self,
        id: &str,
        command: &str,
        arguments: Value,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<CommandResult> {
        let plugin = self.require(id, Capability::Commands)?;
        let request = RpcRequest {
            jsonrpc: "2.0".into(),
            id: 1,
            method: "commands.invoke".into(),
            params: json!({"request":{"command":command,"arguments":arguments},"config":plugin.config,"protocol_version":PROTOCOL_VERSION}),
        };
        let value = run_plugin_cancellable(plugin, &request, Some(cancelled))
            .with_context(|| format!("Plugin {} (commands.invoke)", plugin.id))?;
        ensure!(
            !cancelled.load(std::sync::atomic::Ordering::Acquire),
            "Plugin command cancelled"
        );
        self.command_result(plugin, value)
    }

    /// Dispatch a cached, validated descriptor with a frozen pane snapshot.
    /// No commands.list request or shell interpolation occurs during dispatch.
    pub fn invoke_command_line_result(
        &self,
        id: &str,
        descriptor: &CommandDescriptor,
        context: CommandContext,
        user_args: Vec<String>,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<CommandResult> {
        self.require(id, Capability::Commands)?;
        descriptor.validate()?;
        let spec = descriptor
            .command_line
            .as_ref()
            .context("Command has no command-line registration")?;
        context.validate()?;
        spec.check_location(&context.location)?;
        spec.validate_arguments(&user_args)?;
        self.invoke_command_result_cancellable(
            id,
            &descriptor.id,
            json!({"location":context.location,"selected":context.selected,"user_args":user_args}),
            cancelled,
        )
    }

    fn command_result(&self, plugin: &PluginManifest, value: Value) -> Result<CommandResult> {
        let Some(action) = value.get("host_action") else {
            return Ok(CommandResult::Value(value));
        };
        if action.as_str() == Some("agent") {
            ensure!(
                plugin.supports(Capability::Agent),
                "Plugin {} did not declare Agent capability",
                plugin.id
            );
            let request: AgentLaunchRequest = serde_json::from_value(
                value
                    .get("request")
                    .cloned()
                    .context("Agent host action is missing request")?,
            )
            .context("Invalid agent launch request")?;
            ensure!(request.prompt.len() <= 16_384, "Agent prompt is too long");
            return Ok(CommandResult::Agent(request));
        }
        ensure!(
            action.as_str() == Some("terminal"),
            "Unknown plugin host action"
        );
        ensure!(
            plugin.supports(Capability::Terminal),
            "Plugin {} did not declare the Terminal capability",
            plugin.id
        );
        ensure!(
            plugin.permissions.terminal,
            "Plugin {} has no terminal permission",
            plugin.id
        );
        let request: TerminalLaunchRequest = serde_json::from_value(
            value
                .get("request")
                .cloned()
                .context("Terminal host action is missing request")?,
        )
        .context("Invalid terminal launch request")?;
        validate_terminal_request(plugin, &request)?;
        Ok(CommandResult::Terminal(request))
    }

    /// Provider method names are intentionally closed. Mutating methods require
    /// an explicit write grant; local paths also obey the read-root grants.
    pub fn provider_request(&self, id: &str, method: &str, params: Value) -> Result<Value> {
        let plugin = self.require(id, Capability::Provider)?;
        ensure!(
            [
                "list", "stat", "read", "write", "mkdir", "copy", "move", "remove"
            ]
            .contains(&method),
            "Unknown provider method {method}"
        );
        ensure!(
            plugin.permissions.network || !plugin.permissions.read_roots.is_empty(),
            "Provider plugin {id} requires a read root or network permission"
        );
        if ["write", "mkdir", "copy", "move", "remove"].contains(&method) {
            ensure!(
                plugin.permissions.write,
                "Provider plugin {id} has no write permission"
            );
        }
        self.call(plugin, &format!("provider.{method}"), params)
    }

    fn require(&self, id: &str, capability: Capability) -> Result<&PluginManifest> {
        let plugin = self.plugin(id)?;
        ensure!(plugin.enabled, "Plugin {id} is disabled in settings");
        ensure!(
            plugin.supports(capability),
            "Plugin {id} did not declare the {capability:?} capability"
        );
        Ok(plugin)
    }

    fn call(&self, plugin: &PluginManifest, method: &str, params: Value) -> Result<Value> {
        let request = RpcRequest {
            jsonrpc: "2.0".into(),
            id: 1,
            method: method.into(),
            params: json!({"request": params, "config": plugin.config, "protocol_version": PROTOCOL_VERSION}),
        };
        run_plugin(plugin, &request).with_context(|| format!("Plugin {} ({method})", plugin.id))
    }
}

/// Read only the generic plugin section of the application's strict JSON file.
/// Plain registry discovery remains isolated from ambient user configuration.
pub fn read_plugin_overrides(path: impl AsRef<Path>) -> Result<BTreeMap<String, PluginOverrides>> {
    let path = path.as_ref();
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("Read plugin settings {}", path.display()));
        }
    };
    let mut bytes = Vec::new();
    file.take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_REQUEST_BYTES,
        "Plugin settings file exceeds 1 MiB"
    );
    let value: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("Invalid settings.json at {}", path.display()))?;
    ensure!(value.is_object(), "settings.json must contain an object");
    let Some(plugins) = value.get("plugins") else {
        return Ok(BTreeMap::new());
    };
    let overrides: BTreeMap<String, PluginOverrides> = serde_json::from_value(plugins.clone())
        .context("settings.json plugins must be an object of plugin settings")?;
    ensure!(
        overrides.len() <= 1024,
        "Plugin settings exceed 1024 entries"
    );
    for (id, settings) in &overrides {
        ensure!(
            !id.is_empty()
                && id.len() <= 128
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)),
            "Invalid plugin settings id {id}"
        );
        ensure!(
            settings.config.is_object(),
            "Plugin {id} config must be an object"
        );
    }
    Ok(overrides)
}

fn merge_config(base: &mut Value, overrides: &Value) {
    if let (Some(base), Some(overrides)) = (base.as_object_mut(), overrides.as_object()) {
        for (key, value) in overrides {
            if let Some(existing) = base.get_mut(key) {
                merge_config(existing, value);
            } else {
                base.insert(key.clone(), value.clone());
            }
        }
    } else {
        *base = overrides.clone();
    }
}

fn validate_terminal_request(
    plugin: &PluginManifest,
    request: &TerminalLaunchRequest,
) -> Result<()> {
    ensure!(
        !request.program.is_empty()
            && request.program.len() <= 4096
            && !request.program.contains('\0'),
        "Invalid terminal program"
    );
    let program = Path::new(&request.program);
    ensure!(
        program.is_absolute() && program.is_file(),
        "Terminal program must be an existing absolute file path"
    );
    ensure!(
        request.cwd.is_absolute() && request.cwd.is_dir(),
        "Terminal cwd must be an existing absolute directory"
    );
    authorized_path(plugin, &request.cwd.to_string_lossy())?;
    ensure!(
        request.args.len() <= 1024
            && request
                .args
                .iter()
                .all(|arg| arg.len() <= 32768 && !arg.contains('\0')),
        "Terminal arguments exceed limits or contain NUL"
    );
    ensure!(
        !request.title.is_empty()
            && request.title.chars().count() <= 128
            && !request.title.chars().any(char::is_control),
        "Invalid terminal title"
    );
    ensure!(
        request.env.len() <= 64,
        "Terminal environment exceeds 64 variables"
    );
    for (key, value) in &request.env {
        ensure!(
            !key.is_empty()
                && key.len() <= 128
                && !key.as_bytes()[0].is_ascii_digit()
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "Invalid terminal environment name"
        );
        ensure!(
            value.len() <= 32768 && !value.contains('\0'),
            "Invalid terminal environment value"
        );
    }
    let bytes: usize = request.args.iter().map(String::len).sum::<usize>()
        + request
            .env
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>();
    ensure!(
        bytes <= 131072,
        "Terminal arguments and environment exceed 128 KiB"
    );
    Ok(())
}

fn authorized_path(plugin: &PluginManifest, path: &str) -> Result<PathBuf> {
    let path = Path::new(path)
        .canonicalize()
        .with_context(|| format!("Search root does not exist: {path}"))?;
    ensure!(
        plugin
            .permissions
            .read_roots
            .iter()
            .any(|root| path.starts_with(root)),
        "Plugin {} has no read grant for {}",
        plugin.id,
        path.display()
    );
    Ok(path)
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    // The process was placed in a new group before spawn. Killing that group
    // also closes pipes inherited by subprocesses that remain after a timeout.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn run_plugin(plugin: &PluginManifest, request: &RpcRequest) -> Result<Value> {
    run_plugin_cancellable(plugin, request, None)
}

fn run_plugin_cancellable(
    plugin: &PluginManifest,
    request: &RpcRequest,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Value> {
    let is_cancelled =
        || cancelled.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire));
    ensure!(!is_cancelled(), "Plugin command cancelled");
    let mut input = serde_json::to_vec(request)?;
    ensure!(
        input.len() < MAX_REQUEST_BYTES,
        "Plugin request exceeds 1 MiB"
    );
    input.push(b'\n');
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
    let mut child = command
        .spawn()
        .context("Could not start plugin executable")?;
    let mut stdin = child.stdin.take().context("Plugin stdin unavailable")?;
    thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let stdout = child.stdout.take().context("Plugin stdout unavailable")?;
    let stderr = child.stderr.take().context("Plugin stderr unavailable")?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut output = Vec::new();
        let result = stdout
            .take((MAX_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut output);
        let _ = tx.send(result.map(|_| output));
    });
    let (err_tx, err_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut stderr = stderr;
        let mut captured = Vec::new();
        let mut chunk = [0; 4096];
        while let Ok(n) = stderr.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let remaining = MAX_STDERR_BYTES.saturating_sub(captured.len());
            captured.extend_from_slice(&chunk[..n.min(remaining)]);
        }
        let _ = err_tx.send(captured);
    });
    let deadline = Instant::now() + Duration::from_millis(plugin.timeout_ms);
    let mut output = None;
    let status = loop {
        if is_cancelled() {
            terminate(&mut child);
            bail!("Plugin command cancelled");
        }
        if let Ok(data) = rx.try_recv() {
            let data = match data {
                Ok(data) => data,
                Err(error) => {
                    terminate(&mut child);
                    return Err(error.into());
                }
            };
            if data.len() > MAX_RESPONSE_BYTES {
                terminate(&mut child);
                bail!("Plugin response exceeds 8 MiB");
            }
            output = Some(data);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                terminate(&mut child);
                return Err(error.into());
            }
        }
        if Instant::now() >= deadline {
            terminate(&mut child);
            bail!("Plugin timed out after {} ms", plugin.timeout_ms);
        }
        thread::sleep(Duration::from_millis(5));
    };
    terminate(&mut child);
    ensure!(!is_cancelled(), "Plugin command cancelled");
    let output = match output {
        Some(output) => output,
        None => rx
            .recv_timeout(Duration::from_millis(200))
            .context("Plugin output pipe did not close")??,
    };
    ensure!(
        output.len() <= MAX_RESPONSE_BYTES,
        "Plugin response exceeds 8 MiB"
    );
    if !status.success() {
        let stderr = err_rx
            .recv_timeout(Duration::from_millis(200))
            .unwrap_or_default();
        bail!(
            "Plugin exited with {status}: {}",
            String::from_utf8_lossy(&stderr).trim()
        );
    }
    let text = std::str::from_utf8(&output).context("Plugin output is not UTF-8")?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let line = lines
        .next()
        .context("Plugin returned no JSON-RPC response")?;
    ensure!(
        lines.next().is_none(),
        "Plugin must return exactly one JSON-RPC response; use stderr for logs"
    );
    let response: Value = serde_json::from_str(line).context("Plugin returned malformed JSON")?;
    ensure!(
        response.get("jsonrpc") == Some(&json!("2.0")),
        "Plugin response has invalid jsonrpc version"
    );
    ensure!(
        response.get("id") == Some(&json!(request.id)),
        "Plugin response id does not match request"
    );
    ensure!(
        response.get("result").is_some() != response.get("error").is_some(),
        "Plugin response must contain exactly one of result or error"
    );
    if let Some(error) = response.get("error") {
        let error: RpcError =
            serde_json::from_value(error.clone()).context("Invalid JSON-RPC error")?;
        bail!("{} (code {})", error.message, error.code);
    }
    Ok(response["result"].clone())
}

/// Serve exactly one request and exit. The handler sees the complete params
/// envelope: `request`, `config` and `protocol_version`. Logs belong on stderr.
pub fn serve_once(
    handler: impl FnOnce(&str, Value) -> std::result::Result<Value, RpcError>,
) -> Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .lock()
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_line(&mut input)?;
    ensure!(input.len() <= MAX_REQUEST_BYTES, "Request is too large");
    let request: RpcRequest = serde_json::from_str(&input).context("Invalid JSON-RPC request")?;
    ensure!(request.jsonrpc == "2.0", "Unsupported JSON-RPC version");
    let response = match handler(&request.method, request.params) {
        Ok(result) => json!({"jsonrpc":"2.0", "id":request.id, "result":result}),
        Err(error) => json!({"jsonrpc":"2.0", "id":request.id, "error":error}),
    };
    let output = serde_json::to_vec(&response)?;
    ensure!(output.len() <= MAX_RESPONSE_BYTES, "Response is too large");
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&output)?;
    stdout.write_all(b"\n")?;
    Ok(())
}
