use crate::{Entry, Location};
use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use serde_json::Value;
use std::{
    fs,
    io::Read,
    process::{Command, Stdio},
    time::UNIX_EPOCH,
};

pub(crate) fn list(location: &Location) -> Result<Vec<Entry>> {
    match location {
        Location::Plugin { plugin, path } => {
            let value = plugin_request(plugin, "list", serde_json::json!({"path":path}))?;
            let items = value
                .as_array()
                .context("Plugin listing must be an array")?;
            items
                .iter()
                .map(|v| {
                    let name = v["name"]
                        .as_str()
                        .context("Plugin entry requires a name")?
                        .to_owned();
                    Ok(Entry {
                        location: location.join(&name)?,
                        name,
                        is_dir: v["is_dir"].as_bool().unwrap_or(false),
                        is_symlink: v["is_symlink"].as_bool().unwrap_or(false),
                        size: v["size"].as_u64().unwrap_or(0),
                        modified: v["modified"].as_u64(),
                    })
                })
                .collect()
        }
        Location::Local { path } => fs::read_dir(path)
            .with_context(|| format!("Cannot open {}", path.display()))?
            .map(|r| {
                let r = r?;
                let metadata = fs::symlink_metadata(r.path())?;
                let is_symlink = metadata.file_type().is_symlink();
                let is_dir = if is_symlink {
                    r.path().is_dir()
                } else {
                    metadata.is_dir()
                };
                Ok(Entry {
                    name: r.file_name().to_string_lossy().into_owned(),
                    location: Location::local(r.path()),
                    is_dir,
                    is_symlink,
                    size: metadata.len(),
                    modified: metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs()),
                })
            })
            .collect(),
        Location::Ssh { host, path } => {
            let script = "import os,sys,json,stat\np=sys.argv[1]\nr=[]\nwith os.scandir(p) as entries:\n for e in entries:\n  s=e.stat(follow_symlinks=False)\n  r.append(dict(name=e.name,is_dir=e.is_dir(),is_symlink=e.is_symlink(),size=s.st_size,modified=int(s.st_mtime)))\nprint(json.dumps(r))";
            let data = ssh(host, script, &[path], None)?;
            let values: Vec<Value> = serde_json::from_slice(&data)
                .context("Invalid SSH listing; remote requires python3")?;
            values
                .into_iter()
                .map(|v| {
                    let name = v["name"]
                        .as_str()
                        .context("Missing remote filename")?
                        .to_owned();
                    Ok(Entry {
                        location: location.join(&name)?,
                        name,
                        is_dir: v["is_dir"].as_bool().unwrap_or(false),
                        is_symlink: v["is_symlink"].as_bool().unwrap_or(false),
                        size: v["size"].as_u64().unwrap_or(0),
                        modified: v["modified"].as_u64(),
                    })
                })
                .collect()
        }
        Location::S3 {
            bucket,
            prefix,
            endpoint,
        } => {
            let mut entries = Vec::new();
            let prefix = if prefix.is_empty() {
                String::new()
            } else {
                format!("{}/", prefix.trim_end_matches('/'))
            };
            let mut token: Option<String> = None;
            loop {
                let mut cmd = aws(endpoint);
                cmd.args([
                    "s3api",
                    "list-objects-v2",
                    "--bucket",
                    bucket,
                    "--prefix",
                    &prefix,
                    "--delimiter",
                    "/",
                    "--max-keys",
                    "1000",
                    "--no-paginate",
                    "--output",
                    "json",
                ]);
                if let Some(t) = &token {
                    cmd.args(["--continuation-token", t]);
                }
                let value: Value = serde_json::from_slice(&run(cmd)?)?;
                if let Some(items) = value["CommonPrefixes"].as_array() {
                    for item in items {
                        if let Some(key) = item["Prefix"].as_str() {
                            let name = key.trim_end_matches('/').rsplit('/').next().unwrap_or("");
                            entries.push(Entry {
                                name: name.into(),
                                location: Location::S3 {
                                    bucket: bucket.clone(),
                                    prefix: key.to_owned(),
                                    endpoint: endpoint.clone(),
                                },
                                is_dir: true,
                                is_symlink: false,
                                size: 0,
                                modified: None,
                            });
                        }
                    }
                }
                if let Some(items) = value["Contents"].as_array() {
                    for item in items {
                        let key = item["Key"].as_str().context("Missing S3 object key")?;
                        if key == prefix {
                            continue;
                        }
                        let name = key.strip_prefix(&prefix).unwrap_or(key);
                        entries.push(Entry {
                            name: name.into(),
                            location: location.join(name)?,
                            is_dir: false,
                            is_symlink: false,
                            size: item["Size"].as_u64().unwrap_or(0),
                            modified: None,
                        });
                    }
                }
                if value["IsTruncated"].as_bool() != Some(true) {
                    break;
                }
                token = Some(
                    value["NextContinuationToken"]
                        .as_str()
                        .context("S3 omitted pagination token")?
                        .to_owned(),
                );
            }
            Ok(entries)
        }
        Location::Git {
            repo,
            reference,
            path,
        } => {
            let tree = if path.is_empty() {
                reference.clone()
            } else {
                format!("{reference}:{path}")
            };
            let mut c = Command::new("git");
            c.env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_NO_LAZY_FETCH", "1");
            c.arg("-C").arg(repo).args(["ls-tree", "-z", "-l", &tree]);
            let data = run(c)?;
            data.split(|b| *b == 0)
                .filter(|v| !v.is_empty())
                .map(|v| {
                    let text = String::from_utf8_lossy(v);
                    let (meta, name) = text.split_once('\t').context("Invalid Git tree listing")?;
                    let fields: Vec<_> = meta.split_whitespace().collect();
                    ensure!(fields.len() >= 4, "Invalid Git tree entry");
                    Ok(Entry {
                        name: name.into(),
                        location: location.join(name)?,
                        is_dir: fields[1] == "tree",
                        is_symlink: fields[0] == "120000",
                        size: fields[3].parse().unwrap_or(0),
                        modified: None,
                    })
                })
                .collect()
        }
    }
}

pub(crate) struct ReadResult {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}
pub(crate) fn read_prefix(location: &Location, limit: usize) -> Result<ReadResult> {
    if let Location::Plugin { plugin, path } = location {
        let value = plugin_request(
            plugin,
            "read",
            serde_json::json!({"path":path,"max_bytes":limit}),
        )?;
        let mut bytes = if let Some(data) = value["data_base64"].as_str() {
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .context("Plugin returned invalid base64 data")?
        } else {
            value["text"]
                .as_str()
                .context("Plugin read requires text or data_base64")?
                .as_bytes()
                .to_vec()
        };
        let truncated = value["truncated"].as_bool().unwrap_or(false) || bytes.len() > limit;
        bytes.truncate(limit);
        return Ok(ReadResult { bytes, truncated });
    }
    Ok(ReadResult {
        bytes: read_prefix_bytes(location, limit)?,
        truncated: false,
    })
}
fn read_prefix_bytes(location: &Location, limit: usize) -> Result<Vec<u8>> {
    match location {
        Location::Plugin { .. } => bail!("Plugin read must use provider dispatch"),
        Location::Local { path } => {
            let mut out = Vec::new();
            fs::File::open(path)?
                .take(limit as u64)
                .read_to_end(&mut out)?;
            Ok(out)
        }
        Location::Ssh { host, path } => ssh(
            host,
            "import sys\nwith open(sys.argv[1],'rb') as f: sys.stdout.buffer.write(f.read(int(sys.argv[2])))",
            &[path, &limit.to_string()],
            None,
        ),
        Location::S3 {
            bucket,
            prefix,
            endpoint,
        } => {
            ensure!(!prefix.is_empty(), "Select an object to preview");
            let metadata = s3_head(location)?;
            if metadata["ContentLength"].as_u64() == Some(0) {
                return Ok(Vec::new());
            }
            let tmp = temp_path("preview");
            let reservation = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            drop(reservation);
            let mut c = aws(endpoint);
            c.args([
                "s3api",
                "get-object",
                "--bucket",
                bucket,
                "--key",
                prefix,
                "--range",
                &format!("bytes=0-{}", limit.saturating_sub(1)),
            ])
            .arg(&tmp);
            let result = run(c).and_then(|_| fs::read(&tmp).map_err(Into::into));
            let _ = fs::remove_file(&tmp);
            result
        }
        Location::Git {
            repo,
            reference,
            path,
        } => {
            ensure!(!path.is_empty(), "Select a Git file to preview");
            let mut c = Command::new("git");
            c.env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_NO_LAZY_FETCH", "1");
            c.arg("-C")
                .arg(repo)
                .args(["show", &format!("{reference}:{path}")]);
            read_command_prefix(c, limit)
        }
    }
}

pub(crate) fn read_all(location: &Location) -> Result<Vec<u8>> {
    // Cross-provider copy is intentionally capped until a streaming provider API is added.
    let result = read_prefix(location, 128 * 1024 * 1024 + 1)?;
    ensure!(
        !result.truncated,
        "Provider returned a truncated file; copying was refused to prevent data loss"
    );
    let data = result.bytes;
    ensure!(
        data.len() <= 128 * 1024 * 1024,
        "Cross-provider copies currently support files up to 128 MiB; use the provider CLI for larger files"
    );
    Ok(data)
}
pub(crate) fn write_new(location: &Location, data: &[u8]) -> Result<()> {
    use std::io::Write;
    match location {
        Location::Plugin { plugin, path } => {
            let mut request = serde_json::json!({"path":path,"data_base64":base64::engine::general_purpose::STANDARD.encode(data),"create_new":true});
            if let Ok(text) = std::str::from_utf8(data) {
                request["text"] = Value::String(text.to_owned());
            }
            plugin_request(plugin, "write", request).map(|_| ())
        }
        Location::Local { path } => {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
            if let Err(e) = f.write_all(data).and_then(|_| f.sync_all()) {
                drop(f);
                let _ = fs::remove_file(path);
                return Err(e.into());
            }
            Ok(())
        }
        Location::Ssh { host, path } => {
            ssh(
                host,
                "import sys,os\np=sys.argv[1]\nf=open(p,'xb')\ntry:\n f.write(sys.stdin.buffer.read());f.flush();os.fsync(f.fileno())\nexcept BaseException:\n f.close();os.unlink(p);raise\nf.close()",
                &[path],
                Some(data),
            )?;
            Ok(())
        }
        Location::S3 {
            bucket,
            prefix,
            endpoint,
        } => {
            ensure!(!prefix.is_empty(), "Destination must include an object key");
            let temp = temp_path("upload");
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            f.write_all(data)?;
            drop(f);
            let mut c = aws(endpoint);
            c.args([
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
            .arg(&temp);
            let result = run(c).map(|_| ());
            let _ = fs::remove_file(temp);
            result
        }
        Location::Git { .. } => {
            bail!("Git snapshots are read-only; open the repository as a local directory to edit")
        }
    }
}
pub(crate) fn ssh(
    host: &str,
    script: &str,
    args: &[&str],
    input: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let c = ssh_command(host, script, args)?;
    run_with_input(c, input.map(ToOwned::to_owned), 60)
}

pub(crate) fn ssh_command(host: &str, script: &str, args: &[&str]) -> Result<Command> {
    crate::location::validate_host(host)?;
    let remote = format!(
        "python3 -c {} {}",
        shell_quote(script),
        args.iter()
            .map(|s| shell_quote(s))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let mut c = Command::new("ssh");
    c.stdin(Stdio::null());
    c.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "--",
        host,
        &remote,
    ]);
    Ok(c)
}

pub(crate) fn aws(endpoint: &Option<String>) -> Command {
    let mut c = Command::new("aws");
    c.stdin(Stdio::null());
    c.env("AWS_PAGER", "");
    c.args(["--cli-connect-timeout", "10", "--cli-read-timeout", "60"]);
    if let Some(e) = endpoint {
        c.args(["--endpoint-url", e]);
    }
    c
}
pub(crate) fn run(mut c: Command) -> Result<Vec<u8>> {
    c.stdin(Stdio::null());
    run_with_input(c, None, 60)
}
pub(crate) fn run_long(c: Command) -> Result<Vec<u8>> {
    run_with_input(c, None, 900)
}
fn run_with_input(mut c: Command, input: Option<Vec<u8>>, timeout_secs: u64) -> Result<Vec<u8>> {
    if input.is_some() {
        c.stdin(Stdio::piped());
    }
    let name = c.get_program().to_string_lossy().into_owned();
    let mut child = c
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!("Could not run {name}; install it and configure credentials first")
        })?;
    let writer = input.map(|bytes| {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("piped stdin");
        std::thread::spawn(move || stdin.write_all(&bytes))
    });
    let stdout = child.stdout.take().context("Command stdout unavailable")?;
    let stderr = child.stderr.take().context("Command stderr unavailable")?;
    let output = std::thread::spawn(move || {
        let mut data = Vec::new();
        stdout
            .take(32 * 1024 * 1024 + 1)
            .read_to_end(&mut data)
            .map(|_| data)
    });
    let errors = std::thread::spawn(move || {
        let mut data = Vec::new();
        stderr.take(65536).read_to_end(&mut data).map(|_| data)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let data = output
        .join()
        .map_err(|_| anyhow::anyhow!("Command output reader failed"))??;
    let error = errors
        .join()
        .map_err(|_| anyhow::anyhow!("Command error reader failed"))??;
    let status =
        status.with_context(|| format!("{name} timed out after {timeout_secs} seconds"))?;
    ensure!(
        data.len() <= 32 * 1024 * 1024,
        "{name} output exceeded 32 MiB; narrow this directory or use the provider CLI"
    );
    ensure!(
        status.success(),
        "{name} failed: {}",
        String::from_utf8_lossy(&error).trim()
    );
    if let Some(writer) = writer {
        writer
            .join()
            .map_err(|_| anyhow::anyhow!("Command input writer failed"))??;
    }
    Ok(data)
}
fn read_command_prefix(mut command: Command, limit: usize) -> Result<Vec<u8>> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut data = Vec::new();
    let result = child
        .stdout
        .take()
        .context("Command stdout unavailable")?
        .take(limit as u64)
        .read_to_end(&mut data);
    let status = child.try_wait()?;
    if status.is_none() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    result?;
    ensure!(
        !data.is_empty() || status.success(),
        "Cannot read Git object"
    );
    Ok(data)
}
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn temp_path(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "buckets-{label}-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    ))
}

pub(crate) fn plugin_registry() -> Result<buckets_plugin::PluginRegistry> {
    let directory = std::env::var_os("BUCKETS_PLUGIN_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("BUCKETS_CONFIG_DIR")
                .map(|p| std::path::PathBuf::from(p).join("plugins"))
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|p| std::path::PathBuf::from(p).join(".config/buckets/plugins"))
        })
        .context("Plugin directory unavailable")?;
    let settings = std::env::var_os("BUCKETS_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|p| std::path::PathBuf::from(p).join(".config/buckets"))
        })
        .context("Settings directory unavailable")?
        .join("settings.json");
    buckets_plugin::PluginRegistry::discover_with_settings_file(directory, settings)
}
pub(crate) fn plugin_request(id: &str, method: &str, params: Value) -> Result<Value> {
    plugin_registry()?.provider_request(id, method, params)
}

pub(crate) fn s3_head(location: &Location) -> Result<Value> {
    let Location::S3 {
        bucket,
        prefix,
        endpoint,
    } = location
    else {
        bail!("Expected an S3 object")
    };
    let mut command = aws(endpoint);
    command.args([
        "s3api",
        "head-object",
        "--bucket",
        bucket,
        "--key",
        prefix,
        "--output",
        "json",
    ]);
    serde_json::from_slice(&run(command)?).context("Invalid S3 object metadata")
}
pub(crate) fn s3_delete_if_match(location: &Location, etag: &str) -> Result<()> {
    let Location::S3 {
        bucket,
        prefix,
        endpoint,
    } = location
    else {
        bail!("Expected an S3 object")
    };
    ensure!(!prefix.is_empty(), "Cannot delete a bucket root");
    let mut command = aws(endpoint);
    command.args([
        "s3api",
        "delete-object",
        "--bucket",
        bucket,
        "--key",
        prefix,
        "--if-match",
        etag,
    ]);
    run(command).map(|_| ())
}
