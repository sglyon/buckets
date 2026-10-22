//! Storage, safe file operations, pane state, and search independent of the desktop renderer.
mod agent;
mod location;
pub use agent::{AgentBroker, AgentRoot};
mod operations;
mod providers;
mod search;
mod spool;
mod state;
mod transfer;
use anyhow::Result;
pub use location::Location;
pub use search::{SearchEngine, SearchHit, SearchQuery};
use serde::{Deserialize, Serialize};
pub use state::{Pane, Workspace};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub location: Location,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<u64>,
}
#[derive(Debug, Clone)]
pub struct Preview {
    pub text: String,
    pub truncated: bool,
    pub binary: bool,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct Backend;
impl Backend {
    pub fn list(&self, location: &Location) -> Result<Vec<Entry>> {
        let mut entries = providers::list(location)?;
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(entries)
    }
    pub fn preview(&self, location: &Location, max_bytes: usize) -> Result<Preview> {
        let limit = max_bytes.min(4 * 1024 * 1024);
        let result = providers::read_prefix(location, limit.saturating_add(1))?;
        let data = result.bytes;
        let truncated = result.truncated || data.len() > limit;
        let bytes = &data[..data.len().min(limit)];
        let binary = bytes.contains(&0);
        Ok(Preview {
            text: if binary {
                format!("Binary file · {} bytes sampled", bytes.len())
            } else {
                String::from_utf8_lossy(bytes).into_owned()
            },
            truncated,
            binary,
        })
    }
    pub fn search(&self, root: &Location, query: &SearchQuery) -> Result<Vec<SearchHit>> {
        search::search(self, root, query)
    }
}
