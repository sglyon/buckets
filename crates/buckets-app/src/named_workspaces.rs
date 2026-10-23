//! Named workspace metadata. Restoring a snapshot opens locations as file panes;
//! it never serializes or replays terminal commands, credentials, or agent jobs.
use anyhow::{Context, Result, ensure};
use buckets_core::Location;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: usize = 1_048_576;
const MAX_WORKSPACES: usize = 64;
const MAX_PANES: usize = 6;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneLayout {
    #[default]
    Horizontal,
    Vertical,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Modified,
    Type,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Column {
    Name,
    Size,
    Modified,
    Type,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PaneView {
    pub sort: SortKey,
    pub descending: bool,
    pub directories_first: bool,
    pub columns: Vec<Column>,
}
impl Default for PaneView {
    fn default() -> Self {
        Self {
            sort: SortKey::Name,
            descending: false,
            directories_first: true,
            columns: vec![Column::Name, Column::Size, Column::Type],
        }
    }
}
impl PaneView {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.columns.first() == Some(&Column::Name) && self.columns.len() <= 4,
            "Pane columns must start with Name and contain at most four columns"
        );
        for (index, column) in self.columns.iter().enumerate() {
            ensure!(
                !self.columns[..index].contains(column),
                "Pane columns must not repeat"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePane {
    pub location: String,
    #[serde(default = "unit_weight")]
    pub weight: f32,
    #[serde(default)]
    pub view: PaneView,
}
fn unit_weight() -> f32 {
    1.
}
fn enabled() -> bool {
    true
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSnapshot {
    pub workspace_id: String,
    pub panes: Vec<WorkspacePane>,
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub layout: PaneLayout,
    #[serde(default = "enabled")]
    pub sidebar: bool,
    #[serde(default = "enabled")]
    pub preview: bool,
    #[serde(default)]
    pub hidden: bool,
}
impl Default for WorkspaceSnapshot {
    fn default() -> Self {
        Self {
            workspace_id: String::new(),
            panes: vec![],
            active: 0,
            layout: PaneLayout::default(),
            sidebar: true,
            preview: true,
            hidden: false,
        }
    }
}
impl WorkspaceSnapshot {
    pub fn validate(&self) -> Result<()> {
        validate_id(&self.workspace_id)?;
        ensure!(
            !self.panes.is_empty() && self.panes.len() <= MAX_PANES,
            "A named workspace requires 1..6 panes"
        );
        ensure!(
            self.active < self.panes.len(),
            "Active pane is out of range"
        );
        for pane in &self.panes {
            ensure!(
                pane.weight.is_finite() && pane.weight > 0. && pane.weight <= 1000.,
                "Pane weights must be finite and between 0 and 1000"
            );
            pane.view.validate()?;
            validate_location(&pane.location)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedWorkspace {
    pub id: String,
    pub name: String,
    pub snapshot: WorkspaceSnapshot,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    workspaces: Vec<NamedWorkspace>,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            version: 1,
            workspaces: vec![],
        }
    }
}
impl Document {
    fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= MAX_BYTES, "Named workspaces exceed 1 MiB");
        let document: Self =
            serde_json::from_slice(bytes).context("Invalid named workspaces JSON")?;
        document.validate()?;
        Ok(document)
    }
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "Unsupported named workspaces version");
        ensure!(
            self.workspaces.len() <= MAX_WORKSPACES,
            "Keep at most 64 named workspaces"
        );
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for workspace in &self.workspaces {
            validate_id(&workspace.id)?;
            validate_name(&workspace.name)?;
            ensure!(
                ids.insert(&workspace.id),
                "Named workspace IDs must be unique"
            );
            ensure!(
                names.insert(workspace.name.trim().to_lowercase()),
                "Named workspace names must be unique"
            );
            workspace.snapshot.validate()?;
        }
        Ok(())
    }
}

pub struct NamedWorkspaceStore {
    path: PathBuf,
    document: Document,
    original: Option<Vec<u8>>,
}
impl NamedWorkspaceStore {
    /// Missing files remain absent until the first explicit save. Invalid files
    /// are returned as errors rather than replaced by empty defaults.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let original = read_optional(&path)?;
        let document = original
            .as_deref()
            .map(Document::parse)
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            path,
            document,
            original,
        })
    }
    pub fn entries(&self) -> &[NamedWorkspace] {
        &self.document.workspaces
    }
    pub fn get(&self, id: &str) -> Option<&NamedWorkspace> {
        self.entries().iter().find(|workspace| workspace.id == id)
    }
    /// Saving an existing name updates that snapshot and retains its record ID.
    /// The snapshot's workspace identity is distinct from this management ID.
    pub fn save(&mut self, name: &str, snapshot: WorkspaceSnapshot) -> Result<String> {
        let name = name.trim();
        validate_name(name)?;
        snapshot.validate()?;
        let mut next = self.document.clone();
        let id = if let Some(existing) = next
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.name.to_lowercase() == name.to_lowercase())
        {
            existing.name = name.to_owned();
            existing.snapshot = snapshot;
            existing.id.clone()
        } else {
            ensure!(
                next.workspaces.len() < MAX_WORKSPACES,
                "Keep at most 64 named workspaces"
            );
            let id = format!(
                "saved-{:x}-{:x}-{:x}",
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            );
            next.workspaces.push(NamedWorkspace {
                id: id.clone(),
                name: name.to_owned(),
                snapshot,
            });
            id
        };
        self.persist(next)?;
        Ok(id)
    }
    pub fn rename(&mut self, id: &str, name: &str) -> Result<()> {
        let name = name.trim();
        validate_name(name)?;
        let mut next = self.document.clone();
        let workspace = next
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == id)
            .context("Named workspace no longer exists; reload before editing")?;
        workspace.name = name.to_owned();
        self.persist(next)
    }
    /// Removes only saved metadata; files and agent state remain untouched.
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let mut next = self.document.clone();
        let before = next.workspaces.len();
        next.workspaces.retain(|workspace| workspace.id != id);
        ensure!(
            next.workspaces.len() != before,
            "Named workspace no longer exists; reload before editing"
        );
        self.persist(next)
    }
    pub fn reload(&mut self) -> Result<()> {
        let original = read_optional(&self.path)?;
        let document = original
            .as_deref()
            .map(Document::parse)
            .transpose()?
            .unwrap_or_default();
        self.original = original;
        self.document = document;
        Ok(())
    }
    fn persist(&mut self, next: Document) -> Result<()> {
        next.validate()?;
        let mut bytes = serde_json::to_vec_pretty(&next)?;
        bytes.push(b'\n');
        ensure!(bytes.len() <= MAX_BYTES, "Named workspaces exceed 1 MiB");
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let file_name = self
            .path
            .file_name()
            .context("Named workspace path requires a file name")?
            .to_string_lossy();
        let lock_path = parent.join(format!(".{file_name}.lock"));
        let lock = open_lock(&lock_path)?;
        lock.try_lock().context(
            "Named workspaces are being saved by another process; retry after it finishes",
        )?;
        self.check_original()?;
        if self.original.as_deref() == Some(bytes.as_slice()) {
            return Ok(());
        }
        let temporary_path = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut temporary = Temporary::create(temporary_path)?;
        temporary.file.write_all(&bytes)?;
        temporary.file.sync_all()?;
        self.check_original()?;
        if self.original.is_some() {
            fs::rename(&temporary.path, &self.path).context("Atomically save named workspaces")?;
        } else {
            fs::hard_link(&temporary.path, &self.path)
                .context("Named workspaces were created externally; reload before saving")?;
        }
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        self.document = next;
        self.original = Some(bytes);
        Ok(())
    }
    fn check_original(&self) -> Result<()> {
        ensure!(
            read_optional(&self.path)? == self.original,
            "Named workspaces changed externally; reload before saving to preserve those edits"
        );
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.trim().is_empty()
            && name.chars().count() <= 128
            && !name.chars().any(char::is_control),
        "Workspace name must contain 1..128 characters without control characters"
    );
    Ok(())
}
fn validate_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 160
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)),
        "Invalid workspace identity"
    );
    Ok(())
}
fn validate_location(source: &str) -> Result<()> {
    ensure!(
        !source.is_empty() && source.len() <= 8192 && !source.chars().any(char::is_control),
        "Workspace location requires 1..8192 bytes without control characters"
    );
    let location = Location::parse(source).context("Invalid named workspace location")?;
    if let Location::Local { .. } = &location {
        ensure!(
            source.starts_with('/') || source.starts_with("file://"),
            "Save absolute workspace locations"
        );
    }
    if source.contains("://") {
        let parsed = url::Url::parse(source)?;
        ensure!(
            parsed.fragment().is_none() && parsed.password().is_none(),
            "Workspace locations cannot contain passwords or fragments"
        );
        ensure!(
            parsed.scheme() == "ssh" || parsed.username().is_empty(),
            "Only SSH locations may specify a username; keep credentials outside workspace metadata"
        );
        let allowed = match parsed.scheme() {
            "s3" => &["endpoint"][..],
            "git" => &["ref", "path"][..],
            _ => &[][..],
        };
        let mut keys = HashSet::new();
        for (key, _) in parsed.query_pairs() {
            ensure!(
                allowed.contains(&key.as_ref()) && keys.insert(key.into_owned()),
                "Workspace location contains an unsupported or repeated parameter"
            );
        }
    }
    if let Location::S3 {
        endpoint: Some(endpoint),
        ..
    } = &location
    {
        let endpoint = url::Url::parse(endpoint)?;
        ensure!(
            endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.query().is_none()
                && endpoint.fragment().is_none(),
            "Keep S3 endpoint credentials outside workspace metadata"
        );
    }
    // Check decoded data too: percent encoding must not conceal control text.
    ensure!(
        !location.display().chars().any(char::is_control),
        "Workspace location contains control characters"
    );
    match &location {
        Location::Local { path } => ensure!(
            !path
                .as_os_str()
                .as_encoded_bytes()
                .iter()
                .any(|byte| *byte < 32 || *byte == 127),
            "Workspace path contains control characters"
        ),
        Location::Ssh { host, path } => ensure!(
            !host.chars().any(char::is_control) && !path.chars().any(char::is_control),
            "Workspace path contains control characters"
        ),
        Location::Plugin { path, .. } => ensure!(
            !path.chars().any(char::is_control),
            "Workspace path contains control characters"
        ),
        Location::Git {
            repo,
            reference,
            path,
        } => ensure!(
            !repo
                .as_os_str()
                .as_encoded_bytes()
                .iter()
                .any(|byte| *byte < 32 || *byte == 127)
                && !reference.chars().any(char::is_control)
                && !path.chars().any(char::is_control),
            "Workspace Git location contains control characters"
        ),
        Location::S3 { prefix, .. } => ensure!(
            !prefix.chars().any(char::is_control),
            "Workspace path contains control characters"
        ),
    }
    Ok(())
}
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Inspect named workspaces file"),
    };
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Named workspaces must be a regular file, not a symlink"
    );
    ensure!(
        metadata.len() <= MAX_BYTES as u64,
        "Named workspaces exceed 1 MiB"
    );
    let file = File::open(path)?;
    check_same_file(&metadata, &file.metadata()?)?;
    let mut bytes = vec![];
    file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_BYTES, "Named workspaces exceed 1 MiB");
    Ok(Some(bytes))
}
fn open_lock(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).read(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path)?;
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "Workspace save lock must be a regular file, not a symlink"
            );
            let file = File::open(path)?;
            check_same_file(&metadata, &file.metadata()?)?;
            Ok(file)
        }
        Err(error) => Err(error).context("Open named workspace save lock"),
    }
}
fn check_same_file(before: &fs::Metadata, after: &fs::Metadata) -> Result<()> {
    ensure!(after.is_file(), "Workspace file changed while opening");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            before.dev() == after.dev() && before.ino() == after.ino(),
            "Workspace file changed while opening"
        );
    }
    #[cfg(not(unix))]
    {
        ensure!(
            before.len() == after.len(),
            "Workspace file changed while opening"
        );
    }
    Ok(())
}
struct Temporary {
    path: PathBuf,
    file: File,
}
impl Temporary {
    fn create(path: PathBuf) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .context("Create temporary named workspace metadata")?;
        Ok(Self { path, file })
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
