//! Large native-provider transfers use private temporary files, never whole-file buffers.
use crate::{Location, providers};
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;

pub(crate) struct Spool {
    file: NamedTempFile,
    pub size: u64,
    pub digest: Vec<u8>,
    permissions: Option<u32>,
}
impl Spool {
    pub fn download(location: &Location) -> Result<Self> {
        Self::download_checked(location, None)
    }
    pub(crate) fn download_checked(
        location: &Location,
        expected: Option<&serde_json::Value>,
    ) -> Result<Self> {
        let permissions = permissions(location)?;
        let mut file = NamedTempFile::new().context("Cannot create private transfer spool")?;
        match location {
            Location::Local { path } => {
                let mut source = fs::File::open(path)?;
                std::io::copy(&mut source, file.as_file_mut())?;
            }
            Location::Ssh { host, path } => {
                let command = providers::ssh_command(
                    host,
                    "import sys,shutil\nwith open(sys.argv[1],'rb') as source: shutil.copyfileobj(source,sys.stdout.buffer,1024*1024)",
                    &[path],
                )?;
                stream_download(command, file.as_file().try_clone()?)?;
            }
            Location::S3 {
                bucket,
                prefix,
                endpoint,
            } => {
                let mut command = providers::aws(endpoint);
                command.args(["s3api", "get-object", "--bucket", bucket, "--key", prefix]);
                if let Some(metadata) = expected {
                    command.args([
                        "--if-match",
                        metadata["ETag"].as_str().context("S3 omitted ETag")?,
                    ]);
                    if let Some(version) = metadata["VersionId"].as_str().filter(|v| *v != "null") {
                        command.args(["--version-id", version]);
                    }
                }
                command.arg(file.path());
                providers::run_long(command)?;
            }
            Location::Git {
                repo,
                reference,
                path,
            } => {
                let mut command = Command::new("git");
                command
                    .env("GIT_TERMINAL_PROMPT", "0")
                    .env("GIT_NO_LAZY_FETCH", "1")
                    .arg("-C")
                    .arg(repo)
                    .args(["show", &format!("{reference}:{path}")]);
                stream_download(command, file.as_file().try_clone()?)?;
            }
            Location::Plugin { .. } => {
                file.write_all(&providers::read_all(location)?)?;
            }
        }
        file.as_file_mut().flush()?;
        file.as_file_mut().sync_all()?;
        let size = fs::metadata(file.path())?.len();
        let digest = hash_file(file.path())?;
        Ok(Self {
            file,
            size,
            digest,
            permissions,
        })
    }
    pub fn upload(&self, location: &Location) -> Result<()> {
        match location {
            Location::Local { path } => {
                let mut source = fs::File::open(self.file.path())?;
                let mut destination = fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(path)?;
                let identity = destination.metadata()?;
                let result = (|| {
                    std::io::copy(&mut source, &mut destination)?;
                    destination.flush()?;
                    destination.sync_all()?;
                    #[cfg(unix)]
                    if let Some(mode) = self.permissions {
                        use std::os::unix::fs::PermissionsExt;
                        destination.set_permissions(fs::Permissions::from_mode(mode))?;
                    }
                    Ok(())
                })();
                if result.is_err() && same_identity(&identity, path) {
                    let _ = fs::remove_file(path);
                }
                result
            }
            Location::Ssh { host, path } => {
                let script = "import sys,os,hashlib\np=sys.argv[1];expected=int(sys.argv[2]);digest=sys.argv[3]\nwith open(p,'xb') as target:\n identity=os.fstat(target.fileno())\n try:\n  count=0;hasher=hashlib.sha256()\n  while True:\n   chunk=sys.stdin.buffer.read(1024*1024)\n   if not chunk:break\n   target.write(chunk);hasher.update(chunk);count+=len(chunk)\n  if count!=expected or hasher.hexdigest()!=digest:raise IOError('Upload was incomplete; partial destination removed')\n  target.flush();os.fsync(target.fileno())\n  if int(sys.argv[4])>=0:os.fchmod(target.fileno(),int(sys.argv[4]))\n except BaseException:\n  current=os.lstat(p)\n  if current.st_ino==identity.st_ino and current.st_dev==identity.st_dev:os.unlink(p)\n  raise";
                let digest = self
                    .digest
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                let mut command = providers::ssh_command(
                    host,
                    script,
                    &[
                        path,
                        &self.size.to_string(),
                        &digest,
                        &self
                            .permissions
                            .map(|p| p.to_string())
                            .unwrap_or_else(|| "-1".into()),
                    ],
                )?;
                command.stdin(Stdio::from(fs::File::open(self.file.path())?));
                providers::run_long(command).map(|_| ())
            }
            Location::S3 {
                bucket,
                prefix,
                endpoint,
            } => {
                let mut command = providers::aws(endpoint);
                command
                    .args([
                        "s3api",
                        "put-object",
                        "--bucket",
                        bucket,
                        "--key",
                        prefix,
                        "--if-none-match",
                        "*",
                        "--body",
                    ])
                    .arg(self.file.path());
                providers::run_long(command).map(|_| ())
            }
            Location::Plugin { .. } => {
                ensure!(
                    self.size <= 8 * 1024 * 1024,
                    "Plugin file transfer exceeds the bounded protocol; this plugin needs a streaming provider extension"
                );
                providers::write_new(location, &fs::read(self.file.path())?)
            }
            Location::Git { .. } => bail!("Git snapshots are read-only"),
        }
    }
}
pub(crate) fn permissions(location: &Location) -> Result<Option<u32>> {
    match location {
        #[cfg(unix)]
        Location::Local { path } => {
            use std::os::unix::fs::PermissionsExt;
            Ok(Some(fs::metadata(path)?.permissions().mode() & 0o777))
        }
        Location::Ssh { host, path } => {
            let bytes = providers::ssh(
                host,
                "import os,sys\nprint(os.stat(sys.argv[1]).st_mode&0o777)",
                &[path],
                None,
            )?;
            Ok(Some(std::str::from_utf8(&bytes)?.trim().parse()?))
        }
        Location::Git {
            repo,
            reference,
            path,
        } => {
            let mut command = Command::new("git");
            command.arg("-C").arg(repo).args([
                "ls-tree",
                "-r",
                "-d",
                "--full-tree",
                reference,
                "--",
                path,
            ]);
            let result = providers::run(command)?;
            if !result.is_empty() {
                return Ok(Some(0o755));
            }
            let mut command = Command::new("git");
            command.arg("-C").arg(repo).args([
                "ls-tree",
                "-r",
                "--full-tree",
                reference,
                "--",
                path,
            ]);
            let result = providers::run(command)?;
            Ok(Some(if result.starts_with(b"100755 ") {
                0o755
            } else {
                0o644
            }))
        }
        _ => Ok(None),
    }
}
pub(crate) fn apply_permissions(location: &Location, mode: Option<u32>) -> Result<()> {
    let Some(mode) = mode else { return Ok(()) };
    match location {
        #[cfg(unix)]
        Location::Local { path } => {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
        }
        Location::Ssh { host, path } => {
            providers::ssh(
                host,
                "import os,sys\nos.chmod(sys.argv[1],int(sys.argv[2]))",
                &[path, &mode.to_string()],
                None,
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn same_identity(metadata: &fs::Metadata, path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::symlink_metadata(path)
            .is_ok_and(|current| current.dev() == metadata.dev() && current.ino() == metadata.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = (metadata, path);
        false
    }
}
pub(crate) fn hash_file(path: &std::path::Path) -> Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut chunk = [0; 65536];
    loop {
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        digest.update(&chunk[..count]);
    }
    Ok(digest.finalize().to_vec())
}
fn stream_download(mut command: Command, mut destination: fs::File) -> Result<()> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .context("Could not start provider download")?;
    let mut stdout = child.stdout.take().context("Provider stdout unavailable")?;
    let mut stderr = child.stderr.take().context("Provider stderr unavailable")?;
    let output = std::thread::spawn(move || std::io::copy(&mut stdout, &mut destination));
    let error = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.by_ref().take(65536).read_to_end(&mut bytes);
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(900);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let copied = output
        .join()
        .map_err(|_| anyhow::anyhow!("Download worker failed"))?;
    let errors = error.join().unwrap_or_default();
    let status =
        status.context("Provider download timed out after 15 minutes; source was preserved")?;
    ensure!(
        status.success(),
        "Provider download failed: {}",
        String::from_utf8_lossy(&errors)
    );
    copied?;
    Ok(())
}
