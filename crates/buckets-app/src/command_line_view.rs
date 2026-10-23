//! The bottom command line adapts typed commands to existing application services.
use crate::{
    Browser, Command, Pane, command_id,
    command_line::{self, CommandSpec, Completion, HistoryStore, Invocation},
    session_path, theme, ui,
};
use buckets_core::Location;
use gpui::{prelude::*, *};
use gpui_component::input::Input;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

actions!(
    command_bar,
    [
        Complete,
        CompletePrevious,
        HistoryPrevious,
        HistoryNext,
        Submit,
        Dismiss
    ]
);

pub(crate) fn plugin_message(value: &serde_json::Value) -> String {
    value
        .as_str()
        .or_else(|| value.get("message").and_then(serde_json::Value::as_str))
        .unwrap_or("Plugin command complete")
        .chars()
        .take(4096)
        .collect()
}
fn matching_history(entries: &[String], draft: &str) -> Vec<usize> {
    let draft = draft.to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.to_lowercase().contains(&draft).then_some(index))
        .collect()
}
fn history_step(index: Option<usize>, previous: bool, len: usize) -> Option<usize> {
    let last = len.checked_sub(1)?;
    if previous {
        Some(index.map_or(last, |index| index.saturating_sub(1)))
    } else {
        index.and_then(|index| (index < last).then_some(index + 1))
    }
}
fn consume_programmatic_change(pending: &mut usize) -> bool {
    if *pending == 0 {
        return false;
    }
    *pending -= 1;
    true
}
fn frozen_selection_matches(pane: &Pane, origin: &Origin) -> bool {
    pane.chosen()
        .iter()
        .map(Location::display)
        .collect::<Vec<_>>()
        == origin.selection
        && pane.entries.get(pane.cursor).map(|entry| &entry.location) == origin.cursor.as_ref()
}
fn cached_paths(
    entries: &[buckets_core::Entry],
    prefix: &str,
    directories: bool,
) -> Vec<(String, String)> {
    let prefix = prefix.to_lowercase();
    entries
        .iter()
        .filter(|entry| {
            (!directories || entry.is_dir) && entry.name.to_lowercase().starts_with(&prefix)
        })
        .take(12)
        .map(|entry| {
            (
                format!("{}{}", entry.name, if entry.is_dir { "/" } else { "" }),
                "Current directory".into(),
            )
        })
        .collect()
}
fn completion_window(index: usize, compact: bool) -> (usize, usize) {
    let size = if compact { 2 } else { 4 };
    (index / size * size, size)
}

pub(crate) fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("tab", Complete, Some("BucketsCommandLine")),
        KeyBinding::new("shift-tab", CompletePrevious, Some("BucketsCommandLine")),
        KeyBinding::new("up", HistoryPrevious, Some("BucketsCommandLine")),
        KeyBinding::new("down", HistoryNext, Some("BucketsCommandLine")),
        KeyBinding::new("enter", Submit, Some("BucketsCommandLine")),
        KeyBinding::new("escape", Dismiss, Some("BucketsCommandLine")),
    ]
}

#[derive(Clone)]
pub(crate) struct Origin {
    pub(crate) pane: u64,
    pub(crate) location: Location,
    pub(crate) workspace_revision: u64,
    pub(crate) plugin_generation: u64,
    pub(crate) selection: Vec<String>,
    cursor: Option<Location>,
}

pub(crate) struct CommandLineUi {
    pub(crate) open: bool,
    origin: Option<Origin>,
    specs: Vec<CommandSpec>,
    candidates: Vec<Completion>,
    completion_seed: Option<String>,
    completion_index: usize,
    history: Option<HistoryStore>,
    entries: Vec<String>,
    history_queue: VecDeque<String>,
    history_saving: bool,
    history_index: Option<usize>,
    history_matches: Vec<usize>,
    scratch: String,
    pub(crate) pending: Option<Arc<AtomicBool>>,
    pub(crate) error: Option<String>,
    history_warning: Option<String>,
    programmatic_changes: usize,
    pub(crate) pending_history: Option<String>,
}
impl CommandLineUi {
    pub(crate) fn new() -> Self {
        let loaded = HistoryStore::load(session_path().with_file_name("command-history.json"));
        let (history, entries, history_warning) = match loaded {
            Ok(store) => {
                let entries = store.entries().to_vec();
                (Some(store), entries, None)
            }
            Err(error) => (
                None,
                vec![],
                Some(format!("History unavailable: {error:#}")),
            ),
        };
        Self {
            open: false,
            origin: None,
            specs: vec![],
            candidates: vec![],
            completion_seed: None,
            completion_index: 0,
            history,
            entries,
            history_queue: VecDeque::new(),
            history_saving: false,
            history_index: None,
            history_matches: vec![],
            scratch: String::new(),
            pending: None,
            error: None,
            history_warning,
            programmatic_changes: 0,
            pending_history: None,
        }
    }
}

impl Browser {
    pub(crate) fn frozen_command_origin(&self) -> Origin {
        let pane = &self.panes[self.active];
        Origin {
            pane: pane.id,
            location: pane.location.clone(),
            workspace_revision: self.workspace_revision,
            plugin_generation: self.plugin_generation,
            selection: pane.chosen().iter().map(Location::display).collect(),
            cursor: pane
                .entries
                .get(pane.cursor)
                .map(|entry| entry.location.clone()),
        }
    }
    pub(crate) fn refresh_command_specs(&mut self) {
        let mut specs = command_line::builtins();
        for (command, label, _) in crate::COMMANDS {
            if let Some(id) = command_id(*command) {
                specs.push(CommandSpec {
                    name: id.into(),
                    aliases: vec![],
                    usage: id.into(),
                    description: (*label).into(),
                });
            }
        }
        for command in &self.plugin_commands {
            let mut aliases = vec![];
            let mut arguments = String::new();
            if let Some(cli) = &command.descriptor.command_line {
                aliases.push(cli.name.clone());
                aliases.extend(cli.aliases.clone());
                for argument in &cli.arguments {
                    arguments.push(' ');
                    arguments.push(if argument.required { '<' } else { '[' });
                    arguments.push_str(&argument.name);
                    if argument.variadic {
                        arguments.push_str("...");
                    }
                    arguments.push(if argument.required { '>' } else { ']' });
                }
            }
            let name = format!("plugin:{}/{}", command.plugin, command.id);
            let location = self
                .command_line
                .origin
                .as_ref()
                .map(|origin| &origin.location)
                .unwrap_or(&self.panes[self.active].location)
                .display();
            let unavailable = command
                .descriptor
                .command_line
                .as_ref()
                .and_then(|cli| cli.check_location(&location).err());
            let description = unavailable.map_or_else(
                || command.descriptor.description.clone(),
                |error| {
                    format!(
                        "Unavailable here: {error} · {}",
                        command.descriptor.description
                    )
                },
            );
            specs.push(CommandSpec {
                usage: format!("{name}{arguments}"),
                name,
                aliases,
                description,
            });
        }
        self.command_line.specs = specs;
    }
    pub(crate) fn open_command_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command_line.open {
            self.input.focus_handle(cx).focus(window);
            return;
        }
        if self.settings_open
            || self.operation_ui.open
            || self.prompt.is_some()
            || self.help
            || self.quicklook
            || !self.panes[self.active].is_files()
        {
            return;
        }
        if self.panes.iter().any(|pane| pane.name_filter.is_some()) {
            self.finish_name_filter(true, cx);
        }
        self.refresh_command_specs();
        self.command_line.origin = Some(self.frozen_command_origin());
        self.command_line.open = true;
        self.command_line.error = None;
        self.command_line.history_index = None;
        self.command_line.completion_seed = None;
        self.command_line.completion_index = 0;
        self.command_line.programmatic_changes = 0;
        self.set_command_input(String::new(), window, cx);
        self.input.focus_handle(cx).focus(window);
        self.update_command_candidates(cx);
        cx.notify();
    }
    pub(crate) fn close_command_line(
        &mut self,
        cancel: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cancel {
            self.cancel_command_job();
            self.command_line.pending_history = None;
        }
        self.command_line.open = false;
        self.command_line.origin = None;
        self.command_line.candidates.clear();
        self.focus_active(window, cx);
        cx.notify();
    }
    pub(crate) fn cancel_command_job(&mut self) {
        if let Some(cancel) = &self.command_line.pending {
            cancel.store(true, Ordering::Release);
        }
    }
    pub(crate) fn command_origin_valid(&self, origin: &Origin) -> bool {
        origin.workspace_revision == self.workspace_revision
            && origin.plugin_generation == self.plugin_generation
            && self.panes.iter().any(|pane| {
                pane.id == origin.pane && pane.is_files() && pane.location == origin.location
            })
    }
    pub(crate) fn command_line_changed(&mut self, cx: &mut Context<Self>) {
        if consume_programmatic_change(&mut self.command_line.programmatic_changes) {
            return;
        }
        self.command_line.error = None;
        self.command_line.completion_seed = None;
        self.command_line.completion_index = 0;
        self.command_line.history_index = None;
        self.update_command_candidates(cx);
        cx.notify();
    }
    fn set_command_input(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        self.command_line.programmatic_changes += 1;
        self.input
            .update(cx, |input, cx| input.set_value(value, window, cx));
    }
    fn update_command_candidates(&mut self, cx: &App) {
        self.command_line.candidates =
            self.command_candidates(self.input.read(cx).value().as_str());
    }
    fn command_candidates(&self, value: &str) -> Vec<Completion> {
        let names = command_line::completion(value, &self.command_line.specs);
        if !names.is_empty() {
            return names;
        }
        let Some(argument) = command_line::argument_completion(value) else {
            return vec![];
        };
        let Ok(spec) = command_line::resolve_command(&argument.command, &self.command_line.specs)
        else {
            return vec![];
        };
        let definition = self
            .plugin_commands
            .iter()
            .find(|entry| format!("plugin:{}/{}", entry.plugin, entry.id) == spec.name)
            .and_then(|entry| entry.descriptor.command_line.as_ref())
            .and_then(|metadata| {
                metadata.arguments.get(argument.index).or_else(|| {
                    metadata
                        .arguments
                        .last()
                        .filter(|definition| definition.variadic)
                })
            });
        let directory = matches!(spec.name.as_str(), "cd" | "split")
            || definition.is_some_and(|definition| {
                definition.kind == buckets_plugin::CommandArgumentKind::Directory
            });
        let prefix = argument.prefix.to_lowercase();
        let values = if let Some(definition) = definition
            .filter(|definition| definition.kind == buckets_plugin::CommandArgumentKind::Choice)
        {
            definition
                .choices
                .iter()
                .filter(|choice| choice.to_lowercase().starts_with(&prefix))
                .map(|choice| (choice.clone(), "Choice".to_owned()))
                .collect::<Vec<_>>()
        } else if directory
            || definition.is_some_and(|definition| {
                definition.kind == buckets_plugin::CommandArgumentKind::Path
            })
        {
            if argument.prefix.contains('/') {
                return vec![];
            }
            let Some(origin) = self.command_line.origin.as_ref() else {
                return vec![];
            };
            let Some(pane) = self.panes.iter().find(|pane| pane.id == origin.pane) else {
                return vec![];
            };
            cached_paths(&pane.entries, &prefix, directory)
        } else {
            vec![]
        };
        values
            .into_iter()
            .take(12)
            .map(|(candidate, detail)| Completion {
                value: format!(
                    "{}{}",
                    &value[..argument.replace_from],
                    command_line::quote_argument(&candidate)
                ),
                label: candidate,
                detail,
            })
            .collect()
    }
    pub(crate) fn complete_command_line(
        &mut self,
        backwards: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.command_line.pending.is_some() {
            return;
        }
        if self.command_line.completion_seed.is_none() {
            let value = self.input.read(cx).value().to_string();
            if self.input.read(cx).cursor() != value.len() {
                return;
            }
            self.command_line.candidates = self.command_candidates(&value);
            self.command_line.completion_seed = Some(value);
            self.command_line.completion_index = if backwards {
                self.command_line.candidates.len().saturating_sub(1)
            } else {
                0
            };
        } else if !self.command_line.candidates.is_empty() {
            let len = self.command_line.candidates.len();
            self.command_line.completion_index =
                (self.command_line.completion_index + if backwards { len - 1 } else { 1 }) % len;
        }
        let Some(candidate) = self
            .command_line
            .candidates
            .get(self.command_line.completion_index)
        else {
            return;
        };
        let value = candidate.value.clone();
        self.set_command_input(value, window, cx);
        cx.notify();
    }
    pub(crate) fn command_history(
        &mut self,
        previous: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.command_line.pending.is_some() || self.command_line.entries.is_empty() {
            return;
        }
        if self.command_line.history_index.is_none() {
            if !previous {
                return;
            }
            self.command_line.scratch = self.input.read(cx).value().to_string();
            self.command_line.history_matches =
                matching_history(&self.command_line.entries, &self.command_line.scratch);
        }
        if self.command_line.history_matches.is_empty() {
            return;
        }
        let target = history_step(
            self.command_line.history_index,
            previous,
            self.command_line.history_matches.len(),
        );
        let value = target
            .map(|index| {
                self.command_line.entries[self.command_line.history_matches[index]].clone()
            })
            .unwrap_or_else(|| self.command_line.scratch.clone());
        self.command_line.history_index = target;
        self.command_line.completion_seed = None;
        self.command_line.completion_index = 0;
        self.set_command_input(value, window, cx);
        self.update_command_candidates(cx);
        cx.notify();
    }
    fn save_command_history(&mut self, submitted: Option<String>, cx: &mut Context<Self>) {
        if let Some(text) = submitted {
            self.command_line.entries.retain(|entry| entry != &text);
            self.command_line.entries.push(text.clone());
            if self.command_line.entries.len() > 100 {
                self.command_line.entries.remove(0);
            }
            if self.command_line.history.is_some() || self.command_line.history_saving {
                self.command_line.history_queue.push_back(text);
            }
        }
        if self.command_line.history_saving || self.command_line.history_queue.is_empty() {
            return;
        }
        let Some(mut store) = self.command_line.history.take() else {
            return;
        };
        let batch = std::mem::take(&mut self.command_line.history_queue);
        self.command_line.history_saving = true;
        let task = cx.background_executor().spawn(async move {
            let result = batch.iter().try_for_each(|text| store.record(text));
            (store, result)
        });
        cx.spawn(async move |this, cx| {
            let (store, result) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.command_line.history_saving = false;
                this.command_line.history = Some(store);
                match result {
                    Ok(()) => {
                        if this.command_line.history_queue.is_empty() {
                            this.command_line.entries = this
                                .command_line
                                .history
                                .as_ref()
                                .unwrap()
                                .entries()
                                .to_vec();
                        }
                    }
                    Err(error) => {
                        this.command_line.history_warning =
                            Some(format!("History was not saved: {error:#}"));
                        this.command_line.history_queue.clear();
                    }
                }
                this.save_command_history(None, cx);
                cx.notify();
            });
        })
        .detach();
    }
    pub(crate) fn finish_command_history(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.command_line.pending_history.take() {
            self.save_command_history(Some(text), cx);
        }
    }
    pub(crate) fn submit_command_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command_line.pending.is_some() {
            self.command_line.error = Some("A command is preparing. Escape cancels it.".into());
            cx.notify();
            return;
        }
        let Some(origin) = self.command_line.origin.clone() else {
            return;
        };
        if !self.command_origin_valid(&origin) {
            self.command_line.error =
                Some("The source pane changed. Escape, then open the command line again.".into());
            cx.notify();
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        let result = (|| -> anyhow::Result<()> {
            let invocation = command_line::parse(&text, &origin.location)?;
            match invocation {
                Invocation::Shell { plan, .. } => {
                    anyhow::ensure!(
                        self.panes.len() < 6,
                        "Six panes are open. Close one before running a shell command."
                    );
                    self.command_line.pending_history = Some(text.clone());
                    self.spawn_command_terminal(
                        buckets_plugin::TerminalLaunchRequest {
                            program: plan.program,
                            args: plan.args,
                            cwd: plan.cwd,
                            title: plan.title,
                            env: Default::default(),
                        },
                        origin.clone(),
                        window,
                        cx,
                    );
                }
                Invocation::App { command, args } => {
                    let spec = command_line::resolve_command(&command, &self.command_line.specs)?;
                    let name = spec.name.clone();
                    if let Some(index) = self
                        .plugin_commands
                        .iter()
                        .position(|entry| format!("plugin:{}/{}", entry.plugin, entry.id) == name)
                    {
                        if let Some(cli) = &self.plugin_commands[index].descriptor.command_line {
                            cli.check_location(&origin.location.display())?;
                            cli.validate_arguments(&args)?;
                        } else {
                            anyhow::ensure!(
                                args.is_empty(),
                                "This plugin command accepts no command-line arguments"
                            );
                        }
                        self.command_line.pending_history = Some(text.clone());
                        self.invoke_plugin_command_line(index, args, origin.clone(), window, cx);
                    } else {
                        self.execute_app_command_line(&name, &args, &origin, window, cx)?;
                    }
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                if self.command_line.pending.is_none() {
                    self.save_command_history(Some(text), cx);
                }
            }
            Err(error) => {
                self.command_line.error = Some(format!("{error:#}"));
                cx.notify();
            }
        }
    }
    fn execute_app_command_line(
        &mut self,
        name: &str,
        args: &[String],
        origin: &Origin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let index = self
            .panes
            .iter()
            .position(|pane| pane.id == origin.pane)
            .ok_or_else(|| anyhow::anyhow!("Source pane closed"))?;
        let destination = match name {
            "cd" => {
                anyhow::ensure!(args.len() == 1, "Usage: cd <location>");
                Some(command_line::resolve_location(&origin.location, &args[0])?)
            }
            "split" => {
                anyhow::ensure!(args.len() <= 1, "Usage: split [location]");
                anyhow::ensure!(
                    self.panes.len() < 6,
                    "Six panes are open. Close one before splitting."
                );
                args.first()
                    .map(|arg| command_line::resolve_location(&origin.location, arg))
                    .transpose()?
            }
            _ => None,
        };
        let command = match name {
            "cd" => None,
            "split" => Some(Command::Split),
            "rename" => {
                anyhow::ensure!(args.len() == 1, "Usage: rename <name>");
                let target = origin
                    .cursor
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Select a file to rename"))?;
                let cursor = self.panes[index]
                    .entries
                    .iter()
                    .position(|entry| &entry.location == target)
                    .ok_or_else(|| {
                        anyhow::anyhow!("Selected file changed; reopen the command line")
                    })?;
                self.panes[index].cursor = cursor;
                Some(Command::Rename)
            }
            "settings" => {
                anyhow::ensure!(args.is_empty(), "Usage: settings");
                Some(Command::Settings)
            }
            _ => {
                anyhow::ensure!(
                    args.is_empty(),
                    "This action takes no arguments; run it to open its normal controls"
                );
                Some(
                    crate::COMMANDS
                        .iter()
                        .find_map(|(command, _, _)| {
                            (command_id(*command) == Some(name)).then_some(*command)
                        })
                        .ok_or_else(|| anyhow::anyhow!("Unknown command: {name}"))?,
                )
            }
        };
        if crate::keybindings::spec(name)
            .is_some_and(|spec| spec.scope == crate::keybindings::Scope::Files)
        {
            let pane = &self.panes[index];
            anyhow::ensure!(
                frozen_selection_matches(pane, origin),
                "Selection changed. Escape and reopen the command line before running a file action."
            );
        }
        self.active = index;
        self.close_command_line(false, window, cx);
        if let Some(command) = command {
            self.command(command, window, cx);
        }
        if name == "rename" {
            let value = args[0].clone();
            self.input
                .update(cx, |input, cx| input.set_value(value, window, cx));
            self.submit(window, cx);
        } else if let Some(location) = destination {
            self.navigate(location, cx);
        }
        Ok(())
    }
    pub(crate) fn render_command_line(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = theme::palette(cx);
        let context = self
            .command_line
            .origin
            .as_ref()
            .map(|origin| format!("Pane {} · {}", origin.pane, origin.location.display()))
            .unwrap_or_default();
        let compact = window.viewport_size().width < px(1100.);
        let (completion_start, completion_count) =
            completion_window(self.command_line.completion_index, compact);
        let mut panel = div()
            .id("command-line")
            .flex_shrink_0()
            .max_h(window.viewport_size().height * 0.45)
            .overflow_y_scroll()
            .border_t_1()
            .border_color(rgb(colors.accent))
            .bg(rgb(colors.chrome))
            .px_4()
            .py_2();
        panel = panel.child(
            div()
                .text_size(ui(10.))
                .text_color(rgb(colors.muted))
                .truncate()
                .child(context),
        );
        panel =
            panel.child(
                div()
                    .mt_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_color(rgb(colors.accent)).child(":"))
                    .child(div().flex_1().min_w_0().child(
                        Input::new(&self.input).disabled(self.command_line.pending.is_some()),
                    )),
            );
        if let Some(message) = self
            .command_line
            .error
            .as_ref()
            .or(self.command_line.history_warning.as_ref())
        {
            panel = panel.child(
                div()
                    .mt_1()
                    .text_size(ui(11.))
                    .text_color(rgb(colors.danger))
                    .whitespace_normal()
                    .child(message.clone()),
            );
        }
        if self.command_line.pending.is_some() {
            panel = panel.child(
                div()
                    .mt_1()
                    .text_color(rgb(colors.warning))
                    .child("Preparing command · Escape cancels"),
            );
        } else {
            panel = panel.children(
                self.command_line
                    .candidates
                    .iter()
                    .enumerate()
                    .skip(completion_start)
                    .take(completion_count)
                    .map(|(index, item)| {
                        div()
                            .id(("command-completion", index))
                            .py_1()
                            .flex()
                            .gap_2()
                            .text_size(ui(11.))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(colors.surface)))
                            .child(
                                div()
                                    .text_color(rgb(
                                        if self.command_line.completion_seed.is_some()
                                            && index == self.command_line.completion_index
                                        {
                                            colors.accent
                                        } else {
                                            colors.text
                                        },
                                    ))
                                    .child(item.label.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(colors.muted))
                                    .child(item.detail.clone()),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(candidate) = this.command_line.candidates.get(index) {
                                    let value = candidate.value.clone();
                                    this.set_command_input(value, window, cx);
                                    this.input.focus_handle(cx).focus(window);
                                    cx.notify();
                                }
                            }))
                    }),
            );
        }
        panel.child(div().mt_1().text_size(ui(10.)).text_color(rgb(colors.muted)).whitespace_normal().child("Tab Complete (paths in current folder) · ↑↓ Matching history · Enter Run · Esc Return · ! Shell"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn matching_history_cycles_contains_matches_and_restores_the_draft_after_latest() {
        let entries = vec![
            "cd docs".into(),
            "nvim café.txt".into(),
            "split docs".into(),
        ];
        assert_eq!(matching_history(&entries, "DOCS"), [0, 2]);
        assert_eq!(matching_history(&entries, "CAFÉ"), [1]);
        assert_eq!(matching_history(&entries, ""), [0, 1, 2]);
        assert!(matching_history(&entries, "missing").is_empty());
        let latest = history_step(None, true, 2);
        assert_eq!(latest, Some(1));
        assert_eq!(history_step(latest, true, 2), Some(0));
        assert_eq!(history_step(Some(0), true, 2), Some(0));
        assert_eq!(history_step(Some(0), false, 2), Some(1));
        assert_eq!(history_step(latest, false, 2), None);
        assert_eq!(history_step(None, true, 0), None);
    }
    #[::core::prelude::v1::test]
    fn programmatic_input_changes_do_not_reset_completion_or_history_cycles() {
        let mut pending = 2;
        assert!(consume_programmatic_change(&mut pending));
        assert!(consume_programmatic_change(&mut pending));
        assert!(!consume_programmatic_change(&mut pending));
        assert_eq!(pending, 0);
        for compact in [false, true] {
            for index in 0..12 {
                let (start, len) = completion_window(index, compact);
                assert!((start..start + len).contains(&index));
                assert!(len <= 4);
            }
        }
    }
    fn entry(name: &str, directory: bool) -> buckets_core::Entry {
        buckets_core::Entry {
            name: name.into(),
            location: Location::local(format!("/work/{name}")),
            is_dir: directory,
            is_symlink: false,
            size: 0,
            modified: None,
        }
    }
    #[::core::prelude::v1::test]
    fn path_completion_uses_cached_entries_and_roundtrips_quoted_unicode_directories() {
        let entries = vec![
            entry("Café notes", true),
            entry("café.txt", false),
            entry("other", true),
        ];
        let candidates = cached_paths(&entries, "CAFÉ", true);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0, "Café notes/");
        let argument = command_line::argument_completion("cd 'Caf").unwrap();
        let completed = format!(
            "{}{}",
            &"cd 'Caf"[..argument.replace_from],
            command_line::quote_argument(&candidates[0].0)
        );
        assert_eq!(
            command_line::parse(&completed, &Location::local("/work")).unwrap(),
            Invocation::App {
                command: "cd".into(),
                args: vec!["Café notes/".into()]
            }
        );
        assert_eq!(cached_paths(&entries, "café", false).len(), 2);
    }
    #[::core::prelude::v1::test]
    fn frozen_file_actions_refuse_pointer_changes_to_cursor_or_marks() {
        let mut pane = Pane::new(1, Location::local("/work"));
        pane.entries = vec![entry("a", false), entry("b", false)];
        let origin = Origin {
            pane: 1,
            location: pane.location.clone(),
            workspace_revision: 0,
            plugin_generation: 0,
            selection: pane.chosen().iter().map(Location::display).collect(),
            cursor: Some(pane.entries[0].location.clone()),
        };
        assert!(frozen_selection_matches(&pane, &origin));
        pane.cursor = 1;
        assert!(!frozen_selection_matches(&pane, &origin));
        pane.cursor = 0;
        pane.selected.insert(1);
        assert!(!frozen_selection_matches(&pane, &origin));
    }
}
