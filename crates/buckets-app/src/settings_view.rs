//! Keyboard-first preferences workspace. The owner persists every event before
//! applying it; this view retains an independent JSON draft until explicit save.
use crate::account_state::AccountPresentation;
use crate::settings::Settings;
use crate::shortcut_recorder::{Outcome, Recorder, key_identity};
use crate::{key, theme, ui};
use buckets_plugin::{
    AccountAction, AccountRequest, Capability, PluginManifest, SettingField, SettingKind,
};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputEvent, InputState};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
const CATEGORY_NAMES: &[&str] = &[
    "All",
    "Appearance",
    "Browsing",
    "Bookmarks",
    "Plugins",
    "Shortcuts",
    "Advanced",
];

#[derive(Clone)]
pub enum Change {
    AppearanceFont { terminal: bool, value: String },
    AppearanceColor(String, Option<String>),
    AppearanceReset,
    Keybinding(String, Option<Vec<String>>),
    Font(f32),
    Hidden(bool),
    Sidebar(bool),
    Preview(bool),
    PluginEnabled(String, bool),
    PluginField(String, String, Option<Value>),
    Reset,
    SaveBookmark(crate::settings::Bookmark),
    RenameBookmark(usize, String),
    RemoveBookmark(usize),
}
#[derive(Clone)]
pub enum SettingsEvent {
    Account {
        plugin: String,
        request: AccountRequest,
    },
    CancelAccount(String),
    AccountUsage(String),
    Change(Change),
    SaveJson {
        source: String,
        base: String,
    },
    Reload,
    Close,
    CancelClose,
}
#[derive(Clone)]
enum Action {
    AppearanceFont(bool),
    AppearanceFontReset(bool),
    AppearanceColor(String),
    AppearanceColorReset(String),
    AppearanceReset,
    Shortcut(String),
    ShortcutReset(String),
    FontDown,
    FontUp,
    FontReset,
    Hidden,
    Sidebar,
    Preview,
    PluginEnabled(String),
    Account(String, AccountRequest),
    AccountCancel(String),
    AccountModel(String, String),
    AccountUsage(String),
    BookmarkRename(usize),
    BookmarkRemove(usize),
    Field(String, Box<SettingField>, Value),
    Json,
    Reload,
    Reset,
}
#[derive(Clone)]
struct Row {
    section: String,
    title: String,
    description: String,
    value: String,
    action: Action,
}
enum Edit {
    AppearanceFont(bool),
    AppearanceColor(String),
    Plugin { plugin: String, field: SettingField },
    Bookmark(usize),
}
actions!(preferences, [NextControl, PreviousControl, SaveJson]);

struct Recording {
    id: String,
    input: Recorder,
}

pub struct SettingsView {
    recorder_focus: FocusHandle,
    recording: Option<Recording>,
    recording_saving: bool,
    appearance_saving: bool,
    recording_held: BTreeSet<String>,
    activation_key: Option<Keystroke>,
    focus: FocusHandle,
    search: Entity<InputState>,
    editor: Entity<InputState>,
    field_input: Entity<InputState>,
    subscriptions: Vec<Subscription>,
    settings: Settings,
    plugins: Vec<PluginManifest>,
    plugin_shortcuts: Vec<(String, String, Vec<String>)>,
    accounts: BTreeMap<String, AccountPresentation>,
    account_confirmation: Option<(String, String)>,
    account_announced: BTreeSet<String>,
    path: PathBuf,
    raw: String,
    draft_base: String,
    dirty: bool,
    json_open: bool,
    editing: Option<Edit>,
    selected: usize,
    category: usize,
    scroll: ScrollHandle,
    message: String,
    error: bool,
    close_pending: bool,
    reload_pending: bool,
    close_after_save: bool,
    exit_requested: bool,
}
impl EventEmitter<SettingsEvent> for SettingsView {}
impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl SettingsView {
    pub fn new(
        settings: Settings,
        path: PathBuf,
        raw: String,
        plugins: Vec<PluginManifest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.bind_keys([
            KeyBinding::new("tab", NextControl, Some("BucketsSettings")),
            KeyBinding::new("shift-tab", PreviousControl, Some("BucketsSettings")),
            KeyBinding::new("cmd-s", SaveJson, Some("BucketsSettings")),
        ]);
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search settings or plugins…"));
        let editor = cx.new(|cx| {
            let mut input = InputState::new(window, cx).multi_line(true).rows(12);
            input.set_value(raw.clone(), window, cx);
            input
        });
        let field_input = cx.new(|cx| InputState::new(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(&search, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.category = 0;
                    this.selected = 0;
                    this.scroll.scroll_to_item(0);
                    cx.notify();
                } else if matches!(event, InputEvent::Focus) {
                    this.selected = 0;
                    cx.notify();
                }
            }),
            cx.subscribe_in(&editor, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.dirty = this.editor.read(cx).value().as_str() != this.draft_base;
                    cx.notify();
                }
            }),
            cx.subscribe_in(&field_input, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.commit_field(window, cx);
                }
            }),
        ];
        let recorder_focus = cx.focus_handle();
        let weak = cx.weak_entity();
        let recorder_window = window.window_handle();
        let interceptor = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != recorder_window {
                return;
            }
            let _ = weak.update(cx, |this, cx| {
                this.intercept_recording(&event.keystroke, window, cx)
            });
        });
        let blur = cx.on_blur(&recorder_focus, window, |this, window, cx| {
            if this.recording.is_some() && !this.recorder_focus.is_focused(window) {
                this.finish_recording(None, cx);
                this.recording_held.clear();
            }
        });
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.finish_recording(None, cx);
                this.recording_held.clear();
            }
        });
        let mut subscriptions = subscriptions;
        subscriptions.extend([interceptor, blur, activation]);
        Self {
            recorder_focus,
            recording: None,
            recording_saving: false,
            appearance_saving: false,
            recording_held: BTreeSet::new(),
            activation_key: None,
            focus: cx.focus_handle(),
            search,
            editor,
            field_input,
            subscriptions,
            settings,
            plugins,
            plugin_shortcuts: vec![],
            accounts: BTreeMap::new(),
            account_confirmation: None,
            account_announced: BTreeSet::new(),
            path,
            raw: raw.clone(),
            draft_base: raw,
            dirty: false,
            json_open: false,
            editing: None,
            selected: 0,
            category: 0,
            scroll: ScrollHandle::new(),
            message: "Changes save immediately. JSON saves only when you choose Save.".into(),
            error: false,
            close_pending: false,
            reload_pending: false,
            close_after_save: false,
            exit_requested: false,
        }
    }
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window);
        if self.recording.is_some() {
            self.recorder_focus.focus(window);
            return;
        }
        if self.close_pending || self.reload_pending {
            // Confirmation actions keep the focus, rather than the hidden editor.
        } else if self.json_open {
            self.selected = 0;
            self.editor.focus_handle(cx).focus(window);
        } else if self.editing.is_some() {
            self.field_input.focus_handle(cx).focus(window);
        } else {
            self.selected = 0;
            self.search.focus_handle(cx).focus(window);
        }
        cx.notify();
    }
    pub fn sync(
        &mut self,
        settings: Settings,
        raw: String,
        plugins: Vec<PluginManifest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings = settings;
        self.plugins = plugins;
        self.raw = raw.clone();
        if !self.dirty {
            self.draft_base = raw.clone();
            self.editor
                .update(cx, |input, cx| input.set_value(raw, window, cx));
        }
        cx.notify();
    }
    pub fn catalog(
        &mut self,
        settings: Settings,
        plugins: Vec<PluginManifest>,
        cx: &mut Context<Self>,
    ) {
        self.settings = settings;
        self.plugins = plugins;
        cx.notify();
    }
    pub fn accounts(
        &mut self,
        accounts: BTreeMap<String, AccountPresentation>,
        cx: &mut Context<Self>,
    ) {
        for (plugin, presentation) in &accounts {
            if let Some(state) = &presentation.state {
                let identity = format!(
                    "{plugin}:{}",
                    state.active_account_id.as_deref().unwrap_or_default()
                );
                if state.first_connection && self.account_announced.insert(identity) {
                    self.account_confirmation = Some((
                        state.plan_label.clone().unwrap_or_else(|| "Account connected".into()),
                        state.message.clone().unwrap_or_else(|| "Eligible requests use the connected account’s plan. Manage usage with your provider.".into()),
                    ));
                    self.selected = 0;
                }
            }
        }
        self.accounts = accounts;
        cx.notify();
    }
    pub fn focus_plugin(&mut self, plugin: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty || self.account_confirmation.is_some() {
            return;
        }
        let name = self
            .plugins
            .iter()
            .find(|manifest| manifest.id == plugin)
            .map(|manifest| manifest.name.clone())
            .unwrap_or_else(|| plugin.to_owned());
        self.search
            .update(cx, |search, cx| search.set_value(name, window, cx));
        self.category = 4;
        self.json_open = false;
        self.editing = None;
        self.selected = 2;
        self.focus.focus(window);
        self.scroll.scroll_to_item(0);
        cx.notify();
    }
    pub fn shortcuts(
        &mut self,
        catalog: Vec<(String, String, Vec<String>)>,
        cx: &mut Context<Self>,
    ) {
        self.plugin_shortcuts = catalog;
        cx.notify();
    }
    pub fn result(
        &mut self,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) => {
                if self.recording_saving {
                    self.finish_recording(Some(window), cx);
                }
                if self.appearance_saving {
                    self.appearance_saving = false;
                    self.editing = None;
                    self.focus.focus(window);
                }
                self.message = "Saved to settings.json".into();
                self.error = false;
            }
            Err(e) => {
                self.recording_saving = false;
                self.appearance_saving = false;
                self.message = e;
                self.error = true;
                self.close_after_save = false;
            }
        }
        if self.close_after_save && !self.error {
            self.dirty = false;
            self.draft_base = self.raw.clone();
            self.editor.update(cx, |input, cx| {
                input.set_value(self.raw.clone(), window, cx)
            });
            self.close_after_save = false;
            self.close_pending = false;
            cx.emit(SettingsEvent::Close);
        }
        cx.notify();
    }
    pub fn json_saved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dirty = false;
        self.draft_base = self.raw.clone();
        self.editor.update(cx, |input, cx| {
            input.set_value(self.raw.clone(), window, cx)
        });
    }
    pub fn warning(&mut self, message: String, cx: &mut Context<Self>) {
        self.message = message;
        self.error = true;
        cx.notify();
    }
    pub fn has_unsaved_json(&self) -> bool {
        self.dirty
    }
    pub fn request_exit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_close(window, cx);
        self.exit_requested = true;
        self.message = "Unsaved JSON. Save or discard it to quit, or keep editing.".into();
        cx.notify();
    }
    pub fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_recording(None, cx);
        self.recording_held.clear();
        self.exit_requested = false;
        if self.dirty {
            self.editing = None;
            self.close_pending = true;
            self.reload_pending = false;
            self.focus.focus(window);
            self.selected = 0;
            self.message = "Unsaved JSON. Save, discard the draft, or keep editing.".into();
            cx.notify();
        } else {
            cx.emit(SettingsEvent::Close);
        }
    }
    fn request_reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty {
            self.reload_pending = true;
            self.close_pending = false;
            self.focus.focus(window);
            self.selected = 0;
            self.message = "Reloading discards the JSON draft. Discard or keep editing.".into();
            cx.notify();
        } else {
            cx.emit(SettingsEvent::Reload);
        }
    }
    fn rows(&self, cx: &App) -> Vec<Row> {
        let mut rows = vec![
            Row {
                section: "Appearance".into(),
                title: "Decrease text size".into(),
                description: "Shared by files, previews, settings and terminals. Minimum 9pt."
                    .into(),
                value: if self.settings.font_size <= 9. {
                    "Minimum · 9pt".into()
                } else {
                    format!("{}pt  −", self.settings.font_size)
                },
                action: Action::FontDown,
            },
            Row {
                section: "Appearance".into(),
                title: "Increase text size".into(),
                description: "Maximum 24pt. Configure zoom keys in Shortcuts.".into(),
                value: if self.settings.font_size >= 24. {
                    "Maximum · 24pt".into()
                } else {
                    format!("{}pt  +", self.settings.font_size)
                },
                action: Action::FontUp,
            },
            Row {
                section: "Appearance".into(),
                title: "Reset text size".into(),
                description: "Restore the default 13pt.".into(),
                value: "Reset".into(),
                action: Action::FontReset,
            },
            Row {
                section: "Browsing".into(),
                title: "Show hidden files".into(),
                description: "Include dotfiles when browsing directories.".into(),
                value: onoff(self.settings.show_hidden),
                action: Action::Hidden,
            },
            Row {
                section: "Browsing".into(),
                title: "Places sidebar".into(),
                description: "Show locations when the window has room.".into(),
                value: onoff(self.settings.sidebar),
                action: Action::Sidebar,
            },
            Row {
                section: "Browsing".into(),
                title: "File preview".into(),
                description:
                    "Show the side preview when file panes have room. Quick Look has its own shortcut."
                        .into(),
                value: onoff(self.settings.preview),
                action: Action::Preview,
            },
        ];
        let defaults = crate::appearance::Appearance::default();
        for terminal in [false, true] {
            let requested = if terminal {
                &self.settings.appearance.terminal_font_family
            } else {
                &self.settings.appearance.font_family
            };
            let effective = if terminal {
                theme::terminal(cx).font_family
            } else {
                theme::ui_font(cx).to_string()
            };
            let title = if terminal {
                "Terminal font family"
            } else {
                "Interface font family"
            };
            rows.push(Row{section:"Appearance".into(),title:title.into(),description:format!("Effective: {effective}. Enter an installed font family; unavailable fonts use bundled JetBrains Mono. Terminal alignment works best with a monospaced face."),value:requested.clone(),action:Action::AppearanceFont(terminal)});
            let default = if terminal {
                &defaults.terminal_font_family
            } else {
                &defaults.font_family
            };
            if requested != default {
                rows.push(Row {
                    section: "Appearance".into(),
                    title: format!("Reset {title}"),
                    description: "Restore the bundled JetBrains Mono font.".into(),
                    value: "Reset".into(),
                    action: Action::AppearanceFontReset(terminal),
                });
            }
        }
        for (key, title, _) in crate::appearance::COLOR_TOKENS {
            let value = self.settings.appearance.color(key);
            rows.push(Row{section:"Appearance".into(),title:format!("Color / {title}"),description:format!("{key} · Enter a #RRGGBB value. Clear the field to restore the Tokyo Night default."),value:format!("#{value:06X}"),action:Action::AppearanceColor((*key).into())});
            if self.settings.appearance.colors.contains_key(*key) {
                rows.push(Row {
                    section: "Appearance".into(),
                    title: format!("Reset color / {title}"),
                    description: format!("Remove the {key} override."),
                    value: "Reset".into(),
                    action: Action::AppearanceColorReset((*key).into()),
                });
            }
        }
        rows.push(Row{section:"Appearance".into(),title:"Reset fonts and colors".into(),description:"Restore Tokyo Night, bundled fonts and the terminal ANSI palette; keep text size and other preferences.".into(),value:"Reset theme".into(),action:Action::AppearanceReset});
        for (index, bookmark) in self.settings.bookmarks.iter().enumerate() {
            rows.push(Row {
                section: "Bookmarks".into(),
                title: bookmark.name.clone(),
                description: bookmark.location.clone(),
                value: "Rename".into(),
                action: Action::BookmarkRename(index),
            });
            rows.push(Row {
                section: "Bookmarks".into(),
                title: format!("Remove {}", bookmark.name),
                description: "Remove this saved location; its files are kept.".into(),
                value: "Remove".into(),
                action: Action::BookmarkRemove(index),
            });
        }
        for plugin in &self.plugins {
            let enabled = self
                .settings
                .plugins
                .get(&plugin.id)
                .and_then(|p| p.enabled)
                .unwrap_or(true);
            rows.push(Row {
                section: "Plugins".into(),
                title: plugin.name.clone(),
                description: format!(
                    "{} · {}",
                    plugin.id,
                    if enabled {
                        "Enabled for new commands, search and decorations"
                    } else {
                        "Disabled; running terminals remain open"
                    }
                ),
                value: onoff(enabled),
                action: Action::PluginEnabled(plugin.id.clone()),
            });
            if plugin.supports(Capability::Accounts) && enabled {
                self.account_rows(plugin, &mut rows);
            }
            for field in &plugin.settings_fields {
                let value = plugin
                    .config
                    .get(&field.key)
                    .cloned()
                    .unwrap_or(Value::Null);
                rows.push(Row {
                    section: "Plugins".into(),
                    title: format!("{} / {}", plugin.name, field.title),
                    description: field.description.clone(),
                    value: display_value(&value),
                    action: Action::Field(plugin.id.clone(), Box::new(field.clone()), value),
                });
            }
        }
        let mut shortcuts = crate::keybindings::ACTIONS
            .iter()
            .map(|spec| {
                (
                    spec.id.to_owned(),
                    spec.title.to_owned(),
                    spec.defaults
                        .iter()
                        .map(|key| (*key).to_owned())
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        shortcuts.extend(self.plugin_shortcuts.iter().cloned());
        for id in self.settings.keybindings.keys() {
            if !shortcuts.iter().any(|(existing, _, _)| existing == id) {
                shortcuts.push((id.clone(), format!("Unavailable command · {id}"), vec![]));
            }
        }
        for (id, title, defaults) in shortcuts {
            let effective = self.settings.keybindings.get(&id).unwrap_or(&defaults);
            let scope = match crate::keybindings::spec(&id)
                .map(|spec| spec.scope)
                .unwrap_or(crate::keybindings::Scope::Files)
            {
                crate::keybindings::Scope::Files => "Files",
                crate::keybindings::Scope::Workspace => "Workspace",
                crate::keybindings::Scope::Global => "Global",
            };
            rows.push(Row{section:"Shortcuts".into(),title:title.clone(),description:format!("{scope} · {id} · Enter to record one shortcut. Alternatives can be edited in JSON. Default: {}",if defaults.is_empty(){"Unbound".to_owned()}else{defaults.join(" ")}),value:if effective.is_empty(){"Unbound".into()}else{effective.join(" ")},action:Action::Shortcut(id.clone())});
            if self.settings.keybindings.contains_key(&id) {
                rows.push(Row {
                    section: "Shortcuts".into(),
                    title: format!("Reset {title}"),
                    description: format!(
                        "{scope} · {id} · Remove this override and use the default bindings."
                    ),
                    value: "Reset".into(),
                    action: Action::ShortcutReset(id),
                });
            }
        }
        rows.extend([
            Row{section:"Advanced".into(),title:"Edit settings.json".into(),description:"Full JSON editor, including plugin settings without form metadata.".into(),value:if self.dirty{"Unsaved draft"}else{"Open editor"}.into(),action:Action::Json},
            Row{section:"Advanced".into(),title:"Reload from disk".into(),description:"Read external changes. Invalid files keep the last working preferences.".into(),value:"Reload".into(),action:Action::Reload},
            Row{section:"Advanced".into(),title:"Reset appearance and browsing".into(),description:"Restore core defaults; keep plugin configuration and other JSON keys.".into(),value:"Reset".into(),action:Action::Reset},
        ]);
        let query = self.search.read(cx).value().to_lowercase();
        rows.into_iter()
            .filter(|r| {
                (self.category == 0 || r.section == CATEGORY_NAMES[self.category])
                    && format!("{} {} {}", r.section, r.title, r.description)
                        .to_lowercase()
                        .contains(&query)
            })
            .collect()
    }
    fn account_rows(&self, plugin: &PluginManifest, rows: &mut Vec<Row>) {
        let presentation = self.accounts.get(&plugin.id);
        let state = presentation.and_then(|p| p.state.as_ref());
        let busy = presentation.is_some_and(|p| p.busy);
        let action = |action, account_id| {
            Action::Account(plugin.id.clone(), AccountRequest { action, account_id })
        };
        let active = state.and_then(|state| {
            state
                .accounts
                .iter()
                .find(|account| Some(&account.id) == state.active_account_id.as_ref())
        });
        rows.push(Row {
            section: "Plugins".into(),
            title: format!("{} / Account status", plugin.name),
            description: if !presentation.is_some_and(|status| status.error || status.busy) && state.is_some_and(|state| !state.using_plan && !state.sign_in_required) && active.is_some_and(|account| account.connected) {
                "The selected model provider is still active. Change Model provider to use this connection.".into()
            } else {
                presentation.map(|p| p.message.clone()).filter(|value| !value.is_empty()).unwrap_or_else(|| "Sign in or refresh the saved account connection.".into())
            },
            value: if busy {
                "Working…".into()
            } else {
                active
                    .map(|account| account.label.clone())
                    .unwrap_or_else(|| "Not connected".into())
            },
            action: action(AccountAction::Status, None),
        });
        if busy {
            rows.push(Row {
                section: "Plugins".into(),
                title: format!("{} / Cancel connection", plugin.name),
                description: "Stop this account operation and keep the current workspace.".into(),
                value: "Cancel".into(),
                action: Action::AccountCancel(plugin.id.clone()),
            });
            return;
        }
        let sign_in_label = state
            .and_then(|s| s.sign_in_label.clone())
            .unwrap_or_else(|| "Connect account".into());
        rows.push(Row { section: "Plugins".into(), title: format!("{} / {}", plugin.name, sign_in_label), description: if active.is_some() { "Connect another account or workspace in your browser.".into() } else { "Use an eligible account plan. API keys and local endpoints remain available in Model provider.".into() }, value: if active.is_some() { "Add account".into() } else { "Sign in".into() }, action: action(AccountAction::SignIn, None) });
        if let Some(state) = state {
            for account in &state.accounts {
                let selected = state.active_account_id.as_deref() == Some(account.id.as_str());
                rows.push(Row {
                    section: "Plugins".into(),
                    title: format!("{} / {}", plugin.name, account.label),
                    description: if account.plan_enabled {
                        "Plan usage is enabled for this registration.".into()
                    } else {
                        "Reconnect to authorize eligible plan usage.".into()
                    },
                    value: if selected && account.connected && account.plan_enabled {
                        "Active"
                    } else if account.connected && account.plan_enabled {
                        "Use account"
                    } else if account.connected {
                        "Enable plan usage"
                    } else {
                        "Reconnect"
                    }
                    .into(),
                    action: action(
                        if account.connected && account.plan_enabled {
                            AccountAction::Select
                        } else {
                            AccountAction::SignIn
                        },
                        Some(account.id.clone()),
                    ),
                });
            }
            if let Some(request) = crate::account_state::reconnect_active(state) {
                rows.push(Row {
                    section: "Plugins".into(),
                    title: format!(
                        "{} / Reconnect {}",
                        plugin.name,
                        active.map(|account| account.label.as_str()).unwrap_or("account")
                    ),
                    description: "Reauthorize this saved account in your browser if a request asks you to sign in again.".into(),
                    value: "Reconnect".into(),
                    action: Action::Account(plugin.id.clone(), request),
                });
            }
            if let Some(active) = active.filter(|account| account.connected) {
                rows.push(Row {
                    section: "Plugins".into(),
                    title: format!("{} / Sign out", plugin.name),
                    description: format!("Disconnect {} on this installation.", active.label),
                    value: "Sign out".into(),
                    action: action(AccountAction::SignOut, Some(active.id.clone())),
                });
            }
            if let Some(url) = &state.usage_url {
                rows.push(Row {
                    section: "Plugins".into(),
                    title: format!("{} / Manage usage", plugin.name),
                    description: "Review plan limits and app access with your provider.".into(),
                    value: "Open in browser".into(),
                    action: Action::AccountUsage(url.clone()),
                });
            }
            if (state.using_plan || state.sign_in_required)
                && active.is_some_and(|account| account.connected && account.plan_enabled)
            {
                rows.push(Row { section: "Plugins".into(), title: format!("{} / Available models", plugin.name), description: "Refresh the models available to the active account. Select a row below to use it.".into(), value: "Refresh models".into(), action: action(AccountAction::Models, None) });
                let selected_model = plugin
                    .config
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                rows.push(Row {
                    section: "Plugins".into(),
                    title: format!("{} / Automatic model", plugin.name),
                    description: "Use the first model available to the active account.".into(),
                    value: if selected_model.is_empty() {
                        "Selected"
                    } else {
                        "Use automatic"
                    }
                    .into(),
                    action: Action::AccountModel(plugin.id.clone(), String::new()),
                });
                for (index, model) in state.models.iter().enumerate() {
                    rows.push(Row {
                        section: "Plugins".into(),
                        title: format!("{} / {}", plugin.name, model.name),
                        description: model.id.clone(),
                        value: if model.id == selected_model {
                            "Selected"
                        } else if selected_model.is_empty() && index == 0 {
                            "Automatic"
                        } else {
                            "Use model"
                        }
                        .into(),
                        action: Action::AccountModel(plugin.id.clone(), model.id.clone()),
                    });
                }
            }
        }
    }
    fn row_disabled(&self, row: &Row) -> bool {
        matches!(&row.action, Action::Account(plugin, _) if self.accounts.get(plugin).is_some_and(|status| status.busy))
            || matches!(row.action, Action::FontUp) && self.settings.font_size >= 24.
            || matches!(row.action, Action::FontDown) && self.settings.font_size <= 9.
    }
    fn cycle_category(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.category =
            (self.category as isize + delta).rem_euclid(CATEGORY_NAMES.len() as isize) as usize;
        self.selected = 1;
        self.editing = None;
        self.focus.focus(window);
        self.scroll.scroll_to_item(0);
        cx.notify();
    }
    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected != index {
            self.editing = None;
        }
        self.selected = index;
        if self.account_confirmation.take().is_some() {
            self.focus.focus(window);
            self.selected = 0;
            cx.notify();
            return;
        }
        if self.close_pending || self.reload_pending {
            self.confirm(index, window, cx);
            return;
        }
        if self.json_open {
            match index {
                0 => self.editor.focus_handle(cx).focus(window),
                1 => self.save_json(cx),
                2 => {
                    self.json_open = false;
                    self.focus.focus(window);
                }
                3 => self.request_reload(window, cx),
                _ => self.request_close(window, cx),
            };
            return;
        }
        if index == 0 {
            self.search.focus_handle(cx).focus(window);
            return;
        }
        if index == 1 {
            self.cycle_category(1, window, cx);
            return;
        }
        let rows = self.rows(cx);
        let Some(row) = rows.get(index - 2) else {
            self.request_close(window, cx);
            return;
        };
        if self.row_disabled(row) {
            self.message = if matches!(row.action, Action::Account(..)) {
                "An account operation is in progress. Choose Cancel connection to stop it."
            } else {
                "Text size is at its limit · Reset restores 13pt"
            }
            .into();
            self.error = false;
            cx.notify();
            return;
        }
        match row.action.clone() {
            Action::Account(plugin, request) => cx.emit(SettingsEvent::Account { plugin, request }),
            Action::AccountCancel(plugin) => cx.emit(SettingsEvent::CancelAccount(plugin)),
            Action::AccountUsage(url) => cx.emit(SettingsEvent::AccountUsage(url)),
            Action::AccountModel(plugin, model) => cx.emit(SettingsEvent::Change(
                Change::PluginField(plugin, "model".into(), Some(Value::String(model))),
            )),
            Action::AppearanceFont(terminal) => {
                let value = if terminal {
                    self.settings.appearance.terminal_font_family.clone()
                } else {
                    self.settings.appearance.font_family.clone()
                };
                self.field_input
                    .update(cx, |input, cx| input.set_value(value, window, cx));
                self.editing = Some(Edit::AppearanceFont(terminal));
                self.field_input.focus_handle(cx).focus(window);
            }
            Action::AppearanceFontReset(terminal) => {
                cx.emit(SettingsEvent::Change(Change::AppearanceFont {
                    terminal,
                    value: theme::BUNDLED_FONT.into(),
                }))
            }
            Action::AppearanceColor(key) => {
                let value = format!("#{:06X}", self.settings.appearance.color(&key));
                self.field_input
                    .update(cx, |input, cx| input.set_value(value, window, cx));
                self.editing = Some(Edit::AppearanceColor(key));
                self.field_input.focus_handle(cx).focus(window);
            }
            Action::AppearanceColorReset(key) => {
                cx.emit(SettingsEvent::Change(Change::AppearanceColor(key, None)))
            }
            Action::AppearanceReset => cx.emit(SettingsEvent::Change(Change::AppearanceReset)),
            Action::Shortcut(id) => {
                self.editing = None;
                self.recording = Some(Recording {
                    id,
                    input: Recorder::new(self.activation_key.as_ref()),
                });
                self.recording_saving = false;
                self.error = false;
                self.message="Press a shortcut, then Tab to Apply and Enter. One recorded chord replaces its alternatives.".into();
                self.recorder_focus.focus(window);
            }
            Action::ShortcutReset(id) => {
                cx.emit(SettingsEvent::Change(Change::Keybinding(id, None)))
            }
            Action::FontDown => cx.emit(SettingsEvent::Change(Change::Font(
                (self.settings.font_size - 1.).max(9.),
            ))),
            Action::FontUp => cx.emit(SettingsEvent::Change(Change::Font(
                (self.settings.font_size + 1.).min(24.),
            ))),
            Action::FontReset => cx.emit(SettingsEvent::Change(Change::Font(13.))),
            Action::Hidden => cx.emit(SettingsEvent::Change(Change::Hidden(
                !self.settings.show_hidden,
            ))),
            Action::Sidebar => cx.emit(SettingsEvent::Change(Change::Sidebar(
                !self.settings.sidebar,
            ))),
            Action::Preview => cx.emit(SettingsEvent::Change(Change::Preview(
                !self.settings.preview,
            ))),
            Action::BookmarkRename(index) => {
                if let Some(bookmark) = self.settings.bookmarks.get(index) {
                    self.field_input.update(cx, |input, cx| {
                        input.set_value(bookmark.name.clone(), window, cx)
                    });
                    self.editing = Some(Edit::Bookmark(index));
                    self.field_input.focus_handle(cx).focus(window);
                }
            }
            Action::BookmarkRemove(index) => {
                cx.emit(SettingsEvent::Change(Change::RemoveBookmark(index)))
            }
            Action::PluginEnabled(id) => {
                let enabled = self
                    .settings
                    .plugins
                    .get(&id)
                    .and_then(|p| p.enabled)
                    .unwrap_or(true);
                cx.emit(SettingsEvent::Change(Change::PluginEnabled(id, !enabled)));
            }
            Action::Field(id, field, value) => match field.kind {
                SettingKind::Bool => cx.emit(SettingsEvent::Change(Change::PluginField(
                    id,
                    field.key,
                    Some(Value::Bool(!value.as_bool().unwrap_or(false))),
                ))),
                SettingKind::Select => {
                    if !field.options.is_empty() {
                        let next = field
                            .options
                            .iter()
                            .position(|v| Some(v.as_str()) == value.as_str())
                            .map_or(0, |i| (i + 1) % field.options.len());
                        cx.emit(SettingsEvent::Change(Change::PluginField(
                            id,
                            field.key,
                            Some(Value::String(field.options[next].clone())),
                        )));
                    }
                }
                _ => {
                    self.field_input.update(cx, |input, cx| {
                        input.set_value(
                            if value.is_null() {
                                String::new()
                            } else {
                                display_value(&value)
                            },
                            window,
                            cx,
                        )
                    });
                    self.editing = Some(Edit::Plugin {
                        plugin: id,
                        field: *field,
                    });
                    self.field_input.focus_handle(cx).focus(window);
                }
            },
            Action::Json => {
                self.json_open = true;
                self.selected = 0;
                self.editor.focus_handle(cx).focus(window);
            }
            Action::Reload => self.request_reload(window, cx),
            Action::Reset => cx.emit(SettingsEvent::Change(Change::Reset)),
        }
        cx.notify();
    }
    fn commit_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.appearance_saving {
            return;
        }
        let Some(edit) = self.editing.as_ref() else {
            return;
        };
        let text = self.field_input.read(cx).value().to_string();
        if matches!(edit, Edit::AppearanceFont(_) | Edit::AppearanceColor(_)) {
            let mut appearance = self.settings.appearance.clone();
            let change = match edit {
                Edit::AppearanceFont(terminal) => {
                    let value = if text.trim().is_empty() {
                        theme::BUNDLED_FONT.to_owned()
                    } else {
                        text.trim().to_owned()
                    };
                    if *terminal {
                        appearance.terminal_font_family = value.clone();
                    } else {
                        appearance.font_family = value.clone();
                    }
                    Change::AppearanceFont {
                        terminal: *terminal,
                        value,
                    }
                }
                Edit::AppearanceColor(key) => {
                    let value = (!text.trim().is_empty()).then(|| text.trim().to_owned());
                    if let Some(value) = &value {
                        appearance.colors.insert(key.clone(), value.clone());
                    } else {
                        appearance.colors.remove(key);
                    }
                    Change::AppearanceColor(key.clone(), value)
                }
                _ => unreachable!(),
            };
            if let Err(error) = appearance.validate() {
                self.warning(format!("Appearance not saved: {error:#}"), cx);
                return;
            }
            self.appearance_saving = true;
            cx.emit(SettingsEvent::Change(change));
            return;
        }
        if let Edit::Bookmark(index) = edit {
            cx.emit(SettingsEvent::Change(Change::RenameBookmark(*index, text)));
            self.editing = None;
            self.focus.focus(window);
            cx.notify();
            return;
        }
        let Edit::Plugin { plugin, field } = edit else {
            return;
        };
        let value = if text.is_empty() {
            None
        } else if field.kind == SettingKind::Number {
            match text
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
            {
                Some(n) => Some(Value::Number(n)),
                None => {
                    self.warning(
                        "Enter a valid finite number, or clear the field to use its default."
                            .into(),
                        cx,
                    );
                    return;
                }
            }
        } else {
            Some(Value::String(text))
        };
        cx.emit(SettingsEvent::Change(Change::PluginField(
            plugin.clone(),
            field.key.clone(),
            value,
        )));
        self.editing = None;
        self.focus.focus(window);
        cx.notify();
    }
    fn save_json(&mut self, cx: &mut Context<Self>) {
        cx.emit(SettingsEvent::SaveJson {
            source: self.editor.read(cx).value().to_string(),
            base: self.draft_base.clone(),
        });
    }
    fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.close_pending && index == 0 {
            self.close_after_save = true;
            self.save_json(cx);
            return;
        }
        let discard = if self.close_pending {
            index == 1
        } else {
            index == 0
        };
        if discard {
            self.dirty = false;
            self.draft_base = self.raw.clone();
            self.editor
                .update(cx, |i, cx| i.set_value(self.raw.clone(), window, cx));
            if self.close_pending {
                self.close_pending = false;
                cx.emit(SettingsEvent::Close);
            } else {
                self.reload_pending = false;
                cx.emit(SettingsEvent::Reload);
            }
        } else {
            self.close_pending = false;
            self.reload_pending = false;
            self.exit_requested = false;
            cx.emit(SettingsEvent::CancelClose);
            self.json_open = true;
            self.editor.focus_handle(cx).focus(window);
        }
        cx.notify();
    }
    pub fn cancel_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_recording(Some(window), cx);
    }
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }
    fn finish_recording(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        if let Some(recording) = self.recording.take() {
            self.recording_held.extend(recording.input.held_keys());
        }
        self.recording_saving = false;
        if let Some(window) = window {
            self.focus.focus(window);
        }
        cx.notify();
    }
    fn recording_error(&self) -> Option<String> {
        let recording = self.recording.as_ref()?;
        if let Some(error) = recording.input.error() {
            return Some(error.to_owned());
        }
        let chord = recording.input.chord()?;
        let mut candidate = self.settings.keybindings.clone();
        candidate.insert(recording.id.clone(), vec![chord.to_owned()]);
        crate::keybindings::validate(&candidate)
            .err()
            .map(|error| error.to_string())
    }
    fn intercept_recording(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.recording.is_none() {
            if self.recording_held.contains(&key_identity(stroke)) {
                cx.stop_propagation();
            }
            return;
        }
        if !self.recorder_focus.is_focused(window) || !window.is_window_active() {
            self.finish_recording(None, cx);
            self.recording_held.clear();
            return;
        }
        cx.stop_propagation();
        if self.recording_saving {
            return;
        }
        let outcome = self.recording.as_mut().unwrap().input.key_down(stroke);
        match outcome {
            Outcome::Apply => self.apply_recording(false, window, cx),
            Outcome::Clear => self.apply_recording(true, window, cx),
            Outcome::Cancel => {
                self.finish_recording(Some(window), cx);
                self.message = "Shortcut recording cancelled; saved bindings are unchanged.".into();
                self.error = false;
            }
            Outcome::Updated => {
                if let Some(error) = self.recording_error() {
                    self.message = format!("Shortcut not saved: {error}");
                    self.error = true;
                } else {
                    self.message="Tab / Shift-Tab chooses Capture, Apply, Clear or Cancel. Enter activates a chosen action.".into();
                    self.error = false;
                }
            }
            Outcome::Ignored => {}
        }
        cx.notify();
        cx.stop_propagation();
    }
    fn apply_recording(&mut self, clear: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if self.recording_saving {
            return;
        }
        let Some(recording) = &self.recording else {
            return;
        };
        let id = recording.id.clone();
        let keys = if clear {
            vec![]
        } else {
            let Some(chord) = recording.input.chord() else {
                self.warning(
                    "Press a shortcut before choosing Apply, or choose Clear to disable it.".into(),
                    cx,
                );
                return;
            };
            vec![chord.to_owned()]
        };
        let mut candidate = self.settings.keybindings.clone();
        candidate.insert(id.clone(), keys.clone());
        if let Err(error) = crate::keybindings::validate(&candidate) {
            self.warning(format!("Shortcut not saved: {error:#}"), cx);
            return;
        }
        self.recording_saving = true;
        self.message = "Saving shortcut…".into();
        self.error = false;
        cx.emit(SettingsEvent::Change(Change::Keybinding(id, Some(keys))));
        cx.notify();
    }
    fn recording_button(
        &self,
        label: &'static str,
        index: usize,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let colors = theme::palette(cx);
        let selected = self
            .recording
            .as_ref()
            .is_some_and(|recording| recording.input.control() == index);
        let scroll = self.scroll.clone();
        div()
            .relative()
            .id(("recorder-action", index))
            .px_3()
            .py_2()
            .border_1()
            .rounded_md()
            .border_color(rgb(if selected {
                colors.accent
            } else {
                colors.border
            }))
            .bg(rgb(if selected {
                colors.border
            } else {
                colors.surface
            }))
            .cursor_pointer()
            .child(label)
            .when(selected, |el| {
                el.child(
                    canvas(
                        move |bounds, window, cx| {
                            let viewport = scroll.bounds();
                            let mut offset = scroll.offset();
                            let shift = if bounds.top() < viewport.top() {
                                viewport.top() - bounds.top()
                            } else if bounds.bottom() > viewport.bottom() {
                                viewport.bottom() - bounds.bottom()
                            } else {
                                px(0.)
                            };
                            if shift.abs() > px(0.5) {
                                offset.y += shift;
                                let scroll = scroll.clone();
                                window.defer(cx, move |_, cx| {
                                    scroll.set_offset(offset);
                                    cx.refresh_windows();
                                });
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if this.recording_saving {
                    return;
                }
                this.recorder_focus.focus(window);
                if let Some(recording) = &mut this.recording {
                    recording.input.set_control(index);
                }
                match index {
                    1 => this.apply_recording(false, window, cx),
                    2 => this.apply_recording(true, window, cx),
                    3 => {
                        this.finish_recording(Some(window), cx);
                        this.message =
                            "Shortcut recording cancelled; saved bindings are unchanged.".into();
                        this.error = false;
                    }
                    _ => {
                        this.message = "Press the replacement shortcut. Tab moves to Apply.".into();
                        this.error = false;
                    }
                }
                cx.notify();
            }))
    }
    fn render_field_editor(&self) -> Div {
        let scroll = self.scroll.clone();
        div()
            .relative()
            .mt_3()
            .child(Input::new(&self.field_input))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let viewport = scroll.bounds();
                        let mut offset = scroll.offset();
                        let shift = if bounds.top() < viewport.top() {
                            viewport.top() - bounds.top()
                        } else if bounds.bottom() > viewport.bottom() {
                            viewport.bottom() - bounds.bottom()
                        } else {
                            px(0.)
                        };
                        if shift.abs() > px(0.5) {
                            offset.y += shift;
                            let scroll = scroll.clone();
                            window.defer(cx, move |_, cx| {
                                scroll.set_offset(offset);
                                cx.refresh_windows();
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
    }
    fn render_recorder(&self, cx: &Context<Self>) -> AnyElement {
        let colors = theme::palette(cx);
        let Some(recording) = &self.recording else {
            return div().into_any_element();
        };
        let preview = recording
            .input
            .chord()
            .map(|chord| crate::display_bindings(&[chord.to_owned()]))
            .unwrap_or_else(|| "Press shortcut".into());
        div().id("shortcut-recorder").track_focus(&self.recorder_focus).mt_3().p_3().border_1().border_color(rgb(colors.accent)).rounded_md().bg(rgb(colors.chrome)).on_click(cx.listener(|_,_,_,cx|cx.stop_propagation()))
            .child(div().flex().flex_wrap().items_center().gap_3().child(div().text_color(rgb(colors.accent)).child(preview)).child(div().text_size(ui(11.)).text_color(rgb(colors.muted)).child(if self.recording_saving{"Saving…"}else{"Not saved · replaces all alternatives"})))
            .child(div().mt_3().flex().flex_wrap().gap_2().child(self.recording_button("Capture",0,cx)).child(self.recording_button("Apply",1,cx)).child(self.recording_button("Clear",2,cx)).child(self.recording_button("Cancel",3,cx)))
            .child(div().mt_2().text_size(ui(11.)).line_height(ui(17.)).text_color(rgb(colors.muted)).whitespace_normal().child("Press a chord on Capture. Tab to Apply, then Enter to save. Clear disables; Escape cancels. Use JSON for alternatives."))
            .into_any_element()
    }
    fn move_focus(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.commit_field(window, cx);
            if self.editing.is_some() {
                return;
            }
        }
        let count = if self.account_confirmation.is_some() {
            1
        } else if self.close_pending {
            3
        } else if self.reload_pending {
            2
        } else if self.json_open {
            5
        } else {
            self.rows(cx).len() + 3
        };
        self.selected = (self.selected as isize + delta).rem_euclid(count as isize) as usize;
        self.focus.focus(window);
        if !self.close_pending && !self.reload_pending && self.selected == 0 {
            if self.json_open {
                self.editor.focus_handle(cx).focus(window);
            } else {
                self.search.focus_handle(cx).focus(window);
            }
        }
        if !self.json_open && self.selected > 1 {
            self.scroll.scroll_to_item(self.selected - 2);
        }
        cx.notify();
    }
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = event.keystroke.key.as_str();
        let m = event.keystroke.modifiers;
        if self.account_confirmation.is_some() {
            if matches!(k, "enter" | "space" | "escape") && !event.is_held {
                self.account_confirmation = None;
                self.selected = 0;
                self.focus.focus(window);
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        // Settings uses Tab for control navigation, including in the multiline
        // JSON editor. Capture before the input's indentation handling.
        if k == "tab" && !m.platform && !m.control && !m.alt {
            self.move_focus(if m.shift { -1 } else { 1 }, window, cx);
            cx.stop_propagation();
            return;
        }
        if m.platform && k == "s" {
            self.save_json(cx);
            cx.stop_propagation();
            return;
        }
        if k == "escape" {
            if self.appearance_saving {
                cx.stop_propagation();
                return;
            }
            if self.editing.take().is_some() {
                self.focus.focus(window);
            } else if self.close_pending || self.reload_pending {
                self.close_pending = false;
                self.reload_pending = false;
                self.exit_requested = false;
                cx.emit(SettingsEvent::CancelClose);
                self.json_open = true;
                self.selected = 0;
                self.editor.focus_handle(cx).focus(window);
            } else {
                self.request_close(window, cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.focus.is_focused(window)
            && !self.json_open
            && !self.close_pending
            && !self.reload_pending
            && self.selected == 1
            && matches!(k, "left" | "right")
        {
            self.cycle_category(if k == "left" { -1 } else { 1 }, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.focus.is_focused(window) && !m.platform {
            match k {
                "enter" | "space" => {
                    if event.is_held {
                        return;
                    }
                    self.activation_key = Some(event.keystroke.clone());
                    self.activate(self.selected, window, cx);
                    self.activation_key = None;
                }
                "down" => self.move_focus(1, window, cx),
                "up" => self.move_focus(-1, window, cx),
                "home" => {
                    self.selected = 0;
                    self.move_focus(0, window, cx);
                }
                "end" => {
                    self.selected = if self.json_open {
                        4
                    } else {
                        self.rows(cx).len() + 2
                    };
                    self.move_focus(0, window, cx);
                }
                _ => return,
            }
            cx.stop_propagation();
        }
    }
    fn button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        index: usize,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let colors = theme::palette(cx);
        div()
            .id(id)
            .px_3()
            .py_2()
            .border_1()
            .border_color(rgb(if self.selected == index {
                colors.accent
            } else {
                colors.border
            }))
            .rounded_md()
            .bg(rgb(colors.surface))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(colors.border)))
            .child(label.into())
            .on_click(cx.listener(move |this, _, w, cx| this.activate(index, w, cx)))
    }
}
impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::palette(cx);
        let font_notice = theme::font_notice(cx);
        let message = font_notice.as_ref().map_or_else(
            || self.message.clone(),
            |notice| format!("{} · {notice}", self.message),
        );
        let _ = &self.subscriptions;
        let confirming = self.close_pending || self.reload_pending;
        let content = if let Some((title, message)) = &self.account_confirmation {
            div()
                .id("account-confirm-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_6()
                .child(
                    div()
                        .text_size(ui(18.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.clone()),
                )
                .child(
                    div()
                        .mt_3()
                        .max_w(ui(650.))
                        .whitespace_normal()
                        .child(message.clone()),
                )
                .child(
                    div()
                        .mt_5()
                        .child(self.button("account-confirm", "Got it", 0, cx)),
                )
                .into_any_element()
        } else if confirming {
            let mut actions = div().flex().flex_wrap().gap_3();
            if self.close_pending {
                actions = actions
                    .child(self.button(
                        "save-close",
                        if self.exit_requested {
                            "Save and quit"
                        } else {
                            "Save and close"
                        },
                        0,
                        cx,
                    ))
                    .child(self.button("discard-close", "Discard draft", 1, cx))
                    .child(self.button("keep-edit", "Keep editing", 2, cx));
            } else {
                actions = actions
                    .child(self.button("discard-reload", "Discard and reload", 0, cx))
                    .child(self.button("keep-reload", "Keep editing", 1, cx));
            }
            div()
                .p_6()
                .flex_1()
                .flex()
                .flex_col()
                .gap_5()
                .child(div().text_size(ui(18.)).child("Unsaved settings.json"))
                .child("Your saved preferences stay active until this draft is saved.")
                .child(actions)
                .into_any_element()
        } else if self.json_open {
            div().flex_1().min_h_0().flex().flex_col().gap_3().p_4()
                .child(div().flex().flex_wrap().items_center().gap_3().child(self.button("json-save","Save JSON · ⌘S",1,cx)).child(self.button("json-form","Back to form",2,cx)).child(self.button("json-reload","Reload from disk",3,cx)).child(self.button("json-close","Close settings",4,cx)).child(div().text_color(rgb(if self.dirty{colors.accent}else{colors.muted})).child(if self.dirty{"Unsaved draft"}else{"Saved"})))
                .child(div().flex_1().min_h_0().child(Input::new(&self.editor).h_full()))
                .child(div().text_size(ui(11.)).text_color(rgb(colors.muted)).child("Strict JSON · Tab moves to actions · External changes are checked before saving"))
                .into_any_element()
        } else {
            let rows = self.rows(cx);
            let mut previous = String::new();
            let body = div()
                .id("settings-scroll")
                .track_scroll(&self.scroll)
                .overflow_y_scroll()
                .flex_1()
                .min_h_0()
                .px_4()
                .pb_4()
                .children(rows.iter().enumerate().map(|(i, row)| {
                    let disabled = self.row_disabled(row);
                    let heading = previous != row.section;
                    previous = row.section.clone();
                    let mut item = div()
                        .id(("setting", i))
                        .px_3()
                        .py_3()
                        .mb_1()
                        .border_1()
                        .border_color(rgb(if self.selected == i + 2 {
                            colors.accent
                        } else {
                            colors.bg
                        }))
                        .rounded_md()
                        .bg(rgb(if self.selected == i + 2 {
                            colors.surface
                        } else {
                            colors.bg
                        }))
                        .when(!disabled, |el| el.cursor_pointer())
                        .hover(|s| s.bg(rgb(colors.surface)))
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.focus.focus(w);
                            this.activate(i + 2, w, cx);
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(div().flex_1().min_w_0().child(row.title.clone()))
                                .child(
                                    div()
                                        .text_color(rgb(if disabled {
                                            colors.muted
                                        } else {
                                            colors.success
                                        }))
                                        .max_w(ui(260.))
                                        .truncate()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .when_some(
                                                    if let Action::AppearanceColor(key) =
                                                        &row.action
                                                    {
                                                        Some(self.settings.appearance.color(key))
                                                    } else {
                                                        None
                                                    },
                                                    |el, color| {
                                                        el.child(
                                                            div()
                                                                .size(ui(14.))
                                                                .flex_shrink_0()
                                                                .border_1()
                                                                .border_color(rgb(colors.border))
                                                                .bg(rgb(color)),
                                                        )
                                                    },
                                                )
                                                .child(row.value.clone()),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .mt_1()
                                .text_size(ui(11.))
                                .line_height(ui(17.))
                                .text_color(rgb(colors.muted))
                                .whitespace_normal()
                                .child(row.description.clone()),
                        );
                    if self.recording.as_ref().is_some_and(
                        |recording| matches!(&row.action,Action::Shortcut(id) if id==&recording.id),
                    ) {
                        item = item.child(self.render_recorder(cx));
                    }
                    if self.editing.is_some() && self.selected == i + 2 {
                        item = item.child(self.render_field_editor()).child(
                            div()
                                .mt_2()
                                .text_size(ui(11.))
                                .text_color(rgb(colors.muted))
                                .child(if matches!(self.editing, Some(Edit::Bookmark(_))) {
                                    "Enter to save the name · Escape to cancel"
                                } else {
                                    "Enter to apply · Escape to cancel · Empty restores defaults"
                                }),
                        );
                    }
                    div()
                        .when(heading, |d| {
                            d.child(
                                div()
                                    .pt_5()
                                    .pb_3()
                                    .px_3()
                                    .text_size(ui(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(row.section.clone()),
                            )
                        })
                        .child(item)
                }))
                .when(rows.is_empty(), |d| {
                    d.child(div().p_6().text_color(rgb(colors.muted)).child(
                        if self.category == 3 && self.search.read(cx).value().is_empty() {
                            "No bookmarks yet. Save a location from the Go menu or command palette."
                        } else {
                            "No settings match this category and search."
                        },
                    ))
                })
                .child(div().mt_4().child(self.button(
                    "settings-close",
                    "Return to workspace · Esc",
                    rows.len() + 2,
                    cx,
                )));
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_4()
                        .pb_1()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().min_w_0().child(Input::new(&self.search)))
                        .child(self.button(
                            "settings-category",
                            format!("Category: {}", CATEGORY_NAMES[self.category]),
                            1,
                            cx,
                        )),
                )
                .child(body)
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(colors.bg))
            .text_color(rgb(colors.text))
            .font_family(theme::ui_font(cx))
            .text_size(ui(12.))
            .track_focus(&self.focus)
            .key_context("BucketsSettings")
            .capture_key_down(cx.listener(Self::on_key))
            .capture_key_up(cx.listener(|this, event: &KeyUpEvent, _, _| {
                this.recording_held.remove(&key_identity(&event.keystroke));
                if let Some(recording) = &mut this.recording {
                    recording.input.key_up(&event.keystroke);
                }
            }))
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::IndentInline, w, cx| {
                    this.move_focus(1, w, cx);
                    cx.stop_propagation();
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::OutdentInline, w, cx| {
                    this.move_focus(-1, w, cx);
                    cx.stop_propagation();
                }),
            )
            .capture_action(cx.listener(|this, _: &NextControl, w, cx| {
                this.move_focus(1, w, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &PreviousControl, w, cx| {
                this.move_focus(-1, w, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &SaveJson, _, cx| {
                this.save_json(cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .px_5()
                    .py_4()
                    .bg(rgb(colors.chrome))
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(ui(22.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Settings"),
                            )
                            .child(key(
                                crate::binding_label(&self.settings.keybindings, "app.settings"),
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_size(ui(11.))
                            .text_color(rgb(colors.muted))
                            .whitespace_normal()
                            .child(self.path.display().to_string()),
                    ),
            )
            .child(content)
            .child(
                div()
                    .flex_shrink_0()
                    .bg(rgb(colors.chrome))
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .text_size(ui(11.))
                    .text_color(rgb(if self.error {
                        colors.danger
                    } else if font_notice.is_some() {
                        colors.warning
                    } else {
                        colors.muted
                    }))
                    .whitespace_normal()
                    .child(message)
                    .child(
                        div().mt_1().text_color(rgb(colors.muted)).child(
                            "Tab / ⇧Tab Focus    Enter Change    ⌘S Save JSON    Esc Return",
                        ),
                    ),
            )
    }
}
fn onoff(value: bool) -> String {
    if value { "On" } else { "Off" }.into()
}
fn display_value(value: &Value) -> String {
    match value {
        Value::Null => "Default".into(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}
