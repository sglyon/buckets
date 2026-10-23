//! Durable application preferences. Workspace navigation is stored separately.
use anyhow::{Context, Result, bail, ensure};
use buckets_core::Location;
use buckets_plugin::{PluginOverrides, PluginRegistry};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_SETTINGS_BYTES: usize = 1_048_576;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A named durable location. Provider-specific host, path, endpoint and Git
/// revision are encoded by Location rather than split into lossy UI fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub location: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
impl Bookmark {
    #[allow(dead_code)] // Public UI helper; settings-only integration tests do not call it.
    pub fn new(name: impl Into<String>, location: impl AsRef<str>) -> Result<Self> {
        let name = name.into().trim().to_owned();
        let location = Location::parse(location.as_ref())?.display();
        let bookmark = Self {
            name,
            location,
            extra: Default::default(),
        };
        bookmark.validate()?;
        Ok(bookmark)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.name.trim().is_empty()
                && self.name.chars().count() <= 128
                && !self.name.chars().any(char::is_control),
            "Bookmark name must contain 1..128 characters without control characters"
        );
        ensure!(
            !self.location.is_empty()
                && self.location.len() <= 8192
                && !self.location.chars().any(char::is_control),
            "Bookmark location must contain 1..8192 bytes without control characters"
        );
        let location = Location::parse(&self.location)
            .with_context(|| format!("Invalid location for bookmark {}", self.name))?;
        if let Location::S3 {
            endpoint: Some(endpoint),
            ..
        } = &location
        {
            let endpoint = url::Url::parse(endpoint).context("Invalid bookmark S3 endpoint")?;
            ensure!(
                endpoint.username().is_empty()
                    && endpoint.password().is_none()
                    && endpoint.query().is_none()
                    && endpoint.fragment().is_none(),
                "Bookmark S3 endpoints must contain only scheme, host, port and path; keep credentials in your AWS configuration"
            );
        }
        if matches!(location, Location::Local { .. }) {
            ensure!(
                self.location.starts_with('/') || self.location.starts_with("file://"),
                "Bookmark local locations must be absolute; save a full path or file URL"
            );
        }
        Ok(())
    }
    #[allow(dead_code)] // Available to hosts for deduplicating user-visible bookmarks.
    pub fn canonical_location(&self) -> Result<String> {
        Ok(Location::parse(&self.location)?.display())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub font_size: f32,
    pub appearance: crate::appearance::Appearance,
    pub show_hidden: bool,
    pub sidebar: bool,
    pub preview: bool,
    pub plugins: BTreeMap<String, PluginOverrides>,
    pub bookmarks: Vec<Bookmark>,
    pub places: crate::places::Places,
    pub keybindings: crate::keybindings::Overrides,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 13.,
            appearance: Default::default(),
            show_hidden: false,
            sidebar: true,
            preview: true,
            plugins: Default::default(),
            bookmarks: Default::default(),
            places: Default::default(),
            keybindings: Default::default(),
            extra: Default::default(),
        }
    }
}
impl Settings {
    pub fn parse(source: &str) -> Result<Self> {
        ensure!(source.len() <= MAX_SETTINGS_BYTES, "Settings exceed 1 MiB");
        let value: Value = serde_json::from_str(source)
            .context("Invalid settings.json; use strict JSON without comments")?;
        ensure!(value.is_object(), "settings.json must contain an object");
        let settings: Self =
            serde_json::from_value(value).context("Invalid application settings")?;
        settings.validate()?;
        Ok(settings)
    }
    pub fn validate(&self) -> Result<()> {
        self.appearance.validate()?;
        self.places.validate()?;
        crate::keybindings::validate(&self.keybindings)?;
        ensure!(
            self.font_size.is_finite() && (9.0..=24.0).contains(&self.font_size),
            "font_size must be between 9 and 24"
        );
        ensure!(
            self.plugins.len() <= 1024,
            "Settings contain more than 1024 plugin entries"
        );
        ensure!(
            self.bookmarks.len() <= 256,
            "Settings contain more than 256 bookmarks"
        );
        for bookmark in &self.bookmarks {
            bookmark.validate()?;
        }
        for (id, plugin) in &self.plugins {
            ensure!(
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)),
                "Invalid plugin settings id {id}"
            );
            ensure!(
                plugin.config.is_object(),
                "Plugin {id} config must be an object"
            );
        }
        Ok(())
    }
    pub fn validate_plugins(&self, catalog: &PluginRegistry) -> Result<()> {
        self.validate()?;
        catalog.with_settings(&self.plugins)?;
        Ok(())
    }
}

pub struct SettingsStore {
    path: PathBuf,
    settings: Settings,
    original: Vec<u8>,
    catalog: Option<PluginRegistry>,
    invalid_source: bool,
}
impl SettingsStore {
    // Strict convenience entry point for callers that do not need repair mode.
    #[allow(dead_code)]
    pub fn load(path: impl AsRef<Path>, legacy: Settings) -> Result<Self> {
        let (store, warning) = Self::recover(path, legacy)?;
        if let Some(warning) = warning {
            bail!(warning);
        }
        Ok(store)
    }
    /// Retain malformed source for explicit JSON repair; never rewrite it from
    /// fallback defaults. Preference toggles are blocked until the repair saves.
    pub fn recover(path: impl AsRef<Path>, legacy: Settings) -> Result<(Self, Option<String>)> {
        legacy.validate()?;
        let path = path.as_ref().to_path_buf();
        if !path.exists() {
            let mut store = Self {
                path,
                settings: legacy,
                original: Vec::new(),
                catalog: None,
                invalid_source: false,
            };
            let settings = store.settings.clone();
            store.persist(settings, false, true)?;
            return Ok((store, None));
        }
        let original = read_source(&path)?;
        let parsed = std::str::from_utf8(&original)
            .context("settings.json is not UTF-8")
            .and_then(Settings::parse);
        let (settings, warning) = match parsed {
            Ok(settings) => (settings, None),
            Err(error) => (legacy, Some(format!("{}: {error:#}", path.display()))),
        };
        let invalid_source = warning.is_some();
        Ok((
            Self {
                path,
                settings,
                original,
                catalog: None,
                invalid_source,
            },
            warning,
        ))
    }
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn raw_json(&self) -> String {
        String::from_utf8_lossy(&self.original).into_owned()
    }
    pub fn set_plugin_catalog(&mut self, catalog: &PluginRegistry) {
        self.catalog = Some(catalog.clone());
    }
    pub fn update(&mut self, change: impl FnOnce(&mut Settings)) -> Result<()> {
        ensure!(
            !self.invalid_source,
            "settings.json needs repair; edit and save its JSON or reload a corrected file before changing preferences"
        );
        let mut next = self.settings.clone();
        change(&mut next);
        self.persist(next, false, false)
    }
    pub fn replace_json(&mut self, source: &str) -> Result<()> {
        let settings = Settings::parse(source)?;
        self.persist(settings, true, false)
    }
    pub fn reload(&mut self) -> Result<()> {
        let source = read_source(&self.path)?;
        let parsed = std::str::from_utf8(&source)
            .context("settings.json is not UTF-8")
            .and_then(Settings::parse)
            .and_then(|next| {
                if let Some(catalog) = &self.catalog {
                    next.validate_plugins(catalog)?;
                }
                Ok(next)
            });
        // An explicit reload acknowledges the latest source even when invalid.
        // Keep applying the last good values, but expose the actual broken JSON
        // for a deliberate repair rather than trapping the editor on stale text.
        self.original = source;
        let next = match parsed {
            Ok(next) => next,
            Err(error) => {
                self.invalid_source = true;
                return Err(error);
            }
        };
        self.settings = next;
        self.invalid_source = false;
        Ok(())
    }
    fn persist(&mut self, next: Settings, repair: bool, create: bool) -> Result<()> {
        next.validate()?;
        if let Some(catalog) = &self.catalog {
            next.validate_plugins(catalog)?;
        }
        let mut bytes = serde_json::to_vec_pretty(&next)?;
        bytes.push(b'\n');
        ensure!(bytes.len() <= MAX_SETTINGS_BYTES, "Settings exceed 1 MiB");
        if !create
            && !repair
            && serde_json::to_value(&next)? == serde_json::to_value(&self.settings)?
        {
            return Ok(());
        }
        let parent = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let lock_path = parent.join(".settings.json.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(lock_path).context("Open settings save lock")?;
        lock.try_lock()
            .context("Settings are being saved by another process; retry after it finishes")?;
        // Keep the inode persistent. Closing this descriptor releases the OS
        // lock even after a crash; unlinking it could allow two different locks.
        let _lock = lock;
        self.check_original(create)?;
        let name = format!(
            ".settings.{}.{}.tmp",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let temp_path = parent.join(name);
        let mut temporary = FileGuard::create(temp_path, "Cannot create temporary settings file")?;
        temporary.file.write_all(&bytes)?;
        temporary.file.sync_all()?;
        self.check_original(create)?;
        if create {
            fs::hard_link(&temporary.path, &self.path)
                .context("settings.json was created externally; reload before saving")?;
        } else {
            fs::rename(&temporary.path, &self.path).context("Atomically replace settings.json")?;
        }
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        self.settings = next;
        self.original = bytes;
        self.invalid_source = false;
        Ok(())
    }
    fn check_original(&self, create: bool) -> Result<()> {
        if create {
            ensure!(
                !self.path.exists(),
                "settings.json was created externally; reload before saving"
            );
            return Ok(());
        }
        let current = read_source(&self.path)
            .context("Settings changed or became unavailable; reload before saving")?;
        ensure!(
            current == self.original,
            "settings.json changed externally; reload before saving to preserve those edits"
        );
        ensure!(
            !fs::symlink_metadata(&self.path)?.file_type().is_symlink(),
            "settings.json is a symlink; edit its target directly to preserve the link"
        );
        Ok(())
    }
}

fn read_source(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::metadata(path).with_context(|| format!("Read {}", path.display()))?;
    ensure!(
        metadata.len() <= MAX_SETTINGS_BYTES as u64,
        "settings.json exceeds 1 MiB"
    );
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take((MAX_SETTINGS_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_SETTINGS_BYTES,
        "settings.json exceeds 1 MiB"
    );
    Ok(bytes)
}
struct FileGuard {
    path: PathBuf,
    file: fs::File,
}
impl FileGuard {
    fn create(path: PathBuf, message: &str) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).with_context(|| message.to_owned())?;
        Ok(Self { path, file })
    }
}
impl Drop for FileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
