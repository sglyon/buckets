//! Provider-neutral account presentation and a host-owned credential vault.
//! Credentials never enter settings or agent conversations. A lease serializes
//! a complete refresh/read/replace across processes and is released on drop.
use super::{AgentService, Capability, PluginRegistry};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountAction {
    #[default]
    Status,
    SignIn,
    SignOut,
    Select,
    Models,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountRequest {
    pub action: AccountAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}
impl AccountRequest {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.account_id
                .as_ref()
                .is_none_or(|id| safe_text(id, 256) && !id.is_empty()),
            "Invalid account identifier"
        );
        ensure!(
            self.action != AccountAction::Select || self.account_id.is_some(),
            "Choose an account to select"
        );
        Ok(())
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountInfo {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub plan_enabled: bool,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountModel {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountState {
    #[serde(default)]
    pub accounts: Vec<AccountInfo>,
    #[serde(default)]
    pub active_account_id: Option<String>,
    #[serde(default)]
    pub models: Vec<AccountModel>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub plan_enabled: bool,
    #[serde(default)]
    pub using_plan: bool,
    #[serde(default)]
    pub sign_in_required: bool,
    #[serde(default)]
    pub first_connection: bool,
    #[serde(default)]
    pub usage_url: Option<String>,
    #[serde(default)]
    pub sign_in_label: Option<String>,
    #[serde(default)]
    pub plan_label: Option<String>,
}
impl AccountState {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.accounts.len() <= 64 && self.models.len() <= 512,
            "Account result exceeds limits"
        );
        let mut ids = BTreeSet::new();
        for account in &self.accounts {
            ensure!(
                !account.id.is_empty()
                    && safe_text(&account.id, 256)
                    && safe_text(&account.label, 512),
                "Invalid account information"
            );
            ensure!(ids.insert(&account.id), "Duplicate account identifier");
        }
        ensure!(
            self.active_account_id
                .as_ref()
                .is_none_or(|id| ids.contains(id)),
            "Active account is not in account list"
        );
        let mut models = BTreeSet::new();
        for model in &self.models {
            ensure!(
                !model.id.is_empty() && safe_text(&model.id, 256) && safe_text(&model.name, 512),
                "Invalid account model"
            );
            ensure!(models.insert(&model.id), "Duplicate account model");
        }
        ensure!(
            self.message.as_ref().is_none_or(|text| text.len() <= 4096),
            "Account message exceeds limits"
        );
        ensure!(
            self.sign_in_label
                .as_ref()
                .is_none_or(|text| safe_text(text, 128))
                && self
                    .plan_label
                    .as_ref()
                    .is_none_or(|text| safe_text(text, 128)),
            "Invalid account label"
        );
        ensure!(
            self.usage_url
                .as_ref()
                .is_none_or(|url| safe_https_url(url)),
            "Account usage URL must use HTTPS"
        );
        Ok(())
    }
}
fn safe_text(value: &str, max: usize) -> bool {
    value.len() <= max && !value.chars().any(char::is_control)
}
fn safe_https_url(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !authority.is_empty()
        && authority
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-:[]".contains(&byte))
        && !value.chars().any(char::is_whitespace)
        && !value.contains('\\')
        && safe_text(value, 4096)
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountEvent {
    pub message: String,
}

pub struct AccountService {
    inner: AgentService,
}
impl PluginRegistry {
    pub fn start_accounts(&self, id: &str) -> Result<AccountService> {
        Ok(AccountService {
            inner: self.start_duplex(id, Capability::Accounts)?,
        })
    }
}
impl AccountService {
    pub fn invoke(
        &mut self,
        request: AccountRequest,
        host: impl FnMut(&str, Value) -> Result<Value>,
        progress: impl FnMut(AccountEvent),
        cancel: &AtomicBool,
    ) -> Result<AccountState> {
        self.inner.invoke_accounts(request, host, progress, cancel)
    }
    pub fn shutdown(&mut self) {
        self.inner.shutdown();
    }
    pub fn is_running(&mut self) -> bool {
        self.inner.is_running()
    }
}

const MAX_CREDENTIAL_BYTES: usize = 262_144;
static NEXT_LEASE: AtomicU64 = AtomicU64::new(1);
struct Lease {
    key: String,
    _file: File,
}
/// A private, portable on-disk vault. The host supplies its own configuration
/// root and a manifest ID, never a path chosen by the plugin. Debug intentionally
/// omits the record data. This is not the macOS Keychain.
pub struct CredentialBroker {
    directory: PrivateDirectory,
    leases: BTreeMap<String, Lease>,
}
impl CredentialBroker {
    pub fn new(root: impl AsRef<Path>, plugin_id: &str) -> Result<Self> {
        ensure!(safe_key(plugin_id), "Invalid credential namespace");
        ensure!(
            root.as_ref().is_absolute(),
            "Credential root must be absolute"
        );
        let root = PrivateDirectory::open(root.as_ref())?;
        let directory = root.child(plugin_id)?;
        Ok(Self {
            directory,
            leases: BTreeMap::new(),
        })
    }
    pub fn dispatch(&mut self, method: &str, params: Value) -> Result<Value> {
        match method {
            "credentials.read" => {
                let key = parameter(&params, "key")?;
                ensure!(safe_key(key), "Invalid credential key");
                Ok(match self.read(key)? {
                    Some(value) => json!({"value":value}),
                    None => json!({}),
                })
            }
            "credentials.acquire" => {
                let key = parameter(&params, "key")?;
                ensure!(safe_key(key), "Invalid credential key");
                ensure!(self.leases.len() < 8, "Too many credential leases");
                ensure!(
                    !self.leases.values().any(|lease| lease.key == key),
                    "Credential store is busy; retry"
                );
                let file = self.directory.open_file(&format!("{key}.lock"), true)?;
                file.try_lock()
                    .map_err(|_| anyhow::anyhow!("Credential store is busy; retry"))?;
                let value = self.read(key)?;
                let lease = format!(
                    "{}-{}",
                    std::process::id(),
                    NEXT_LEASE.fetch_add(1, Ordering::Relaxed)
                );
                self.leases.insert(
                    lease.clone(),
                    Lease {
                        key: key.into(),
                        _file: file,
                    },
                );
                Ok(match value {
                    Some(value) => json!({"lease":lease,"value":value}),
                    None => json!({"lease":lease}),
                })
            }
            "credentials.commit" => {
                let lease = parameter(&params, "lease")?;
                let held = self
                    .leases
                    .remove(lease)
                    .context("Credential lease is not active")?;
                if let Some(value) = params.get("value") {
                    if value.is_null() {
                        self.directory.remove(&format!("{}.json", held.key))?;
                    } else {
                        self.write(&held.key, value)?;
                    }
                }
                Ok(json!({}))
            }
            "credentials.release" => {
                let lease = parameter(&params, "lease")?;
                self.leases
                    .remove(lease)
                    .context("Credential lease is not active")?;
                Ok(json!({}))
            }
            _ => bail!("Unknown credential operation"),
        }
    }
    fn read(&self, key: &str) -> Result<Option<Value>> {
        let mut file = match self.directory.open_file(&format!("{key}.json"), false) {
            Ok(file) => file,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let mut data = Vec::new();
        (&mut file)
            .take((MAX_CREDENTIAL_BYTES + 1) as u64)
            .read_to_end(&mut data)
            .context("Read protected credentials")?;
        ensure!(
            data.len() <= MAX_CREDENTIAL_BYTES,
            "Credential record exceeds limits"
        );
        Ok(Some(serde_json::from_slice(&data).map_err(|_| {
            anyhow::anyhow!("Credential record is invalid")
        })?))
    }
    fn write(&self, key: &str, value: &Value) -> Result<()> {
        let data = serde_json::to_vec(value).context("Encode credential record")?;
        ensure!(
            data.len() <= MAX_CREDENTIAL_BYTES,
            "Credential record exceeds limits"
        );
        self.directory.replace(&format!("{key}.json"), &data)
    }
}
fn parameter<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .context("Missing credential operation parameter")
}
fn safe_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

struct PrivateDirectory {
    handle: File,
    path: PathBuf,
}
impl PrivateDirectory {
    fn open(path: &Path) -> Result<Self> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(path)
            .context("Create private credential directory")?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        }
        let handle = options
            .open(path)
            .context("Open private credential directory without following links")?;
        verify_directory(&handle, path)?;
        Ok(Self {
            handle,
            path: path.into(),
        })
    }
    fn child(&self, key: &str) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            let name = std::ffi::CString::new(key)?;
            let created = unsafe { libc::mkdirat(self.handle.as_raw_fd(), name.as_ptr(), 0o700) };
            if created != 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(std::io::Error::last_os_error()).context("Create credential namespace");
            }
            let fd = unsafe {
                libc::openat(
                    self.handle.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            ensure!(
                fd >= 0,
                "Credential namespace must be a private non-symlink directory"
            );
            let handle = unsafe { File::from_raw_fd(fd) };
            let path = self.path.join(key);
            verify_directory(&handle, &path)?;
            Ok(Self { handle, path })
        }
        #[cfg(not(unix))]
        {
            Self::open(&self.path.join(key))
        }
    }
    fn open_file(&self, name: &str, create: bool) -> Result<File> {
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            let name = std::ffi::CString::new(name)?;
            let flags = libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | libc::O_NONBLOCK
                | if create {
                    libc::O_RDWR | libc::O_CREAT
                } else {
                    libc::O_RDONLY
                };
            let fd = unsafe { libc::openat(self.handle.as_raw_fd(), name.as_ptr(), flags, 0o600) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("Open protected credential record");
            }
            let file = unsafe { File::from_raw_fd(fd) };
            verify_file(&file)?;
            Ok(file)
        }
        #[cfg(not(unix))]
        {
            let path = self.path.join(name);
            ensure!(
                !std::fs::symlink_metadata(&path).is_ok_and(|value| value.file_type().is_symlink()),
                "Credential record must not be a symlink"
            );
            let file = OpenOptions::new()
                .read(true)
                .write(create)
                .create(create)
                .truncate(false)
                .open(path)?;
            verify_file(&file)?;
            Ok(file)
        }
    }
    fn replace(&self, name: &str, data: &[u8]) -> Result<()> {
        // Validate any existing destination without following a link. Renaming
        // still uses directory-relative syscalls, so parent swaps cannot redirect.
        match self.open_file(name, false) {
            Ok(_) => {}
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
            Err(error) => return Err(error),
        }
        let temporary = format!(
            ".{}-{}.tmp",
            std::process::id(),
            NEXT_LEASE.fetch_add(1, Ordering::Relaxed)
        );
        let result = (|| -> Result<()> {
            let mut file = self.create_new(&temporary)?;
            file.write_all(data)
                .context("Write protected credential record")?;
            file.sync_all()
                .context("Flush protected credential record")?;
            self.rename(&temporary, name)?;
            self.handle
                .sync_all()
                .context("Flush credential directory")?;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.remove(&temporary);
        }
        result
    }
    fn create_new(&self, name: &str) -> Result<File> {
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            let name = std::ffi::CString::new(name)?;
            let fd = unsafe {
                libc::openat(
                    self.handle.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("Create private credential record");
            }
            Ok(unsafe { File::from_raw_fd(fd) })
        }
        #[cfg(not(unix))]
        {
            Ok(OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.path.join(name))?)
        }
    }
    fn rename(&self, from: &str, to: &str) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let from = std::ffi::CString::new(from)?;
            let to = std::ffi::CString::new(to)?;
            let result = unsafe {
                libc::renameat(
                    self.handle.as_raw_fd(),
                    from.as_ptr(),
                    self.handle.as_raw_fd(),
                    to.as_ptr(),
                )
            };
            if result != 0 {
                return Err(std::io::Error::last_os_error()).context("Replace credential record");
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Ok(std::fs::rename(self.path.join(from), self.path.join(to))?)
        }
    }
    fn remove(&self, name: &str) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let name = std::ffi::CString::new(name)?;
            let result = unsafe { libc::unlinkat(self.handle.as_raw_fd(), name.as_ptr(), 0) };
            if result != 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound
            {
                return Err(std::io::Error::last_os_error()).context("Remove credential record");
            }
            self.handle.sync_all()?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            match std::fs::remove_file(self.path.join(name)) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            }
        }
    }
}
fn verify_directory(file: &File, path: &Path) -> Result<()> {
    let metadata = file.metadata()?;
    ensure!(metadata.is_dir(), "Credential root must be a directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "Credential directory must be owned by current user"
        );
        file.set_permissions(std::fs::Permissions::from_mode(0o700))
            .context("Make credential directory private")?;
    }
    #[cfg(not(unix))]
    ensure!(
        !std::fs::symlink_metadata(path)?.file_type().is_symlink(),
        "Credential directory must not be a symlink"
    );
    let _ = path;
    Ok(())
}
fn verify_file(file: &File) -> Result<()> {
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file(),
        "Credential record must be a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.nlink() == 1,
            "Credential record must be privately owned without hard links"
        );
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .context("Make credential record private")?;
    }
    Ok(())
}
