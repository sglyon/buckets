//! Provider-aware, bounded read tools for trusted agent plugins. Grants are
//! supplied by the application from explicit workspace locations, never models.
use crate::{Backend, Location, SearchEngine, SearchQuery};
use anyhow::{Context, Result, bail, ensure};
use buckets_plugin::{AgentResult, IndexRoot, SearchMode, SearchRequest, WorkspaceContext};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentRoot {
    pub location: String,
    #[serde(default = "yes")]
    pub respect_gitignore: bool,
    #[serde(default)]
    pub follow_symlinks: bool,
}
fn yes() -> bool {
    true
}

pub struct AgentBroker {
    context: WorkspaceContext,
    roots: Vec<(AgentRoot, Location)>,
    allow_network: bool,
}
impl AgentBroker {
    pub fn new(
        context: WorkspaceContext,
        roots: Vec<AgentRoot>,
        allow_network: bool,
    ) -> Result<Self> {
        ensure!(
            !context.workspace_id.is_empty() && context.workspace_id.len() <= 128,
            "Invalid workspace identity"
        );
        ensure!(
            context.panes.len() <= 32 && roots.len() <= 64,
            "Too many agent panes or roots"
        );
        let roots = roots
            .into_iter()
            .map(|grant| {
                let mut root = Location::parse(&grant.location)?;
                validate_components(&root)?;
                match &mut root {
                    Location::Local { path } => {
                        *path = path.canonicalize().context("Agent root does not exist")?
                    }
                    Location::Git { repo, .. } => {
                        *repo = repo
                            .canonicalize()
                            .context("Agent repository does not exist")?
                    }
                    _ => {}
                }
                ensure!(
                    !grant.follow_symlinks,
                    "Agent indexing does not support following symlinks"
                );
                Ok((grant, root))
            })
            .collect::<Result<_>>()?;
        let mut broker = Self {
            context,
            roots,
            allow_network,
        };
        let mut panes = std::mem::take(&mut broker.context.panes);
        for pane in &mut panes {
            ensure!(
                pane.id.len() <= 128 && pane.entries.len() <= 256 && pane.selected.len() <= 256,
                "Agent snapshot exceeds bounds"
            );
            broker.authorize(&pane.location)?;
            for location in &pane.selected {
                broker
                    .authorize(location)
                    .context("Selected file is outside the agent roots")?;
            }
            if pane
                .cursor
                .as_ref()
                .is_some_and(|location| broker.authorize(location).is_err())
            {
                pane.cursor = None;
            }
            pane.entries
                .retain(|entry| broker.authorize(&entry.location).is_ok());
        }
        broker.context.panes = panes;
        Ok(broker)
    }
    /// Validate a model-provided reference against a host grant. Local symlinks
    /// undergo real canonical containment. SSH paths are additionally realpath
    /// checked on the server before any content/traversal operation.
    pub fn authorize(&self, input: &str) -> Result<Location> {
        ensure!(
            input.len() <= 8192 && !input.chars().any(char::is_control),
            "Invalid agent location"
        );
        let mut location = Location::parse(input)?;
        validate_components(&location)?;
        if let Location::Local { path } = &mut location {
            *path = path
                .canonicalize()
                .context("Agent location does not exist")?;
        }
        if let Location::Git { repo, .. } = &mut location {
            *repo = repo
                .canonicalize()
                .context("Agent repository does not exist")?;
        }
        ensure!(
            self.allow_network || !matches!(location, Location::Ssh { .. } | Location::S3 { .. }),
            "Agent plugin has no network grant"
        );
        ensure!(
            self.roots.iter().any(|(_, root)| contains(root, &location)),
            "Agent location is outside the granted roots: {input}"
        );
        Ok(location)
    }
    fn respect_gitignore(&self, location: &Location) -> bool {
        self.roots
            .iter()
            .filter(|(_, root)| contains(root, location))
            .any(|(grant, _)| grant.respect_gitignore)
    }
    fn check_remote(&self, location: &Location) -> Result<()> {
        self.check_remote_references(std::slice::from_ref(location))
    }
    fn check_remote_references(&self, locations: &[Location]) -> Result<()> {
        let mut hosts: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
        for location in locations {
            if let Location::Ssh { host, path } = location {
                hosts.entry(host).or_default().push(path);
            }
        }
        for (host, paths) in hosts {
            let roots = self
                .roots
                .iter()
                .filter_map(|(_, root)| match root {
                    Location::Ssh {
                        host: root_host,
                        path,
                    } if root_host == host => Some(path),
                    _ => None,
                })
                .collect::<Vec<_>>();
            crate::providers::ssh(
                host,
                "import os,sys,json\npaths=json.loads(sys.argv[1]);roots=[os.path.realpath(r) for r in json.loads(sys.argv[2])]\nfor p in paths:\n real=os.path.realpath(p)\n if not any(os.path.commonpath([r,real])==r for r in roots): raise RuntimeError('Agent SSH symlink escapes granted roots')\nprint('ok')",
                &[
                    &serde_json::to_string(&paths)?,
                    &serde_json::to_string(&roots)?,
                ],
                None,
            )?;
        }
        Ok(())
    }
    pub fn handle(&self, method: &str, params: &Value) -> Result<Value> {
        if method == "workspace.snapshot" {
            return Ok(serde_json::to_value(&self.context)?);
        }
        ensure!(
            ["files.list", "files.read", "files.search"].contains(&method),
            "Unknown agent host method {method}"
        );
        let location = self.authorize(
            params
                .get("location")
                .and_then(Value::as_str)
                .context("Host tool requires a location")?,
        )?;
        self.check_remote(&location)?;
        match method {
            "files.list" => {
                let limit = limit(params, 200, 1000)?;
                let entries = Backend.list(&location)?;
                let mut visible = Vec::new();
                for entry in entries {
                    // Do not expose a local symlink that resolves outside scope.
                    if self.authorize(&entry.location.display()).is_err() {
                        continue;
                    }
                    visible.push(json!({"location":entry.location.display(),"name":entry.name,"is_dir":entry.is_dir,"size":entry.size}));
                    if visible.len() > limit {
                        break;
                    }
                }
                let truncated = visible.len() > limit;
                visible.truncate(limit);
                Ok(json!({"entries":visible,"truncated":truncated}))
            }
            "files.read" => {
                let max_bytes = params
                    .get("max_bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(32_768);
                ensure!(
                    (1..=262_144).contains(&max_bytes),
                    "Read size must be 1..262144 bytes"
                );
                let preview = Backend.preview(&location, max_bytes as usize)?;
                Ok(
                    json!({"location":location.display(),"name":location.name(),"text":preview.text,"truncated":preview.truncated,"binary":preview.binary}),
                )
            }
            "files.search" => {
                let query = params
                    .get("query")
                    .and_then(Value::as_str)
                    .context("Search requires a query")?;
                ensure!(query.len() <= 16384, "Search query exceeds 16 KiB");
                let limit = limit(params, 100, 1000)?;
                let files_only = params
                    .get("files_only")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                ensure!(
                    params.get("files_only").is_none_or(Value::is_boolean),
                    "files_only must be a boolean"
                );
                // Index policy can be made stricter, never silently relaxed.
                let respect = self.respect_gitignore(&location)
                    || params
                        .get("respect_gitignore")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                let results = if let Some(plugin) = params.get("plugin").and_then(Value::as_str) {
                    ensure!(
                        !files_only,
                        "File-only index enumeration requires builtin, fd, or rg search"
                    );
                    let Location::Local { path } = &location else {
                        bail!("Semantic search plugins currently require local roots");
                    };
                    let mode = match params
                        .get("mode")
                        .and_then(Value::as_str)
                        .unwrap_or("hybrid")
                    {
                        "semantic" => SearchMode::Semantic,
                        "hybrid" => SearchMode::Hybrid,
                        "filename" => SearchMode::Filename,
                        "content" => SearchMode::Content,
                        _ => bail!("Unknown search plugin mode"),
                    };
                    crate::providers::plugin_registry()?.search(plugin,SearchRequest{query:query.into(),roots:vec![IndexRoot{path:path.to_string_lossy().into_owned(),respect_gitignore:respect,follow_symlinks:false}],limit,mode,config:Value::Null})?.into_iter().map(|hit|json!({"location":hit.path,"name":Location::parse(&hit.path).map(|l|l.name()).unwrap_or_default(),"line":hit.line,"snippet":hit.snippet,"score":hit.score})).collect::<Vec<_>>()
                } else {
                    let engine = match params
                        .get("engine")
                        .and_then(Value::as_str)
                        .unwrap_or("builtin")
                    {
                        "auto" => SearchEngine::Auto,
                        "builtin" => SearchEngine::Builtin,
                        "rg" => SearchEngine::Rg,
                        "fd" => SearchEngine::Fd,
                        "fff" => SearchEngine::Fff,
                        _ => bail!("Unknown search engine"),
                    };
                    {let request=SearchQuery{text:query.into(),engine,hidden:false,respect_gitignore:respect,max_results:limit};if files_only {crate::search::search_files(&location,&request)}else{Backend.search(&location,&request)}}?.into_iter().map(|hit|json!({"location":hit.location.display(),"name":hit.name,"line":hit.line,"snippet":hit.snippet})).collect::<Vec<_>>()
                };
                let mut safe = Vec::new();
                for mut hit in results {
                    if let Ok(target) = self.authorize(
                        hit["location"]
                            .as_str()
                            .context("Search omitted location")?,
                    ) {
                        hit["is_dir"] =
                            json!(target.local_path().is_some_and(std::path::Path::is_dir));
                        safe.push(hit);
                    }
                }
                if let Location::Ssh { host, .. } = &location {
                    let paths = safe
                        .iter()
                        .map(
                            |hit| match Location::parse(hit["location"].as_str().unwrap_or(""))? {
                                Location::Ssh { path, .. } => Ok(path),
                                _ => bail!("Remote search returned another provider"),
                            },
                        )
                        .collect::<Result<Vec<_>>>()?;
                    let roots = self
                        .roots
                        .iter()
                        .filter_map(|(_, root)| {
                            if let Location::Ssh {
                                host: root_host,
                                path,
                            } = root
                            {
                                if root_host == host {
                                    Some(path.clone())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>();
                    let output = crate::providers::ssh(
                        host,
                        "import os,sys,json\npaths=json.loads(sys.argv[1]);roots=[os.path.realpath(r) for r in json.loads(sys.argv[2])]\nresult={}\nfor p in paths:\n real=os.path.realpath(p)\n if any(os.path.commonpath([r,real])==r for r in roots): result[p]=os.path.isdir(p)\nprint(json.dumps(result))",
                        &[
                            &serde_json::to_string(&paths)?,
                            &serde_json::to_string(&roots)?,
                        ],
                        None,
                    )?;
                    let kinds: std::collections::BTreeMap<String, bool> =
                        serde_json::from_slice(&output).context("Invalid SSH search metadata")?;
                    safe.retain_mut(|hit| {
                        let Ok(Location::Ssh { path, .. }) =
                            Location::parse(hit["location"].as_str().unwrap_or(""))
                        else {
                            return false;
                        };
                        if let Some(is_dir) = kinds.get(&path) {
                            hit["is_dir"] = json!(is_dir);
                            true
                        } else {
                            false
                        }
                    });
                }
                let truncated = safe.len() >= limit;
                Ok(json!({"results":safe,"truncated":truncated}))
            }
            _ => unreachable!(),
        }
    }
    pub fn validate_result(&self, result: &AgentResult) -> Result<()> {
        ensure!(
            result.answer.len() <= 262144
                && result.memory.as_ref().is_none_or(|s| s.len() <= 65536),
            "Agent text exceeds bounds"
        );
        ensure!(
            result.results.len() <= 1000 && result.citations.len() <= 256,
            "Too many agent references"
        );
        let mut references = Vec::new();
        for citation in &result.citations {
            references.push(self.authorize(&citation.location)?);
            ensure!(
                citation.line.is_none_or(|n| n > 0),
                "Citation lines must be positive"
            );
            ensure!(
                citation.label.as_ref().is_none_or(|s| s.len() <= 1024),
                "Citation label is too long"
            );
        }
        let mut file_references = Vec::new();
        for hit in &result.results {
            let target = self.authorize(&hit.location)?;
            ensure!(
                hit.name == target.name(),
                "Agent result name does not match its authorized location"
            );
            ensure!(!hit.is_dir, "Agent result panes accept files only");
            if let Some(path) = target.local_path() {
                ensure!(
                    path.is_file(),
                    "Agent result location is not a regular file"
                );
            }
            references.push(target.clone());
            file_references.push(target);
            ensure!(
                hit.score.is_finite() && hit.line.is_none_or(|n| n > 0),
                "Invalid result score or line"
            );
            ensure!(
                hit.name.len() <= 1024
                    && hit.snippet.as_ref().is_none_or(|s| s.len() <= 16384)
                    && hit.reason.as_ref().is_none_or(|s| s.len() <= 4096),
                "Agent result exceeds text bounds"
            );
        }
        self.check_remote_references(&references)?;
        let mut hosts: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
        for target in &file_references {
            if let Location::Ssh { host, path } = target {
                hosts.entry(host).or_default().push(path);
            }
        }
        for (host, paths) in hosts {
            crate::providers::ssh(
                host,
                "import os,sys,json\nfor p in json.loads(sys.argv[1]):\n if not os.path.isfile(p): raise RuntimeError('Agent result location is not a regular file')\nprint('ok')",
                &[&serde_json::to_string(&paths)?],
                None,
            )?;
        }
        Ok(())
    }
}
fn limit(params: &Value, default: usize, max: usize) -> Result<usize> {
    let n = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(default as u64);
    ensure!(n > 0 && n <= max as u64, "Tool limit must be 1..{max}");
    Ok(n as usize)
}
fn validate_components(location: &Location) -> Result<()> {
    if let Location::S3 {
        endpoint: Some(endpoint),
        ..
    } = location
    {
        let endpoint = url::Url::parse(endpoint).context("Invalid agent S3 endpoint")?;
        ensure!(
            endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.query().is_none()
                && endpoint.fragment().is_none(),
            "Agent S3 endpoints must not contain credentials, query parameters, or fragments; use AWS configuration for credentials"
        );
    }
    let path = match location {
        Location::Local { path } => path.to_string_lossy(),
        Location::Git { path, .. } | Location::Plugin { path, .. } | Location::Ssh { path, .. } => {
            path.as_str().into()
        }
        Location::S3 { prefix, .. } => prefix.as_str().into(),
    };
    ensure!(
        !path.split('/').any(|p| p == ".." || p == "."),
        "Agent paths must not contain traversal components"
    );
    Ok(())
}
fn path_inside(root: &str, path: &str) -> bool {
    let root = root.trim_end_matches('/');
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}
fn contains(root: &Location, location: &Location) -> bool {
    match (root, location) {
        (Location::Local { path: a }, Location::Local { path: b }) => b.starts_with(a),
        (Location::Ssh { host: a, path: ap }, Location::Ssh { host: b, path: bp }) => {
            a == b && path_inside(ap, bp)
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
        ) => a == b && ae == be && (ap.is_empty() || path_inside(ap, bp)),
        (
            Location::Git {
                repo: a,
                reference: ar,
                path: ap,
            },
            Location::Git {
                repo: b,
                reference: br,
                path: bp,
            },
        ) => a == b && ar == br && (ap.is_empty() || path_inside(ap, bp)),
        (
            Location::Plugin {
                plugin: a,
                path: ap,
            },
            Location::Plugin {
                plugin: b,
                path: bp,
            },
        ) => a == b && path_inside(ap, bp),
        _ => false,
    }
}
