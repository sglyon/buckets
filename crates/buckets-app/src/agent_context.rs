//! Bounded context admission for agent services. No provider IO runs on the UI thread.
use anyhow::{Context, Result, ensure};
use buckets_core::{AgentBroker, AgentRoot, Location};
use buckets_plugin::{AgentMode, PluginManifest, WorkspaceContext};
use std::path::{Path, PathBuf};

/// Unique host-launch epoch: admission IDs must never collide after a restart.
pub fn launch_epoch() -> &'static str {
    static EPOCH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    EPOCH.get_or_init(|| {
        format!(
            "{:x}-{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            std::process::id()
        )
    })
}

/// Opaque bounded admission IDs distinguish fresh questions across launches.
pub fn admission_id(workspace: &str, pane: u64, generation: u64, revision: u64) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&(launch_epoch(), workspace, pane, generation, revision))
        .expect("admission serialization");
    format!("request-{:x}", Sha256::digest(bytes))
}

/// A local cancellation flag must never hide a failed durable receipt or timeout.
pub fn cancellation_message(error: Option<&str>) -> (String, bool) {
    match error {
        None | Some("Agent request cancelled" | "Request cancelled") => (
            "Cancelled. Submit a new request to run again.".into(),
            false,
        ),
        Some(error) => (
            format!("Work stopped; cancellation could not be confirmed: {error}"),
            true,
        ),
    }
}

/// An explicit initial root set defines a workspace. Ordinary navigation does
/// not change this identity; restoring a session uses its saved identity.
pub fn workspace_for_launch(
    directory: &Path,
    explicit_roots: Option<&[Location]>,
    restored: Option<&str>,
) -> String {
    if let Some(roots) = explicit_roots {
        use sha2::{Digest, Sha256};
        let mut names = roots
            .iter()
            .map(|location| {
                let mut location = location.clone();
                let path = match &mut location {
                    Location::Local { path } => Some(path),
                    Location::Git { repo, .. } => Some(repo),
                    _ => None,
                };
                if let Some(path) = path {
                    // Resolve existing symlinks before `..`: lexical popping can
                    // otherwise merge physically different explicit workspaces.
                    if let Ok(canonical) = path.canonicalize() {
                        *path = canonical;
                    } else {
                        // For missing/offline paths, retain ParentDir semantics.
                        // A later successful launch may establish a canonical ID.
                        *path = path
                            .components()
                            .filter(|component| !matches!(component, std::path::Component::CurDir))
                            .collect();
                    }
                }
                location.display()
            })
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        let bytes = serde_json::to_vec(&names).expect("string list serialization");
        return format!("workspace-{:x}", Sha256::digest(bytes));
    }
    if let Some(id) = restored.filter(|id| {
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    }) {
        return id.to_owned();
    }
    workspace_identity(directory, None)
}

/// Search/index use the explicit index scope. Asking can search all admitted
/// pane roots as well as configured roots, using their admitted policies.
pub fn dispatch_tool(
    mode: AgentMode,
    read: &AgentBroker,
    search: Option<&AgentBroker>,
    method: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value> {
    if matches!(mode, AgentMode::Search | AgentMode::Index)
        && method == "files.search"
        && let Some(search) = search
    {
        return search.handle(method, params);
    }
    read.handle(method, params)
}

pub fn workspace_identity(directory: &Path, restored: Option<&str>) -> String {
    fn valid(id: &str) -> bool {
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    }
    let path = directory.join("workspace.id");
    if let Ok(id) = std::fs::read_to_string(&path)
        && valid(id.trim())
    {
        return id.trim().to_owned();
    }
    let id = restored
        .filter(|id| valid(id))
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "workspace-{:x}-{:x}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                std::process::id()
            )
        });
    let _ = std::fs::create_dir_all(directory);
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        let _ = file.write_all(id.as_bytes());
        let _ = file.sync_all();
    } else if let Ok(existing) = std::fs::read_to_string(path)
        && valid(existing.trim())
    {
        return existing.trim().to_owned();
    }
    id
}

/// Root admission intersects explicitly open panes with manifest grants. A broad
/// manifest grant never expands a pane root. Out-of-root links are excluded.
pub fn admit_context(
    mut context: WorkspaceContext,
    manifest: &PluginManifest,
) -> Result<(WorkspaceContext, Vec<AgentRoot>, Option<Vec<AgentRoot>>)> {
    let local_grants = manifest
        .permissions
        .read_roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .collect::<Vec<_>>();
    let mut roots = vec![];
    for pane in &context.panes {
        let location = Location::parse(&pane.location)?;
        let local = match &location {
            Location::Local { path } => Some(path),
            Location::Git { repo, .. } => Some(repo),
            _ => None,
        };
        let allowed = if let Some(path) = local {
            path.canonicalize()
                .is_ok_and(|path| local_grants.iter().any(|root| path.starts_with(root)))
        } else {
            manifest.permissions.network
        };
        if allowed {
            roots.push(AgentRoot {
                location: location.display(),
                respect_gitignore: true,
                follow_symlinks: false,
            });
        }
    }
    let configured = match manifest.config.get("index_roots") {
        None => vec![],
        Some(value) => value
            .as_array()
            .context("AI index_roots must be an array of {location,respect_gitignore} objects")?
            .clone(),
    };
    ensure!(
        configured.len() <= 64,
        "At most 64 index roots may be configured"
    );
    let mut search_roots = vec![];
    for (index, value) in configured.iter().enumerate() {
        let location = value
            .get("location")
            .and_then(serde_json::Value::as_str)
            .with_context(|| format!("index_roots[{index}].location is required"))?;
        let respect_gitignore = value
            .get("respect_gitignore")
            .and_then(serde_json::Value::as_bool)
            .with_context(|| {
                format!("index_roots[{index}].respect_gitignore must be explicitly true or false")
            })?;
        ensure!(
            !value
                .get("follow_symlinks")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            "index_roots[{index}] cannot follow symlinks"
        );
        let parsed =
            Location::parse(location).with_context(|| format!("Invalid index root {index}"))?;
        let local = match &parsed {
            Location::Local { path } => Some(path),
            Location::Git { repo, .. } => Some(repo),
            _ => None,
        };
        if let Some(path) = local {
            let path = path
                .canonicalize()
                .with_context(|| format!("Configured index root does not exist: {location}"))?;
            ensure!(
                local_grants.iter().any(|root| path.starts_with(root)),
                "Configured index root is outside this plugin's read permissions: {location}"
            );
        } else {
            ensure!(
                manifest.permissions.network,
                "Configured remote index root requires this plugin's network permission: {location}"
            );
        }
        search_roots.push(AgentRoot {
            location: parsed.display(),
            respect_gitignore,
            follow_symlinks: false,
        });
    }
    roots.extend(search_roots.iter().cloned());
    let empty = WorkspaceContext {
        workspace_id: context.workspace_id.clone(),
        revision: context.revision,
        panes: vec![],
    };
    let broker = AgentBroker::new(empty, roots.clone(), manifest.permissions.network)?;
    context.panes.retain_mut(|pane| {
        if broker.authorize(&pane.location).is_err() {
            return false;
        }
        pane.selected
            .retain(|location| broker.authorize(location).is_ok());
        if pane
            .cursor
            .as_ref()
            .is_some_and(|location| broker.authorize(location).is_err())
        {
            pane.cursor = None;
        }
        pane.entries
            .retain(|entry| broker.authorize(&entry.location).is_ok());
        true
    });
    ensure!(
        serde_json::to_vec(&context)?.len() <= 524_288,
        "Agent context exceeds 512 KiB"
    );
    Ok((
        context,
        roots,
        (!search_roots.is_empty()).then_some(search_roots),
    ))
}
pub fn storage_directory(config: &Path, plugin: &str, workspace: &str) -> PathBuf {
    let plugin = if matches!(plugin, "." | "..") || plugin.contains(['/', '\\']) {
        format!(
            "plugin-{}",
            plugin
                .bytes()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
    } else {
        plugin.to_owned()
    };
    config.join("agents").join(plugin).join(workspace)
}

#[cfg(test)]
mod tests {
    #[test]
    fn cancellation_failure_is_not_hidden_by_local_intent() {
        let (message, error) = super::cancellation_message(Some(
            "Could not durably record explicit agent cancellation: disk full",
        ));
        assert!(error);
        assert!(message.contains("disk full"));
        assert!(super::cancellation_message(Some("Agent request timed out")).1);
        assert!(!super::cancellation_message(Some("Agent request cancelled")).1);
    }
    #[test]
    fn admission_ids_are_bounded_for_the_longest_workspace_identity() {
        let workspace = "w".repeat(128);
        let first = super::admission_id(&workspace, u64::MAX, u64::MAX, 0);
        assert!(first.len() <= 128);
        assert_eq!(
            first,
            super::admission_id(&workspace, u64::MAX, u64::MAX, 0)
        );
        assert_ne!(
            first,
            super::admission_id(&workspace, u64::MAX, u64::MAX, 1)
        );
    }
}
