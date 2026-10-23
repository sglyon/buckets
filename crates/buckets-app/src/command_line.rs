//! Typed command text, pure discovery/path planning and explicit command history.
use anyhow::{Context, Result, bail, ensure};
use buckets_core::Location;
use buckets_plugin::ExclusiveFileLock;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_LINE: usize = 4096;
const MAX_HISTORY: usize = 100;
const MAX_HISTORY_BYTES: usize = 262_144;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellPlan {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    App { command: String, args: Vec<String> },
    Shell { source: String, plan: ShellPlan },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: String,
    pub aliases: Vec<String>,
    pub usage: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub value: String,
    pub label: String,
    pub detail: String,
}

fn validate_line(input: &str) -> Result<()> {
    ensure!(input.len() <= MAX_LINE, "Command exceeds 4096 bytes");
    ensure!(
        !input.chars().any(|c| c.is_control() && c != '\t'),
        "Commands must be one line without control characters"
    );
    Ok(())
}

fn body(input: &str) -> &str {
    let input = input.trim_start();
    input.strip_prefix(':').unwrap_or(input)
}

pub fn parse(input: &str, active: &Location) -> Result<Invocation> {
    validate_line(input)?;
    let input = body(input);
    if let Some(source) = input.strip_prefix('!') {
        ensure!(!source.trim().is_empty(), "Enter a shell command after :!");
        return Ok(Invocation::Shell {
            source: source.into(),
            plan: shell_plan(active, source)?,
        });
    }
    let mut words = words(input)?.into_iter();
    let command = words.next().context("Enter an application command")?;
    ensure!(!command.is_empty(), "Command name must not be empty");
    Ok(Invocation::App {
        command,
        args: words.collect(),
    })
}

/// Argument quoting is shell-like text parsing only: no expansion, substitution
/// or execution. Single quotes are literal; backslash escapes outside them.
fn words(input: &str) -> Result<Vec<String>> {
    Ok(lex(input, false)?
        .0
        .into_iter()
        .map(|token| token.value)
        .collect())
}

struct Token {
    value: String,
    start: usize,
}

fn lex(input: &str, unfinished: bool) -> Result<(Vec<Token>, bool)> {
    let mut result = vec![];
    let mut word = String::new();
    let mut started = false;
    let mut start = 0;
    let mut quote = None;
    let mut chars = input.char_indices();
    while let Some((offset, ch)) = chars.next() {
        if !started && !(ch.is_whitespace() && quote.is_none()) {
            start = offset;
        }
        match ch {
            '\\' if quote != Some('\'') => {
                let (_, escaped) = chars
                    .next()
                    .context("Command ends with an incomplete escape")?;
                if quote == Some('"') && !matches!(escaped, '"' | '\\' | '$' | '`') {
                    word.push('\\');
                }
                word.push(escaped);
                started = true;
            }
            '\'' | '"' if quote == Some(ch) => quote = None,
            '\'' | '"' if quote.is_none() => {
                quote = Some(ch);
                started = true;
            }
            ch if ch.is_whitespace() && quote.is_none() => {
                if started {
                    result.push(Token {
                        value: std::mem::take(&mut word),
                        start,
                    });
                    started = false;
                }
            }
            ch => {
                word.push(ch);
                started = true;
            }
        }
    }
    ensure!(
        unfinished || quote.is_none(),
        "Command has an unmatched quote"
    );
    if started {
        result.push(Token { value: word, start });
    }
    Ok((result, started))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentCompletion {
    pub command: String,
    pub index: usize,
    pub prefix: String,
    pub replace_from: usize,
}

/// Cursor-at-end argument projection. Incomplete final quotes are allowed only
/// for discovery; execution still uses the strict parser above.
pub fn argument_completion(input: &str) -> Option<ArgumentCompletion> {
    validate_line(input).ok()?;
    let command_body = body(input);
    if command_body.starts_with('!') {
        return None;
    }
    let (tokens, open) = lex(command_body, true).ok()?;
    let command = tokens.first()?.value.clone();
    if command.is_empty() || tokens.len() == 1 && open {
        return None;
    }
    let (index, prefix, replace_from) = if open {
        let last = tokens.last()?;
        (
            tokens.len() - 2,
            last.value.clone(),
            input.len() - command_body.len() + last.start,
        )
    } else {
        (tokens.len() - 1, String::new(), input.len())
    };
    Some(ArgumentCompletion {
        command,
        index,
        prefix,
        replace_from,
    })
}

pub fn builtins() -> Vec<CommandSpec> {
    [
        ("split", "split [directory]", "Split the active pane"),
        ("cd", "cd <directory>", "Navigate the active pane"),
        ("rename", "rename <name>", "Rename the selected file"),
        ("settings", "settings", "Open Settings"),
    ]
    .into_iter()
    .map(|(name, usage, description)| CommandSpec {
        name: name.into(),
        aliases: vec![],
        usage: usage.into(),
        description: description.into(),
    })
    .collect()
}

pub fn resolve_command<'a>(token: &str, commands: &'a [CommandSpec]) -> Result<&'a CommandSpec> {
    let primary = commands
        .iter()
        .filter(|spec| spec.name == token)
        .collect::<Vec<_>>();
    if primary.len() == 1 {
        return Ok(primary[0]);
    }
    ensure!(primary.is_empty(), "Duplicate command name {token}");
    let aliases = commands
        .iter()
        .filter(|spec| spec.aliases.iter().any(|alias| alias == token))
        .collect::<Vec<_>>();
    ensure!(
        aliases.len() <= 1,
        "Ambiguous command {token}; use its qualified plugin:<plugin>/<command> name"
    );
    aliases
        .first()
        .copied()
        .with_context(|| format!("Unknown command {token}"))
}

/// Completion never parses arguments, performs I/O or invokes a plugin. The UI
/// may separately project cached provider entries for path arguments.
pub fn completion(input: &str, commands: &[CommandSpec]) -> Vec<Completion> {
    if validate_line(input).is_err() {
        return vec![];
    }
    let prefix = body(input);
    if prefix.starts_with('!') || prefix.chars().any(char::is_whitespace) {
        return vec![];
    }
    let colon = if input.trim_start().starts_with(':') {
        ":"
    } else {
        ""
    };
    let mut seen = HashSet::new();
    commands
        .iter()
        .filter_map(|spec| {
            let matches_prefix = spec.name.starts_with(prefix)
                || spec.aliases.iter().any(|name| name.starts_with(prefix));
            if !matches_prefix {
                return None;
            }
            let name = spec
                .aliases
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(spec.name.as_str()))
                .find(|name| {
                    name.starts_with(prefix)
                        && resolve_command(name, commands)
                            .is_ok_and(|resolved| std::ptr::eq(resolved, spec))
                })
                .or_else(|| {
                    resolve_command(&spec.name, commands)
                        .ok()
                        .filter(|resolved| std::ptr::eq(*resolved, spec))
                        .map(|_| spec.name.as_str())
                })?;
            if !seen.insert(name.to_owned()) {
                return None;
            }
            Some(Completion {
                value: format!("{colon}{name} "),
                label: name.into(),
                detail: format!("{} · {}", spec.usage, spec.description),
            })
        })
        .take(12)
        .collect()
}

/// Resolve a path within the active provider, never against process cwd.
pub fn resolve_location(active: &Location, argument: &str) -> Result<Location> {
    validate_line(argument)?;
    ensure!(!argument.is_empty(), "Enter a directory");
    if argument.contains("://") {
        return Location::parse(argument);
    }
    if argument.starts_with('~') {
        ensure!(
            matches!(active, Location::Local { .. }),
            "Remote ~ cannot use your local home; enter an absolute remote directory"
        );
        ensure!(
            argument == "~" || argument.starts_with("~/"),
            "Use ~ or ~/ for your local home, or an explicit directory"
        );
        return Location::parse(argument);
    }
    Ok(match active {
        Location::Local { path } => {
            ensure!(
                path.is_absolute(),
                "Active local directory must be absolute"
            );
            let joined = path.join(argument);
            let mut normalized = PathBuf::new();
            for component in joined.components() {
                match component {
                    Component::CurDir => {}
                    Component::ParentDir => {
                        normalized.pop();
                    }
                    component => normalized.push(component.as_os_str()),
                }
            }
            Location::local(normalized)
        }
        Location::Ssh { host, path } => Location::Ssh {
            host: host.clone(),
            path: provider_path(path, argument, true),
        },
        Location::S3 {
            bucket,
            prefix,
            endpoint,
        } => Location::S3 {
            bucket: bucket.clone(),
            prefix: provider_path(prefix, argument, false),
            endpoint: endpoint.clone(),
        },
        Location::Git {
            repo,
            reference,
            path,
        } => Location::Git {
            repo: repo.clone(),
            reference: reference.clone(),
            path: provider_path(path, argument, false),
        },
        Location::Plugin { plugin, path } => Location::Plugin {
            plugin: plugin.clone(),
            path: provider_path(path, argument, true),
        },
    })
}

fn provider_path(base: &str, argument: &str, absolute: bool) -> String {
    let joined = if argument.starts_with('/') {
        argument.into()
    } else {
        format!("{base}/{argument}")
    };
    let mut components = vec![];
    for component in joined.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            component => components.push(component),
        }
    }
    format!(
        "{}{}",
        if absolute { "/" } else { "" },
        components.join("/")
    )
}

pub fn quote_argument(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

pub fn shell_plan(active: &Location, source: &str) -> Result<ShellPlan> {
    validate_line(source)?;
    ensure!(!source.trim().is_empty(), "Enter a shell command after :!");
    match active {
        Location::Local { path } => {
            ensure!(
                path.is_absolute(),
                "Active local directory must be absolute"
            );
            Ok(ShellPlan {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), source.into()],
                cwd: path.clone(),
                title: "Command".into(),
            })
        }
        Location::Ssh { host, path } => {
            ensure!(
                !host.is_empty()
                    && !host.starts_with('-')
                    && host
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || ".-_@[]:".contains(c)),
                "Invalid SSH host; use a configured host name without options"
            );
            ensure!(
                path.starts_with('/'),
                "SSH command directory must be absolute"
            );
            validate_line(path)?;
            Ok(ShellPlan {
                program: "ssh".into(),
                args: vec![
                    "-t".into(),
                    "--".into(),
                    host.clone(),
                    format!("cd -- {} && {{ {source}\n}}", quote_argument(path)),
                ],
                cwd: PathBuf::from("/"),
                title: format!("Command · {host}"),
            })
        }
        _ => bail!(
            "Shell commands require a local or SSH directory; this provider cannot launch a terminal"
        ),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryFile {
    version: u32,
    entries: Vec<String>,
}

pub struct HistoryStore {
    path: PathBuf,
    entries: Vec<String>,
    original: Option<Vec<u8>>,
}

impl HistoryStore {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let original = read_history(&path)?;
        let entries = if let Some(source) = &original {
            let file: HistoryFile =
                serde_json::from_slice(source).context("Invalid command history JSON")?;
            ensure!(file.version == 1, "Unsupported command history version");
            ensure!(
                file.entries.len() <= MAX_HISTORY,
                "Command history exceeds 100 entries"
            );
            for entry in &file.entries {
                validate_line(entry)?;
                ensure!(
                    !body(entry).trim().is_empty() && body(entry).trim() != "!",
                    "Command history contains an empty command"
                );
            }
            file.entries
        } else {
            vec![]
        };
        Ok(Self {
            path,
            entries,
            original,
        })
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Call only after explicit accepted submission, never for drafts,
    /// completion previews or cancelled input. No environment is captured.
    pub fn record(&mut self, input: &str) -> Result<()> {
        validate_line(input)?;
        let body = body(input).trim();
        if body.is_empty() || body == "!" || self.entries.last().is_some_and(|last| last == input) {
            return Ok(());
        }
        let mut entries = self.entries.clone();
        entries.push(input.into());
        if entries.len() > MAX_HISTORY {
            entries.remove(0);
        }
        let bytes = loop {
            let bytes = serde_json::to_vec_pretty(&HistoryFile {
                version: 1,
                entries: entries.clone(),
            })?;
            if bytes.len() <= MAX_HISTORY_BYTES {
                break bytes;
            }
            entries.remove(0);
        };
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let lock_path = self.path.with_extension("lock");
        reject_non_regular(&lock_path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options
            .open(&lock_path)
            .context("Open command history lock")?;
        verify_inode(&lock_path, &lock)?;
        let _lock = ExclusiveFileLock::try_acquire(lock)
            .context("Command history is being saved by another process")?;
        ensure!(
            read_history(&self.path)? == self.original,
            "Command history changed externally; reopen it before saving"
        );
        let temporary = parent.join(format!(
            ".command-history.{}.{}.tmp",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut staged = StagedHistory {
            file: options.open(&temporary)?,
            path: temporary,
        };
        staged.file.write_all(&bytes)?;
        staged.file.sync_all()?;
        ensure!(
            read_history(&self.path)? == self.original,
            "Command history changed externally; reopen it before saving"
        );
        if self.original.is_none() {
            fs::hard_link(&staged.path, &self.path)?;
        } else {
            fs::rename(&staged.path, &self.path)?;
        }
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        self.entries = entries;
        self.original = Some(bytes);
        Ok(())
    }
}

struct StagedHistory {
    file: fs::File,
    path: PathBuf,
}
impl Drop for StagedHistory {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn reject_non_regular(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            metadata.is_file(),
            "Command history and lock must be regular files, not symlinks or special files"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn verify_inode(path: &Path, file: &fs::File) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(metadata.is_file(), "Command history must be a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        ensure!(
            metadata.dev() == opened.dev() && metadata.ino() == opened.ino(),
            "Command history changed while opening"
        );
    }
    Ok(())
}

fn read_history(path: &Path) -> Result<Option<Vec<u8>>> {
    reject_non_regular(path)?;
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    verify_inode(path, &file)?;
    let mut bytes = vec![];
    file.take((MAX_HISTORY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_HISTORY_BYTES,
        "Command history exceeds 256 KiB"
    );
    Ok(Some(bytes))
}
