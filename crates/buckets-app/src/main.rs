mod account_session;
mod account_state;
mod agent_context;
mod agent_session;
mod agent_transcript;
mod agent_view;
mod appearance;
mod application_icon;
mod bootstrap;
mod browser_controls;
mod clipboard;
mod font_zoom;
mod keybindings;
mod menus;
mod name_filter;
mod named_workspaces;
mod native_drag;
mod operations_session;
mod pane_view;
mod settings;
mod settings_view;
mod shortcut_recorder;
mod theme;
mod watch;
use anyhow::{Result, anyhow};
use buckets_core::{Backend, Entry, Location, SearchEngine, SearchQuery};
use buckets_plugin::{
    Capability, CommandResult, Decoration, DecorationRequest, DecorationTone, IndexRoot,
    PluginRegistry, SearchMode, SearchRequest,
};
use buckets_terminal::{TerminalSession, TerminalSpec, TerminalView};
use gpui::{prelude::*, *};
use gpui_component::{
    Root, Theme, ThemeMode,
    input::{Input, InputEvent, InputState},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{borrow::Cow, collections::BTreeSet, path::PathBuf};

// Incumbent dimensions are authored at the default 13pt rem.
fn ui(value: f32) -> Rems {
    rems(value / font_zoom::DEFAULT_POINTS)
}

struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let shape = match path {
            "icons/folder.svg" => "<path d='M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v10H3Z'/>",
            "icons/file.svg" => "<path d='M6 3h8l4 4v14H6Z'/><path d='M14 3v5h4M9 12h6M9 16h6'/>",
            "icons/chevron-right.svg" => "<path d='m9 5 7 7-7 7'/>",
            "icons/chevron-down.svg" => "<path d='m5 9 7 7 7-7'/>",
            "icons/close.svg" | "icons/x.svg" => "<path d='m6 6 12 12M18 6 6 18'/>",
            "icons/search.svg" => "<circle cx='10.5' cy='10.5' r='6.5'/><path d='m16 16 5 5'/>",
            "icons/check.svg" => "<path d='m4 12 5 5L20 6'/>",
            _ => return Ok(None),
        };
        Ok(Some(Cow::Owned(format!("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='white' stroke-width='1.6' stroke-linecap='round' stroke-linejoin='round'>{shape}</svg>").into_bytes())))
    }
    fn list(&self, _: &str) -> Result<Vec<SharedString>> {
        Ok(vec![])
    }
}
fn icon(name: &'static str, color: u32) -> Svg {
    svg()
        .path(format!("icons/{name}.svg"))
        .size(ui(16.))
        .text_color(rgb(color))
}
fn key(label: impl Into<SharedString>, cx: &App) -> Div {
    let colors = theme::palette(cx);
    div()
        .px_1()
        .rounded_sm()
        .bg(rgb(colors.border))
        .text_color(rgb(colors.text))
        .text_size(ui(11.))
        .child(label.into())
}
fn hint(label: &'static str, shortcut: impl Into<SharedString>, cx: &App) -> Div {
    let colors = theme::palette(cx);
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(key(shortcut, cx))
        .child(div().text_color(rgb(colors.muted)).child(label))
}
fn human_size(n: u64) -> String {
    if n >= 1_073_741_824 {
        format!("{:.1}G", n as f64 / 1_073_741_824.)
    } else if n >= 1_048_576 {
        format!("{:.1}M", n as f64 / 1_048_576.)
    } else if n >= 1024 {
        format!("{:.1}K", n as f64 / 1024.)
    } else {
        n.to_string()
    }
}

#[derive(Clone)]
struct FileDrag {
    locations: Vec<Location>,
    label: String,
}
impl Render for FileDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::palette(cx);
        div()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(rgb(colors.surface))
            .border_1()
            .border_color(rgb(colors.accent))
            .text_color(rgb(colors.text))
            .font_family(theme::ui_font(cx))
            .child(self.label.clone())
    }
}
enum PaneContent {
    Files,
    Terminal(Entity<TerminalView>),
    Agent(Entity<agent_view::AgentView>),
}
struct NameFilterState {
    query: String,
    indices: Arc<Vec<usize>>,
    previous_cursor: Option<Location>,
    previous_selected: std::collections::HashSet<Location>,
}
fn broader_search(query: &str) -> bool {
    ["fd:", "rg:", "fff:", "plugin:"]
        .iter()
        .any(|prefix| query.starts_with(prefix))
}
struct Pane {
    content: PaneContent,
    terminal_observation: Option<Subscription>,
    agent_subscription: Option<Subscription>,
    pending_reveal: Option<Location>,
    result_details: std::collections::HashMap<String, String>,
    id: u64,
    location: Location,
    entries: Vec<Entry>,
    name_filter: Option<NameFilterState>,
    cursor: usize,
    selected: BTreeSet<usize>,
    selection_anchor: Option<usize>,
    history: Vec<Location>,
    forward: Vec<Location>,
    loading: bool,
    refresh_pending: bool,
    decorations: std::collections::HashMap<String, Decoration>,
    error: Option<String>,
    generation: u64,
    search: Option<String>,
    scroll: UniformListScrollHandle,
    view: named_workspaces::PaneView,
    weight: f32,
}
impl Pane {
    fn new(id: u64, location: Location) -> Self {
        Self {
            content: PaneContent::Files,
            terminal_observation: None,
            agent_subscription: None,
            pending_reveal: None,
            result_details: Default::default(),
            id,
            location,
            entries: vec![],
            name_filter: None,
            cursor: 0,
            selected: BTreeSet::new(),
            selection_anchor: None,
            history: vec![],
            forward: vec![],
            loading: true,
            refresh_pending: false,
            decorations: Default::default(),
            error: None,
            generation: 0,
            search: None,
            scroll: UniformListScrollHandle::new(),
            view: Default::default(),
            weight: 1.,
        }
    }
    fn replace_entries(&mut self, mut entries: Vec<Entry>) {
        pane_view::sort(&mut entries, &self.view);
        let cursor = self.entries.get(self.cursor).map(|e| e.location.clone());
        let selected = self
            .selected
            .iter()
            .filter_map(|i| self.entries.get(*i).map(|e| e.location.clone()))
            .collect::<std::collections::HashSet<_>>();
        self.entries = entries;
        self.cursor = cursor
            .and_then(|location| self.entries.iter().position(|e| e.location == location))
            .unwrap_or(self.cursor.min(self.entries.len().saturating_sub(1)));
        self.selected = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| selected.contains(&e.location))
            .map(|(i, _)| i)
            .collect();
        self.selection_anchor = None;
        self.refresh_name_filter();
    }
    fn select_visible_range(&mut self, anchor: usize, target: usize) {
        if let Some(filter) = &self.name_filter {
            let start = filter.indices.iter().position(|i| *i == anchor);
            let end = filter.indices.iter().position(|i| *i == target);
            if let Some(end) = end {
                let start = start.unwrap_or(end);
                self.selected = filter.indices[start.min(end)..=start.max(end)]
                    .iter()
                    .copied()
                    .collect();
            }
        } else {
            self.selected = (anchor.min(target)..=anchor.max(target)).collect();
        }
    }
    fn start_name_filter(&mut self) {
        if self.name_filter.is_some() {
            return;
        }
        self.name_filter = Some(NameFilterState {
            query: String::new(),
            indices: Arc::new((0..self.entries.len()).collect()),
            previous_cursor: self.entries.get(self.cursor).map(|e| e.location.clone()),
            previous_selected: self
                .selected
                .iter()
                .filter_map(|i| self.entries.get(*i).map(|e| e.location.clone()))
                .collect(),
        });
    }
    fn refresh_name_filter(&mut self) {
        if let Some(filter) = &mut self.name_filter {
            self.selection_anchor = None;
            filter.indices = Arc::new(if broader_search(&filter.query) {
                (0..self.entries.len()).collect()
            } else {
                name_filter::matching_indices(
                    self.entries.iter().map(|e| e.name.as_str()),
                    &filter.query,
                )
            });
            if !filter.indices.contains(&self.cursor) {
                self.cursor = filter.indices.first().copied().unwrap_or(0);
            }
        }
    }
    fn finish_name_filter(&mut self, restore: bool) {
        if let Some(filter) = self.name_filter.take()
            && restore
        {
            if let Some(cursor) = filter
                .previous_cursor
                .and_then(|location| self.entries.iter().position(|e| e.location == location))
            {
                self.cursor = cursor;
            }
            self.selected = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| filter.previous_selected.contains(&entry.location))
                .map(|(i, _)| i)
                .collect();
            self.selection_anchor = None;
        }
    }
    fn visible_count(&self) -> usize {
        self.name_filter
            .as_ref()
            .map_or(self.entries.len(), |filter| filter.indices.len())
    }
    fn visible_cursor(&self) -> usize {
        self.name_filter
            .as_ref()
            .and_then(|filter| filter.indices.iter().position(|i| *i == self.cursor))
            .unwrap_or(self.cursor)
    }
    fn is_files(&self) -> bool {
        matches!(self.content, PaneContent::Files)
    }
    fn is_terminal(&self) -> bool {
        matches!(self.content, PaneContent::Terminal(_))
    }
    fn chosen(&self) -> Vec<Location> {
        if self.visible_count() == 0 {
            return vec![];
        }
        let indices = if self.selected.is_empty() {
            vec![self.cursor]
        } else {
            self.selected.iter().copied().collect()
        };
        let mut seen = std::collections::HashSet::new();
        indices
            .into_iter()
            .filter_map(|i| self.entries.get(i).map(|e| e.location.clone()))
            .filter(|location| seen.insert(location.clone()))
            .collect()
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Prompt {
    Location,
    Search,
    Rename,
    NewFolder,
    NewFile,
    CopyTo,
    MoveTo,
    Delete,
    Palette,
    BookmarkName(Option<usize>),
    WorkspaceName(Option<usize>),
    RemoveWorkspace(usize),
}
impl Prompt {
    fn title(self) -> &'static str {
        match self {
            Self::WorkspaceName(_) => "Save named workspace",
            Self::RemoveWorkspace(_) => "Remove saved workspace?",
            Self::Location => "Go to location",
            Self::Search => "Search this directory",
            Self::Rename => "Rename",
            Self::NewFolder => "New folder",
            Self::NewFile => "New file",
            Self::CopyTo => "Copy to directory",
            Self::MoveTo => "Move to directory",
            Self::Delete => "Move selected items to Trash?",
            Self::Palette => "Run a command",
            Self::BookmarkName(_) => "Name this bookmark",
        }
    }
    fn detail(self) -> &'static str {
        match self {
            Self::WorkspaceName(_) => {
                "Save locations and pane layout. Terminal commands and agent requests are never replayed."
            }
            Self::RemoveWorkspace(_) => {
                "Type remove to remove this saved layout. Files remain unchanged."
            }
            Self::Location => "Local path, ssh://host/path, s3://bucket/prefix or git://…",
            Self::Search => {
                "Filename search by default. Prefix rg: for contents, fd: or fff: to choose an engine."
            }
            Self::BookmarkName(_) => {
                "Save this full location in settings.json. SSH credentials use your agent; S3 credentials stay in your environment."
            }
            Self::Delete => "Type trash and press Enter to confirm. Escape cancels.",
            Self::Palette => "Type to filter commands. Use ↑↓ to choose and Enter to run.",
            Self::CopyTo | Self::MoveTo => {
                "Enter a destination directory. Existing files are never overwritten."
            }
            _ => "Enter a name. Escape cancels.",
        }
    }
}
#[derive(Clone, Copy)]
enum Command {
    Location,
    Search,
    Split,
    Close,
    Next,
    Rename,
    NewFolder,
    NewFile,
    Copy,
    Cut,
    Paste,
    CopyTo,
    MoveTo,
    Duplicate,
    Delete,
    Refresh,
    Hidden,
    Preview,
    Sidebar,
    Help,
    Back,
    Forward,
    Parent,
    SelectAll,
    Home,
    Plugins,
    PluginSearch(usize),
    PluginInvoke(usize),
    ReloadPlugins,
    QuickLook,
    FontIncrease,
    FontDecrease,
    FontReset,
    Settings,
    Desktop,
    Downloads,
    Workspace,
    Root,
    PluginLocation(usize),
    BookmarkCurrent,
    OpenBookmarks,
    GoBookmark(usize),
    RenameBookmark(usize),
    RemoveBookmark(usize),
    Sort(named_workspaces::SortKey),
    SortDescending,
    DirectoriesFirst,
    ToggleColumn(named_workspaces::Column),
    Layout(named_workspaces::PaneLayout),
    GrowPane,
    ShrinkPane,
    EqualPanes,
    SaveWorkspace,
    ReloadWorkspaces,
    OpenWorkspace(usize),
    RenameWorkspace(usize),
    RemoveWorkspace(usize),
    Operations,
}
const COMMANDS: &[(Command, &str, &str)] = &[
    (Command::Location, "Go to location / connect", ""),
    (Command::BookmarkCurrent, "Bookmark this location", ""),
    (Command::OpenBookmarks, "Open bookmarks", ""),
    (Command::Search, "Filter names / search files", ""),
    (Command::Split, "Split pane", ""),
    (Command::Close, "Close pane", ""),
    (Command::Next, "Focus next pane", ""),
    (Command::Rename, "Rename selected file", ""),
    (Command::NewFolder, "New folder", ""),
    (Command::NewFile, "New file", ""),
    (Command::Copy, "Copy selection", ""),
    (Command::Cut, "Cut selection", ""),
    (Command::Paste, "Paste into this pane", ""),
    (Command::CopyTo, "Copy to directory", ""),
    (Command::MoveTo, "Move to directory", ""),
    (Command::Duplicate, "Duplicate selection", ""),
    (Command::Delete, "Move selection to Trash", ""),
    (Command::Refresh, "Refresh directory", ""),
    (Command::Hidden, "Toggle hidden files", ""),
    (Command::Preview, "Toggle file preview", ""),
    (Command::Sidebar, "Toggle places sidebar", ""),
    (Command::Help, "Keyboard reference", ""),
    (Command::Back, "Go back", ""),
    (Command::Forward, "Go forward", ""),
    (Command::Parent, "Go to parent", ""),
    (Command::SelectAll, "Select all files", ""),
    (Command::Home, "Go to home directory", ""),
    (Command::QuickLook, "Quick Look selected file", ""),
    (Command::FontIncrease, "Increase text size", ""),
    (Command::FontDecrease, "Decrease text size", ""),
    (Command::FontReset, "Reset text size", ""),
    (Command::Settings, "Open Settings", ""),
    (Command::Desktop, "Go to Desktop", ""),
    (Command::Downloads, "Go to Downloads", ""),
    (Command::Workspace, "Go to workspace", ""),
    (Command::Root, "Go to root", ""),
    (Command::Plugins, "Open plugin directory", ""),
    (Command::ReloadPlugins, "Reload plugins", ""),
    (Command::Operations, "Show operations and recovery", ""),
    (Command::SaveWorkspace, "Save named workspace", ""),
    (Command::ReloadWorkspaces, "Reload named workspaces", ""),
    (
        Command::Sort(named_workspaces::SortKey::Name),
        "Sort pane by name",
        "",
    ),
    (
        Command::Sort(named_workspaces::SortKey::Size),
        "Sort pane by size",
        "",
    ),
    (
        Command::Sort(named_workspaces::SortKey::Modified),
        "Sort pane by modified date",
        "",
    ),
    (
        Command::Sort(named_workspaces::SortKey::Type),
        "Sort pane by type",
        "",
    ),
    (Command::SortDescending, "Reverse pane sort direction", ""),
    (Command::DirectoriesFirst, "Toggle directories first", ""),
    (
        Command::ToggleColumn(named_workspaces::Column::Size),
        "Toggle Size column",
        "",
    ),
    (
        Command::ToggleColumn(named_workspaces::Column::Modified),
        "Toggle Modified column",
        "",
    ),
    (
        Command::ToggleColumn(named_workspaces::Column::Type),
        "Toggle Type column",
        "",
    ),
    (
        Command::Layout(named_workspaces::PaneLayout::Horizontal),
        "Arrange panes side by side",
        "",
    ),
    (
        Command::Layout(named_workspaces::PaneLayout::Vertical),
        "Arrange panes top to bottom",
        "",
    ),
    (Command::GrowPane, "Grow active pane", ""),
    (Command::ShrinkPane, "Shrink active pane", ""),
    (Command::EqualPanes, "Equalize pane sizes", ""),
];
fn command_id(command: Command) -> Option<&'static str> {
    match command {
        Command::Operations => Some("view.operations"),
        Command::SaveWorkspace => Some("workspace.save"),
        Command::ReloadWorkspaces => Some("workspace.reload"),
        Command::GrowPane => Some("pane.grow"),
        Command::ShrinkPane => Some("pane.shrink"),
        Command::EqualPanes => Some("pane.equalize"),
        Command::Sort(named_workspaces::SortKey::Name) => Some("pane.sort_name"),
        Command::Sort(named_workspaces::SortKey::Size) => Some("pane.sort_size"),
        Command::Sort(named_workspaces::SortKey::Modified) => Some("pane.sort_modified"),
        Command::Sort(named_workspaces::SortKey::Type) => Some("pane.sort_type"),
        Command::SortDescending => Some("pane.sort_reverse"),
        Command::DirectoriesFirst => Some("pane.directories_first"),
        Command::ToggleColumn(named_workspaces::Column::Size) => Some("pane.column_size"),
        Command::ToggleColumn(named_workspaces::Column::Modified) => Some("pane.column_modified"),
        Command::ToggleColumn(named_workspaces::Column::Type) => Some("pane.column_type"),
        Command::Layout(named_workspaces::PaneLayout::Horizontal) => Some("pane.layout_horizontal"),
        Command::Layout(named_workspaces::PaneLayout::Vertical) => Some("pane.layout_vertical"),

        Command::Location => Some("location.open"),
        Command::BookmarkCurrent => Some("bookmark.add"),
        Command::OpenBookmarks => Some("bookmark.open"),
        Command::Search => Some("file.filter"),
        Command::Split => Some("pane.split"),
        Command::Close => Some("pane.close"),
        Command::Next => Some("pane.next"),
        Command::Rename => Some("file.rename"),
        Command::NewFolder => Some("file.new_folder"),
        Command::NewFile => Some("file.new"),
        Command::Copy => Some("file.copy"),
        Command::Cut => Some("file.cut"),
        Command::Paste => Some("file.paste"),
        Command::CopyTo => Some("file.copy_to"),
        Command::MoveTo => Some("file.move_to"),
        Command::Duplicate => Some("file.duplicate"),
        Command::Delete => Some("file.trash"),
        Command::Refresh => Some("file.refresh"),
        Command::Hidden => Some("file.toggle_hidden"),
        Command::Preview => Some("view.preview"),
        Command::Sidebar => Some("view.sidebar"),
        Command::Help => Some("app.help"),
        Command::Back => Some("location.back"),
        Command::Forward => Some("location.forward"),
        Command::Parent => Some("location.parent"),
        Command::SelectAll => Some("file.select_all"),
        Command::Home => Some("location.home"),
        Command::QuickLook => Some("file.quick_look"),
        Command::FontIncrease => Some("font.increase"),
        Command::FontDecrease => Some("font.decrease"),
        Command::FontReset => Some("font.reset"),
        Command::Settings => Some("app.settings"),
        Command::Desktop => Some("location.desktop"),
        Command::Downloads => Some("location.downloads"),
        Command::Workspace => Some("location.workspace"),
        Command::Root => Some("location.root"),
        Command::Plugins => Some("plugin.directory"),
        Command::ReloadPlugins => Some("plugin.reload"),
        _ => None,
    }
}
fn binding_label(overrides: &keybindings::Overrides, id: &str) -> String {
    display_bindings(&keybindings::resolved(overrides, id))
}
fn display_bindings(keys: &[String]) -> String {
    let Some(first) = keys.first() else {
        return "Unbound".into();
    };
    let label = first
        .replace("cmd-", "⌘")
        .replace("ctrl-", "⌃")
        .replace("alt-", "⌥")
        .replace("shift-", "⇧")
        .replace("backspace", "⌫")
        .replace("enter", "Enter")
        .replace("space", "Space")
        .replace("right", "→")
        .replace("left", "←")
        .replace("up", "↑")
        .replace("down", "↓");
    if keys.len() > 1 {
        format!("{label} (+{})", keys.len() - 1)
    } else {
        label
    }
}
/// macOS reports shifted punctuation as the produced symbol with Shift cleared.
/// Keep canonical settings/conflict keys, but register the equivalent literal
/// plus forms so configured shifted-equals chords match native events too.
fn runtime_binding_keys(canonical: &str) -> Vec<String> {
    let mut result = vec![];
    if let Ok(mut stroke) = Keystroke::parse(canonical)
        && stroke.key == "="
        && stroke.modifiers.shift
    {
        stroke.key = "+".into();
        stroke.modifiers.shift = false;
        result.push(stroke.unparse());
        stroke.modifiers.shift = true;
        result.push(stroke.unparse());
    }
    result.push(canonical.to_owned());
    result
}
const APP_KEYMAP: KeyBindingMetaIndex = KeyBindingMetaIndex(0x42554b54);
fn merged_key_bindings<'a>(
    existing: impl Iterator<Item = &'a KeyBinding>,
    replacements: Vec<KeyBinding>,
) -> Vec<KeyBinding> {
    existing
        .filter(|binding| binding.meta() != Some(APP_KEYMAP))
        .cloned()
        .chain(replacements)
        .collect()
}
#[derive(Serialize, Deserialize, Default)]
struct Session {
    #[serde(default)]
    workspace_id: Option<String>,
    locations: Vec<String>,
    active: usize,
    hidden: bool,
    #[serde(default)]
    sidebar: Option<bool>,
    #[serde(default)]
    preview: Option<bool>,
    #[serde(default)]
    font_size: Option<f32>,
    #[serde(default)]
    layout: named_workspaces::PaneLayout,
    #[serde(default)]
    views: Vec<named_workspaces::PaneView>,
    #[serde(default)]
    weights: Vec<f32>,
}
enum PluginOutcome {
    Terminal(TerminalSession, PathBuf),
    Agent(buckets_plugin::AgentLaunchRequest),
    Value,
}
struct PluginCommand {
    plugin: String,
    id: String,
    title: String,
    shortcut: Option<String>,
}
#[derive(Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace=buckets,no_json)]
struct PluginShortcut {
    plugin: String,
    command: String,
}

struct Browser {
    workspace_id: String,
    workspace_revision: u64,
    exit_after_agent: Option<u64>,
    agent_settings_fingerprint: String,
    panes: Vec<Pane>,
    active: usize,
    next_id: u64,
    focus: FocusHandle,
    input: Entity<InputState>,
    _subscription: Subscription,
    _activation: Subscription,
    _quit: Subscription,
    prompt: Option<Prompt>,
    bookmark_target: Option<Location>,
    palette_cursor: usize,
    help: bool,
    quicklook: bool,
    sidebar: bool,
    show_preview: bool,
    hidden: bool,
    font_size: font_zoom::FontSize,
    settings_store: Option<settings::SettingsStore>,
    settings_view: Option<Entity<settings_view::SettingsView>>,
    settings_subscription: Option<Subscription>,
    settings_open: bool,
    exit_after_settings: bool,
    settings_warning: Option<String>,
    plugin_generation: u64,
    account_sessions: account_state::AccountSessions,
    menu_state: Option<menus::MenuState>,
    decoration_epoch: u64,
    preview: String,
    preview_title: String,
    preview_generation: u64,
    preview_scroll: ScrollHandle,
    help_scroll: ScrollHandle,
    palette_scroll: ScrollHandle,
    plugins: Arc<PluginRegistry>,
    plugin_commands: Vec<PluginCommand>,
    plugin_bindings: std::collections::BTreeMap<String, Vec<String>>,
    keymap_conflicts: Vec<String>,
    clipboard: Option<(Vec<Location>, bool)>,
    status: String,
    status_error: bool,
    operations: usize,
    operation_ui: operations_session::OperationUi,
    named_workspaces: Option<named_workspaces::NamedWorkspaceStore>,
    pane_layout: named_workspaces::PaneLayout,
    layout_epoch: u64,
    watcher: Option<watch::LocalWatcher>,
}
impl Browser {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        let input = cx.new(|cx| InputState::new(window, cx));
        let sub = cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
            InputEvent::PressEnter { .. } => this.submit(window, cx),
            InputEvent::Change => {
                if this.prompt == Some(Prompt::Search) {
                    this.filter_names(cx);
                    return;
                }
                this.palette_cursor = 0;
                this.palette_scroll.scroll_to_item(0);
                cx.notify();
            }
            _ => {}
        });
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                for i in 0..this.panes.len() {
                    if !this.panes[i].is_files() {
                        continue;
                    }
                    if this.panes[i].search.is_some() {
                        this.load_decorations(i, cx);
                    } else {
                        this.panes[i].refresh_pending = true;
                    }
                }
            }
        });
        let quit = cx.on_app_quit(|this, cx| {
            this.save();
            for pane in &this.panes {
                if let PaneContent::Terminal(view) = &pane.content {
                    view.update(cx, |view, _| view.close());
                } else if let PaneContent::Agent(view) = &pane.content {
                    view.update(cx, |view, cx| {
                        view.invalidate("Application is closing.", cx)
                    });
                }
            }
            async {}
        });
        let session = std::fs::read(session_path())
            .ok()
            .and_then(|v| serde_json::from_slice::<Session>(&v).ok())
            .unwrap_or_default();
        let legacy = settings::Settings {
            font_size: font_zoom::FontSize::from_saved(session.font_size).points(),
            show_hidden: session.hidden,
            sidebar: session.sidebar.unwrap_or(true),
            preview: session.preview.unwrap_or(true),
            ..Default::default()
        };
        let (settings_store, mut settings_warning) = match settings::SettingsStore::recover(
            session_path().with_file_name("settings.json"),
            legacy.clone(),
        ) {
            Ok((store, warning)) => (Some(store), warning),
            Err(error) => (
                None,
                Some(format!("Could not open settings.json: {error:#}")),
            ),
        };
        let preferences = settings_store
            .as_ref()
            .map(|store| store.settings().clone())
            .unwrap_or(legacy);
        let args: Vec<String> = std::env::args().skip(1).collect();
        let explicit_workspace = !args.is_empty();
        let locations = if explicit_workspace {
            args
        } else if !session.locations.is_empty() {
            session.locations
        } else {
            vec![
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("/"))
                    .to_string_lossy()
                    .into_owned(),
                std::env::var("HOME").unwrap_or_else(|_| "/".into()),
            ]
        };
        let mut panes: Vec<_> = locations
            .into_iter()
            .take(6)
            .filter_map(|s| Location::parse(&s).ok())
            .enumerate()
            .map(|(i, l)| Pane::new(i as u64, l))
            .collect();
        if panes.is_empty() {
            panes.push(Pane::new(0, Location::parse("/").expect("root location")));
        }
        for (index, pane) in panes.iter_mut().enumerate() {
            pane.view = session
                .views
                .get(index)
                .filter(|view| view.validate().is_ok())
                .cloned()
                .unwrap_or_default();
            pane.weight = session
                .weights
                .get(index)
                .copied()
                .filter(|v| v.is_finite() && *v > 0. && *v <= 1000.)
                .unwrap_or(1.);
        }
        let named_workspaces = match named_workspaces::NamedWorkspaceStore::load(
            session_path().with_file_name("workspaces.json"),
        ) {
            Ok(store) => Some(store),
            Err(error) => {
                let notice = format!("Named workspaces unavailable: {error:#}");
                settings_warning = Some(
                    settings_warning
                        .map_or_else(|| notice.clone(), |warning| format!("{warning} · {notice}")),
                );
                None
            }
        };
        let font_size = font_zoom::FontSize::from_saved(Some(preferences.font_size));
        theme::apply(&preferences.appearance, font_size.points(), cx);
        if let Some(notice) = theme::font_notice(cx) {
            settings_warning = Some(
                settings_warning
                    .map_or_else(|| notice.clone(), |warning| format!("{warning} · {notice}")),
            );
        }
        window.set_rem_size(px(font_size.points()));
        let active = session.active.min(panes.len() - 1);
        let watcher = watch::LocalWatcher::new();
        let watcher_error = watcher.as_ref().err().map(|error| {
            format!("Live updates unavailable: {error:#} · Refresh from the File menu")
        });
        let initial_roots = panes
            .iter()
            .map(|pane| pane.location.clone())
            .collect::<Vec<_>>();
        let workspace_id = agent_context::workspace_for_launch(
            session_path().parent().unwrap(),
            explicit_workspace.then_some(initial_roots.as_slice()),
            session.workspace_id.as_deref(),
        );
        let agent_settings_fingerprint =
            serde_json::to_string(&preferences.plugins).unwrap_or_default();
        let mut this = Self {
            workspace_id,
            workspace_revision: 0,
            exit_after_agent: None,
            agent_settings_fingerprint,
            next_id: panes.len() as u64,
            panes,
            active,
            focus,
            input,
            _subscription: sub,
            _activation: activation,
            _quit: quit,
            prompt: None,
            bookmark_target: None,
            palette_cursor: 0,
            help: false,
            quicklook: false,
            sidebar: preferences.sidebar,
            show_preview: preferences.preview,
            hidden: preferences.show_hidden,
            settings_store,
            settings_warning: settings_warning.clone(),
            settings_view: None,
            settings_subscription: None,
            settings_open: false,
            exit_after_settings: false,
            plugin_generation: 0,
            account_sessions: Default::default(),
            menu_state: None,
            decoration_epoch: 0,
            font_size,
            preview: String::new(),
            preview_title: String::new(),
            preview_generation: 0,
            preview_scroll: ScrollHandle::new(),
            help_scroll: ScrollHandle::new(),
            palette_scroll: ScrollHandle::new(),
            plugins: Arc::new(PluginRegistry::default()),
            plugin_commands: vec![],
            plugin_bindings: Default::default(),
            keymap_conflicts: vec![],
            clipboard: None,
            status: settings_warning
                .clone()
                .or(watcher_error.clone())
                .unwrap_or_else(|| "Ready · Open commands from the View menu".into()),
            status_error: settings_warning.is_some() || watcher_error.is_some(),
            operations: 0,
            operation_ui: operations_session::OperationUi::new(
                session_path().with_file_name("operations"),
            ),
            named_workspaces,
            pane_layout: session.layout,
            layout_epoch: 0,
            watcher: watcher.ok(),
        };
        for i in 0..this.panes.len() {
            this.reload(i, cx);
        }
        this.load_operation_history(cx);
        this.discover_plugins(cx);
        this.sync_watches();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(std::time::Duration::from_millis(700)).await;
                if this
                    .update(cx, |this, cx| {
                        let changed = match this.watcher.as_ref().map(|watcher| watcher.changed()) {
                            Some(Ok(changed)) => changed,
                            Some(Err(error)) => {
                                this.message(
                                    format!("Live updates unavailable: {error:#} · Refresh from the File menu"),
                                    true,
                                    cx,
                                );
                                false
                            }
                            None => false,
                        };
                        if changed {
                            for pane in &mut this.panes {
                                if pane.is_files() && pane.location.local_path().is_some() {
                                    pane.refresh_pending = true;
                                }
                            }
                        }
                        for i in 0..this.panes.len() {
                            if this.panes[i].refresh_pending
                                && !this.panes[i].loading
                                && this.panes[i].search.is_none()
                            {
                                this.panes[i].refresh_pending = false;
                                this.reload(i, cx);
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        this
    }
    fn sync_watches(&mut self) {
        let roots = self
            .panes
            .iter()
            .filter(|p| p.is_files())
            .filter_map(|p| p.location.local_path().map(|path| path.to_path_buf()))
            .collect();
        if let Some(watcher) = &mut self.watcher
            && let Err(error) = watcher.sync(roots)
        {
            self.status =
                format!("Live updates unavailable: {error:#} · Refresh from the File menu");
            self.status_error = true;
        }
    }
    fn save(&mut self) {
        self.sync_watches();
        let path = session_path();
        if let Some(p) = path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let session = Session {
            workspace_id: Some(self.workspace_id.clone()),
            locations: self.panes.iter().map(|p| p.location.display()).collect(),
            active: self.active,
            hidden: self.hidden,
            sidebar: Some(self.sidebar),
            preview: Some(self.show_preview),
            font_size: Some(self.font_size.points()),
            layout: self.pane_layout,
            views: self.panes.iter().map(|pane| pane.view.clone()).collect(),
            weights: self.panes.iter().map(|pane| pane.weight).collect(),
        };
        if let Ok(data) = serde_json::to_vec_pretty(&session) {
            let _ = std::fs::write(path, data);
        }
    }
    fn change_font_size(
        &mut self,
        adjustment: font_zoom::Adjustment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut font = self.font_size;
        font.adjust(adjustment);
        self.update_preference(settings_view::Change::Font(font.points()), window, cx);
    }
    fn current_settings(&self) -> settings::Settings {
        self.settings_store
            .as_ref()
            .map(|s| s.settings().clone())
            .unwrap_or_else(|| settings::Settings {
                font_size: self.font_size.points(),
                show_hidden: self.hidden,
                sidebar: self.sidebar,
                preview: self.show_preview,
                ..Default::default()
            })
    }
    fn sync_settings_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = &self.settings_view {
            let settings = self.current_settings();
            let raw = self
                .settings_store
                .as_ref()
                .map(|s| s.raw_json())
                .unwrap_or_else(|| serde_json::to_string_pretty(&settings).unwrap_or_default());
            let plugins = self.plugins.plugins().to_vec();
            let shortcuts = self.shortcut_catalog();
            view.update(cx, |view, cx| {
                view.sync(settings, raw, plugins, window, cx);
                view.shortcuts(shortcuts, cx);
            });
        }
    }
    fn recording_shortcut(&self, cx: &App) -> bool {
        self.settings_open
            && self
                .settings_view
                .as_ref()
                .is_some_and(|view| view.read(cx).is_recording())
    }
    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.operation_ui.open = false;
        if self.recording_shortcut(cx) {
            return;
        }
        if self.settings_view.is_none() {
            let preferences = self.current_settings();
            let raw = self
                .settings_store
                .as_ref()
                .map(|s| s.raw_json())
                .unwrap_or_else(|| serde_json::to_string_pretty(&preferences).unwrap_or_default());
            let plugins = self.plugins.plugins().to_vec();
            let view = cx.new(|cx| {
                settings_view::SettingsView::new(
                    preferences,
                    self.settings_store
                        .as_ref()
                        .map(|store| store.path().to_path_buf())
                        .unwrap_or_else(|| session_path().with_file_name("settings.json")),
                    raw,
                    plugins,
                    window,
                    cx,
                )
            });
            let subscription = cx.subscribe_in(&view, window, |this, _, event, window, cx| {
                this.settings_event(event, window, cx)
            });
            if let Some(warning) = &self.settings_warning {
                view.update(cx, |view, cx| view.warning(warning.clone(), cx));
            }
            let shortcuts = self.shortcut_catalog();
            view.update(cx, |view, cx| view.shortcuts(shortcuts, cx));
            self.settings_view = Some(view);
            self.settings_subscription = Some(subscription);
        }
        self.settings_open = true;
        self.sync_account_views(cx);
        self.refresh_account_states(cx);
        if let Some(view) = &self.settings_view {
            view.update(cx, |view, cx| view.focus(window, cx));
        }
        cx.notify();
    }
    fn apply_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.current_settings();
        let hidden_changed = self.hidden != settings.show_hidden;
        self.hidden = settings.show_hidden;
        self.sidebar = settings.sidebar;
        self.show_preview = settings.preview;
        self.font_size = font_zoom::FontSize::from_saved(Some(settings.font_size));
        theme::apply(&settings.appearance, self.font_size.points(), cx);
        let terminal_appearance = theme::terminal(cx);
        for pane in &self.panes {
            if let PaneContent::Terminal(view) = &pane.content {
                view.update(cx, |view, cx| {
                    view.set_appearance(terminal_appearance.clone(), cx)
                });
            }
        }
        window.set_rem_size(px(self.font_size.points()));
        self.palette_scroll.scroll_to_item(self.palette_cursor);
        if hidden_changed {
            for i in 0..self.panes.len() {
                self.reload(i, cx);
            }
        }
        self.decoration_epoch += 1;
        let agent_settings = serde_json::to_string(&settings.plugins).unwrap_or_default();
        if self.agent_settings_fingerprint != agent_settings {
            self.agent_settings_fingerprint = agent_settings;
            self.account_sessions.clear();
            self.sync_account_views(cx);
            self.clear_private_agents(
                "Plugin settings changed. Run a new request to use them.",
                cx,
            );
        }
        if let Ok(registry) = self.plugins.with_settings(&settings.plugins) {
            self.plugins = Arc::new(registry);
            self.plugin_commands
                .retain(|command| self.plugins.is_enabled(&command.plugin));
            for i in 0..self.panes.len() {
                self.load_decorations(i, cx);
            }
        }
        self.rebuild_keymap(cx);
        self.load_preview(cx);
        self.sync_settings_view(window, cx);
        cx.refresh_windows();
    }
    fn update_preference(
        &mut self,
        change: settings_view::Change,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let plugin_changed = matches!(
            change,
            settings_view::Change::PluginEnabled(..) | settings_view::Change::PluginField(..)
        );
        let result = self.settings_store.as_mut().ok_or_else(|| {
            anyhow!("Settings file is unavailable; check its directory permissions, then reopen Buckets")
        }).and_then(|store| store.update(|settings| match change {
            settings_view::Change::Font(size) => settings.font_size = size,
            settings_view::Change::AppearanceFont{terminal,value} => {if terminal {settings.appearance.terminal_font_family=value;}else{settings.appearance.font_family=value;}},
            settings_view::Change::AppearanceColor(key,value) => {if let Some(value)=value{settings.appearance.colors.insert(key,value);}else{settings.appearance.colors.remove(&key);}},
            settings_view::Change::AppearanceReset => {let extra=std::mem::take(&mut settings.appearance.extra);settings.appearance=appearance::Appearance::default();settings.appearance.extra=extra;},
            settings_view::Change::Hidden(value) => settings.show_hidden = value,
            settings_view::Change::Sidebar(value) => settings.sidebar = value,
            settings_view::Change::Preview(value) => settings.preview = value,
            settings_view::Change::PluginEnabled(id, enabled) => {
                settings.plugins.entry(id).or_default().enabled = Some(enabled);
            }
            settings_view::Change::PluginField(id, key, value) => {
                let plugin = settings.plugins.entry(id).or_default();
                if !plugin.config.is_object() {
                    plugin.config = serde_json::json!({});
                }
                if let Some(config) = plugin.config.as_object_mut() {
                    if let Some(value) = value {
                        config.insert(key, value);
                    } else {
                        config.remove(&key);
                    }
                }
            }
            settings_view::Change::SaveBookmark(bookmark)=>{
                if let Some(existing)=settings.bookmarks.iter_mut().find(|b|b.canonical_location().ok().as_deref()==Some(bookmark.location.as_str())){existing.name=bookmark.name;existing.location=bookmark.location;}else{settings.bookmarks.push(bookmark);}
            },
            settings_view::Change::RenameBookmark(index,name)=>{if let Some(bookmark)=settings.bookmarks.get_mut(index){bookmark.name=name.trim().to_owned();}},
            settings_view::Change::RemoveBookmark(index)=>{if index<settings.bookmarks.len(){settings.bookmarks.remove(index);}},
            settings_view::Change::Keybinding(id, value) => {
                if let Some(value)=value { settings.keybindings.insert(id,value); } else { settings.keybindings.remove(&id); }
            }
            settings_view::Change::Reset => {
                settings.font_size = 13.;
                settings.show_hidden = false;
                settings.sidebar = true;
                settings.preview = true;
            }
        }));
        match result {
            Ok(()) => {
                self.settings_warning = None;
                self.apply_preferences(window, cx);
                if plugin_changed {
                    self.discover_plugins(cx);
                }
                self.message(
                    if self.keymap_conflicts.is_empty() {
                        format!("Settings saved · Text size {}pt", self.font_size.points())
                    } else {
                        format!(
                            "Settings saved; conflicting shortcuts skipped: {}",
                            self.keymap_conflicts.join(" · ")
                        )
                    },
                    !self.keymap_conflicts.is_empty(),
                    cx,
                );
                if let Some(view) = &self.settings_view {
                    view.update(cx, |view, cx| {
                        view.result(Ok(()), window, cx);
                        if !self.keymap_conflicts.is_empty() {
                            view.warning(self.status.clone(), cx);
                        }
                    });
                }
            }
            Err(error) => {
                self.settings_error(format!("Could not save settings: {error:#}"), window, cx)
            }
        }
    }
    fn settings_error(&mut self, error: String, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_warning = Some(error.clone());
        self.message(error.clone(), true, cx);
        if let Some(view) = &self.settings_view {
            view.update(cx, |view, cx| view.result(Err(error), window, cx));
        }
    }
    fn settings_event(
        &mut self,
        event: &settings_view::SettingsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            settings_view::SettingsEvent::Account { plugin, request } => {
                self.run_account(plugin.clone(), request.clone(), cx);
            }
            settings_view::SettingsEvent::CancelAccount(plugin) => {
                self.account_sessions.cancel(plugin);
                self.sync_account_views(cx);
            }
            settings_view::SettingsEvent::AccountUsage(url) => {
                if let Ok(url) = account_state::external_url(url) {
                    cx.open_url(&url);
                }
            }
            settings_view::SettingsEvent::Change(change) => {
                self.update_preference(change.clone(), window, cx)
            }
            settings_view::SettingsEvent::CancelClose => {
                if std::env::var_os("BUCKETS_TRACE_EXIT").is_some() {
                    eprintln!("settings exit: cancelled");
                }
                self.exit_after_settings = false;
                cx.notify();
            }
            settings_view::SettingsEvent::Close => {
                if std::env::var_os("BUCKETS_TRACE_EXIT").is_some() {
                    eprintln!("settings close: quit={}", self.exit_after_settings);
                }
                self.settings_open = false;
                self.account_sessions.cancel_all();
                self.sync_account_views(cx);
                if std::mem::take(&mut self.exit_after_settings) {
                    self.save();
                    self.request_quit(window, cx);
                    return;
                }
                if self.prompt.is_some() {
                    self.input.focus_handle(cx).focus(window);
                } else {
                    self.focus_active(window, cx);
                }
                cx.notify();
            }
            settings_view::SettingsEvent::Reload => {
                let result = self
                    .settings_store
                    .as_mut()
                    .ok_or_else(|| anyhow!("Settings unavailable"))
                    .and_then(|store| store.reload());
                match result {
                    Ok(()) => {
                        self.settings_warning = None;
                        self.apply_preferences(window, cx);
                        self.discover_plugins(cx);
                        self.message("Settings reloaded from disk", false, cx);
                        if let Some(view) = &self.settings_view {
                            view.update(cx, |view, cx| {
                                view.result(Ok(()), window, cx);
                                if !self.keymap_conflicts.is_empty() {
                                    view.warning(self.status.clone(), cx);
                                }
                            });
                        }
                    }
                    Err(error) => {
                        self.sync_settings_view(window, cx);
                        self.settings_error(format!("Reload failed: {error:#}"), window, cx);
                    }
                }
            }
            settings_view::SettingsEvent::SaveJson { source, base } => {
                let result=self.settings_store.as_mut().ok_or_else(||anyhow!("Settings unavailable")).and_then(|store|{if store.raw_json()!=*base{return Err(anyhow!("Preferences changed after this JSON draft began. Keep this draft, or choose Reload from disk to discard it and start from the current file."));}store.replace_json(source)});
                match result {
                    Ok(()) => {
                        self.settings_warning = None;
                        self.apply_preferences(window, cx);
                        self.discover_plugins(cx);
                        self.message("Settings JSON saved", false, cx);
                        if let Some(view) = &self.settings_view {
                            view.update(cx, |view, cx| {
                                view.json_saved(window, cx);
                                view.result(Ok(()), window, cx);
                            });
                        }
                    }
                    Err(error) => {
                        self.settings_error(format!("JSON was not saved: {error:#}"), window, cx)
                    }
                }
            }
        }
    }
    fn message(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        let text = text.into();
        if let Some(notice) = theme::font_notice(cx) {
            self.status = if text.contains(&notice) {
                text
            } else {
                format!("{text} · {notice}")
            };
            self.status_error = true;
        } else {
            self.status = text;
            self.status_error = error;
        }
        cx.notify();
    }
    fn reload(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.panes[index].is_files() {
            return;
        }
        let pane = &mut self.panes[index];
        pane.loading = true;
        pane.error = None;
        pane.generation += 1;
        let generation = pane.generation;
        let id = pane.id;
        let location = pane.location.clone();
        let hidden = self.hidden;
        let task = cx.background_executor().spawn(async move {
            Backend.list(&location).map(|entries| {
                entries
                    .into_iter()
                    .filter(|e| hidden || !e.name.starts_with('.'))
                    .collect::<Vec<_>>()
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(i) = this
                    .panes
                    .iter()
                    .position(|p| p.id == id && p.generation == generation)
                else {
                    return;
                };
                let p = &mut this.panes[i];
                p.loading = false;
                match result {
                    Ok(entries) => {
                        p.replace_entries(entries);
                        if let Some(target) = p.pending_reveal.take()
                            && let Some(index) =
                                p.entries.iter().position(|entry| entry.location == target)
                        {
                            p.cursor = index;
                            p.scroll.scroll_to_item(index, ScrollStrategy::Top);
                        }
                        p.search = None;
                    }
                    Err(e) => {
                        p.error = Some(format!("{e:#}"));
                    }
                }
                this.panes[i].refresh_name_filter();
                this.load_decorations(i, cx);
                if i == this.active {
                    this.load_preview(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn navigate(&mut self, location: Location, cx: &mut Context<Self>) {
        if !self.panes[self.active].is_files() {
            self.message("Switch to a file pane to browse a location", false, cx);
            return;
        }
        let p = &mut self.panes[self.active];
        p.loading = true;
        p.error = None;
        p.generation += 1;
        let generation = p.generation;
        let id = p.id;
        let hidden = self.hidden;
        let target = location.clone();
        let task = cx.background_executor().spawn(async move {
            Backend.list(&target).map(|entries| {
                entries
                    .into_iter()
                    .filter(|e| hidden || !e.name.starts_with('.'))
                    .collect::<Vec<_>>()
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(i) = this
                    .panes
                    .iter()
                    .position(|p| p.id == id && p.generation == generation)
                else {
                    return;
                };
                let p = &mut this.panes[i];
                p.loading = false;
                match result {
                    Ok(entries) => {
                        p.history.push(p.location.clone());
                        p.forward.clear();
                        p.location = location;
                        p.cursor = 0;
                        p.selected.clear();
                        p.selection_anchor = None;
                        p.search = None;
                        p.entries = entries;
                        p.finish_name_filter(false);
                        this.load_decorations(i, cx);
                        if i == this.active {
                            this.load_preview(cx);
                        }
                        this.save();
                        this.message("Location opened", false, cx);
                    }
                    Err(e) => this.message(
                        format!("Could not open location: {e:#} · Choose another location from the Go menu"),
                        true,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn load_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_generation += 1;
        let generation = self.preview_generation;
        if !self.panes[self.active].is_files() || self.panes[self.active].visible_count() == 0 {
            self.preview.clear();
            self.preview_title.clear();
            return;
        }
        let Some(entry) = self.panes[self.active]
            .entries
            .get(self.panes[self.active].cursor)
        else {
            self.preview.clear();
            self.preview_title.clear();
            return;
        };
        self.preview_title = entry.location.name();
        self.preview_scroll.set_offset(point(px(0.), px(0.)));
        if entry.is_dir {
            self.preview = format!(
                "Directory\n\n{} to browse\n{} for Quick Look · x to mark",
                self.binding_hint_id("file.open"),
                self.binding_hint(Command::QuickLook)
            );
            return;
        }
        if !self.show_preview && !self.quicklook {
            return;
        }
        self.preview = "Loading preview…".into();
        let location = entry.location.clone();
        let detail = self.panes[self.active]
            .result_details
            .get(&location.display())
            .cloned();
        let task = cx
            .background_executor()
            .spawn(async move { Backend.preview(&location, 24 * 1024) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.preview_generation != generation {
                    return;
                }
                this.preview = match result {
                    Ok(p) if p.binary => {
                        "Binary file\n\nPress Enter to open in its default application.".into()
                    }
                    Ok(p) => format!(
                        "{}{}",
                        p.text,
                        if p.truncated {
                            "\n\n… preview limited to 24 KiB"
                        } else {
                            ""
                        }
                    ),
                    Err(e) => format!("Preview unavailable\n\n{e:#}"),
                };
                if let Some(detail) = detail {
                    this.preview = format!("{detail}\n\n── File preview ──\n\n{}", this.preview);
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn move_cursor(&mut self, delta: isize, mark: bool, cx: &mut Context<Self>) {
        let p = &mut self.panes[self.active];
        let indices = p
            .name_filter
            .as_ref()
            .map(|filter| filter.indices.clone())
            .unwrap_or_else(|| Arc::new((0..p.entries.len()).collect()));
        if indices.is_empty() {
            return;
        }
        let position = indices.iter().position(|i| *i == p.cursor).unwrap_or(0);
        let anchor = if mark {
            *p.selection_anchor.get_or_insert(p.cursor)
        } else {
            p.selection_anchor = None;
            p.cursor
        };
        let next = position.saturating_add_signed(delta).min(indices.len() - 1);
        p.cursor = indices[next];
        if mark {
            let anchor = indices
                .iter()
                .position(|i| *i == anchor)
                .unwrap_or(position);
            p.selected = indices[anchor.min(next)..=anchor.max(next)]
                .iter()
                .copied()
                .collect();
        }
        p.scroll.scroll_to_item(next, ScrollStrategy::Top);
        self.load_preview(cx);
        cx.notify();
    }
    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.panes[self.active].visible_count() == 0 {
            return;
        }
        if self.prompt == Some(Prompt::Search) {
            self.finish_name_filter(false, cx);
            self.prompt = None;
            self.focus.focus(window);
        }
        let Some(e) = self.panes[self.active]
            .entries
            .get(self.panes[self.active].cursor)
            .cloned()
        else {
            return;
        };
        if e.is_dir {
            self.navigate(e.location, cx);
        } else {
            let Some(local_path) = e.location.local_path() else {
                self.message("Remote file preview is available. Copy the file to a local pane to open it in another app.",false,cx);
                return;
            };
            let location = local_path.to_string_lossy().into_owned();
            self.operation(
                "Open file",
                move || {
                    let path = Location::parse(&location)?;
                    let text = path.display();
                    let output = std::process::Command::new(if cfg!(target_os = "macos") {
                        "open"
                    } else {
                        "xdg-open"
                    })
                    .arg(&text)
                    .output()?;
                    if !output.status.success() {
                        return Err(anyhow!(
                            String::from_utf8_lossy(&output.stderr).into_owned()
                        ));
                    }
                    Ok(())
                },
                cx,
            );
        }
        self.focus.focus(window);
    }
    fn filter_names(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_string();
        let pane = &mut self.panes[self.active];
        if let Some(filter) = &mut pane.name_filter {
            filter.query = query;
        }
        pane.refresh_name_filter();
        let cursor = pane.visible_cursor();
        pane.scroll.scroll_to_item(cursor, ScrollStrategy::Top);
        self.load_preview(cx);
        cx.notify();
    }
    fn finish_name_filter(&mut self, restore: bool, cx: &mut Context<Self>) {
        for pane in &mut self.panes {
            if pane.name_filter.is_some() {
                pane.finish_name_filter(restore);
                pane.scroll.scroll_to_item(pane.cursor, ScrollStrategy::Top);
            }
        }
        self.load_preview(cx);
        cx.notify();
    }
    fn begin(&mut self, mode: Prompt, window: &mut Window, cx: &mut Context<Self>) {
        if !self.panes[self.active].is_files()
            && !matches!(
                mode,
                Prompt::Palette | Prompt::WorkspaceName(_) | Prompt::RemoveWorkspace(_)
            )
        {
            self.message("Switch to a file pane to browse or search", false, cx);
            return;
        }
        if self.panes.iter().any(|pane| pane.name_filter.is_some()) {
            self.finish_name_filter(true, cx);
        }
        if mode == Prompt::Search {
            self.panes[self.active].start_name_filter();
        }
        self.help = false;
        self.quicklook = false;
        self.bookmark_target = match mode {
            Prompt::BookmarkName(Some(i)) => self
                .current_settings()
                .bookmarks
                .get(i)
                .and_then(|b| Location::parse(&b.location).ok()),
            Prompt::BookmarkName(None) => Some(self.panes[self.active].location.clone()),
            _ => None,
        };
        self.prompt = Some(mode);
        self.palette_cursor = 0;
        self.palette_scroll.scroll_to_item(0);
        let value = match mode {
            Prompt::BookmarkName(index) => {
                let location = self
                    .bookmark_target
                    .as_ref()
                    .unwrap_or(&self.panes[self.active].location);
                let canonical = location.display();
                let bookmarks = self.current_settings().bookmarks;
                index
                    .and_then(|i| bookmarks.get(i))
                    .or_else(|| {
                        bookmarks.iter().find(|b| {
                            b.canonical_location().ok().as_deref() == Some(canonical.as_str())
                        })
                    })
                    .map(|b| b.name.clone())
                    .unwrap_or_else(|| {
                        let name = location.name();
                        if name.is_empty() { canonical } else { name }
                    })
            }
            Prompt::WorkspaceName(index) => index
                .and_then(|i| self.named_workspaces.as_ref()?.entries().get(i))
                .map(|entry| entry.name.clone())
                .unwrap_or_default(),
            Prompt::Location => self.panes[self.active].location.display(),
            Prompt::Rename => self.panes[self.active]
                .entries
                .get(self.panes[self.active].cursor)
                .map(|e| e.location.name())
                .unwrap_or_default(),
            Prompt::CopyTo | Prompt::MoveTo => self.panes[(self.active + 1) % self.panes.len()]
                .location
                .display(),
            _ => String::new(),
        };
        self.input.update(cx, |input, cx| {
            input.set_value(value, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }
    fn focus_active(&self, window: &mut Window, cx: &App) {
        match &self.panes[self.active].content {
            PaneContent::Terminal(view) => view.read(cx).focus_handle(cx).focus(window),
            PaneContent::Files => self.focus.focus(window),
            PaneContent::Agent(view) => view.read(cx).focus_handle(cx).focus(window),
        }
    }
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt == Some(Prompt::Search) {
            self.finish_name_filter(true, cx);
        }
        self.prompt = None;
        self.bookmark_target = None;
        self.help = false;
        self.quicklook = false;
        self.focus_active(window, cx);
        cx.notify();
    }
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mode) = self.prompt else {
            return;
        };
        let value = self.input.read(cx).value().to_string();
        let value = if matches!(mode, Prompt::Palette | Prompt::Delete) {
            value.trim().to_string()
        } else {
            value
        };
        if matches!(mode, Prompt::BookmarkName(_)) && value.trim().is_empty() {
            self.message("Enter a bookmark name before saving", true, cx);
            return;
        }
        if value.is_empty() && !matches!(mode, Prompt::Palette | Prompt::Search) {
            return;
        }
        if mode == Prompt::Search && !broader_search(&value) {
            if self.panes[self.active].visible_count() == 0 {
                self.message(
                    "No matching names · change the filter or Escape to clear",
                    false,
                    cx,
                );
                return;
            }
            self.finish_name_filter(false, cx);
            self.dismiss(window, cx);
            self.open(window, cx);
            return;
        }
        if let Prompt::BookmarkName(index) = mode {
            let location = self
                .bookmark_target
                .clone()
                .unwrap_or_else(|| self.panes[self.active].location.clone());
            if let Some(index) = index
                && self
                    .current_settings()
                    .bookmarks
                    .get(index)
                    .and_then(|bookmark| bookmark.canonical_location().ok())
                    .as_deref()
                    != Some(location.display().as_str())
            {
                self.message("This bookmark changed while its name prompt was open. Cancel and choose Rename bookmark again.",true,cx);
                return;
            }
            match settings::Bookmark::new(value, location.display()) {
                Ok(bookmark) => {
                    if let Some(index) = index {
                        self.update_preference(
                            settings_view::Change::RenameBookmark(index, bookmark.name),
                            window,
                            cx,
                        );
                    } else {
                        self.update_preference(
                            settings_view::Change::SaveBookmark(bookmark),
                            window,
                            cx,
                        );
                    }
                    if !self.status_error {
                        self.dismiss(window, cx);
                        self.message(
                            format!(
                                "Bookmark saved · {} → Open bookmarks",
                                self.binding_hint_id("app.palette")
                            ),
                            false,
                            cx,
                        );
                    }
                }
                Err(error) => self.message(format!("Bookmark was not saved: {error:#}"), true, cx),
            }
            return;
        }
        if self.submit_workspace_prompt(mode, &value, window, cx) {
            return;
        }
        let chosen = self.panes[self.active].chosen();
        let root = self.panes[self.active].location.clone();
        if mode == Prompt::Palette {
            if let Some((command, _, _)) = self
                .palette_items()
                .into_iter()
                .filter(|(_, label, _)| label.to_lowercase().contains(&value.to_lowercase()))
                .nth(self.palette_cursor)
            {
                self.dismiss(window, cx);
                self.command(command, window, cx);
            }
            return;
        }
        if mode == Prompt::Delete && value != "trash" {
            self.message("Type trash to confirm, or Escape to cancel", true, cx);
            return;
        }
        self.dismiss(window, cx);
        match mode {
            Prompt::Location => match Location::parse(&value) {
                Ok(l) => self.navigate(l, cx),
                Err(e) => self.message(format!("Invalid location: {e:#}"), true, cx),
            },
            Prompt::Search => self.search(value, cx),
            Prompt::Rename => {
                if let Some(src) = self.panes[self.active]
                    .entries
                    .get(self.panes[self.active].cursor)
                    .map(|entry| entry.location.clone())
                {
                    match src.parent().and_then(|parent| parent.join(&value).ok()) {
                        Some(destination) => self.execute_plan(
                            buckets_core::OperationPlan {
                                label: "Rename".into(),
                                actions: vec![buckets_core::OperationAction::Move {
                                    source: src,
                                    destination,
                                }],
                            },
                            cx,
                        ),
                        None => self.message("Invalid rename destination", true, cx),
                    }
                }
            }
            Prompt::NewFolder | Prompt::NewFile => match root.join(&value) {
                Ok(destination) => self.execute_plan(
                    buckets_core::OperationPlan {
                        label: if mode == Prompt::NewFolder {
                            "Create folder"
                        } else {
                            "Create file"
                        }
                        .into(),
                        actions: vec![if mode == Prompt::NewFolder {
                            buckets_core::OperationAction::CreateDirectory { destination }
                        } else {
                            buckets_core::OperationAction::CreateFile { destination }
                        }],
                    },
                    cx,
                ),
                Err(error) => self.message(format!("Invalid destination: {error:#}"), true, cx),
            },
            Prompt::CopyTo | Prompt::MoveTo => match Location::parse(&value) {
                Ok(dst) => self.transfer(chosen, dst, mode == Prompt::MoveTo, cx),
                Err(e) => self.message(format!("Invalid destination: {e:#}"), true, cx),
            },
            Prompt::Delete => self.execute_plan(
                buckets_core::OperationPlan {
                    label: "Trash".into(),
                    actions: chosen
                        .into_iter()
                        .map(|source| buckets_core::OperationAction::Trash { source })
                        .collect(),
                },
                cx,
            ),
            _ => {}
        }
    }
    fn operation(
        &mut self,
        label: &'static str,
        f: impl FnOnce() -> Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.prompt == Some(Prompt::Search) {
            self.message(
                "Finish the name filter before changing files · Enter to open, Escape to clear",
                false,
                cx,
            );
            return;
        }
        self.operations += 1;
        self.message(format!("{label} in progress…"), false, cx);
        let task = cx.background_executor().spawn(async move { f() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.operations = this.operations.saturating_sub(1);
                match result {
                    Ok(()) => this.message(format!("{label} complete"), false, cx),
                    Err(e) => this.message(format!("{label} failed: {e:#}"), true, cx),
                }
                for i in 0..this.panes.len() {
                    this.reload(i, cx);
                }
            });
        })
        .detach();
    }
    fn transfer(
        &mut self,
        sources: Vec<Location>,
        destination: Location,
        cut: bool,
        cx: &mut Context<Self>,
    ) {
        if sources.is_empty() {
            self.message("Select a file first", true, cx);
            return;
        }
        let actions = sources
            .into_iter()
            .map(|source| {
                let destination = destination.join(&source.name())?;
                Ok(if cut {
                    buckets_core::OperationAction::Move {
                        source,
                        destination,
                    }
                } else {
                    buckets_core::OperationAction::Copy {
                        source,
                        destination,
                    }
                })
            })
            .collect::<Result<Vec<_>>>();
        match actions {
            Ok(actions) => self.execute_plan(
                buckets_core::OperationPlan {
                    label: if cut { "Move" } else { "Copy" }.into(),
                    actions,
                },
                cx,
            ),
            Err(error) => self.message(format!("Invalid transfer: {error:#}"), true, cx),
        }
    }
    fn search(&mut self, text: String, cx: &mut Context<Self>) {
        if text.starts_with("plugin:") {
            self.plugin_search(text, cx);
            return;
        }
        if let Some(query) = text.strip_prefix("fff:")
            && self.plugins.plugins().iter().any(|p| p.id == "fff-search")
        {
            self.plugin_search(format!("plugin:fff-search {}", query.trim()), cx);
            return;
        }
        let (engine, query) = if let Some(q) = text.strip_prefix("rg:") {
            (SearchEngine::Rg, q.trim())
        } else if let Some(q) = text.strip_prefix("fd:") {
            (SearchEngine::Fd, q.trim())
        } else if let Some(q) = text.strip_prefix("fff:") {
            (SearchEngine::Fff, q.trim())
        } else {
            (SearchEngine::Auto, text.as_str())
        };
        let query = SearchQuery {
            text: query.to_string(),
            engine,
            hidden: self.hidden,
            respect_gitignore: true,
            max_results: 10_000,
        };
        let p = &mut self.panes[self.active];
        p.loading = true;
        p.generation += 1;
        let generation = p.generation;
        let id = p.id;
        p.search = Some(text.clone());
        let root = p.location.clone();
        let task = cx.background_executor().spawn(async move {
            Backend.search(&root, &query).map(|hits| {
                hits.into_iter()
                    .map(|hit| search_entry(hit.location, hit.line, &root.display()))
                    .collect()
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(i) = this
                    .panes
                    .iter()
                    .position(|p| p.id == id && p.generation == generation)
                else {
                    return;
                };
                let p = &mut this.panes[i];
                p.loading = false;
                match result {
                    Ok(hits) => {
                        p.entries = hits;
                        p.cursor = 0;
                        p.selected.clear();
                        p.search = Some(text);
                        p.error = None;
                    }
                    Err(e) => p.error = Some(format!("Search failed: {e:#}")),
                }
                this.panes[i].refresh_name_filter();
                this.load_decorations(i, cx);
                if i == this.active {
                    this.load_preview(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        if self.operation_ui.open
            && !matches!(
                command,
                Command::Operations
                    | Command::Close
                    | Command::FontIncrease
                    | Command::FontDecrease
                    | Command::FontReset
            )
        {
            return;
        }
        if self.control_command(command, window, cx) {
            return;
        }
        if self.prompt == Some(Prompt::Search)
            && matches!(
                command,
                Command::Next | Command::Split | Command::Close | Command::GoBookmark(_)
            )
        {
            self.dismiss(window, cx);
        }

        if !self.panes[self.active].is_files()
            && !matches!(
                command,
                Command::Close
                    | Command::Next
                    | Command::Split
                    | Command::Help
                    | Command::Sidebar
                    | Command::Preview
                    | Command::PluginInvoke(_)
                    | Command::ReloadPlugins
                    | Command::FontIncrease
                    | Command::FontDecrease
                    | Command::FontReset
                    | Command::Settings
            )
        {
            self.message(
                format!(
                    "File commands apply to file panes · {} to switch",
                    self.binding_hint(Command::Next)
                ),
                false,
                cx,
            );
            return;
        }
        match command {
            Command::BookmarkCurrent => self.begin(Prompt::BookmarkName(None), window, cx),
            Command::OpenBookmarks => {
                if self.current_settings().bookmarks.is_empty() {
                    self.message(
                        format!(
                            "No bookmarks yet · {} saves this location",
                            self.binding_hint(Command::BookmarkCurrent)
                        ),
                        false,
                        cx,
                    );
                } else {
                    self.begin(Prompt::Palette, window, cx);
                    self.input.update(cx, |input, cx| {
                        input.set_value("Open bookmark:", window, cx)
                    });
                }
            }
            Command::GoBookmark(index) => {
                if let Some(bookmark) = self.current_settings().bookmarks.get(index) {
                    match Location::parse(&bookmark.location) {
                        Ok(location) => self.navigate(location, cx),
                        Err(error) => {
                            self.message(format!("Invalid bookmark: {error:#}"), true, cx)
                        }
                    }
                }
            }
            Command::RenameBookmark(index) => {
                self.begin(Prompt::BookmarkName(Some(index)), window, cx)
            }
            Command::RemoveBookmark(index) => {
                self.update_preference(settings_view::Change::RemoveBookmark(index), window, cx)
            }
            Command::Settings => self.open_settings(window, cx),
            Command::FontIncrease => {
                self.change_font_size(font_zoom::Adjustment::Increase, window, cx)
            }
            Command::FontDecrease => {
                self.change_font_size(font_zoom::Adjustment::Decrease, window, cx)
            }
            Command::FontReset => self.change_font_size(font_zoom::Adjustment::Reset, window, cx),
            Command::Location => self.begin(Prompt::Location, window, cx),
            Command::Search => self.begin(Prompt::Search, window, cx),
            Command::Rename => self.begin(Prompt::Rename, window, cx),
            Command::NewFolder => self.begin(Prompt::NewFolder, window, cx),
            Command::NewFile => self.begin(Prompt::NewFile, window, cx),
            Command::CopyTo => self.begin(Prompt::CopyTo, window, cx),
            Command::MoveTo => self.begin(Prompt::MoveTo, window, cx),
            Command::Delete => self.begin(Prompt::Delete, window, cx),
            Command::Split => {
                if self.panes.len() >= 6 {
                    self.message("Six panes are open. Close one before splitting.", true, cx);
                    return;
                }
                let location = self.panes[self.active].location.clone();
                let id = self.next_id;
                self.next_id += 1;
                let mut pane = Pane::new(id, location);
                pane.view = self.panes[self.active].view.clone();
                self.panes.insert(self.active + 1, pane);
                self.layout_epoch += 1;
                self.active += 1;
                self.reload(self.active, cx);
                self.focus_active(window, cx);
                self.save();
            }
            Command::Close => {
                if self.operation_ui.open {
                    self.operation_ui.open = false;
                    self.focus_active(window, cx);
                    cx.notify();
                    return;
                }
                if let PaneContent::Agent(view) = &self.panes[self.active].content {
                    self.exit_after_agent = None;
                    view.update(cx, |view, cx| view.request_close(false, window, cx));
                    return;
                }
                let terminal = self.panes[self.active].is_terminal();
                if let PaneContent::Terminal(view) = &self.panes[self.active].content {
                    view.update(cx, |view, _| view.close());
                }
                if self.panes.len() == 1 && terminal {
                    let location = self.panes[self.active].location.clone();
                    self.panes[self.active] = Pane::new(self.next_id, location);
                    self.next_id += 1;
                    self.reload(self.active, cx);
                    self.focus_active(window, cx);
                    self.save();
                } else if self.panes.len() > 1 {
                    self.panes.remove(self.active);
                    self.layout_epoch += 1;
                    self.active = self.active.min(self.panes.len() - 1);
                    self.focus_active(window, cx);
                    self.load_preview(cx);
                    self.save();
                } else {
                    self.message(
                        format!(
                            "The last pane stays open. Use {} to quit.",
                            self.binding_hint_id("app.quit")
                        ),
                        false,
                        cx,
                    );
                }
            }
            Command::Next => {
                self.active = (self.active + 1) % self.panes.len();
                self.focus_active(window, cx);
                self.load_preview(cx);
                self.save();
            }
            Command::Refresh => self.reload(self.active, cx),
            Command::Hidden => {
                self.update_preference(settings_view::Change::Hidden(!self.hidden), window, cx)
            }
            Command::Preview => self.update_preference(
                settings_view::Change::Preview(!self.show_preview),
                window,
                cx,
            ),
            Command::Sidebar => {
                self.update_preference(settings_view::Change::Sidebar(!self.sidebar), window, cx)
            }
            Command::Help => {
                self.help = !self.help;
                self.focus.focus(window);
            }
            Command::SelectAll => {
                let p = &mut self.panes[self.active];
                p.selected = (0..p.entries.len()).collect();
            }
            Command::Parent => {
                if let Some(parent) = self.panes[self.active].location.parent() {
                    self.navigate(parent, cx);
                }
            }
            Command::Home => {
                if let Ok(home) = std::env::var("HOME")
                    && let Ok(location) = Location::parse(&home)
                {
                    self.navigate(location, cx);
                }
            }
            Command::Back | Command::Forward => {
                let p = &mut self.panes[self.active];
                let location = if matches!(command, Command::Back) {
                    let l = p.history.pop();
                    if l.is_some() {
                        p.forward.push(p.location.clone());
                    }
                    l
                } else {
                    let l = p.forward.pop();
                    if l.is_some() {
                        p.history.push(p.location.clone());
                    }
                    l
                };
                if let Some(l) = location {
                    p.location = l;
                    p.cursor = 0;
                    p.entries.clear();
                    self.reload(self.active, cx);
                    self.save();
                }
            }
            Command::Copy | Command::Cut => {
                let sources = self.panes[self.active].chosen();
                if !sources.is_empty() {
                    let text = sources
                        .iter()
                        .map(Location::display)
                        .collect::<Vec<_>>()
                        .join("\n");
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    let local_paths = sources
                        .iter()
                        .filter_map(|l| l.local_path().map(|p| p.to_path_buf()))
                        .collect::<Vec<_>>();
                    if local_paths.len() == sources.len()
                        && let Err(e) = clipboard::write_paths(&local_paths)
                    {
                        self.message(format!("File clipboard failed: {e:#}"), true, cx);
                        return;
                    }
                    let n = sources.len();
                    self.clipboard = Some((sources, matches!(command, Command::Cut)));
                    self.message(
                        format!(
                            "{} {n} item(s) · {} to paste",
                            if matches!(command, Command::Cut) {
                                "Cut"
                            } else {
                                "Copied"
                            },
                            self.binding_hint(Command::Paste)
                        ),
                        false,
                        cx,
                    );
                }
            }
            Command::Paste => {
                let paths = match clipboard::read_paths() {
                    Ok(paths) => paths,
                    Err(e) => {
                        self.message(format!("Read file clipboard failed: {e:#}"), true, cx);
                        return;
                    }
                };
                let current = cx
                    .read_from_clipboard()
                    .and_then(|c| c.text())
                    .unwrap_or_default();
                let native_sources = paths.into_iter().map(Location::local).collect::<Vec<_>>();
                let internal = self.clipboard.clone().filter(|(sources, _)| {
                    if native_sources.is_empty() {
                        sources
                            .iter()
                            .map(Location::display)
                            .collect::<Vec<_>>()
                            .join("\n")
                            == current
                    } else {
                        *sources == native_sources
                    }
                });
                let (sources, cut) = internal.unwrap_or_else(|| {
                    (
                        if !native_sources.is_empty() {
                            native_sources
                        } else {
                            current
                                .lines()
                                .filter_map(|line| {
                                    if line.starts_with('/') || line.contains("://") {
                                        Location::parse(line).ok()
                                    } else {
                                        None
                                    }
                                })
                                .collect()
                        },
                        false,
                    )
                });
                let dst = self.panes[self.active].location.clone();
                self.transfer(sources, dst, cut, cx);
            }
            Command::Duplicate => {
                let actions = self.panes[self.active]
                    .chosen()
                    .into_iter()
                    .map(|source| buckets_core::OperationAction::Duplicate { source })
                    .collect();
                self.execute_plan(
                    buckets_core::OperationPlan {
                        label: "Duplicate".into(),
                        actions,
                    },
                    cx,
                );
            }
            Command::QuickLook => {
                self.quicklook = !self.quicklook;
                self.load_preview(cx);
            }
            Command::Desktop | Command::Downloads | Command::Workspace | Command::Root => {
                let path = match command {
                    Command::Desktop => {
                        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Desktop")
                    }
                    Command::Downloads => {
                        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                            .join("Downloads")
                    }
                    Command::Workspace => {
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
                    }
                    _ => PathBuf::from("/"),
                };
                self.navigate(Location::local(path), cx);
            }
            Command::ReloadPlugins => self.discover_plugins(cx),
            Command::PluginLocation(index) => {
                if let Some(plugin) = self.plugins.plugins().get(index) {
                    let value = format!("plugin://{}/", plugin.id);
                    self.begin(Prompt::Location, window, cx);
                    self.input
                        .update(cx, |input, cx| input.set_value(value, window, cx));
                }
            }
            Command::PluginSearch(index) => {
                let Some(plugin) = self.plugins.plugins().get(index) else {
                    return;
                };
                let value = format!("plugin:{} ", plugin.id);
                self.begin(Prompt::Search, window, cx);
                self.input
                    .update(cx, |input, cx| input.set_value(value, window, cx));
            }
            Command::PluginInvoke(index) => self.invoke_plugin(index, window, cx),
            Command::Sort(_)
            | Command::SortDescending
            | Command::DirectoriesFirst
            | Command::ToggleColumn(_)
            | Command::Layout(_)
            | Command::GrowPane
            | Command::ShrinkPane
            | Command::EqualPanes
            | Command::SaveWorkspace
            | Command::ReloadWorkspaces
            | Command::OpenWorkspace(_)
            | Command::RenameWorkspace(_)
            | Command::RemoveWorkspace(_)
            | Command::Operations => unreachable!("handled by control_command"),
            Command::Plugins => {
                let home = std::env::var("HOME").unwrap_or_default();
                let path = PathBuf::from(home).join(".config/buckets/plugins");
                if let Err(e) = std::fs::create_dir_all(&path) {
                    self.message(format!("Cannot create plugin directory: {e}"), true, cx);
                } else if let Ok(l) = Location::parse(&path.to_string_lossy()) {
                    self.navigate(l, cx);
                }
            }
        }
        cx.notify();
    }
    fn invoke_plugin(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = self.plugin_commands.get(index) else {
            return;
        };
        let registry = self.plugins.clone();
        let plugin = command.plugin.clone();
        let title = command.title.clone();
        let command = command.id.clone();
        let origin = self.panes[self.active].id;
        let plugin_result = plugin.clone();
        let epoch = self.plugin_generation;
        let location = self.panes[self.active].location.display();
        let selection = self.panes[self.active]
            .chosen()
            .iter()
            .map(Location::display)
            .collect::<Vec<_>>();
        self.operations += 1;
        self.message("Running plugin command…", false, cx);
        let task = cx.background_executor().spawn(async move {
            match registry.invoke_command_result(
                &plugin,
                &command,
                serde_json::json!({"location":location,"selected":selection}),
            )? {
                CommandResult::Terminal(request) => {
                    let cwd = request.cwd.clone();
                    let session = TerminalSession::spawn(TerminalSpec {
                        program: request.program,
                        args: request.args,
                        cwd: request.cwd,
                        title: request.title,
                        env: request.env,
                    })?;
                    Ok::<_, anyhow::Error>(PluginOutcome::Terminal(session, cwd))
                }
                CommandResult::Agent(intent) => Ok(PluginOutcome::Agent(intent)),
                CommandResult::Value(_) => Ok(PluginOutcome::Value),
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.operations = this.operations.saturating_sub(1);
                    if this.plugin_generation != epoch {
                        this.message(
                            "Plugin settings changed while the command was running. Try again.",
                            true,
                            cx,
                        );
                        return;
                    }
                    match result {
                        Ok(PluginOutcome::Agent(intent)) => {
                            this.open_agent(plugin_result, title, intent, origin, window, cx)
                        }
                        Ok(PluginOutcome::Terminal(session, cwd)) => {
                            let Some(origin_index) = this.panes.iter().position(|p| p.id == origin)
                            else {
                                return;
                            };
                            if this.panes.len() >= 6 {
                                this.message(
                                    "Six panes are open. Close one before opening a terminal.",
                                    true,
                                    cx,
                                );
                                return;
                            }
                            let active_id = this.panes[this.active].id;
                            let focus =
                                active_id == origin && this.prompt.is_none() && !this.settings_open;
                            let view = cx.new(|cx| TerminalView::new(session, window, cx));
                            let appearance = theme::terminal(cx);
                            view.update(cx, |view, cx| view.set_appearance(appearance, cx));
                            let mut pane = Pane::new(this.next_id, Location::local(cwd));
                            this.next_id += 1;
                            pane.loading = false;
                            pane.terminal_observation =
                                Some(cx.observe(&view, |_, _, cx| cx.notify()));
                            pane.content = PaneContent::Terminal(view);
                            this.panes.insert(origin_index + 1, pane);
                            this.active = if focus {
                                origin_index + 1
                            } else {
                                this.panes
                                    .iter()
                                    .position(|p| p.id == active_id)
                                    .unwrap_or(0)
                            };
                            this.quicklook = false;
                            this.help = false;
                            this.load_preview(cx);
                            if focus {
                                this.focus_active(window, cx);
                            }
                            this.save();
                            this.message(
                                format!(
                                    "Terminal opened · {} switches panes · {} closes",
                                    this.binding_hint(Command::Next),
                                    this.binding_hint(Command::Close)
                                ),
                                false,
                                cx,
                            );
                        }
                        Ok(PluginOutcome::Value) => {
                            for pane in &mut this.panes {
                                if pane.is_files() {
                                    pane.refresh_pending = true;
                                }
                            }
                            this.message("Plugin command complete", false, cx);
                        }
                        Err(error) => {
                            this.message(format!("Plugin command failed: {error:#}"), true, cx)
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
    fn palette_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value().to_lowercase();
        let count = self
            .palette_items()
            .iter()
            .filter(|(_, label, _)| label.to_lowercase().contains(value.as_str()))
            .count()
            .min(10);
        if count > 0 {
            self.palette_cursor =
                (self.palette_cursor as isize + delta).rem_euclid(count as isize) as usize;
            self.palette_scroll.scroll_to_item(self.palette_cursor);
        }
        cx.notify();
    }
    /// Return true when an unsaved draft deferred application/window exit.
    fn guard_settings_exit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.recording_shortcut(cx)
            && let Some(view) = self.settings_view.clone()
        {
            view.update(cx, |view, cx| view.cancel_recording(window, cx));
        }
        if std::env::var_os("BUCKETS_TRACE_EXIT").is_some() {
            eprintln!(
                "exit guard: settings_open={}, dirty={}",
                self.settings_open,
                self.settings_view
                    .as_ref()
                    .is_some_and(|view| view.read(cx).has_unsaved_json())
            );
        }

        let Some(view) = self
            .settings_view
            .clone()
            .filter(|view| view.read(cx).has_unsaved_json())
        else {
            if let Some(index)=self.panes.iter().position(|pane|matches!(&pane.content,PaneContent::Agent(view) if view.read(cx).has_unsaved_memory())) {
                self.active=index;self.settings_open=false;self.exit_after_agent=Some(self.panes[index].id);
                if let PaneContent::Agent(view)=&self.panes[index].content{view.update(cx,|view,cx|view.request_close(true,window,cx));}
                cx.notify();return true;
            }
            return false;
        };
        self.exit_after_settings = true;
        self.settings_open = true;
        view.update(cx, |view, cx| view.request_exit(window, cx));
        cx.notify();
        true
    }
    fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.operation_ui.running() {
            self.operation_ui.open = true;
            self.focus.focus(window);
            self.message("A file operation is running. Stop it in Operations, wait for its outcome, then quit again.",true,cx);
            return;
        }
        if self.recording_shortcut(cx) {
            return;
        }
        if !self.guard_settings_exit(window, cx) {
            self.save();
            cx.quit();
        }
    }
    fn menu_command(&mut self, action: &menus::Run, window: &mut Window, cx: &mut Context<Self>) {
        use menus::MenuCommand as M;
        if self.recording_shortcut(cx) {
            return;
        }
        if self.settings_open
            && !matches!(
                action.command,
                M::About
                    | M::Minimize
                    | M::ZoomWindow
                    | M::Close
                    | M::Help
                    | M::Hidden
                    | M::Sidebar
                    | M::Preview
                    | M::FontIncrease
                    | M::FontDecrease
                    | M::FontReset
                    | M::ReloadPlugins
            )
        {
            return;
        }

        if (self.prompt.is_some() || self.help || self.quicklook) && action.command == M::Split {
            return;
        }
        match action.command {
            M::About => {
                let detail = format!(
                    "Buckets {}\nJetBrains Mono is bundled under the OFL.",
                    env!("CARGO_PKG_VERSION")
                );
                drop(window.prompt(
                    PromptLevel::Info,
                    "About Buckets",
                    Some(&detail),
                    &["OK"],
                    cx,
                ));
                return;
            }
            M::Minimize => {
                window.minimize_window();
                return;
            }
            M::ZoomWindow => {
                window.zoom_window();
                return;
            }
            M::Close if self.operation_ui.open => {
                self.operation_ui.open = false;
                self.focus_active(window, cx);
                cx.notify();
                return;
            }
            M::Close if self.settings_open => {
                self.exit_after_settings = false;
                if let Some(view) = &self.settings_view {
                    view.update(cx, |view, cx| view.request_close(window, cx));
                }
                return;
            }
            M::Help if self.settings_open => {
                let detail = COMMANDS
                    .iter()
                    .map(|(command, title, _)| format!("{}  {title}", self.binding_hint(*command)))
                    .collect::<Vec<_>>()
                    .join("\n");
                drop(window.prompt(
                    PromptLevel::Info,
                    "Keyboard Reference",
                    Some(&detail),
                    &["OK"],
                    cx,
                ));
                return;
            }
            M::Close if self.prompt.is_some() || self.help || self.quicklook => {
                self.dismiss(window, cx);
                return;
            }
            M::Open => {
                if self.prompt.is_none()
                    && !self.settings_open
                    && self.panes[self.active].is_files()
                {
                    self.open(window, cx);
                }
                return;
            }
            M::Palette => {
                self.operation_ui.open = false;
                self.begin(Prompt::Palette, window, cx);
                return;
            }
            _ => {}
        }
        let command = match action.command {
            M::Operations => Command::Operations,
            M::SaveWorkspace => Command::SaveWorkspace,
            M::ReloadWorkspaces => Command::ReloadWorkspaces,
            M::GrowPane => Command::GrowPane,
            M::ShrinkPane => Command::ShrinkPane,
            M::EqualPanes => Command::EqualPanes,
            M::SortName => Command::Sort(named_workspaces::SortKey::Name),
            M::SortSize => Command::Sort(named_workspaces::SortKey::Size),
            M::SortModified => Command::Sort(named_workspaces::SortKey::Modified),
            M::SortType => Command::Sort(named_workspaces::SortKey::Type),
            M::SortDescending => Command::SortDescending,
            M::DirectoriesFirst => Command::DirectoriesFirst,
            M::ColumnSize => Command::ToggleColumn(named_workspaces::Column::Size),
            M::ColumnModified => Command::ToggleColumn(named_workspaces::Column::Modified),
            M::ColumnType => Command::ToggleColumn(named_workspaces::Column::Type),
            M::LayoutHorizontal => Command::Layout(named_workspaces::PaneLayout::Horizontal),
            M::LayoutVertical => Command::Layout(named_workspaces::PaneLayout::Vertical),

            M::BookmarkCurrent => Command::BookmarkCurrent,
            M::OpenBookmarks => Command::OpenBookmarks,
            M::NewFile => Command::NewFile,
            M::NewFolder => Command::NewFolder,
            M::Rename => Command::Rename,
            M::CopyTo => Command::CopyTo,
            M::MoveTo => Command::MoveTo,
            M::Duplicate => Command::Duplicate,
            M::Delete => Command::Delete,
            M::Refresh => Command::Refresh,
            M::Search => Command::Search,
            M::QuickLook => Command::QuickLook,
            M::Hidden => Command::Hidden,
            M::Sidebar => Command::Sidebar,
            M::Preview => Command::Preview,
            M::FontIncrease => Command::FontIncrease,
            M::FontDecrease => Command::FontDecrease,
            M::FontReset => Command::FontReset,
            M::Location => Command::Location,
            M::Back => Command::Back,
            M::Forward => Command::Forward,
            M::Parent => Command::Parent,
            M::Home => Command::Home,
            M::Desktop => Command::Desktop,
            M::Downloads => Command::Downloads,
            M::Workspace => Command::Workspace,
            M::Root => Command::Root,
            M::Plugins => Command::Plugins,
            M::ReloadPlugins => Command::ReloadPlugins,
            M::Split => Command::Split,
            M::Close => Command::Close,
            M::Help => Command::Help,
            _ => return,
        };
        let global = matches!(
            command,
            Command::Operations
                | Command::SaveWorkspace
                | Command::ReloadWorkspaces
                | Command::GrowPane
                | Command::ShrinkPane
                | Command::EqualPanes
                | Command::Layout(_)
                | Command::FontIncrease
                | Command::FontDecrease
                | Command::FontReset
                | Command::Sidebar
                | Command::Preview
                | Command::ReloadPlugins
                | Command::Close
                | Command::Split
                | Command::Help
        );
        if (self.settings_open || self.prompt.is_some() || !self.panes[self.active].is_files())
            && !global
        {
            return;
        }
        self.command(command, window, cx);
    }
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.operation_ui.open {
            self.operations_key(event, window, cx);
            return;
        }
        let k = &event.keystroke.key;
        let m = event.keystroke.modifiers;
        if self.settings_open
            || !self.panes[self.active].is_files() && self.prompt.is_none() && !self.help
        {
            return;
        }
        if k == "escape" {
            if self.prompt.is_some() || self.help || self.quicklook {
                self.dismiss(window, cx);
            } else if self.panes[self.active].search.is_some() {
                self.reload(self.active, cx);
            } else {
                self.panes[self.active].selected.clear();
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        if self.help
            && !m.platform
            && !m.control
            && !m.alt
            && matches!(
                k.as_str(),
                "up" | "down" | "pageup" | "pagedown" | "home" | "end"
            )
        {
            let mut offset = self.help_scroll.offset();
            offset.y = match k.as_str() {
                "home" => px(0.),
                "end" => px(-100_000.),
                "up" => offset.y + px(40.),
                "down" => offset.y - px(40.),
                "pageup" => offset.y + window.viewport_size().height * 0.7,
                _ => offset.y - window.viewport_size().height * 0.7,
            };
            self.help_scroll.set_offset(offset);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if self.quicklook {
            if m.alt && !m.platform && !m.control && !m.shift && matches!(k.as_str(), "up" | "down")
            {
                let mut offset = self.preview_scroll.offset();
                offset.y += px(if k == "up" { 100. } else { -100. });
                self.preview_scroll.set_offset(offset);
                cx.notify();
                cx.stop_propagation();
                return;
            }
            if keybindings::normalize(&event.keystroke.unparse()).is_ok_and(|key| {
                keybindings::resolved(&self.current_settings().keybindings, "file.quick_look")
                    .contains(&key)
            }) {
                self.dismiss(window, cx);
                cx.stop_propagation();
                return;
            }
            if m.platform || m.control || m.alt {
                return;
            }
            match k.as_str() {
                "left" | "up" | "k" => self.move_cursor(-1, false, cx),
                "right" | "down" | "j" => self.move_cursor(1, false, cx),
                _ => {
                    return;
                }
            }
            cx.stop_propagation();
            return;
        }
        if self.prompt.is_some() {
            if self.prompt == Some(Prompt::Search)
                && (!m.platform
                    && (matches!(k.as_str(), "up" | "down")
                        || m.control && matches!(k.as_str(), "n" | "p")))
            {
                self.move_cursor(
                    if matches!(k.as_str(), "up" | "p") {
                        -1
                    } else {
                        1
                    },
                    false,
                    cx,
                );
                cx.stop_propagation();
                return;
            }
            if self.prompt == Some(Prompt::Palette) && (k == "up" || k == "down") {
                let value = self.input.read(cx).value().to_lowercase();
                let count = self
                    .palette_items()
                    .iter()
                    .filter(|(_, label, _)| label.to_lowercase().contains(value.as_str()))
                    .count()
                    .min(10);
                if count > 0 {
                    self.palette_cursor = if k == "down" {
                        (self.palette_cursor + 1) % count
                    } else {
                        (self.palette_cursor + count - 1) % count
                    };
                    self.palette_scroll.scroll_to_item(self.palette_cursor);
                }
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        if m.shift && !m.platform && !m.control && !m.alt && (k == "g" || k == "G") {
            self.move_cursor(isize::MAX, false, cx);
            cx.stop_propagation();
            return;
        }
        // Unhandled modified keys belong to the focused input or plugin keymap.
        if m.platform || m.control {
            return;
        }
        if m.alt
            && !m.shift
            && matches!(
                k.as_str(),
                "up" | "down" | "j" | "k" | "pageup" | "pagedown"
            )
        {
            let mut offset = self.preview_scroll.offset();
            offset.y += px(if matches!(k.as_str(), "up" | "k" | "pageup") {
                100.
            } else {
                -100.
            });
            self.preview_scroll.set_offset(offset);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if self.help {
            return;
        }
        if m.alt {
            return;
        }
        match k.as_str() {
            "j" | "down" => self.move_cursor(1, m.shift, cx),
            "k" | "up" => self.move_cursor(-1, m.shift, cx),
            "pagedown" => self.move_cursor(20, m.shift, cx),
            "pageup" => self.move_cursor(-20, m.shift, cx),
            "home" | "g" => {
                self.move_cursor(-(self.panes[self.active].cursor as isize), m.shift, cx)
            }
            "end" | "G" => self.move_cursor(isize::MAX, m.shift, cx),
            "l" | "right" => self.open(window, cx),
            "h" | "left" | "backspace" => self.command(Command::Parent, window, cx),
            "tab" => {
                if m.shift {
                    self.active = (self.active + self.panes.len() - 1) % self.panes.len();
                    self.load_preview(cx);
                    cx.notify();
                } else {
                    self.command(Command::Next, window, cx);
                }
            }
            "x" => {
                let p = &mut self.panes[self.active];
                if !p.entries.is_empty() {
                    if !p.selected.remove(&p.cursor) {
                        p.selected.insert(p.cursor);
                    }
                    self.move_cursor(1, false, cx);
                }
            }
            _ => {
                return;
            }
        }
        cx.stop_propagation();
    }
    fn discover_plugins(&mut self, cx: &mut Context<Self>) {
        self.account_sessions.clear();
        self.sync_account_views(cx);
        self.clear_private_agents(
            "Plugin catalog is reloading. Run a new request after it completes.",
            cx,
        );
        let path = std::env::var_os("BUCKETS_PLUGIN_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| session_path().parent().unwrap().join("plugins"));
        self.plugin_generation += 1;
        let generation = self.plugin_generation;
        let overrides = self.current_settings().plugins;
        let install_bundled = std::env::var_os("BUCKETS_PLUGIN_DIR").is_none();
        let task = cx.background_executor().spawn(async move {
            let notices = if install_bundled {
                bootstrap::ensure_bundled_plugins(&path)?
            } else {
                vec![]
            };
            let registry = PluginRegistry::discover_with_settings(path, &overrides)?;
            let mut commands = vec![];
            for p in registry.plugins() {
                if registry.is_enabled(&p.id) && p.supports(Capability::Commands) {
                    for c in registry.commands(&p.id)? {
                        commands.push(PluginCommand {
                            plugin: p.id.clone(),
                            id: c.id,
                            title: format!("{}: {}", p.name, c.title),
                            shortcut: c.shortcut,
                        });
                    }
                }
            }
            Ok::<_, anyhow::Error>((registry, commands, notices))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.plugin_generation != generation {
                    return;
                }
                match result {
                    Ok((registry, commands, notices)) => {
                        if let Some(store) = &mut this.settings_store {
                            store.set_plugin_catalog(&registry);
                        }
                        this.decoration_epoch += 1;
                        this.plugins = Arc::new(registry);
                        // Metadata refresh does not touch an unsaved JSON draft.
                        if let Some(view) = &this.settings_view {
                            let settings = this.current_settings();
                            let plugins = this.plugins.plugins().to_vec();
                            view.update(cx, |view, cx| view.catalog(settings, plugins, cx));
                        }
                        this.plugin_commands = commands;
                        this.refresh_account_states(cx);
                        this.rebuild_keymap(cx);
                        if let Some(view) = &this.settings_view {
                            let shortcuts = this.shortcut_catalog();
                            view.update(cx, |view, cx| view.shortcuts(shortcuts, cx));
                        }
                        if !notices.is_empty() {
                            this.message(notices.join(" · "), true, cx);
                        }
                        for i in 0..this.panes.len() {
                            this.load_decorations(i, cx);
                        }
                    }
                    Err(e) => this.message(format!("Plugin discovery failed: {e:#}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn load_decorations(&mut self, index: usize, cx: &mut Context<Self>) {
        let decoration_epoch = self.decoration_epoch;
        let pane = &mut self.panes[index];
        pane.decorations.clear();
        let generation = pane.generation;
        let id = pane.id;
        let request = DecorationRequest {
            location: pane.location.display(),
            entries: pane.entries.iter().map(|e| e.location.display()).collect(),
        };
        let registry = self.plugins.clone();
        let plugins = registry
            .plugins()
            .iter()
            .filter(|p| registry.is_enabled(&p.id) && p.supports(Capability::Decorations))
            .map(|p| p.id.clone())
            .collect::<Vec<_>>();
        if plugins.is_empty() || request.entries.is_empty() {
            return;
        }
        let task = cx.background_executor().spawn(async move {
            let mut decorations = std::collections::HashMap::new();
            let mut errors = vec![];
            for plugin in plugins {
                match registry.decorate(&plugin, request.clone()) {
                    Ok(items) => {
                        for item in items {
                            decorations.insert(item.path.clone(), item);
                        }
                    }
                    Err(error) => errors.push(format!("{plugin}: {error:#}")),
                }
            }
            (decorations, errors)
        });
        cx.spawn(async move |this, cx| {
            let (decorations, errors) = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.decoration_epoch != decoration_epoch {
                    return;
                }
                let Some(pane) = this
                    .panes
                    .iter_mut()
                    .find(|p| p.id == id && p.generation == generation)
                else {
                    return;
                };
                pane.decorations = decorations;
                if !errors.is_empty() {
                    this.message(
                        format!("File badges unavailable · {}", errors.join("; ")),
                        true,
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn binding_hint(&self, command: Command) -> String {
        command_id(command)
            .map(|id| self.binding_hint_id(id))
            .unwrap_or_else(|| "Unbound".into())
    }
    fn binding_hint_id(&self, id: &str) -> String {
        binding_label(&self.current_settings().keybindings, id)
    }
    fn shortcut_catalog(&self) -> Vec<(String, String, Vec<String>)> {
        self.plugin_commands
            .iter()
            .map(|c| {
                (
                    keybindings::plugin_id(&c.plugin, &c.id),
                    c.title.clone(),
                    c.shortcut.iter().cloned().collect(),
                )
            })
            .collect()
    }
    fn rebuild_keymap(&mut self, cx: &mut App) {
        let overrides = self.current_settings().keybindings;
        let mut bindings = vec![
            KeyBinding::new("tab", NextPane, Some("Buckets")).with_meta(APP_KEYMAP),
            KeyBinding::new("shift-tab", PreviousPane, Some("Buckets")).with_meta(APP_KEYMAP),
        ];
        let mut claimed = std::collections::HashSet::new();
        for spec in keybindings::ACTIONS {
            let context = match spec.scope {
                keybindings::Scope::Files => Some("Buckets"),
                keybindings::Scope::Workspace => Some("Buckets || BucketsTerminal || BucketsAgent"),
                keybindings::Scope::Global => None,
            };
            for key in keybindings::resolved(&overrides, spec.id) {
                claimed.insert(key.clone());
                for runtime_key in runtime_binding_keys(&key) {
                    if let Some(binding) = menus::binding(spec.id, &runtime_key, context) {
                        bindings.push(binding.with_meta(APP_KEYMAP));
                    }
                }
            }
        }
        let mut conflicts = vec![];
        self.plugin_bindings.clear();
        for command in &self.plugin_commands {
            let id = keybindings::plugin_id(&command.plugin, &command.id);
            let defaults = command.shortcut.iter().cloned().collect::<Vec<_>>();
            for raw in keybindings::resolved_plugin(&overrides, &id, &defaults) {
                match keybindings::validate_plugin_shortcut(&raw) {
                    Ok(key) if claimed.insert(key.clone()) => {
                        self.plugin_bindings
                            .entry(id.clone())
                            .or_default()
                            .push(key.clone());
                        for runtime_key in runtime_binding_keys(&key) {
                            bindings.push(
                                KeyBinding::new(
                                    &runtime_key,
                                    PluginShortcut {
                                        plugin: command.plugin.clone(),
                                        command: command.id.clone(),
                                    },
                                    Some("Buckets"),
                                )
                                .with_meta(APP_KEYMAP),
                            );
                        }
                    }
                    _ => conflicts.push(format!("{id}: {raw}")),
                }
            }
        }
        let merged = merged_key_bindings(cx.key_bindings().borrow().bindings(), bindings);
        cx.clear_key_bindings();
        cx.bind_keys(merged);
        self.menu_state = None;
        self.keymap_conflicts = conflicts.clone();
        if !conflicts.is_empty() {
            self.status = format!(
                "Shortcut conflicts skipped (core commands take priority): {}",
                conflicts.join(" · ")
            );
            self.status_error = true;
            if let Some(view) = &self.settings_view {
                let warning = self.status.clone();
                view.update(cx, |view, cx| view.warning(warning, cx));
            }
        }
    }
    fn palette_items(&self) -> Vec<(Command, String, String)> {
        let mut items = COMMANDS
            .iter()
            .map(|(command, label, _)| (*command, label.to_string(), self.binding_hint(*command)))
            .collect::<Vec<_>>();
        if let Some(store) = &self.named_workspaces {
            for (i, workspace) in store.entries().iter().enumerate() {
                items.push((
                    Command::OpenWorkspace(i),
                    format!("Open workspace: {}", workspace.name),
                    String::new(),
                ));
                items.push((
                    Command::RenameWorkspace(i),
                    format!("Rename workspace: {}", workspace.name),
                    String::new(),
                ));
                items.push((
                    Command::RemoveWorkspace(i),
                    format!("Remove workspace: {}", workspace.name),
                    String::new(),
                ));
            }
        }
        for (i, bookmark) in self.current_settings().bookmarks.iter().enumerate() {
            items.push((
                Command::GoBookmark(i),
                format!("Open bookmark: {} — {}", bookmark.name, bookmark.location),
                String::new(),
            ));
            items.push((
                Command::RenameBookmark(i),
                format!("Rename bookmark: {} — {}", bookmark.name, bookmark.location),
                String::new(),
            ));
            items.push((
                Command::RemoveBookmark(i),
                format!("Remove bookmark: {} — {}", bookmark.name, bookmark.location),
                String::new(),
            ));
        }
        for (i, p) in self.plugins.plugins().iter().enumerate() {
            if self.plugins.is_enabled(&p.id) && p.supports(Capability::Provider) {
                items.push((
                    Command::PluginLocation(i),
                    format!("Browse {}", p.name),
                    String::new(),
                ));
            }
            if self.plugins.is_enabled(&p.id) && p.supports(Capability::Search) {
                items.push((
                    Command::PluginSearch(i),
                    format!("Search with {}", p.name),
                    String::new(),
                ));
            }
        }
        for (i, c) in self.plugin_commands.iter().enumerate() {
            items.push((Command::PluginInvoke(i), c.title.clone(), {
                let id = keybindings::plugin_id(&c.plugin, &c.id);
                let keys = self.plugin_bindings.get(&id).cloned().unwrap_or_default();
                display_bindings(&keys)
            }));
        }
        items
    }
    fn plugin_search(&mut self, text: String, cx: &mut Context<Self>) {
        let Some((id, query)) = text.trim_start_matches("plugin:").split_once(' ') else {
            self.message(
                format!(
                    "Use plugin:plugin-id query, or choose a search plugin in {}",
                    self.binding_hint_id("app.palette")
                ),
                true,
                cx,
            );
            return;
        };
        let id = id.to_string();
        let (mode, query) = if let Some(q) = query.strip_prefix("mode:content ") {
            (SearchMode::Content, q)
        } else if let Some(q) = query.strip_prefix("mode:semantic ") {
            (SearchMode::Semantic, q)
        } else if let Some(q) = query.strip_prefix("mode:hybrid ") {
            (SearchMode::Hybrid, q)
        } else {
            (SearchMode::Filename, query)
        };
        let query = query.to_string();
        let registry = self.plugins.clone();
        let p = &mut self.panes[self.active];
        p.loading = true;
        p.generation += 1;
        let generation = p.generation;
        let pane_id = p.id;
        p.search = Some(text.clone());
        let root = p.location.display();
        let task = cx.background_executor().spawn(async move {
            registry
                .search(
                    &id,
                    SearchRequest {
                        query,
                        roots: vec![IndexRoot {
                            path: root.clone(),
                            respect_gitignore: true,
                            follow_symlinks: false,
                        }],
                        limit: 10_000,
                        mode,
                        config: serde_json::Value::Null,
                    },
                )
                .map(|hits| {
                    hits.into_iter()
                        .filter_map(|hit| {
                            Location::parse(&hit.path)
                                .ok()
                                .map(|location| search_entry(location, hit.line, &root))
                        })
                        .collect()
                })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(i) = this
                    .panes
                    .iter()
                    .position(|p| p.id == pane_id && p.generation == generation)
                else {
                    return;
                };
                let p = &mut this.panes[i];
                p.loading = false;
                match result {
                    Ok(hits) => {
                        p.entries = hits;
                        p.cursor = 0;
                        p.selected.clear();
                        p.search = Some(text);
                        p.error = None;
                    }
                    Err(e) => p.error = Some(format!("Plugin search failed: {e:#}")),
                }
                this.panes[i].refresh_name_filter();
                this.load_decorations(i, cx);
                if i == this.active {
                    this.load_preview(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn render_pane(&self, index: usize, width: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::palette(cx);
        let p = &self.panes[index];
        let active = index == self.active;
        if let PaneContent::Agent(view) = &p.content {
            let title = view.read(cx).title().to_owned();
            return div()
                .id(("agent-pane", p.id))
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .border_r_1()
                .border_color(rgb(colors.border))
                .capture_any_mouse_down(cx.listener(move |this, _, window, cx| {
                    if this.active != index {
                        this.active = index;
                        this.focus_active(window, cx);
                        this.load_preview(cx);
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .flex_shrink_0()
                        .px_3()
                        .py_2()
                        .bg(rgb(if active {
                            colors.surface
                        } else {
                            colors.chrome
                        }))
                        .border_b_1()
                        .border_color(rgb(if active { colors.accent } else { colors.border }))
                        .child(
                            div()
                                .text_size(ui(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(title),
                        ),
                )
                .child(div().flex_1().min_h_0().child(view.clone()))
                .into_any_element();
        }
        if let PaneContent::Terminal(view) = &p.content {
            let title = view.read(cx).title().to_string();
            let exit_status = view.read(cx).exit_status();
            let location = p.location.display();
            return div()
                .id(("terminal-pane", p.id))
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .border_r_1()
                .border_color(rgb(colors.border))
                .capture_any_mouse_down(cx.listener(move |this, _, window, cx| {
                    if this.active != index && this.prompt == Some(Prompt::Search) {
                        this.dismiss(window, cx);
                    }
                    this.active = index;
                    this.focus_active(window, cx);
                    this.load_preview(cx);
                    this.save();
                    cx.notify();
                }))
                .child(
                    div()
                        .h(ui(48.))
                        .flex_shrink_0()
                        .px_4()
                        .flex()
                        .items_center()
                        .gap_3()
                        .bg(rgb(if active { colors.surface } else { colors.bg }))
                        .border_b_1()
                        .border_color(rgb(if active { colors.accent } else { colors.border }))
                        .child(div().text_color(rgb(colors.directory)).child(">_"))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(key(format!("{}", index + 1), cx)),
                )
                .child(
                    div()
                        .h(ui(32.))
                        .px_4()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .text_size(ui(10.))
                        .text_color(rgb(colors.muted))
                        .truncate()
                        .child(location),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_hidden()
                        .child(view.clone()),
                )
                .when_some(exit_status, |el, status| {
                    el.child(
                        div()
                            .h(ui(32.))
                            .flex_shrink_0()
                            .px_3()
                            .flex()
                            .items_center()
                            .border_t_1()
                            .border_color(rgb(colors.border))
                            .bg(rgb(colors.chrome))
                            .text_size(ui(11.))
                            .text_color(rgb(colors.muted))
                            .child(format!(
                                "Process exited: {status} · {} to close",
                                self.binding_hint(Command::Close)
                            )),
                    )
                })
                .into_any_element();
        }
        let projection = p
            .name_filter
            .as_ref()
            .map(|filter| Arc::clone(&filter.indices));
        let count = p.visible_count();
        let id = p.id;
        let location = p.location.display();
        let title = location
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("/")
            .to_string();
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .h(ui(48.))
            .px_4()
            .border_b_1()
            .border_color(rgb(if active { colors.accent } else { colors.border }))
            .bg(rgb(if active { colors.surface } else { colors.bg }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().size(ui(7.)).rounded_full().bg(rgb(if active {
                        colors.success
                    } else {
                        colors.muted
                    })))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(
                        div()
                            .text_size(ui(11.))
                            .text_color(rgb(colors.muted))
                            .child(if active { "ACTIVE" } else { "" }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .text_size(ui(11.))
                            .text_color(rgb(colors.muted))
                            .child(format!("{}", index + 1)),
                    )
                    .child(key("Tab", cx)),
            );
        let shown_columns = Arc::new(pane_view::visible_columns(
            &p.view,
            f32::from(width) / self.font_size.scale(),
        ));
        let row_columns = shown_columns.clone();
        let columns = div()
            .h(ui(28.))
            .flex()
            .items_center()
            .px_4()
            .text_size(ui(10.))
            .text_color(rgb(colors.muted))
            .border_b_1()
            .border_color(rgb(colors.border))
            .child(
                div()
                    .id(("sort-name", id))
                    .flex_1()
                    .cursor_pointer()
                    .child(if p.search.is_some() {
                        "SEARCH RESULTS".into()
                    } else {
                        self.sort_heading("NAME", named_workspaces::SortKey::Name, index)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.active = index;
                        if this.panes[index].view.sort == named_workspaces::SortKey::Name {
                            this.command(Command::SortDescending, window, cx);
                        } else {
                            this.command(
                                Command::Sort(named_workspaces::SortKey::Name),
                                window,
                                cx,
                            );
                        }
                    })),
            )
            .children(
                shown_columns
                    .iter()
                    .copied()
                    .filter(|column| *column != named_workspaces::Column::Name)
                    .map(|column| {
                        let (title, key, width) = match column {
                            named_workspaces::Column::Size => {
                                ("SIZE", named_workspaces::SortKey::Size, 64.)
                            }
                            named_workspaces::Column::Modified => {
                                ("MODIFIED UTC", named_workspaces::SortKey::Modified, 108.)
                            }
                            _ => ("TYPE", named_workspaces::SortKey::Type, 72.),
                        };
                        div()
                            .id(("sort-column", column as usize))
                            .w(ui(width))
                            .text_right()
                            .cursor_pointer()
                            .child(self.sort_heading(title, key, index))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.active = index;
                                if this.panes[index].view.sort == key {
                                    this.command(Command::SortDescending, window, cx);
                                } else {
                                    this.command(Command::Sort(key), window, cx);
                                }
                            }))
                    }),
            );
        let content = if let Some(error) = &p.error {
            div()
                .flex_1()
                .p_6()
                .text_color(rgb(colors.danger))
                .child(error.clone())
                .child(div().mt_4().text_color(rgb(colors.muted)).child(format!(
                    "{} to change location · {} to retry",
                    self.binding_hint(Command::Location),
                    self.binding_hint(Command::Refresh)
                )))
                .into_any_element()
        } else if p.loading && (count == 0 || p.search.is_some()) {
            div()
                .flex_1()
                .p_6()
                .text_color(rgb(colors.muted))
                .child("Reading directory…")
                .into_any_element()
        } else if count == 0 {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(icon("folder", colors.muted))
                .child(if p.name_filter.is_some() {
                    "No matching names"
                } else if p.search.is_some() {
                    "No matching files"
                } else {
                    "This directory is empty"
                })
                .child(
                    div()
                        .text_color(rgb(colors.muted))
                        .text_size(ui(11.))
                        .child(if p.name_filter.is_some() {
                            "Change the filter or Escape to clear".to_owned()
                        } else if p.search.is_some() {
                            format!(
                                "Escape to return · {} to search again",
                                self.binding_hint(Command::Search)
                            )
                        } else {
                            format!(
                                "{} new file · {} new folder",
                                self.binding_hint(Command::NewFile),
                                self.binding_hint(Command::NewFolder)
                            )
                        }),
                )
                .into_any_element()
        } else {
            uniform_list(
                ("files", id as usize),
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                    let Some(index) = this.panes.iter().position(|p| p.id == id) else {
                        return vec![];
                    };
                    range
                        .filter_map(|visible_row| {
                            let row=projection.as_ref().and_then(|indices|indices.get(visible_row)).copied().unwrap_or(visible_row);
                            let p = &this.panes[index];
                            let e = p.entries.get(row)?.clone();
                            let selected = p.selected.contains(&row);
                            let current = p.cursor == row;
                            let active = this.active == index;
                            let entry_location = if e.is_dir {
                                e.location.clone()
                            } else {
                                p.location.clone()
                            };
                            let decoration = p.decorations.get(&e.location.display()).cloned();
                            let target = if e.is_dir {
                                e.location.clone()
                            } else {
                                p.location.clone()
                            };
                            let drag = FileDrag {
                                locations: if selected {
                                    p.chosen()
                                } else {
                                    vec![e.location.clone()]
                                },
                                label: e.name.clone(),
                            };
                            Some(div()
                                .id(("row", row))
                                .w_full()
                                .h(ui(29.))
                                .px_4()
                                .flex()
                                .items_center()
                                .gap_2()
                                .bg(rgb(if current && active {
                                    colors.cursor
                                } else if selected {
                                    colors.marked
                                } else if current {
                                    colors.surface
                                } else {
                                    colors.bg
                                }))
                                .border_l_1()
                                .border_color(rgb(if current && active {
                                    colors.accent
                                } else if selected {
                                    colors.directory
                                } else {
                                    colors.bg
                                }))
                                .hover(|s| s.bg(rgb(colors.surface)))
                                .cursor_pointer()
                                .child(icon(
                                    if e.is_dir { "folder" } else { "file" },
                                    if e.is_dir { colors.directory } else { colors.muted },
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .text_color(rgb(if e.is_dir { colors.directory } else { colors.text }))
                                        .child(e.name.clone()),
                                )
                                .when_some(decoration, |el, decoration| {
                                    let color = match decoration.tone {
                                        DecorationTone::Neutral => colors.muted,
                                        DecorationTone::Accent => colors.accent,
                                        DecorationTone::Success => colors.success,
                                        DecorationTone::Warning => colors.warning,
                                        DecorationTone::Danger => colors.danger,
                                    };
                                    let tooltip = decoration.tooltip.clone();
                                    el.child(
                                        div()
                                            .id(("badge", row))
                                            .w(ui(22.))
                                            .text_center()
                                            .text_size(ui(11.))
                                            .text_color(rgb(color))
                                            .child(decoration.badge)
                                            .tooltip(move |window, cx| {
                                                gpui_component::tooltip::Tooltip::new(
                                                    tooltip.clone(),
                                                )
                                                .build(window, cx)
                                            }),
                                    )
                                })
                                .children(row_columns.iter().copied().filter(|column|*column!=named_workspaces::Column::Name).map(|column|{
                                    let(width,value)=match column{named_workspaces::Column::Size=>(56.,if e.is_dir{String::new()}else{human_size(e.size)}),named_workspaces::Column::Modified=>(100.,pane_view::modified(e.modified)),_=>(64.,pane_view::extension(&e).to_uppercase())};
                                    div().w(ui(width)).flex_shrink_0().text_right().truncate().text_color(rgb(colors.muted)).text_size(ui(10.)).child(value)
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                        if this.active!=index&&this.prompt==Some(Prompt::Search){this.dismiss(window,cx);}
                                        this.active = index;
                                        let p = &mut this.panes[index];
                                        let old_cursor = p.cursor;
                                        p.cursor = row;
                                        if event.modifiers.platform || event.modifiers.control {
                                            if !p.selected.remove(&row) {
                                                p.selected.insert(row);
                                            }
                                            p.selection_anchor = None;
                                        } else if event.modifiers.shift {
                                            let anchor =
                                                *p.selection_anchor.get_or_insert(old_cursor);
                                            p.select_visible_range(anchor,row);
                                        } else {
                                            p.selection_anchor = None;
                                            if !p.selected.contains(&row) {
                                                p.selected.clear();
                                            }
                                        }
                                        if this.prompt==Some(Prompt::Search){this.input.focus_handle(cx).focus(window);}else{this.focus.focus(window);}
                                        this.load_preview(cx);
                                        cx.notify();
                                    }),
                                )
                                .on_click(cx.listener(
                                    move |this, event: &ClickEvent, window, cx| {
                                        if event.click_count() == 2 {
                                            this.active = index;
                                            this.open(window, cx);
                                        }
                                    },
                                ))
                                .when(p.name_filter.is_none(),|el|el.on_drag(drag,|drag,_position,window,cx|{
                                    let bounds=Bounds::new(point(px(0.),px(0.)),window.viewport_size());
                                    if !bounds.contains(&window.mouse_position()){
                                        let paths=drag.locations.iter().filter_map(|location|location.local_path().map(|path|path.to_path_buf())).collect::<Vec<_>>();
                                        if paths.len()==drag.locations.len()&&!paths.is_empty(){
                                            if std::env::var_os("BUCKETS_TRACE_DRAG").is_some(){eprintln!("Native export on first drag movement outside window");}
                                            match native_drag::start(window,&paths){Ok(true)=>window.defer(cx,|window,cx|{cx.stop_active_drag(window);}),Ok(false)=>{},Err(error)=>eprintln!("Native file drag failed: {error:#}")}
                                        }
                                    }
                                    cx.new(|_|drag.clone())
                                }))
                                .on_drop(cx.listener(move |this, drag: &FileDrag, _, cx| {
                                    if std::env::var_os("BUCKETS_TRACE_DRAG").is_some() {
                                        eprintln!(
                                            "Internal row drop {} items",
                                            drag.locations.len()
                                        );
                                    }
                                    this.transfer(
                                        drag.locations.clone(),
                                        target.clone(),
                                        false,
                                        cx,
                                    );
                                }))
                                .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                                    let sources: Vec<Location> = paths
                                        .paths()
                                        .iter()
                                        .filter_map(|path| {
                                            Location::parse(&path.to_string_lossy()).ok()
                                        })
                                        .collect();
                                    if sources.is_empty() {
                                        return;
                                    }
                                    this.transfer(sources, entry_location.clone(), false, cx);
                                })))
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(p.scroll.clone())
            .w_full()
            .flex_1()
            .into_any_element()
        };
        let target = p.location.clone();
        let external_target = target.clone();
        div()
            .id(("pane", id as usize))
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(colors.border))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    if this.active != index {
                        if this.prompt == Some(Prompt::Search) {
                            this.dismiss(window, cx);
                        }
                        this.active = index;
                        this.load_preview(cx);
                    }
                    if this.prompt == Some(Prompt::Search) {
                        this.input.focus_handle(cx).focus(window);
                    } else {
                        this.focus.focus(window);
                    }
                    cx.notify();
                }),
            )
            .on_drop(cx.listener(move |this, drag: &FileDrag, _, cx| {
                this.transfer(drag.locations.clone(), target.clone(), false, cx)
            }))
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                let sources: Vec<Location> = paths
                    .paths()
                    .iter()
                    .filter_map(|path| Location::parse(&path.to_string_lossy()).ok())
                    .collect();
                if sources.is_empty() {
                    return;
                }
                this.transfer(sources, external_target.clone(), false, cx);
            }))
            .child(header)
            .child(
                div()
                    .id(("path", id as usize))
                    .h(ui(36.))
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(ui(11.))
                    .text_color(rgb(colors.muted))
                    .overflow_hidden()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.active = index;
                        this.begin(Prompt::Location, window, cx);
                    }))
                    .child(icon("chevron-right", colors.accent))
                    .child(div().flex_1().min_w_0().truncate().child(location)),
            )
            .when_some(p.search.clone(), |el, query| {
                el.child(
                    div()
                        .px_4()
                        .py_2()
                        .bg(rgb(colors.surface))
                        .text_color(rgb(colors.directory))
                        .text_size(ui(11.))
                        .child(format!("Search: {query} · Esc to clear")),
                )
            })
            .child(columns)
            .child(content)
            .child(
                div()
                    .h(ui(30.))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .text_size(ui(11.))
                    .text_color(rgb(colors.muted))
                    .child(format!(
                        "{count} items{} ",
                        if p.selected.is_empty() {
                            String::new()
                        } else {
                            format!(" · {} marked", p.selected.len())
                        }
                    ))
                    .child(if count == 0 {
                        "0 / 0".into()
                    } else {
                        format!("{} / {count}", p.visible_cursor() + 1)
                    }),
            )
            .into_any_element()
    }
    fn render_overlay(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::palette(cx);
        if self.quicklook {
            let entry = self.panes[self.active]
                .entries
                .get(self.panes[self.active].cursor);
            let image = entry
                .and_then(|entry| entry.location.local_path())
                .filter(|path| {
                    path.extension()
                        .and_then(|s| s.to_str())
                        .is_some_and(|ext| {
                            matches!(
                                ext.to_ascii_lowercase().as_str(),
                                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff"
                            )
                        })
                });
            let content = if let Some(path) = image {
                div()
                    .flex_1()
                    .min_h_0()
                    .p_5()
                    .child(
                        img(path.to_path_buf())
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    )
                    .into_any_element()
            } else {
                div()
                    .id("quicklook-scroll")
                    .track_scroll(&self.preview_scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_6()
                    .text_size(ui(14.))
                    .line_height(ui(23.))
                    .child(self.preview.clone())
                    .into_any_element()
            };
            return div()
                .absolute()
                .inset_0()
                .occlude()
                .p_8()
                .bg(rgba((colors.bg << 8) | 0xe8))
                .child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .bg(rgb(colors.bg))
                        .border_1()
                        .border_color(rgb(colors.border))
                        .rounded_lg()
                        .child(
                            div()
                                .h(ui(56.))
                                .px_5()
                                .flex()
                                .items_center()
                                .justify_between()
                                .border_b_1()
                                .border_color(rgb(colors.border))
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(self.preview_title.clone()),
                                )
                                .child(hint(
                                    "Close Quick Look",
                                    format!("Esc / {}", self.binding_hint(Command::QuickLook)),
                                    cx,
                                )),
                        )
                        .child(content)
                        .child(
                            div()
                                .h(ui(40.))
                                .px_5()
                                .flex()
                                .items_center()
                                .justify_between()
                                .border_t_1()
                                .border_color(rgb(colors.border))
                                .text_color(rgb(colors.muted))
                                .child("← → browse files")
                                .child("⌥↑↓ scroll preview"),
                        ),
                )
                .into_any_element();
        }
        if self.help {
            return div()
                .absolute()
                .inset_0()
                .occlude()
                .bg(rgba((colors.bg << 8) | 0xd9))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .id("keyboard-reference")
                        .track_scroll(&self.help_scroll)
                        .max_h_full()
                        .overflow_y_scroll()
                        .w(ui(780.))
                        .max_w_full()
                        .p_6()
                        .bg(rgb(colors.surface))
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .mb_4()
                                .child(
                                    div()
                                        .text_size(ui(20.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("Keyboard shortcuts"),
                                )
                                .child(key("Esc", cx)),
                        )
                        .child(div().mb_4().text_color(rgb(colors.muted)).child(format!(
                            "{} finds any command. Navigation keys stay fixed.",
                            self.binding_hint_id("app.palette")
                        )))
                        .child(
                            div()
                                .flex()
                                .gap_8()
                                .child(div().flex_1().flex().flex_col().gap_2().children(
                                    COMMANDS.iter().take(14).map(|(command, label, _)| {
                                        div()
                                            .flex()
                                            .justify_between()
                                            .gap_3()
                                            .child(*label)
                                            .child(key(self.binding_hint(*command), cx))
                                    }),
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .children(COMMANDS.iter().skip(14).map(
                                            |(command, label, _)| {
                                                div()
                                                    .flex()
                                                    .justify_between()
                                                    .gap_3()
                                                    .child(*label)
                                                    .child(key(self.binding_hint(*command), cx))
                                            },
                                        ))
                                        .child(div().h(ui(12.)))
                                        .child(hint("Navigate", "↑↓ / j k", cx))
                                        .child(hint("Open", "Enter / l", cx))
                                        .child(hint("Mark and advance", "x", cx))
                                        .child(hint(
                                            "Quick Look",
                                            self.binding_hint(Command::QuickLook),
                                            cx,
                                        ))
                                        .child(hint("Extend selection", "⇧↑↓", cx))
                                        .child(hint("Jump first / last", "Home / End", cx))
                                        .child(hint("Scroll preview", "⌥↑↓", cx)),
                                ),
                        ),
                )
                .into_any_element();
        }
        let Some(mode) = self.prompt else {
            return div().into_any_element();
        };
        let value = self.input.read(cx).value().to_lowercase();
        let scale = self.font_size.scale();
        let viewport_height = f32::from(window.viewport_size().height);
        let top = (110. * scale).min((viewport_height - 480. * scale).max(16.));
        let panel_height = (viewport_height - top - 16.).max(1.);
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgba((colors.bg << 8) | 0xc0))
            .flex()
            .items_start()
            .justify_center()
            .pt(px(top))
            .pb(px(16.))
            .child(
                div()
                    .w(ui(680.))
                    .max_w_full()
                    .max_h(px(panel_height))
                    .h_auto()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_5()
                    .bg(rgb(colors.surface))
                    .border_1()
                    .border_color(rgb(if mode == Prompt::Delete {
                        colors.danger
                    } else {
                        colors.accent
                    }))
                    .rounded_lg()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_size(ui(18.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(mode.title()),
                            )
                            .child(key("Esc", cx)),
                    )
                    .child(
                        div()
                            .text_size(ui(12.))
                            .text_color(rgb(colors.muted))
                            .child(mode.detail()),
                    )
                    .child(Input::new(&self.input))
                    .when(mode == Prompt::Palette, |el| {
                        el.child(
                            div()
                                .id("palette-results")
                                .track_scroll(&self.palette_scroll)
                                .min_h_0()
                                .max_h(px((panel_height - 220. * scale).max(40.)))
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .children(
                                    self.palette_items()
                                        .into_iter()
                                        .filter(|(_, label, _)| {
                                            label.to_lowercase().contains(value.as_str())
                                        })
                                        .take(10)
                                        .enumerate()
                                        .map(|(i, (command, label, shortcut))| {
                                            div()
                                                .id(("command", i))
                                                .px_3()
                                                .py_2()
                                                .rounded_md()
                                                .bg(rgb(if i == self.palette_cursor {
                                                    colors.border
                                                } else {
                                                    colors.surface
                                                }))
                                                .flex()
                                                .justify_between()
                                                .cursor_pointer()
                                                .hover(|s| s.bg(rgb(colors.border)))
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.dismiss(window, cx);
                                                        this.command(command, window, cx);
                                                    },
                                                ))
                                                .child(label)
                                                .child(key(shortcut, cx))
                                        }),
                                ),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_size(ui(11.))
                            .text_color(rgb(colors.muted))
                            .child(if mode == Prompt::Palette {
                                "↑↓ choose · Type to filter"
                            } else {
                                "Changes apply when you press Enter"
                            })
                            .child(hint("Confirm", "Enter", cx)),
                    ),
            )
            .into_any_element()
    }
}
impl Render for Browser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::palette(cx);
        let files = !self.operation_ui.open
            && !self.settings_open
            && self.prompt.is_none()
            && !self.help
            && !self.quicklook
            && self.panes[self.active].is_files();
        let state = menus::MenuState {
            file_actions: files,
            has_selection: !self.panes[self.active].chosen().is_empty(),
            writable: !matches!(self.panes[self.active].location, Location::Git { .. }),
            workspace_actions: !self.operation_ui.open
                && !self.settings_open
                && self.prompt.is_none()
                && !self.help
                && !self.quicklook,
        };
        if self.menu_state != Some(state) {
            menus::set(cx, state);
            self.menu_state = Some(state);
        }
        if self.settings_open
            && let Some(view) = &self.settings_view
        {
            return div()
                .size_full()
                .track_focus(&self.focus)
                .key_context("BucketsSettings")
                .capture_key_down(cx.listener(Self::on_key))
                .on_action(cx.listener(|this, _: &OpenSettings, w, cx| this.open_settings(w, cx)))
                .on_action(cx.listener(Self::menu_command))
                .child(view.clone())
                .into_any_element();
        }
        let compact = window.viewport_size().width < px(1200. * self.font_size.scale());
        let scale = self.font_size.scale();
        let sidebar_width = if self.sidebar && !compact {
            184. * scale
        } else {
            0.
        };
        let preview_width = if compact { 260. } else { 300. } * scale;
        let preview_fits =
            (f32::from(window.viewport_size().width) - sidebar_width - preview_width)
                / self.panes.len() as f32
                >= 240. * scale;
        let show_preview = self.show_preview && self.panes[self.active].is_files() && preview_fits;
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let places = vec![
            ("Home", home.clone()),
            ("Desktop", format!("{home}/Desktop")),
            ("Downloads", format!("{home}/Downloads")),
            ("Workspace", cwd.to_string_lossy().into_owned()),
            ("Root", "/".into()),
        ];
        let bookmarks = self.current_settings().bookmarks;
        let sidebar = div()
            .id("places-sidebar")
            .overflow_y_scroll()
            .w(ui(184.))
            .flex_shrink_0()
            .bg(rgb(colors.chrome))
            .border_r_1()
            .border_color(rgb(colors.border))
            .flex()
            .flex_col()
            .py_4()
            .child(
                div()
                    .px_4()
                    .mb_3()
                    .text_color(rgb(colors.muted))
                    .text_size(ui(11.))
                    .child("PLACES"),
            )
            .children(places.into_iter().enumerate().map(|(i, (label, path))| {
                div()
                    .id(("place", i))
                    .px_4()
                    .h(ui(34.))
                    .flex()
                    .items_center()
                    .gap_3()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(colors.surface)))
                    .child(icon("folder", colors.muted))
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.prompt == Some(Prompt::Search) {
                            this.dismiss(window, cx);
                        }
                        if let Ok(location) = Location::parse(&path) {
                            this.navigate(location, cx);
                        }
                    }))
            }))
            .when(!bookmarks.is_empty(), |el| {
                el.child(
                    div()
                        .px_4()
                        .pt_5()
                        .pb_2()
                        .text_color(rgb(colors.muted))
                        .text_size(ui(11.))
                        .child("BOOKMARKS"),
                )
            })
            .children(bookmarks.into_iter().enumerate().map(|(i, bookmark)| {
                div()
                    .id(("bookmark", i))
                    .px_4()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(colors.surface)))
                    .child(icon("folder", colors.directory))
                    .child(div().flex_1().min_w_0().truncate().child(bookmark.name))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.command(Command::GoBookmark(i), w, cx)
                    }))
            }))
            .child(div().h(ui(28.)))
            .child(
                div()
                    .px_4()
                    .mb_3()
                    .text_color(rgb(colors.muted))
                    .text_size(ui(11.))
                    .child("CONNECTIONS"),
            )
            .child(
                div()
                    .id("connect")
                    .mx_2()
                    .px_2()
                    .py_2()
                    .rounded_md()
                    .flex()
                    .justify_between()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(colors.surface)))
                    .child("Connect location")
                    .child(key(self.binding_hint(Command::Location), cx))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.begin(Prompt::Location, window, cx)),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .mt_2()
                    .text_size(ui(10.))
                    .text_color(rgb(colors.muted))
                    .child("SSH · S3 · Git"),
            );
        let panes = self.render_resizable_panes(
            window,
            px((f32::from(window.viewport_size().width)
                - sidebar_width
                - if show_preview { preview_width } else { 0. })
            .max(1.)),
            cx,
        );
        let preview = div()
            .w(ui(if compact { 260. } else { 300. }))
            .flex_shrink_0()
            .bg(rgb(colors.chrome))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(ui(48.))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .text_size(ui(11.))
                            .text_color(rgb(colors.muted))
                            .child("PREVIEW"),
                    )
                    .child(key(self.binding_hint(Command::Preview), cx)),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .child(self.preview_title.clone()),
            )
            .child(
                div()
                    .id("preview-scroll")
                    .track_scroll(&self.preview_scroll)
                    .flex_1()
                    .overflow_y_scroll()
                    .p_4()
                    .text_size(ui(12.))
                    .line_height(ui(20.))
                    .text_color(rgb(colors.text))
                    .whitespace_normal()
                    .child(self.preview.clone()),
            );
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(colors.bg))
            .text_color(rgb(colors.text))
            .font_family(theme::ui_font(cx))
            .text_size(ui(12.))
            .track_focus(&self.focus)
            .key_context(if self.operation_ui.open {
                "BucketsOperations"
            } else if self.prompt.is_some() {
                "BucketsInput"
            } else if self.help || self.quicklook {
                "BucketsOverlay"
            } else if matches!(self.panes[self.active].content,PaneContent::Agent(_)) {
                "BucketsAgent"
            } else if self.panes[self.active].is_terminal() {
                "BucketsTerminal"
            } else {
                "Buckets"
            })
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<FileDrag>, window, cx| {
                    let bounds = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
                    if bounds.contains(&event.event.position) {
                        return;
                    }
                    let drag = event.drag(cx);
                    let paths = drag
                        .locations
                        .iter()
                        .filter_map(|location| location.local_path().map(|path| path.to_path_buf()))
                        .collect::<Vec<_>>();
                    if paths.len() != drag.locations.len() || paths.is_empty() {
                        return;
                    }
                    match native_drag::start(window, &paths) {
                        Ok(true) => {
                            cx.stop_active_drag(window);
                        }
                        Ok(false) => {}
                        Err(error) => {
                            this.message(format!("Native file drag failed: {error:#}"), true, cx)
                        }
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &OpenSettings, w, cx| this.open_settings(w, cx)))
            .on_action(cx.listener(Self::menu_command))
            .when(
                !self.operation_ui.open && self.prompt.is_none() && !self.help && !self.quicklook && !matches!(self.panes[self.active].content,PaneContent::Agent(_)),
                |el| {
                    el.on_action(cx.listener(|this, _: &gpui_component::input::Copy, w, cx| {
                        if let PaneContent::Terminal(view) = &this.panes[this.active].content {
                            view.update(cx, |view, cx| view.copy(cx));
                        } else {
                            this.command(Command::Copy, w, cx);
                        }
                    }))
                    .on_action(
                        cx.listener(|this, _: &gpui_component::input::SelectAll, w, cx| {
                            if let PaneContent::Terminal(view) = &this.panes[this.active].content {
                                view.update(cx, |view, cx| view.select_all(cx));
                            } else {
                                this.command(Command::SelectAll, w, cx);
                            }
                        }),
                    )
                    .when(
                        !matches!(self.panes[self.active].location, Location::Git { .. })
                            || self.panes[self.active].is_terminal(),
                        |el| {
                            el.on_action(cx.listener(
                                |this, _: &gpui_component::input::Paste, w, cx| {
                                    if let PaneContent::Terminal(view) =
                                        &this.panes[this.active].content
                                    {
                                        view.update(cx, |view, cx| view.paste(cx));
                                    } else {
                                        this.command(Command::Paste, w, cx);
                                    }
                                },
                            ))
                        },
                    )
                    .when(
                        !self.panes[self.active].is_terminal()
                            && !matches!(self.panes[self.active].location, Location::Git { .. }),
                        |el| {
                            el.on_action(cx.listener(
                                |this, _: &gpui_component::input::Cut, w, cx| {
                                    this.command(Command::Cut, w, cx)
                                },
                            ))
                        },
                    )
                },
            )
            .on_action(cx.listener(|this, action: &PluginShortcut, window, cx| {
                if let Some(i) = this
                    .plugin_commands
                    .iter()
                    .position(|c| c.plugin == action.plugin && c.id == action.command)
                {
                    this.command(Command::PluginInvoke(i), window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(
                cx.listener(|this, _: &NextPane, window, cx| {
                    if this.operation_ui.open || this.settings_open || this.prompt.is_some() || this.help || this.quicklook {return;}
                    this.command(Command::Next, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &PreviousPane, window, cx| {
                if this.operation_ui.open || this.settings_open || this.prompt.is_some() || this.help || this.quicklook {return;}
                this.active = (this.active + this.panes.len() - 1) % this.panes.len();
                this.focus_active(window, cx);
                this.load_preview(cx);
                cx.notify();
            }))
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::Escape, window, cx| {
                    if this.prompt.is_some() {
                        this.dismiss(window, cx);
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
                    }
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::MoveUp, _, cx| {
                    if this.prompt==Some(Prompt::Search){this.move_cursor(-1,false,cx);cx.stop_propagation();} else if this.prompt == Some(Prompt::Palette) {
                        this.palette_move(-1, cx);
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
                    }
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::MoveDown, _, cx| {
                    if this.prompt==Some(Prompt::Search){this.move_cursor(1,false,cx);cx.stop_propagation();} else if this.prompt == Some(Prompt::Palette) {
                        this.palette_move(1, cx);
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
                    }
                }),
            )
            .capture_key_down(cx.listener(Self::on_key))
            .child(
                div()
                    .h(ui(58.))
                    .flex_shrink_0()
                    .px_5()
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(rgb(colors.chrome))
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .size(ui(22.))
                                    .bg(rgb(colors.accent))
                                    .rounded_sm()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(rgb(colors.chrome))
                                    .font_weight(FontWeight::BOLD)
                                    .child("b"),
                            )
                            .child(
                                div()
                                    .text_size(ui(17.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("buckets"),
                            )
                            ,
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .child(
                                div()
                                    .id("search-btn")
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, w, cx| {
                                        this.begin(Prompt::Search, w, cx)
                                    }))
                                    .child(hint("Filter", self.binding_hint(Command::Search), cx)),
                            )
                            .child(
                                div()
                                    .id("split-btn")
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, w, cx| {
                                        this.command(Command::Split, w, cx)
                                    }))
                                    .child(if compact{key(self.binding_hint(Command::Split), cx).into_any_element()}else{hint("Split",self.binding_hint(Command::Split), cx).into_any_element()}),
                            )
                            .child(
                                div()
                                    .id("commands-btn")
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .bg(rgb(colors.surface))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, w, cx| {
                                        this.begin(Prompt::Palette, w, cx)
                                    }))
                                    .child(if compact{key(self.binding_hint_id("app.palette"), cx).into_any_element()}else{hint("Commands",self.binding_hint_id("app.palette"), cx).into_any_element()}),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(self.sidebar && !compact, |el| el.child(sidebar))
                    .child(div().flex_1().min_w_0().min_h_0().child(panes))
                    .when(show_preview, |el| el.child(preview)),
            )
            .when(self.prompt==Some(Prompt::Search),|el|{
                let pane=&self.panes[self.active];let query=self.input.read(cx).value();let recursive=broader_search(&query);
                el.child(div().id("name-filter-command-line").flex_shrink_0().bg(rgb(colors.chrome)).border_t_1().border_color(rgb(colors.accent)).px_3().py_2().flex().flex_col().gap_1()
                    .child(div().flex().items_center().gap_3().child(div().text_color(rgb(colors.directory)).child("/")).child(div().flex_1().min_w_0().child(Input::new(&self.input))).child(div().text_size(ui(11.)).text_color(rgb(colors.muted)).child(if recursive{"Search".into()}else{format!("{} / {}",pane.visible_count(),pane.entries.len())})))
                    .child(div().text_size(ui(10.)).text_color(rgb(colors.muted)).child(if recursive{"Enter searches · Esc clear · fd: names · rg: contents · fff: plugin"}else if compact{"↑↓ / Ctrl-N/P · Enter open · Esc clear · fd:/rg:/fff: search"}else{"Filter names · ↑↓ / Ctrl-N/P navigate · Enter opens · Esc restores · fd: / rg: / fff: search"})))
            })
            .child(
                div()
                    .h(ui(32.))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.chrome))
                    .when(self.prompt==Some(Prompt::Search)||matches!(self.panes[self.active].content,PaneContent::Agent(_)),|el|el.hidden())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(if self.status_error { colors.danger } else { colors.success }))
                            .child(if self.operations > 0 {
                                format!(
                                    "{} · {} operation(s) running",
                                    self.status, self.operations
                                )
                            } else {
                                self.status.clone()
                            }),
                    )
                    .when(self.operation_ui.has_records() || self.operation_ui.running(),|el|el.child(div().id("operations-status").ml_3().cursor_pointer().text_size(ui(10.)).text_color(rgb(colors.accent)).child("Operations").on_click(cx.listener(|this,_,window,cx|this.command(Command::Operations,window,cx)))))
                    .child(
                        div()
                            .ml_4()
                            .text_size(ui(10.))
                            .text_color(rgb(colors.muted))
                            .child(format!(
                                "{} · {} pane{}",
                                if self.hidden {
                                    "hidden files shown"
                                } else {
                                    "hidden files off"
                                },
                                self.panes.len(),
                                if self.panes.len() == 1 { "" } else { "s" }
                            )),
                    ),
            )
            .child(
                div()
                    .h(ui(40.))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_5()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.chrome))
                    .text_size(ui(11.))
                    .when(self.prompt==Some(Prompt::Search)||matches!(self.panes[self.active].content,PaneContent::Agent(_)),|el|el.hidden())
                    .when(self.panes[self.active].is_terminal(), |el| {
                        el.child(hint("Switch pane", self.binding_hint(Command::Next), cx))
                            .child(hint("Commands", self.binding_hint_id("app.palette"), cx))
                            .child(hint("Close terminal", self.binding_hint(Command::Close), cx))
                    })
                    .when(!self.panes[self.active].is_terminal(), |el| {
                        el.child(hint("Navigate", "↑↓", cx))
                            .child(hint("Open", self.binding_hint_id("file.open"), cx))
                            .child(hint("Quick Look", self.binding_hint(Command::QuickLook), cx))
                            .child(hint("Mark", "x", cx))
                    })
                    .when(!compact && !self.panes[self.active].is_terminal(), |el| {
                        el.child(hint("Rename", self.binding_hint(Command::Rename), cx))
                            .child(hint("Copy to", self.binding_hint(Command::CopyTo), cx))
                            .child(hint("Move to", self.binding_hint(Command::MoveTo), cx))
                    })
                    .child(div().flex_1())
                    .when(self.panes[self.active].is_terminal(), |el| {
                        el.child(
                            div()
                                .text_size(ui(10.))
                                .text_color(rgb(colors.muted))
                                .child("⌘A Select · ⌘C Copy · ⌘V Paste"),
                        )
                    })
                    .when(!self.panes[self.active].is_terminal(), |el| {
                        el.child(
                            div()
                                .id("help-btn")
                                .cursor_pointer()
                                .on_click(
                                    cx.listener(|this, _, w, cx| {
                                        this.command(Command::Help, w, cx)
                                    }),
                                )
                                .child(hint("All shortcuts", self.binding_hint(Command::Help), cx)),
                        )
                    }),
            )
            .when(self.prompt.is_some_and(|prompt|prompt!=Prompt::Search) || self.help || self.quicklook, |el| {
                el.child(self.render_overlay(window, cx))
            })
            .when(self.operation_ui.open, |el|el.child(self.render_operations(window,cx)))
            .into_any_element()
    }
}
#[cfg(test)]
fn valid_plugin_shortcut(shortcut: &str) -> bool {
    keybindings::validate_plugin_shortcut(shortcut).is_ok_and(|key| {
        !keybindings::ACTIONS
            .iter()
            .flat_map(|spec| spec.defaults.iter())
            .any(|default| keybindings::normalize(default).is_ok_and(|default| default == key))
    })
}
fn search_entry(location: Location, line: Option<u64>, root: &str) -> Entry {
    let display = location.display();
    let prefix = format!("{}/", root.trim_end_matches('/'));
    let label = display.strip_prefix(&prefix).unwrap_or(&display);
    let name = format!(
        "{}{}",
        label,
        line.map(|n| format!(":{n}")).unwrap_or_default()
    );
    let metadata = location
        .local_path()
        .and_then(|path| std::fs::symlink_metadata(path).ok());
    let is_dir = location.local_path().is_some_and(|path| path.is_dir());
    let is_symlink = metadata.as_ref().is_some_and(|m| m.is_symlink());
    let size = metadata.map(|m| m.len()).unwrap_or(0);
    Entry {
        name,
        location,
        is_dir,
        is_symlink,
        size,
        modified: None,
    }
}
fn session_path() -> PathBuf {
    std::env::var_os("BUCKETS_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config/buckets")
        })
        .join("workspace.json")
}
actions!(buckets, [Quit, NextPane, PreviousPane, OpenSettings]);
fn main() {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!(
            "Buckets — keyboard-first multi-pane file browser\n\nUsage: buckets [LOCATION ...]\n\nLocations: /local/path, ssh://host/path, s3://bucket/prefix, git://…\n\nEnvironment:\n  BUCKETS_CONFIG_DIR  Workspace state directory (default ~/.config/buckets)\n  BUCKETS_PLUGIN_DIR  Plugin manifest directory\n\nPress ⌘K for commands, ? for keyboard reference."
        );
        return;
    }
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        if let Err(error) = application_icon::install() {
            eprintln!("Application icon unavailable: {error:#}");
        }
        cx.text_system()
            .add_fonts(vec![Cow::Borrowed(include_bytes!(
                "../assets/fonts/JetBrainsMono-Regular.ttf"
            ))])
            .expect("load bundled JetBrains Mono");
        gpui_component::init(cx);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        Theme::change(ThemeMode::Dark, None, cx);
        theme::apply(&appearance::Appearance::default(), 13., cx);
        menus::install(cx);
        let window_size = std::env::var("BUCKETS_WINDOW_SIZE")
            .ok()
            .and_then(|s| {
                let (w, h) = s.split_once('x')?;
                Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
            })
            .filter(|(w, h)| w.is_finite() && h.is_finite() && *w >= 960. && *h >= 640.)
            .unwrap_or((1440., 900.));
        let bounds = Bounds::centered(None, size(px(window_size.0), px(window_size.1)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(960.), px(640.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Buckets".into()),
                    appears_transparent: false,
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let browser = cx.new(|cx| Browser::new(window, cx));
                browser.update(cx, |browser, cx| browser.rebuild_keymap(cx));
                let quit_browser = browser.downgrade();
                let handle = window.window_handle();
                cx.on_action(move |_: &Quit, cx| {
                    // Global action bubbling still runs while the window is on
                    // GPUI's update stack. Defer until its slot is restored.
                    let quit_browser = quit_browser.clone();
                    cx.defer(move |cx| {
                        if let Err(error) = handle.update(cx, |_, window, cx| {
                            quit_browser.update(cx, |browser, cx| browser.request_quit(window, cx))
                        }) {
                            if cx.windows().is_empty() {
                                cx.quit();
                            } else {
                                eprintln!("Could not request quit: {error:#}");
                            }
                        }
                    });
                });
                let close_browser = browser.downgrade();
                window.on_window_should_close(cx, move |window, cx| {
                    close_browser
                        .update(cx, |browser, cx| {
                            if browser.guard_settings_exit(window, cx) {
                                false
                            } else {
                                browser.save();
                                true
                            }
                        })
                        .unwrap_or(true)
                });
                cx.new(|cx| Root::new(browser, window, cx))
            },
        )
        .expect("open Buckets window");
        cx.activate(true);
    });
}

#[cfg(test)]
mod filter_projection_tests {
    use super::{Entry, Location, Pane};
    use std::sync::Arc;
    fn entry(name: &str) -> Entry {
        Entry {
            name: name.into(),
            location: Location::parse(&format!("/tmp/buckets-filter/{name}")).unwrap(),
            is_dir: false,
            is_symlink: false,
            size: 0,
            modified: None,
        }
    }
    fn pane() -> Pane {
        let mut p = Pane::new(0, Location::parse("/tmp/buckets-filter").unwrap());
        p.entries = vec![
            entry("alpha.txt"),
            entry("nested"),
            entry("café.txt"),
            entry("space name.txt"),
        ];
        p
    }
    fn query(p: &mut Pane, text: &str) {
        p.name_filter.as_mut().unwrap().query = text.into();
        p.refresh_name_filter();
    }
    #[::core::prelude::v1::test]
    fn filter_keeps_canonical_entries_and_open_target() {
        let mut p = pane();
        p.start_name_filter();
        query(&mut p, "spn");
        assert_eq!(p.visible_count(), 1);
        assert_eq!(p.cursor, 3);
        assert_eq!(p.entries.len(), 4);
        p.finish_name_filter(false);
        assert_eq!(p.entries[p.cursor].name, "space name.txt");
    }
    #[::core::prelude::v1::test]
    fn refresh_and_escape_restore_cursor_and_marks_by_location() {
        let mut p = pane();
        p.cursor = 2;
        p.selected = [0, 3].into();
        p.start_name_filter();
        query(&mut p, "spn");
        p.replace_entries(vec![
            entry("café.txt"),
            entry("space name.txt"),
            entry("new.txt"),
            entry("alpha.txt"),
        ]);
        assert_eq!(p.cursor, 3);
        assert_eq!(p.selected, [0, 3].into());
        assert_eq!(p.visible_count(), 1);
        p.finish_name_filter(true);
        assert_eq!(p.cursor, 1);
        assert_eq!(p.selected, [0, 3].into());
        assert_eq!(p.entries[p.cursor].name, "café.txt");
    }
    #[::core::prelude::v1::test]
    fn sorting_preserves_cursor_marks_and_active_filter_by_location() {
        let mut p = pane();
        p.cursor = 2;
        p.selected = [0, 3].into();
        let cursor = p.entries[p.cursor].location.clone();
        let marked = p.chosen();
        p.view.descending = true;
        p.replace_entries(p.entries.clone());
        assert_eq!(p.entries[p.cursor].location, cursor);
        assert_eq!(
            p.chosen()
                .into_iter()
                .collect::<std::collections::HashSet<_>>(),
            marked.into_iter().collect()
        );
        p.start_name_filter();
        query(&mut p, "txt");
        p.view.descending = false;
        p.replace_entries(p.entries.clone());
        assert_eq!(p.visible_count(), 3);
        p.finish_name_filter(true);
        assert_eq!(p.entries[p.cursor].location, cursor);
    }
    #[::core::prelude::v1::test]
    fn legacy_session_defaults_and_layout_view_weights_round_trip() {
        let mut session: super::Session =
            serde_json::from_str(r#"{"locations":["/tmp"],"active":0,"hidden":false}"#).unwrap();
        assert_eq!(
            session.layout,
            super::named_workspaces::PaneLayout::Horizontal
        );
        assert!(session.views.is_empty());
        session.layout = super::named_workspaces::PaneLayout::Vertical;
        session.weights = vec![3.];
        session.views = vec![super::named_workspaces::PaneView {
            sort: super::named_workspaces::SortKey::Modified,
            descending: true,
            ..Default::default()
        }];
        let saved = serde_json::to_string(&session).unwrap();
        let restored: super::Session = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.views, session.views);
        assert_eq!(restored.weights, session.weights);
        assert_eq!(restored.layout, session.layout);
    }
    #[::core::prelude::v1::test]
    fn projected_mouse_range_never_marks_hidden_entries() {
        let mut p = pane();
        p.start_name_filter();
        p.name_filter.as_mut().unwrap().indices = Arc::new(vec![3, 0]);
        p.select_visible_range(3, 0);
        assert_eq!(p.selected, [0, 3].into());
        assert!(!p.selected.contains(&1));
        assert!(!p.selected.contains(&2));
    }
    #[::core::prelude::v1::test]
    fn empty_matches_have_no_operable_selection_and_engine_prefixes_bypass_filter() {
        let mut p = pane();
        p.selected.insert(1);
        p.start_name_filter();
        query(&mut p, "not-a-match");
        assert_eq!(p.visible_count(), 0);
        assert!(p.chosen().is_empty());
        for text in [
            "fd: alpha",
            "rg: content",
            "fff: alpha",
            "plugin:semantic query",
        ] {
            query(&mut p, text);
            assert_eq!(p.visible_count(), 4);
        }
        p.finish_name_filter(true);
        assert_eq!(p.selected, [1].into());
    }
}

#[cfg(test)]
mod keymap_integration_tests {
    use super::{APP_KEYMAP, PluginShortcut, keybindings, menus, merged_key_bindings};
    use gpui::{AsKeystroke, KeyBinding};

    #[::core::prelude::v1::test]
    fn replacing_and_disabling_app_bindings_preserves_native_inputs_and_terminal_keys() {
        let original = [
            KeyBinding::new("cmd-v", gpui_component::input::Paste, Some("Input")),
            KeyBinding::new(
                "tab",
                gpui_component::input::IndentInline,
                Some("BucketsTerminal"),
            ),
            menus::binding("pane.split", "cmd-t", None)
                .unwrap()
                .with_meta(APP_KEYMAP),
            KeyBinding::new(
                "ctrl-e",
                PluginShortcut {
                    plugin: "native-tools".into(),
                    command: "open_nvim".into(),
                },
                Some("Buckets"),
            )
            .with_meta(APP_KEYMAP),
        ];
        let changed = merged_key_bindings(
            original.iter(),
            vec![
                menus::binding("pane.split", "cmd-\\", None)
                    .unwrap()
                    .with_meta(APP_KEYMAP),
            ],
        );
        let keys = changed
            .iter()
            .map(|binding| binding.keystrokes()[0].as_keystroke().unparse())
            .collect::<Vec<_>>();
        assert_eq!(keys, ["cmd-v", "tab", "cmd-\\"]);
        let disabled = merged_key_bindings(changed.iter(), vec![]);
        assert_eq!(disabled.len(), 2);
        assert!(disabled.iter().all(|binding| binding.meta().is_none()));
        assert_eq!(disabled[0].predicate().unwrap().to_string(), "Input");
        assert_eq!(
            disabled[1].predicate().unwrap().to_string(),
            "BucketsTerminal"
        );
    }

    #[::core::prelude::v1::test]
    fn native_plus_forms_match_once_and_disappear_after_remapping_or_disabling() {
        use gpui::Keystroke;
        for (canonical, native, plugin) in [
            ("cmd-shift-=", "cmd-+", false),
            ("ctrl-shift-=", "ctrl-+", true),
        ] {
            let original = super::runtime_binding_keys(canonical)
                .into_iter()
                .map(|key| {
                    if plugin {
                        KeyBinding::new(
                            &key,
                            PluginShortcut {
                                plugin: "example".into(),
                                command: "plus".into(),
                            },
                            Some("Buckets"),
                        )
                        .with_meta(APP_KEYMAP)
                    } else {
                        menus::binding("font.increase", &key, None)
                            .unwrap()
                            .with_meta(APP_KEYMAP)
                    }
                })
                .collect::<Vec<_>>();
            let stroke = Keystroke::parse(native).unwrap();
            assert_eq!(
                original
                    .iter()
                    .filter(
                        |binding| binding.match_keystrokes(std::slice::from_ref(&stroke))
                            == Some(false)
                    )
                    .count(),
                1
            );
            let shifted = Keystroke::parse(&native.replace("-+", "-shift-+")).unwrap();
            assert_eq!(
                original
                    .iter()
                    .filter(
                        |binding| binding.match_keystrokes(std::slice::from_ref(&shifted))
                            == Some(false)
                    )
                    .count(),
                1
            );
            let changed = merged_key_bindings(
                original.iter(),
                vec![
                    menus::binding("font.increase", "cmd-alt-u", None)
                        .unwrap()
                        .with_meta(APP_KEYMAP),
                ],
            );
            assert!(changed.iter().all(|binding| {
                binding.match_keystrokes(std::slice::from_ref(&stroke)) != Some(false)
            }));
            let disabled = merged_key_bindings(original.iter(), vec![]);
            assert!(disabled.is_empty());
        }
        assert_eq!(super::runtime_binding_keys("cmd-="), ["cmd-="]);
    }

    #[::core::prelude::v1::test]
    fn every_configurable_action_has_a_dispatch_binding() {
        for spec in keybindings::ACTIONS {
            assert!(
                menus::binding(spec.id, "cmd-alt-9", None).is_some(),
                "{} has no dispatcher",
                spec.id
            );
        }
    }
}
