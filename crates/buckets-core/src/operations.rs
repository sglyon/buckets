use crate::{Backend, Location, providers};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

impl Backend {
    pub fn create_dir(&self, location: &Location) -> Result<()> {
        match location {
            Location::Plugin { plugin, path } => {
                providers::plugin_request(plugin, "mkdir", serde_json::json!({"path":path}))
                    .map(|_| ())
            }
            Location::Local { path } => {
                fs::create_dir(path).with_context(|| format!("Cannot create {}", path.display()))
            }
            Location::Ssh { host, path } => {
                providers::ssh(host, "import os,sys\nos.mkdir(sys.argv[1])", &[path], None)
                    .map(|_| ())
            }
            Location::S3 {
                bucket,
                prefix,
                endpoint,
            } => {
                ensure!(
                    !prefix.is_empty(),
                    "Cannot create a bucket from the file browser"
                );
                let loc = Location::S3 {
                    bucket: bucket.clone(),
                    prefix: format!("{}/", prefix.trim_end_matches('/')),
                    endpoint: endpoint.clone(),
                };
                providers::write_new(&loc, b"")
            }
            Location::Git { .. } => bail!("Git snapshots are read-only"),
        }
    }
    pub fn create_file(&self, location: &Location) -> Result<()> {
        providers::write_new(location, b"")
    }
    /// Copy without following symlinks or replacing an existing destination.
    pub fn copy(&self, source: &Location, destination: &Location) -> Result<()> {
        ensure!(source != destination, "Source and destination are the same");
        if let (
            Location::Plugin { plugin, path },
            Location::Plugin {
                plugin: other,
                path: dest,
            },
        ) = (source, destination)
            && plugin == other
        {
            return providers::plugin_request(
                plugin,
                "copy",
                serde_json::json!({"source":path,"destination":dest,"no_replace":true}),
            )
            .map(|_| ());
        }
        if let (Location::Local { path: src }, Location::Local { path: dst }) =
            (source, destination)
        {
            return copy_local(src, dst);
        }
        ensure!(
            !matches!(destination, Location::Git { .. }),
            "Git snapshots are read-only"
        );
        self.copy_tree(source, destination)
    }
    /// Atomic, no-replace rename on the same filesystem. Cross-filesystem moves refuse safely.
    pub fn move_entry(&self, source: &Location, destination: &Location) -> Result<()> {
        ensure!(source != destination, "Source and destination are the same");
        match (source, destination) {
            (
                Location::Plugin { plugin, path },
                Location::Plugin {
                    plugin: other,
                    path: dest,
                },
            ) if plugin == other => providers::plugin_request(
                plugin,
                "move",
                serde_json::json!({"source":path,"destination":dest,"no_replace":true}),
            )
            .map(|_| ()),
            (Location::Local { path: src }, Location::Local { path: dst }) => {
                validate_destination(src, dst)?;
                match rename_exclusive(src, dst) {
                    Ok(()) => Ok(()),
                    Err(error) if error.raw_os_error() == Some(18) => {
                        crate::transfer::move_local_verified(src, dst)
                    }
                    Err(error) => Err(error).with_context(|| {
                        format!("Cannot move {} to {}", src.display(), dst.display())
                    }),
                }
            }
            (
                Location::Ssh { host, path },
                Location::Ssh {
                    host: other,
                    path: dest,
                },
            ) if host == other => {
                // Python's rename has overwrite semantics. Linux renameat2 via ctypes provides atomic exclusion.
                providers::ssh(host,"import sys,os,ctypes\na,b=sys.argv[1:]\nif os.path.isdir(a) and os.path.realpath(b).startswith(os.path.realpath(a)+os.sep): raise ValueError('Destination is inside source')\nlib=ctypes.CDLL(None,use_errno=True)\nif sys.platform=='darwin':\n fn=lib.renamex_np;result=fn(os.fsencode(a),os.fsencode(b),4)\nelse:\n fn=getattr(lib,'renameat2',None)\n if fn is None: raise RuntimeError('Remote system lacks safe no-replace rename')\n result=fn(-100,os.fsencode(a),-100,os.fsencode(b),1)\nif result: raise OSError(ctypes.get_errno(),os.strerror(ctypes.get_errno()))",&[path,dest],None).map(|_|())
            }
            (Location::S3 { .. }, _) => self.move_s3(source, destination),
            _ => {
                self.copy_tree(source, destination)?;
                verify_transfer(self, source, destination)?;
                self.trash(source).context("Destination copied and verified; original remains because moving to trash failed")
            }
        }
    }
    pub fn rename(&self, source: &Location, name: &str) -> Result<Location> {
        let target = source
            .parent()
            .context("Cannot rename a filesystem root")?
            .join(name)?;
        self.move_entry(source, &target)?;
        Ok(target)
    }
    pub fn duplicate(&self, source: &Location) -> Result<Location> {
        let parent = source
            .parent()
            .context("Cannot duplicate a filesystem root")?;
        let name = source.name();
        let path = Path::new(&name);
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let ext = path
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let is_dir = self
            .list(&parent)?
            .iter()
            .find(|e| e.location == *source)
            .context("Source no longer exists")?
            .is_dir;
        let siblings = self.list(&parent)?;
        for i in 1..=10000 {
            let suffix = if i == 1 {
                " copy".to_owned()
            } else {
                format!(" copy {i}")
            };
            let n = if is_dir {
                format!("{name}{suffix}")
            } else {
                format!("{stem}{suffix}{ext}")
            };
            if siblings.iter().any(|e| e.name == n) {
                continue;
            }
            let target = parent.join(&n)?;
            self.copy(source, &target)?;
            return Ok(target);
        }
        bail!("No available duplicate name")
    }
    /// Move to the operating system trash. Never permanently deletes user files.
    pub fn trash(&self, location: &Location) -> Result<()> {
        self.trash_with_receipt(location).map(|_| ())
    }
    /// Trash an entry and return its recoverable destination when the provider exposes it.
    pub fn trash_with_receipt(&self, location: &Location) -> Result<Option<Location>> {
        self.trash_with_receipt_at(location, None)
    }
    pub(crate) fn trash_with_receipt_at(
        &self,
        location: &Location,
        trash_override: Option<&Path>,
    ) -> Result<Option<Location>> {
        if let Location::Plugin { plugin, path } = location {
            return providers::plugin_request(
                plugin,
                "remove",
                serde_json::json!({"path":path,"trash":true}),
            )
            .map(|_| None);
        }
        if let Location::Ssh { host, path } = location {
            return remote_trash(host, path).map(|_| None);
        }
        if let Location::S3 {
            bucket,
            prefix,
            endpoint,
        } = location
        {
            ensure!(!prefix.is_empty(), "Cannot trash a bucket root");
            ensure!(
                !prefix.starts_with(".buckets-trash/"),
                "Object is already in Buckets trash"
            );
            let token = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            let target = Location::S3 {
                bucket: bucket.clone(),
                prefix: format!(".buckets-trash/{token}/{}", prefix.trim_start_matches('/')),
                endpoint: endpoint.clone(),
            };
            return self.move_s3(location, &target).map(|_| Some(target));
        }
        let Location::Local { path } = location else {
            bail!(
                "Trash is available for local files only; remote deletion is deliberately unavailable"
            )
        };
        ensure!(path.file_name().is_some(), "Cannot trash a filesystem root");
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("Home directory unavailable")?;
        let canonical = fs::canonicalize(path.parent().context("No parent directory")?)?
            .join(path.file_name().unwrap());
        ensure!(canonical != home, "Cannot trash your home directory");
        #[cfg(target_os = "macos")]
        let trash = home.join(".Trash");
        #[cfg(not(target_os = "macos"))]
        let trash = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("Trash/files");
        let trash = trash_override.map(PathBuf::from).unwrap_or(trash);
        fs::create_dir_all(&trash)?;
        let trash_real = fs::canonicalize(&trash)?;
        ensure!(
            !canonical.starts_with(&trash_real),
            "This item is already in the trash"
        );
        ensure!(
            !trash_real.starts_with(&canonical),
            "Cannot trash a directory that contains the trash"
        );
        let name = path
            .file_name()
            .context("Missing filename")?
            .to_string_lossy();
        for i in 0..10000 {
            let name = if i == 0 {
                name.to_string()
            } else {
                format!("{name}.{i}")
            };
            let target = trash.join(&name);
            if fs::symlink_metadata(&target).is_ok() {
                continue;
            }
            #[cfg(not(target_os = "macos"))]
            {
                write_trash_info(&trash, &name, &canonical)?;
            }
            match rename_exclusive(path, &target) {
                Ok(()) => return Ok(Some(Location::local(target))),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) if e.raw_os_error() == Some(18) => {
                    return crate::transfer::move_local_verified(path, &target)
                        .map(|_| Some(Location::local(target)));
                }
                Err(e) => {
                    #[cfg(not(target_os = "macos"))]
                    {
                        let _ = fs::remove_file(
                            trash
                                .parent()
                                .unwrap()
                                .join("info")
                                .join(format!("{name}.trashinfo")),
                        );
                    }
                    return Err(e).context(
                        "Could not move to trash; cross-filesystem trash is not supported",
                    );
                }
            }
        }
        bail!("Could not find a unique trash name")
    }
}
pub(crate) fn validate_destination(src: &Path, dst: &Path) -> Result<()> {
    ensure!(
        src.file_name().is_some(),
        "Cannot operate on a filesystem root"
    );
    let meta = fs::symlink_metadata(src)
        .with_context(|| format!("Source does not exist: {}", src.display()))?;
    ensure!(
        fs::symlink_metadata(dst).is_err(),
        "Destination already exists: {}",
        dst.display()
    );
    let parent = fs::canonicalize(dst.parent().context("Destination has no parent")?)
        .context("Destination directory does not exist")?;
    if meta.is_dir() {
        let source = fs::canonicalize(src)?;
        ensure!(
            !parent.starts_with(&source),
            "Cannot copy or move a directory inside itself"
        );
    }
    Ok(())
}
pub(crate) fn copy_local(src: &Path, dst: &Path) -> Result<()> {
    validate_destination(src, dst)?;
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(fs::read_link(src)?, dst)?;
            return Ok(());
        }
        #[cfg(not(unix))]
        {
            bail!("Symlink copying is not supported on this platform")
        }
    }
    if meta.is_dir() {
        fs::create_dir(dst)?;
        let result = (|| {
            for item in fs::read_dir(src)? {
                let item = item?;
                copy_local(&item.path(), &dst.join(item.file_name()))?;
            }
            fs::set_permissions(dst, meta.permissions())?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(dst);
        }
        result
    } else {
        ensure!(meta.is_file(), "Special files cannot be copied");
        use std::io::Write;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dst)?;
        let result = (|| {
            let mut input = fs::File::open(src)?;
            io::copy(&mut input, &mut output)?;
            output.flush()?;
            output.sync_all()?;
            fs::set_permissions(dst, meta.permissions())?;
            Ok(())
        })();
        if result.is_err() {
            drop(output);
            let _ = fs::remove_file(dst);
        }
        result
    }
}
#[cfg(unix)]
pub(crate) fn rename_exclusive(src: &Path, dst: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let src = CString::new(src.as_os_str().as_bytes())?;
    let dst = CString::new(dst.as_os_str().as_bytes())?;
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn renamex_np(
                from: *const std::ffi::c_char,
                to: *const std::ffi::c_char,
                flags: u32,
            ) -> i32;
        }
        if unsafe { renamex_np(src.as_ptr(), dst.as_ptr(), 4) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            fn renameat2(
                olddirfd: i32,
                oldpath: *const std::ffi::c_char,
                newdirfd: i32,
                newpath: *const std::ffi::c_char,
                flags: u32,
            ) -> i32;
        }
        if unsafe { renameat2(-100, src.as_ptr(), -100, dst.as_ptr(), 1) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (src, dst);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Safe no-replace rename is unavailable",
        ))
    }
}
#[cfg(not(unix))]
pub(crate) fn rename_exclusive(_src: &Path, _dst: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Safe no-replace rename is unavailable",
    ))
}
#[cfg(not(target_os = "macos"))]
fn write_trash_info(trash: &Path, name: &str, original: &Path) -> Result<()> {
    use std::io::Write;
    let info = trash
        .parent()
        .context("Invalid trash directory")?
        .join("info");
    fs::create_dir_all(&info)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(info.join(format!("{name}.trashinfo")))?;
    let encoded: String =
        url::form_urlencoded::byte_serialize(original.to_string_lossy().as_bytes()).collect();
    file.write_all(format!("[Trash Info]\nPath={encoded}\n").as_bytes())?;
    Ok(())
}

fn verify_transfer(backend: &Backend, source: &Location, destination: &Location) -> Result<()> {
    let entry = backend.stat(source)?;
    if entry.is_symlink {
        ensure!(
            crate::transfer::read_symlink(source)? == crate::transfer::read_symlink(destination)?,
            "Symlink verification failed; original was preserved"
        );
        return Ok(());
    }
    if entry.is_dir {
        for child in backend.list(source)? {
            verify_transfer(backend, &child.location, &destination.join(&child.name)?)?;
        }
        return Ok(());
    }
    let original = crate::spool::Spool::download(source)?;
    let copied = crate::spool::Spool::download(destination)?;
    ensure!(
        original.size == copied.size && original.digest == copied.digest,
        "Transfer verification failed; original was preserved"
    );
    Ok(())
}
fn remote_trash(host: &str, path: &str) -> Result<()> {
    providers::ssh(host, include_str!("ssh_trash.py"), &[path], None).map(|_| ())
}
