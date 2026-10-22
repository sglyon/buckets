use crate::{Backend, Entry, Location};
use anyhow::{Result, ensure};
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct Pane {
    pub location: Location,
    pub entries: Vec<Entry>,
    pub cursor: usize,
    pub selected: BTreeSet<usize>,
    pub history: Vec<Location>,
    pub forward: Vec<Location>,
    pub show_hidden: bool,
}
impl Pane {
    pub fn new(location: Location) -> Self {
        Self {
            location,
            entries: Vec::new(),
            cursor: 0,
            selected: BTreeSet::new(),
            history: Vec::new(),
            forward: Vec::new(),
            show_hidden: false,
        }
    }
    pub fn load(&mut self, backend: &Backend) -> Result<()> {
        let current = self.entries.get(self.cursor).map(|e| e.location.clone());
        let selected: Vec<_> = self
            .selected
            .iter()
            .filter_map(|&i| self.entries.get(i).map(|e| e.location.clone()))
            .collect();
        let entries = backend
            .list(&self.location)?
            .into_iter()
            .filter(|e| self.show_hidden || !e.name.starts_with('.'))
            .collect::<Vec<_>>();
        self.cursor = current
            .and_then(|l| entries.iter().position(|e| e.location == l))
            .unwrap_or(0)
            .min(entries.len().saturating_sub(1));
        self.selected = entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| selected.contains(&e.location).then_some(i))
            .collect();
        self.entries = entries;
        Ok(())
    }
    pub fn navigate(&mut self, backend: &Backend, location: Location) -> Result<()> {
        let mut candidate = Self::new(location);
        candidate.show_hidden = self.show_hidden;
        candidate.load(backend)?;
        self.history.push(self.location.clone());
        self.forward.clear();
        self.location = candidate.location;
        self.entries = candidate.entries;
        self.cursor = 0;
        self.selected.clear();
        Ok(())
    }
    pub fn back(&mut self, backend: &Backend) -> Result<()> {
        if let Some(location) = self.history.last().cloned() {
            let mut candidate = Self::new(location);
            candidate.show_hidden = self.show_hidden;
            candidate.load(backend)?;
            self.history.pop();
            self.forward.push(self.location.clone());
            self.location = candidate.location;
            self.entries = candidate.entries;
            self.cursor = 0;
            self.selected.clear();
        }
        Ok(())
    }
    pub fn go_forward(&mut self, backend: &Backend) -> Result<()> {
        if let Some(location) = self.forward.last().cloned() {
            let mut candidate = Self::new(location);
            candidate.show_hidden = self.show_hidden;
            candidate.load(backend)?;
            self.forward.pop();
            self.history.push(self.location.clone());
            self.location = candidate.location;
            self.entries = candidate.entries;
            self.cursor = 0;
            self.selected.clear();
        }
        Ok(())
    }
    pub fn step(&mut self, delta: isize) {
        self.cursor = self
            .cursor
            .saturating_add_signed(delta)
            .min(self.entries.len().saturating_sub(1));
    }
    pub fn toggle_selected(&mut self) {
        if self.cursor < self.entries.len() && !self.selected.remove(&self.cursor) {
            self.selected.insert(self.cursor);
        }
    }
    pub fn selected_entries(&self) -> Vec<&Entry> {
        if self.selected.is_empty() {
            self.entries.get(self.cursor).into_iter().collect()
        } else {
            self.selected
                .iter()
                .filter_map(|&i| self.entries.get(i))
                .collect()
        }
    }
}
#[derive(Debug, Clone)]
pub struct Workspace {
    pub panes: Vec<Pane>,
    pub active: usize,
}
impl Workspace {
    pub fn new(location: Location) -> Self {
        Self {
            panes: vec![Pane::new(location.clone()), Pane::new(location)],
            active: 0,
        }
    }
    pub fn split(&mut self) -> Result<()> {
        ensure!(
            self.panes.len() < 8,
            "A workspace supports up to eight panes"
        );
        let pane = self.panes[self.active].clone();
        self.panes.insert(self.active + 1, pane);
        self.active += 1;
        Ok(())
    }
    pub fn close_active(&mut self) -> Result<()> {
        ensure!(self.panes.len() > 1, "Keep at least one pane open");
        self.panes.remove(self.active);
        self.active = self.active.min(self.panes.len() - 1);
        Ok(())
    }
    pub fn cycle(&mut self, delta: isize) {
        self.active = (self.active as isize + delta).rem_euclid(self.panes.len() as isize) as usize;
    }
}
