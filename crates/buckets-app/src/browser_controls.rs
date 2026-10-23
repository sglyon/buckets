//! Workspace and pane commands. Views remain projections of provider locations.
use crate::named_workspaces::{PaneLayout, WorkspacePane, WorkspaceSnapshot};
use crate::{Browser, Command, Location, Pane, PaneContent, Prompt};
use gpui::{prelude::*, *};
use gpui_component::resizable::{ResizablePanelGroup, resizable_panel};

impl Browser {
    pub(crate) fn control_command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match command {
            Command::Operations => {
                if !self.operation_ui.running() {
                    self.load_operation_history(cx);
                }
                self.operation_ui.open = true;
                self.prompt = None;
                self.help = false;
                self.quicklook = false;
                self.focus.focus(window);
                cx.notify();
            }
            Command::Sort(key) => {
                self.panes[self.active].view.sort = key;
                self.reorder_pane(cx);
            }
            Command::SortDescending => {
                self.panes[self.active].view.descending ^= true;
                self.reorder_pane(cx);
            }
            Command::DirectoriesFirst => {
                self.panes[self.active].view.directories_first ^= true;
                self.reorder_pane(cx);
            }
            Command::ToggleColumn(column) => {
                let columns = &mut self.panes[self.active].view.columns;
                if columns.contains(&column) {
                    columns.retain(|value| *value != column)
                } else {
                    columns.push(column)
                }
                self.save();
                cx.notify();
            }
            Command::Layout(layout) => {
                self.pane_layout = layout;
                self.layout_epoch += 1;
                self.save();
                cx.notify();
            }
            Command::GrowPane | Command::ShrinkPane => {
                let weight = &mut self.panes[self.active].weight;
                *weight = (*weight
                    * if matches!(command, Command::GrowPane) {
                        1.25
                    } else {
                        0.8
                    })
                .clamp(0.1, 10.);
                self.layout_epoch += 1;
                self.save();
                cx.notify();
            }
            Command::EqualPanes => {
                for pane in &mut self.panes {
                    pane.weight = 1.;
                }
                self.layout_epoch += 1;
                self.save();
                cx.notify();
            }
            Command::ReloadWorkspaces => {
                let result = if let Some(store) = self.named_workspaces.as_mut() {
                    Some(store.reload())
                } else {
                    match crate::named_workspaces::NamedWorkspaceStore::load(
                        crate::session_path().with_file_name("workspaces.json"),
                    ) {
                        Ok(store) => {
                            self.named_workspaces = Some(store);
                            Some(Ok(()))
                        }
                        Err(error) => Some(Err(error)),
                    }
                };
                match result {
                    Some(Ok(())) => self.message("Named workspaces reloaded", false, cx),
                    Some(Err(error)) => self.message(
                        format!("Named workspaces were not reloaded: {error:#}"),
                        true,
                        cx,
                    ),
                    None => self.message("Named workspace storage is unavailable", true, cx),
                }
            }
            Command::SaveWorkspace => self.begin(Prompt::WorkspaceName(None), window, cx),
            Command::RenameWorkspace(index) => {
                self.begin(Prompt::WorkspaceName(Some(index)), window, cx)
            }
            Command::RemoveWorkspace(index) => {
                self.begin(Prompt::RemoveWorkspace(index), window, cx)
            }
            Command::OpenWorkspace(index) => self.open_named_workspace(index, window, cx),
            _ => return false,
        }
        true
    }
    fn reorder_pane(&mut self, cx: &mut Context<Self>) {
        let pane = &mut self.panes[self.active];
        pane.replace_entries(pane.entries.clone());
        pane.scroll
            .scroll_to_item(pane.visible_cursor(), ScrollStrategy::Top);
        self.save();
        self.load_preview(cx);
        cx.notify();
    }
    pub(crate) fn workspace_snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            workspace_id: self.workspace_id.clone(),
            panes: self
                .panes
                .iter()
                .map(|pane| WorkspacePane {
                    location: pane.location.display(),
                    weight: pane.weight,
                    view: pane.view.clone(),
                })
                .collect(),
            active: self.active,
            layout: self.pane_layout,
            sidebar: self.sidebar,
            preview: self.show_preview,
            hidden: self.hidden,
        }
    }
    pub(crate) fn submit_workspace_prompt(
        &mut self,
        mode: Prompt,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !matches!(mode, Prompt::WorkspaceName(_) | Prompt::RemoveWorkspace(_)) {
            return false;
        }
        let snapshot = self.workspace_snapshot();
        let Some(store) = self.named_workspaces.as_mut() else {
            self.message(
                "Named workspace storage is unavailable; check workspaces.json",
                true,
                cx,
            );
            return true;
        };
        let result = match mode {
            Prompt::WorkspaceName(None) => store.save(value, snapshot).map(|_| ()),
            Prompt::WorkspaceName(Some(index)) => {
                let id = store.entries().get(index).map(|entry| entry.id.clone());
                id.ok_or_else(|| anyhow::anyhow!("Workspace no longer exists"))
                    .and_then(|id| store.rename(&id, value))
            }
            Prompt::RemoveWorkspace(index) => {
                if value != "remove" {
                    self.message("Type remove to remove this saved layout", true, cx);
                    return true;
                }
                let id = store.entries().get(index).map(|entry| entry.id.clone());
                id.ok_or_else(|| anyhow::anyhow!("Workspace no longer exists"))
                    .and_then(|id| store.remove(&id))
            }
            _ => unreachable!(),
        };
        match result {
            Ok(()) => {
                self.dismiss(window, cx);
                self.message(if matches!(mode,Prompt::WorkspaceName(None)){"Saved layout. Open it from the palette to use its own workspace notes and agent context."}else if matches!(mode,Prompt::RemoveWorkspace(_)){"Saved layout removed; files are unchanged"}else{"Named workspace renamed"},false,cx);
            }
            Err(error) => self.message(format!("Workspace was not saved: {error:#}"), true, cx),
        }
        true
    }
    fn open_named_workspace(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self
            .settings_view
            .as_ref()
            .filter(|view| view.read(cx).has_unsaved_json())
        {
            let view = view.clone();
            self.settings_open = true;
            view.update(cx, |view, cx| view.request_close(window, cx));
            self.message(
                "Save or discard the settings draft, then open the workspace again",
                true,
                cx,
            );
            return;
        }
        if let Some(index)=self.panes.iter().position(|pane|matches!(&pane.content,PaneContent::Agent(view) if view.read(cx).has_unsaved_memory())) {
            self.active=index;self.focus_active(window,cx);
            self.message("Save or revert workspace notes before opening another workspace",true,cx);return;
        }
        if let Some(index)=self.panes.iter().position(|pane|matches!(&pane.content,PaneContent::Terminal(view) if view.read(cx).exit_status().is_none())) {
            self.active=index;self.focus_active(window,cx);self.message("Close running terminal panes before opening another workspace; their programs will not be stopped automatically",true,cx);return;
        }
        let Some(workspace) = self
            .named_workspaces
            .as_ref()
            .and_then(|store| {
                store
                    .entries()
                    .get(index)
                    .and_then(|entry| store.get(&entry.id))
            })
            .cloned()
        else {
            self.message("Saved workspace no longer exists", true, cx);
            return;
        };
        let snapshot = workspace.snapshot;
        let locations = snapshot
            .panes
            .iter()
            .map(|pane| Location::parse(&pane.location))
            .collect::<anyhow::Result<Vec<_>>>();
        let locations = match locations {
            Ok(locations) => locations,
            Err(error) => {
                self.message(format!("Workspace was not opened: {error:#}"), true, cx);
                return;
            }
        };
        if self.sidebar != snapshot.sidebar
            || self.show_preview != snapshot.preview
            || self.hidden != snapshot.hidden
        {
            let result = self
                .settings_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("Settings storage is unavailable"))
                .and_then(|store| {
                    store.update(|settings| {
                        settings.sidebar = snapshot.sidebar;
                        settings.preview = snapshot.preview;
                        settings.show_hidden = snapshot.hidden;
                    })
                });
            if let Err(error) = result {
                self.message(format!("Workspace was not opened because its browsing preferences could not be saved: {error:#}"),true,cx);
                return;
            }
        }
        for pane in &self.panes {
            match &pane.content {
                PaneContent::Agent(view) => {
                    view.update(cx, |view, cx| view.clear_private("Workspace changed", cx))
                }
                PaneContent::Terminal(view) => view.update(cx, |view, _| view.close()),
                PaneContent::Files => {}
            }
        }
        self.account_sessions.cancel_all();
        self.workspace_revision += 1;
        self.panes = locations
            .into_iter()
            .zip(snapshot.panes)
            .map(|(location, saved)| {
                let mut pane = Pane::new(self.next_id, location);
                self.next_id += 1;
                pane.weight = saved.weight;
                pane.view = saved.view;
                pane
            })
            .collect();
        self.workspace_id = snapshot.workspace_id;
        self.active = snapshot.active.min(self.panes.len() - 1);
        self.pane_layout = snapshot.layout;
        self.sidebar = snapshot.sidebar;
        self.show_preview = snapshot.preview;
        self.hidden = snapshot.hidden;
        self.layout_epoch += 1;
        self.quicklook = false;
        self.help = false;
        self.prompt = None;
        self.preview_generation += 1;
        self.preview.clear();
        self.save();
        for index in 0..self.panes.len() {
            self.reload(index, cx);
        }
        self.focus_active(window, cx);
        self.sync_settings_view(window, cx);
        self.message(format!("Opened workspace: {}", workspace.name), false, cx);
    }
    pub(crate) fn render_resizable_panes(
        &self,
        window: &mut Window,
        available_width: Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let axis = if self.pane_layout == PaneLayout::Horizontal {
            Axis::Horizontal
        } else {
            Axis::Vertical
        };
        let extent = if axis == Axis::Horizontal {
            available_width
        } else {
            window.viewport_size().height
        };
        let total = self
            .panes
            .iter()
            .map(|pane| pane.weight)
            .sum::<f32>()
            .max(0.1);
        let panels = self
            .panes
            .iter()
            .enumerate()
            .map(|(index, pane)| {
                resizable_panel()
                    .size(extent * (pane.weight / total))
                    .size_range(px(48.)..Pixels::MAX)
                    .child(self.render_pane(
                        index,
                        if axis == Axis::Horizontal {
                            available_width * (pane.weight / total)
                        } else {
                            available_width
                        },
                        cx,
                    ))
            })
            .collect::<Vec<_>>();
        let weak = cx.entity().downgrade();
        ResizablePanelGroup::new(("workspace-panes", self.layout_epoch))
            .axis(axis)
            .children(panels)
            .on_resize(move |state, _, cx| {
                let sizes = state
                    .read(cx)
                    .sizes()
                    .iter()
                    .map(|value| f32::from(*value))
                    .collect::<Vec<_>>();
                let total = sizes.iter().sum::<f32>();
                if total > 0. {
                    let _ = weak.update(cx, |this, cx| {
                        for (pane, size) in this.panes.iter_mut().zip(sizes) {
                            pane.weight = (size / total * 10.).clamp(0.1, 10.);
                        }
                        this.save();
                        cx.notify();
                    });
                }
            })
    }
}
impl Browser {
    pub(crate) fn sort_heading(
        &self,
        title: &str,
        key: crate::named_workspaces::SortKey,
        index: usize,
    ) -> String {
        let view = &self.panes[index].view;
        format!(
            "{title}{}",
            if view.sort == key {
                if view.descending { " ↓" } else { " ↑" }
            } else {
                ""
            }
        )
    }
}
