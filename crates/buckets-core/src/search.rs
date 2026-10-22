use crate::{Backend, Location};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Command};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchEngine {
    #[default]
    Auto,
    Fd,
    Rg,
    Fff,
    Builtin,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub engine: SearchEngine,
    pub hidden: bool,
    pub respect_gitignore: bool,
    pub max_results: usize,
}
impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            engine: SearchEngine::Auto,
            hidden: false,
            respect_gitignore: true,
            max_results: 500,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub location: Location,
    pub name: String,
    pub line: Option<u64>,
    pub snippet: Option<String>,
}

pub(crate) fn search(
    _backend: &Backend,
    root: &Location,
    query: &SearchQuery,
) -> Result<Vec<SearchHit>> {
    search_filtered(root, query, false)
}
/// Agent indexing needs regular files counted before the result limit, while
/// ordinary file-browser searches retain their directory results.
pub(crate) fn search_files(root: &Location, query: &SearchQuery) -> Result<Vec<SearchHit>> {
    search_filtered(root, query, true)
}
fn search_filtered(
    root: &Location,
    query: &SearchQuery,
    files_only: bool,
) -> Result<Vec<SearchHit>> {
    ensure!(query.text.len() <= 16384, "Search query is too long");
    let limit = query.max_results.min(10000);
    if limit == 0 {
        return Ok(Vec::new());
    }
    if let Location::Ssh { host, path } = root {
        let mut request = serde_json::to_value(query)?;
        request["files_only"] = serde_json::json!(files_only);
        let request = serde_json::to_string(&request)?;
        let result =
            crate::providers::ssh(host, include_str!("ssh_search.py"), &[path, &request], None)?;
        let values: Vec<serde_json::Value> =
            serde_json::from_slice(&result).context("Invalid remote search response")?;
        return values
            .into_iter()
            .map(|v| {
                let path = v["path"]
                    .as_str()
                    .context("Remote search omitted path")?
                    .to_owned();
                Ok(SearchHit {
                    location: Location::Ssh {
                        host: host.clone(),
                        path,
                    },
                    name: v["name"].as_str().unwrap_or("").to_owned(),
                    line: v["line"].as_u64(),
                    snippet: v["snippet"].as_str().map(ToOwned::to_owned),
                })
            })
            .collect();
    }
    let Location::Local { path } = root else {
        bail!(
            "Built-in search runs on local directories; remote and semantic search can be added by search plugins"
        )
    };
    ensure!(path.is_dir(), "Search root is not a directory");
    match query.engine {
        SearchEngine::Fff => {
            ensure!(
                !files_only,
                "File-only index enumeration requires builtin, fd, or rg search"
            );
            let registry = crate::providers::plugin_registry()?;
            let request = buckets_plugin::SearchRequest {
                query: query.text.clone(),
                roots: vec![buckets_plugin::IndexRoot {
                    path: path.to_string_lossy().into_owned(),
                    respect_gitignore: query.respect_gitignore,
                    follow_symlinks: false,
                }],
                limit,
                mode: buckets_plugin::SearchMode::Filename,
                config: serde_json::Value::Null,
            };
            registry
                .search("fff-search", request)?
                .into_iter()
                .map(|r| {
                    let location = Location::parse(&r.path)?;
                    Ok(SearchHit {
                        name: location.name(),
                        location,
                        line: r.line,
                        snippet: r.snippet,
                    })
                })
                .collect()
        }
        SearchEngine::Fd => fd(path, query, limit, files_only),
        SearchEngine::Rg => rg(path, query, limit),
        SearchEngine::Auto | SearchEngine::Builtin => builtin(path, query, limit, files_only),
    }
}
fn builtin(root: &Path, q: &SearchQuery, limit: usize, files_only: bool) -> Result<Vec<SearchHit>> {
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(!q.hidden)
        .ignore(q.respect_gitignore)
        .git_ignore(q.respect_gitignore)
        .git_exclude(q.respect_gitignore)
        .git_global(q.respect_gitignore)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_name(|a, b| a.cmp(b));
    let needle = q.text.to_lowercase();
    let mut out = Vec::new();
    for item in builder.build() {
        let entry = item.context("Search could not read a directory entry")?;
        if entry.path() == root
            || (files_only && !entry.file_type().is_some_and(|kind| kind.is_file()))
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        let relative = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_string_lossy();
        if relative.to_lowercase().contains(&needle) {
            out.push(SearchHit {
                location: Location::local(entry.path()),
                name: name.into_owned(),
                line: None,
                snippet: Some(relative.into_owned()),
            });
            if out.len() >= limit {
                break;
            }
        }
    }
    Ok(out)
}
fn fd(root: &Path, q: &SearchQuery, limit: usize, files_only: bool) -> Result<Vec<SearchHit>> {
    let mut command = Command::new("fd");
    command.current_dir(root).args([
        "--print0",
        "--color",
        "never",
        "--fixed-strings",
        "--no-require-git",
    ]);
    if files_only {
        command.args(["--type", "f"]);
    }
    if q.hidden {
        command.arg("--hidden");
    }
    if !q.respect_gitignore {
        command.arg("--no-ignore");
    }
    command.args(["--", &q.text, "."]);
    let output = bounded_output(command, limit * 8192, Some(limit), b'\0')?;
    output
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .take(limit)
        .map(|bytes| {
            let path = path_from_bytes(bytes);
            let absolute = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            Ok(SearchHit {
                name: absolute
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                location: Location::local(&absolute),
                line: None,
                snippet: absolute
                    .strip_prefix(root)
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned()),
            })
        })
        .collect()
}
fn rg(root: &Path, q: &SearchQuery, limit: usize) -> Result<Vec<SearchHit>> {
    let mut command = Command::new("rg");
    command.current_dir(root).args([
        "--json",
        "--line-number",
        "--fixed-strings",
        "--max-columns",
        "2000",
        "--glob",
        "!.git/**",
        "--no-require-git",
    ]);
    if q.hidden {
        command.arg("--hidden");
    }
    if !q.respect_gitignore {
        command.arg("--no-ignore");
    }
    command.args(["--", &q.text, "."]);
    let output = bounded_output(command, limit * 16384, Some(limit * 3 + 100), b'\n')?;
    let mut hits = Vec::new();
    for line in output.split(|b| *b == b'\n').filter(|b| !b.is_empty()) {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if value["type"] != "match" {
            continue;
        }
        let data = &value["data"];
        let Some(path) = data["path"]["text"].as_str() else {
            continue;
        };
        let absolute = root.join(path);
        hits.push(SearchHit {
            name: absolute
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            location: Location::local(absolute),
            line: data["line_number"].as_u64(),
            snippet: data["lines"]["text"]
                .as_str()
                .map(|s| s.trim_end().to_owned()),
        });
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}
// Drain stderr concurrently, cap output and stop children once enough results are obtained.
fn bounded_output(
    mut command: Command,
    max_bytes: usize,
    max_delimiters: Option<usize>,
    delimiter: u8,
) -> Result<Vec<u8>> {
    use std::{io::Read, process::Stdio};
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!("Could not start {program}; install it or choose Builtin search")
        })?;
    let mut stderr = child.stderr.take().context("Search stderr unavailable")?;
    let err_task = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stderr.by_ref().take(65536).read_to_end(&mut out);
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        out
    });
    let mut stdout = child.stdout.take().context("Search stdout unavailable")?;
    let child = std::sync::Arc::new(std::sync::Mutex::new(child));
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let watched = child.clone();
    let watchdog = std::thread::spawn(move || {
        if stop_rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .is_err()
        {
            let _ = watched.lock().expect("search child lock").kill();
            true
        } else {
            false
        }
    });
    let mut result = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut lines = 0;
    let mut limited = false;
    loop {
        let n = stdout.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        lines += buffer[..n].iter().filter(|&&b| b == delimiter).count();
        result.extend_from_slice(&buffer[..n]);
        if result.len() >= max_bytes || max_delimiters.is_some_and(|max| lines >= max) {
            limited = true;
            let _ = child.lock().expect("search child lock").kill();
            break;
        }
    }
    drop(stdout);
    let status = loop {
        if let Some(status) = child.lock().expect("search child lock").try_wait()? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let _ = stop_tx.send(());
    let timed_out = watchdog.join().unwrap_or(true);
    ensure!(!timed_out, "{program} search timed out after 60 seconds");
    let error = err_task.join().unwrap_or_default();
    ensure!(
        limited || status.success() || status.code() == Some(1),
        "{program} search failed: {}",
        String::from_utf8_lossy(&error).trim()
    );
    Ok(result)
}
#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> std::path::PathBuf {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::OsStr::from_bytes(bytes).into()
}
#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> std::path::PathBuf {
    String::from_utf8_lossy(bytes).as_ref().into()
}
