//! Pure projection of saved favorites and optional built-in locations.
use crate::settings::Bookmark;
use anyhow::{Result, ensure};
use buckets_core::Location;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::HashSet, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinPlace {
    Home,
    Desktop,
    Downloads,
    LaunchDirectory,
    Root,
}
impl BuiltinPlace {
    pub const ALL: [Self; 5] = [
        Self::Home,
        Self::Desktop,
        Self::Downloads,
        Self::LaunchDirectory,
        Self::Root,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Desktop => "Desktop",
            Self::Downloads => "Downloads",
            Self::LaunchDirectory => "Launch directory",
            Self::Root => "Root",
        }
    }

    pub fn target(self, home: &Path, launch: &Path) -> Location {
        Location::local(match self {
            Self::Home => home.to_path_buf(),
            Self::Desktop => home.join("Desktop"),
            Self::Downloads => home.join("Downloads"),
            Self::LaunchDirectory => launch.to_path_buf(),
            Self::Root => Path::new("/").to_path_buf(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Places {
    pub builtins: Vec<BuiltinPlace>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
impl Default for Places {
    fn default() -> Self {
        Self {
            builtins: BuiltinPlace::ALL.to_vec(),
            extra: Default::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub label: String,
    pub location: Location,
    pub bookmark_index: Option<usize>,
    pub builtin: Option<BuiltinPlace>,
}

impl Places {
    pub fn validate(&self) -> Result<()> {
        let mut seen = HashSet::new();
        ensure!(
            self.builtins.iter().all(|place| seen.insert(*place)),
            "Places must not contain duplicate built-in locations"
        );
        Ok(())
    }

    pub fn toggle(&mut self, place: BuiltinPlace) {
        if self.builtins.contains(&place) {
            self.builtins.retain(|enabled| *enabled != place);
        } else {
            self.builtins.push(place);
        }
    }

    /// Validated saved favorites keep their order and identity. Deduplication
    /// compares typed Location identities (including lexical local path
    /// components), without resolving symlinks, touching local paths or
    /// connecting to remote providers.
    pub fn project(&self, bookmarks: &[Bookmark], home: &Path, launch: &Path) -> Vec<Place> {
        let mut saved_locations = HashSet::new();
        let mut result = Vec::with_capacity(bookmarks.len() + self.builtins.len());
        for (index, bookmark) in bookmarks.iter().enumerate() {
            let Ok(location) = Location::parse(&bookmark.location) else {
                // Settings validation rejects this; keep projection total for
                // hosts constructing an unvalidated transient preference view.
                continue;
            };
            saved_locations.insert(location.clone());
            result.push(Place {
                label: bookmark.name.clone(),
                location,
                bookmark_index: Some(index),
                builtin: None,
            });
        }
        for &builtin in &self.builtins {
            let location = builtin.target(home, launch);
            if saved_locations.insert(location.clone()) {
                result.push(Place {
                    label: builtin.label().into(),
                    location,
                    bookmark_index: None,
                    builtin: Some(builtin),
                });
            }
        }
        result
    }
}
