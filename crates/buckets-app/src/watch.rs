//! Local directory notifications. Remote providers refresh explicitly.
use anyhow::Result;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::mpsc::{self, Receiver},
};

pub struct LocalWatcher {
    watcher: RecommendedWatcher,
    events: Receiver<notify::Result<Event>>,
    roots: HashSet<PathBuf>,
}
impl LocalWatcher {
    pub fn new() -> Result<Self> {
        let (sender, events) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })?;
        Ok(Self {
            watcher,
            events,
            roots: HashSet::new(),
        })
    }
    pub fn sync(&mut self, roots: HashSet<PathBuf>) -> Result<()> {
        for old in self.roots.difference(&roots) {
            self.watcher.unwatch(old)?;
        }
        let mut active = self
            .roots
            .intersection(&roots)
            .cloned()
            .collect::<HashSet<_>>();
        for root in roots.difference(&self.roots) {
            self.watcher.watch(root, RecursiveMode::NonRecursive)?;
            active.insert(root.clone());
        }
        self.roots = active;
        Ok(())
    }
    pub fn changed(&self) -> Result<bool> {
        let mut changed = false;
        while let Ok(event) = self.events.try_recv() {
            if !event?.kind.is_access() {
                changed = true;
            }
        }
        Ok(changed)
    }
}
