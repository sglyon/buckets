//! Transfer code keeps verification and progress reporting outside the renderer.
use crate::{Backend, Entry, Location, providers};
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Default)]
struct Progress {
    files: usize,
    bytes: u64,
}
impl Backend {
    pub fn stat(&self, location: &Location) -> Result<Entry> {
        if let Location::Local { path } = location {
            let metadata = fs::symlink_metadata(path)?;
            return Ok(Entry {
                name: location.name(),
                location: location.clone(),
                is_dir: path.is_dir(),
                is_symlink: metadata.file_type().is_symlink(),
                size: metadata.len(),
                modified: metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
            });
        }
        if let Some(parent) = location.parent() {
            let entries = self.list(&parent)?;
            if let Some(entry) = entries
                .iter()
                .find(|entry| entry.location == *location)
                .or_else(|| {
                    entries
                        .iter()
                        .find(|entry| entry.is_dir && same_directory(&entry.location, location))
                })
            {
                return Ok(entry.clone());
            }
        }
        bail!("Source does not exist: {location}")
    }
    pub(crate) fn copy_tree(&self, source: &Location, destination: &Location) -> Result<()> {
        ensure!(
            !contains_location(source, destination),
            "Cannot copy a directory inside itself"
        );
        validate_remote_destination(source, destination)?;
        let mut progress = Progress::default();
        let result = copy_one(self, source, destination, &mut progress);
        result.with_context(||format!("Transfer stopped after {} files ({} bytes). Completed copies remain at {}; source files were preserved",progress.files,progress.bytes,destination))
    }
    pub(crate) fn move_s3(&self, source: &Location, destination: &Location) -> Result<()> {
        ensure!(
            !contains_location(source, destination),
            "Cannot move a directory inside itself"
        );
        let entry = self.stat(source)?;
        if entry.is_dir {
            let marker = directory_location(source);
            let marker_before = match providers::s3_head(&marker) {
                Ok(metadata) => {
                    ensure!(
                        metadata["ContentLength"].as_u64() == Some(0),
                        "A nonempty S3 object ending with '/' cannot be treated as a directory marker; source was preserved"
                    );
                    Some(metadata)
                }
                Err(error) if object_missing(&error) => None,
                Err(error) => return Err(error),
            };
            ensure_destination_absent(self, destination)?;
            self.create_dir(destination)?;
            let entries = self.list(source)?;
            for (completed, child) in entries.into_iter().enumerate() {
                let target = destination.join(&child.name)?;
                if let Err(error) = self.move_s3(&child.location, &target) {
                    bail!(
                        "S3 directory move stopped after {completed} entries: {error:#}. Completed entries are at {destination}; remaining originals are at {source}"
                    )
                }
            }
            ensure!(
                self.list(source)?.is_empty(),
                "Source directory changed during transfer; new source entries were preserved"
            );
            if let Some(before) = marker_before {
                let current = providers::s3_head(&marker)?;
                ensure!(
                    before["ETag"] == current["ETag"]
                        && before["VersionId"] == current["VersionId"],
                    "Source directory marker changed; both locations were preserved"
                );
                providers::s3_delete_if_match(
                    &marker,
                    before["ETag"].as_str().context("S3 omitted marker ETag")?,
                )?;
            }
            return Ok(());
        }
        let before = providers::s3_head(source)?;
        let etag = before["ETag"].as_str().context("S3 omitted source ETag")?;
        let spool = crate::spool::Spool::download_checked(source, Some(&before))?;
        let after_read = providers::s3_head(source)?;
        ensure!(
            before["ETag"] == after_read["ETag"] && before["VersionId"] == after_read["VersionId"],
            "Source changed while reading; move refused"
        );
        ensure!(
            before["ContentLength"].as_u64() == Some(spool.size),
            "Source length changed while reading"
        );
        spool.upload(destination)?;
        let verification = crate::spool::Spool::download(destination)
            .context("Destination exists, but verification failed; original was preserved")?;
        ensure!(
            spool.size == verification.size && spool.digest == verification.digest,
            "Destination verification failed; original was preserved and the destination copy remains"
        );
        let after = providers::s3_head(source)?;
        ensure!(
            before["ETag"] == after["ETag"] && before["VersionId"] == after["VersionId"],
            "Source changed during transfer; both copies were preserved"
        );
        providers::s3_delete_if_match(source,etag).context("Destination was copied and verified, but conditional source removal failed; both copies remain")
    }
}
fn copy_one(
    backend: &Backend,
    source: &Location,
    destination: &Location,
    progress: &mut Progress,
) -> Result<()> {
    let entry = backend.stat(source)?;
    ensure_destination_absent(backend, destination)?;
    if entry.is_symlink {
        let target = read_symlink(source)?;
        match destination {
            #[cfg(unix)]
            Location::Local { path } => std::os::unix::fs::symlink(target, path)?,
            Location::Ssh { host, path } => {
                providers::ssh(
                    host,
                    "import os,sys\nos.symlink(sys.argv[1],sys.argv[2])",
                    &[&target, path],
                    None,
                )?;
            }
            _ => bail!("Destination provider cannot preserve symlinks"),
        }
        progress.files += 1;
        return Ok(());
    }
    if entry.is_dir {
        backend.create_dir(destination)?;
        for child in backend.list(source)? {
            copy_one(
                backend,
                &child.location,
                &destination.join(&child.name)?,
                progress,
            )?;
        }
        crate::spool::apply_permissions(destination, crate::spool::permissions(source)?)?;
    } else {
        let spool = crate::spool::Spool::download(source)?;
        ensure!(
            spool.size == entry.size,
            "Source size changed or provider returned incomplete data; copying was refused"
        );
        spool.upload(destination)?;
        progress.files += 1;
        progress.bytes += spool.size;
    }
    Ok(())
}
fn ensure_destination_absent(backend: &Backend, location: &Location) -> Result<()> {
    if let Location::Local { path } = location {
        match fs::symlink_metadata(path) {
            Ok(_) => bail!("Destination already exists: {location}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
    let parent = location
        .parent()
        .context("Cannot replace a filesystem root")?;
    let entries = backend.list(&parent)?;
    ensure!(
        !entries
            .iter()
            .any(|e| e.location == *location || same_directory(&e.location, location)),
        "Destination already exists: {location}"
    );
    Ok(())
}
fn same_directory(a: &Location, b: &Location) -> bool {
    match (a, b) {
        (
            Location::S3 {
                bucket: a,
                prefix: ap,
                endpoint: ae,
            },
            Location::S3 {
                bucket: b,
                prefix: bp,
                endpoint: be,
            },
        ) => a == b && ae == be && ap.trim_end_matches('/') == bp.trim_end_matches('/'),
        _ => a == b,
    }
}
fn directory_location(location: &Location) -> Location {
    if let Location::S3 {
        bucket,
        prefix,
        endpoint,
    } = location
    {
        Location::S3 {
            bucket: bucket.clone(),
            prefix: format!("{}/", prefix.trim_end_matches('/')),
            endpoint: endpoint.clone(),
        }
    } else {
        location.clone()
    }
}
fn contains_location(source: &Location, destination: &Location) -> bool {
    match (source, destination) {
        (Location::Local { path: a }, Location::Local { path: b }) => {
            let a = fs::canonicalize(a).ok();
            let parent = b.parent().and_then(|p| fs::canonicalize(p).ok());
            a.zip(parent).is_some_and(|(a, b)| b.starts_with(a))
        }
        (Location::Ssh { host: a, path: ap }, Location::Ssh { host: b, path: bp }) => {
            a == b && nested(ap, bp)
        }
        (
            Location::S3 {
                bucket: a,
                prefix: ap,
                endpoint: ae,
            },
            Location::S3 {
                bucket: b,
                prefix: bp,
                endpoint: be,
            },
        ) => a == b && ae == be && nested(ap, bp),
        (
            Location::Plugin {
                plugin: a,
                path: ap,
            },
            Location::Plugin {
                plugin: b,
                path: bp,
            },
        ) => a == b && nested(ap, bp),
        _ => false,
    }
}
fn nested(a: &str, b: &str) -> bool {
    b == a || b.starts_with(&format!("{}/", a.trim_end_matches('/')))
}

#[derive(Debug, PartialEq, Eq)]
struct Fingerprint {
    kind: u8,
    size: u64,
    modified: Option<std::time::SystemTime>,
    digest: Vec<u8>,
}
fn snapshot(root: &Path) -> Result<BTreeMap<PathBuf, Fingerprint>> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Fingerprint>) -> Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        let (kind, digest) = if metadata.file_type().is_symlink() {
            (
                2,
                fs::read_link(path)?.as_os_str().as_encoded_bytes().to_vec(),
            )
        } else if metadata.is_dir() {
            (1, Vec::new())
        } else {
            ensure!(
                metadata.is_file(),
                "Special files cannot be moved across filesystems"
            );
            let mut file = fs::File::open(path)?;
            let mut hasher = Sha256::new();
            let mut buffer = [0; 65536];
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
            }
            (0, hasher.finalize().to_vec())
        };
        out.insert(
            path.strip_prefix(root)?.to_owned(),
            Fingerprint {
                kind,
                size: if kind == 0 { metadata.len() } else { 0 },
                modified: metadata.modified().ok(),
                digest,
            },
        );
        if kind == 1 {
            for item in fs::read_dir(path)? {
                walk(root, &item?.path(), out)?;
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out)?;
    Ok(out)
}
fn same_content(a: &BTreeMap<PathBuf, Fingerprint>, b: &BTreeMap<PathBuf, Fingerprint>) -> bool {
    a.len() == b.len()
        && a.iter().all(|(path, a)| {
            b.get(path)
                .is_some_and(|b| a.kind == b.kind && a.size == b.size && a.digest == b.digest)
        })
}
pub(crate) fn move_local_verified(source: &Path, destination: &Path) -> Result<()> {
    let before = snapshot(source)?;
    crate::operations::copy_local(source, destination)?;
    let after = snapshot(source)?;
    let copied = snapshot(destination)?;
    ensure!(
        before == after && same_content(&before, &copied),
        "Source changed or destination verification failed; both paths were preserved"
    );
    let parent = source.parent().context("Source has no parent")?;
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let staging = parent.join(format!(".buckets-move-{}-{token}", std::process::id()));
    crate::operations::rename_exclusive(source, &staging)?;
    let staged = snapshot(&staging)?;
    if staged != after {
        let restored = crate::operations::rename_exclusive(&staging, source).is_ok();
        bail!(
            "Source changed before finalization; destination remains. Original {}",
            if restored {
                "was restored".to_owned()
            } else {
                format!("is recoverable at {}", staging.display())
            }
        )
    }
    let result = if fs::symlink_metadata(&staging)?.is_dir() {
        fs::remove_dir_all(&staging)
    } else {
        fs::remove_file(&staging)
    };
    result.with_context(|| {
        format!(
            "Destination was verified; source cleanup failed and remaining original data is at {}",
            staging.display()
        )
    })
}

fn object_missing(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}");
    text.contains("(404)")
        || text.contains("(NoSuchKey)")
        || text.contains("(NotFound)")
        || text.ends_with("aws failed: NotFound")
        || text.ends_with("aws failed: Not Found")
}

pub(crate) fn read_symlink(location: &Location) -> Result<String> {
    match location {
        Location::Local { path } => Ok(fs::read_link(path)?.to_string_lossy().into_owned()),
        Location::Ssh { host, path } => Ok(serde_json::from_slice::<String>(&providers::ssh(
            host,
            "import os,sys,json\nprint(json.dumps(os.readlink(sys.argv[1])))",
            &[path],
            None,
        )?)?),
        Location::Git { .. } => Ok(String::from_utf8(providers::read_all(location)?)?),
        _ => bail!("Provider does not expose symlink targets"),
    }
}
fn validate_remote_destination(source: &Location, destination: &Location) -> Result<()> {
    if let (
        Location::Ssh { host, path },
        Location::Ssh {
            host: other,
            path: target,
        },
    ) = (source, destination)
        && host == other
    {
        providers::ssh(
            host,
            "import os,sys\na,b=sys.argv[1:]\nif os.path.isdir(a) and not os.path.islink(a):\n source=os.path.realpath(a);parent=os.path.realpath(os.path.dirname(b))\n if parent==source or parent.startswith(source+os.sep): raise ValueError('Cannot copy a directory inside itself through a symlink alias')",
            &[path, target],
            None,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verified_move_preserves_contents_and_symlinks() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("source");
        let dst = t.path().join("target");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("data"), "contents").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("data", src.join("link")).unwrap();
        move_local_verified(&src, &dst).unwrap();
        assert!(!src.exists());
        assert_eq!(fs::read_to_string(dst.join("data")).unwrap(), "contents");
        #[cfg(unix)]
        assert_eq!(
            fs::read_link(dst.join("link")).unwrap(),
            PathBuf::from("data")
        );
    }
    #[test]
    fn failed_verified_move_preserves_source_and_destination() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("source");
        let dst = t.path().join("target");
        fs::write(&src, "source").unwrap();
        fs::write(&dst, "existing").unwrap();
        assert!(move_local_verified(&src, &dst).is_err());
        assert_eq!(fs::read_to_string(src).unwrap(), "source");
        assert_eq!(fs::read_to_string(dst).unwrap(), "existing");
    }
}
