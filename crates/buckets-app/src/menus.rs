//! Native menu structure. The workspace owns contextual command execution.
use gpui::{Action, App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType, actions};
use gpui_component::input;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum MenuCommand {
    Operations,
    SaveWorkspace,
    ReloadWorkspaces,
    GrowPane,
    ShrinkPane,
    EqualPanes,
    SortName,
    SortSize,
    SortModified,
    SortType,
    SortDescending,
    DirectoriesFirst,
    ColumnSize,
    ColumnModified,
    ColumnType,
    LayoutHorizontal,
    LayoutVertical,

    About,
    Open,
    NewFile,
    NewFolder,
    Rename,
    CopyTo,
    MoveTo,
    Duplicate,
    Delete,
    Refresh,
    Search,
    QuickLook,
    Hidden,
    Sidebar,
    Preview,
    FontIncrease,
    FontDecrease,
    FontReset,
    Palette,
    Location,
    BookmarkCurrent,
    OpenBookmarks,
    Back,
    Forward,
    Parent,
    Home,
    Desktop,
    Downloads,
    Workspace,
    Root,
    Plugins,
    ReloadPlugins,
    Split,
    Close,
    Minimize,
    ZoomWindow,
    Help,
}

#[derive(Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = buckets_menu, no_json)]
pub struct Run {
    pub command: MenuCommand,
}

actions!(
    buckets_menu,
    [
        HideApplication,
        HideOtherApplications,
        ShowAllApplications,
        Unavailable
    ]
);

/// Native menu validation only sees action types. Unavailable file operations
/// use an action with no handler, rather than a handler that silently does nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuState {
    pub workspace_actions: bool,
    pub file_actions: bool,
    pub has_selection: bool,
    pub writable: bool,
}

pub fn install(cx: &mut App) {
    cx.on_action(|_: &HideApplication, cx| cx.hide());
    cx.on_action(|_: &HideOtherApplications, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAllApplications, cx| cx.unhide_other_apps());
    cx.bind_keys([
        KeyBinding::new("cmd-h", HideApplication, None),
        KeyBinding::new("cmd-alt-h", HideOtherApplications, None),
        KeyBinding::new(
            "cmd-m",
            Run {
                command: MenuCommand::Minimize,
            },
            None,
        ),
    ]);
    set(cx, MenuState::default());
}

/// Build one configurable app binding. File clipboard actions share the
/// native Edit menu actions; editor and terminal contexts remain independent.
pub fn binding(id: &str, key: &str, context: Option<&str>) -> Option<KeyBinding> {
    let command = match id {
        "view.operations" => MenuCommand::Operations,
        "workspace.save" => MenuCommand::SaveWorkspace,
        "workspace.reload" => MenuCommand::ReloadWorkspaces,
        "pane.grow" => MenuCommand::GrowPane,
        "pane.shrink" => MenuCommand::ShrinkPane,
        "pane.equalize" => MenuCommand::EqualPanes,
        "pane.sort_name" => MenuCommand::SortName,
        "pane.sort_size" => MenuCommand::SortSize,
        "pane.sort_modified" => MenuCommand::SortModified,
        "pane.sort_type" => MenuCommand::SortType,
        "pane.sort_reverse" => MenuCommand::SortDescending,
        "pane.directories_first" => MenuCommand::DirectoriesFirst,
        "pane.column_size" => MenuCommand::ColumnSize,
        "pane.column_modified" => MenuCommand::ColumnModified,
        "pane.column_type" => MenuCommand::ColumnType,
        "pane.layout_horizontal" => MenuCommand::LayoutHorizontal,
        "pane.layout_vertical" => MenuCommand::LayoutVertical,

        "app.quit" => return Some(KeyBinding::new(key, crate::Quit, context)),
        "app.settings" => return Some(KeyBinding::new(key, crate::OpenSettings, context)),
        "pane.next" => return Some(KeyBinding::new(key, crate::NextPane, context)),
        "pane.previous" => return Some(KeyBinding::new(key, crate::PreviousPane, context)),
        "file.copy" => return Some(KeyBinding::new(key, input::Copy, context)),
        "file.cut" => return Some(KeyBinding::new(key, input::Cut, context)),
        "file.paste" => return Some(KeyBinding::new(key, input::Paste, context)),
        "file.select_all" => return Some(KeyBinding::new(key, input::SelectAll, context)),
        "pane.split" => MenuCommand::Split,
        "pane.close" => MenuCommand::Close,
        "app.palette" => MenuCommand::Palette,
        "app.help" => MenuCommand::Help,
        "font.increase" => MenuCommand::FontIncrease,
        "font.decrease" => MenuCommand::FontDecrease,
        "font.reset" => MenuCommand::FontReset,
        "location.open" => MenuCommand::Location,
        "location.back" => MenuCommand::Back,
        "location.forward" => MenuCommand::Forward,
        "location.parent" => MenuCommand::Parent,
        "location.home" => MenuCommand::Home,
        "location.desktop" => MenuCommand::Desktop,
        "location.downloads" => MenuCommand::Downloads,
        "location.workspace" => MenuCommand::Workspace,
        "location.root" => MenuCommand::Root,
        "file.open" => MenuCommand::Open,
        "file.rename" => MenuCommand::Rename,
        "file.new" => MenuCommand::NewFile,
        "file.new_folder" => MenuCommand::NewFolder,
        "file.copy_to" => MenuCommand::CopyTo,
        "file.move_to" => MenuCommand::MoveTo,
        "file.duplicate" => MenuCommand::Duplicate,
        "file.trash" => MenuCommand::Delete,
        "file.refresh" => MenuCommand::Refresh,
        "file.filter" => MenuCommand::Search,
        "file.quick_look" => MenuCommand::QuickLook,
        "file.toggle_hidden" => MenuCommand::Hidden,
        "view.preview" => MenuCommand::Preview,
        "view.sidebar" => MenuCommand::Sidebar,
        "bookmark.add" => MenuCommand::BookmarkCurrent,
        "bookmark.open" => MenuCommand::OpenBookmarks,
        "plugin.reload" => MenuCommand::ReloadPlugins,
        "plugin.directory" => MenuCommand::Plugins,
        _ => return None,
    };
    Some(KeyBinding::new(key, Run { command }, context))
}

/// Call only when the state changes; terminal output should not rebuild menus.
pub fn set(cx: &App, state: MenuState) {
    cx.set_menus(build(state));
}

fn item(label: &str, command: MenuCommand, available: bool) -> MenuItem {
    if available {
        MenuItem::action(label.to_owned(), Run { command })
    } else {
        MenuItem::action(label.to_owned(), Unavailable)
    }
}

fn build(state: MenuState) -> Vec<Menu> {
    let file = state.file_actions;
    let selected = file && state.has_selection;
    let writable = file && state.writable;
    let mutable_selection = selected && state.writable;
    let separator = MenuItem::separator;
    vec![
        Menu {
            name: "Buckets".into(),
            items: vec![
                item("About Buckets", MenuCommand::About, true),
                separator(),
                MenuItem::action("Settings…", crate::OpenSettings),
                separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                separator(),
                MenuItem::action("Hide Buckets", HideApplication),
                MenuItem::action("Hide Others", HideOtherApplications),
                MenuItem::action("Show All", ShowAllApplications),
                separator(),
                MenuItem::action("Quit Buckets", crate::Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                item("Open Selected", MenuCommand::Open, selected),
                item("New File…", MenuCommand::NewFile, writable),
                item("New Folder…", MenuCommand::NewFolder, writable),
                separator(),
                item("Rename…", MenuCommand::Rename, mutable_selection),
                item("Duplicate", MenuCommand::Duplicate, mutable_selection),
                item("Copy to Directory…", MenuCommand::CopyTo, selected),
                item("Move to Directory…", MenuCommand::MoveTo, mutable_selection),
                item("Move to Trash…", MenuCommand::Delete, mutable_selection),
                separator(),
                item("Operations and Recovery…", MenuCommand::Operations, true),
                item("Close Pane", MenuCommand::Close, true),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                // No file/terminal fallback for Undo or Redo: only real focused
                // input widgets provide these actions and enable their items.
                MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
                separator(),
                MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
                separator(),
                MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
                separator(),
                item("Search Files / Contents…", MenuCommand::Search, file),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                item(
                    "Command Palette…",
                    MenuCommand::Palette,
                    state.workspace_actions,
                ),
                separator(),
                item("Increase Text Size", MenuCommand::FontIncrease, true),
                item("Decrease Text Size", MenuCommand::FontDecrease, true),
                item("Reset Text Size", MenuCommand::FontReset, true),
                separator(),
                item("Toggle Places Sidebar", MenuCommand::Sidebar, true),
                item("Toggle File Preview", MenuCommand::Preview, true),
                item("Toggle Hidden Files", MenuCommand::Hidden, file),
                item("Quick Look", MenuCommand::QuickLook, selected),
                separator(),
                item("Refresh Directory", MenuCommand::Refresh, file),
            ],
        },
        Menu {
            name: "Go".into(),
            items: vec![
                item("Back", MenuCommand::Back, file),
                item("Forward", MenuCommand::Forward, file),
                item("Parent Directory", MenuCommand::Parent, file),
                item("Go to Location / Connect…", MenuCommand::Location, file),
                item(
                    "Bookmark This Location…",
                    MenuCommand::BookmarkCurrent,
                    file,
                ),
                item("Open Bookmarks…", MenuCommand::OpenBookmarks, file),
                separator(),
                item("Home", MenuCommand::Home, file),
                item("Desktop", MenuCommand::Desktop, file),
                item("Downloads", MenuCommand::Downloads, file),
                item("Workspace", MenuCommand::Workspace, file),
                item("Filesystem Root", MenuCommand::Root, file),
                separator(),
                item("Plugin Directory", MenuCommand::Plugins, file),
                item("Reload Plugins", MenuCommand::ReloadPlugins, true),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                item("Minimize", MenuCommand::Minimize, true),
                item("Zoom", MenuCommand::ZoomWindow, true),
                separator(),
                item(
                    "Save Named Workspace…",
                    MenuCommand::SaveWorkspace,
                    state.workspace_actions,
                ),
                item(
                    "Arrange Side by Side",
                    MenuCommand::LayoutHorizontal,
                    state.workspace_actions,
                ),
                item(
                    "Arrange Top to Bottom",
                    MenuCommand::LayoutVertical,
                    state.workspace_actions,
                ),
                item(
                    "Grow Active Pane",
                    MenuCommand::GrowPane,
                    state.workspace_actions,
                ),
                item(
                    "Shrink Active Pane",
                    MenuCommand::ShrinkPane,
                    state.workspace_actions,
                ),
                item(
                    "Equalize Pane Sizes",
                    MenuCommand::EqualPanes,
                    state.workspace_actions,
                ),
                separator(),
                item("Split Pane", MenuCommand::Split, state.workspace_actions),
                if state.workspace_actions {
                    MenuItem::action("Next Pane", crate::NextPane)
                } else {
                    MenuItem::action("Next Pane", Unavailable)
                },
                if state.workspace_actions {
                    MenuItem::action("Previous Pane", crate::PreviousPane)
                } else {
                    MenuItem::action("Previous Pane", Unavailable)
                },
            ],
        },
        Menu {
            name: "Help".into(),
            items: vec![item("Keyboard Reference", MenuCommand::Help, true)],
        },
    ]
}
