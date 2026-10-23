//! Cancellable command execution reuses the existing PTY and plugin host.
use crate::{
    Browser, Pane, PaneContent,
    command_line_view::{Origin, plugin_message},
    theme,
};
use buckets_core::Location;
use gpui::{prelude::*, *};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

impl Browser {
    pub(crate) fn spawn_command_terminal(
        &mut self,
        request: buckets_plugin::TerminalLaunchRequest,
        origin: Origin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.command_line.pending = Some(cancel.clone());
        self.launch_terminal_request(request, origin, cancel, window, cx);
    }
    fn launch_terminal_request(
        &mut self,
        request: buckets_plugin::TerminalLaunchRequest,
        origin: Origin,
        cancel: Arc<AtomicBool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.command_origin_valid(&origin)
            || cancel.load(Ordering::Acquire)
            || self.panes.len() >= 6
        {
            self.command_line.pending = None;
            self.command_line.pending_history = None;
            self.command_line.error =
                Some("Terminal was not started; its source changed or six panes are open".into());
            self.message(
                "Terminal was not started; its source changed or six panes are open",
                true,
                cx,
            );
            return;
        }
        let worker_cancel = cancel.clone();
        let location = if matches!(origin.location, Location::Ssh { .. }) {
            origin.location.clone()
        } else {
            Location::local(request.cwd.clone())
        };
        self.operations += 1;
        let task = cx.background_executor().spawn(async move {
            anyhow::ensure!(!worker_cancel.load(Ordering::Acquire), "Command cancelled");
            let session =
                buckets_terminal::TerminalSession::spawn(buckets_terminal::TerminalSpec {
                    program: request.program,
                    args: request.args,
                    cwd: request.cwd,
                    title: request.title,
                    env: request.env,
                })?;
            if worker_cancel.load(Ordering::Acquire) {
                session.close();
                anyhow::bail!("Command cancelled");
            }
            Ok::<_, anyhow::Error>(session)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.operations = this.operations.saturating_sub(1);
                    let current = this
                        .command_line
                        .pending
                        .as_ref()
                        .is_some_and(|pending| Arc::ptr_eq(pending, &cancel));
                    if current {
                        this.command_line.pending = None;
                    }
                    if cancel.load(Ordering::Acquire)
                        || !this.command_origin_valid(&origin)
                        || !current
                    {
                        if current {
                            this.command_line.pending_history = None;
                        }
                        if let Ok(session) = result {
                            session.close();
                        }
                        cx.notify();
                        return;
                    }
                    match result {
                        Ok(session) => {
                            if this.command_line.open {
                                this.close_command_line(false, window, cx);
                            }
                            if this.attach_terminal_session(
                                session,
                                location,
                                origin.pane,
                                window,
                                cx,
                            ) {
                                this.finish_command_history(cx);
                            } else {
                                this.command_line.pending_history = None;
                            }
                        }
                        Err(error) => {
                            this.command_line.pending_history = None;
                            this.command_line.error = Some(format!("Terminal failed: {error:#}"));
                            this.message(format!("Terminal failed: {error:#}"), true, cx);
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
    pub(crate) fn attach_terminal_session(
        &mut self,
        session: buckets_terminal::TerminalSession,
        location: Location,
        origin: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(index) = self.panes.iter().position(|pane| pane.id == origin) else {
            session.close();
            return false;
        };
        if self.panes.len() >= 6 {
            session.close();
            self.message(
                "Six panes are open. Close one before opening a terminal.",
                true,
                cx,
            );
            return false;
        }
        let active = self.panes[self.active].id;
        let focus = active == origin
            && self.prompt.is_none()
            && !self.settings_open
            && !self.command_line.open;
        let view = cx.new(|cx| buckets_terminal::TerminalView::new(session, window, cx));
        view.update(cx, |view, cx| view.set_appearance(theme::terminal(cx), cx));
        let mut pane = Pane::new(self.next_id, location);
        self.next_id += 1;
        pane.loading = false;
        pane.terminal_observation = Some(cx.observe(&view, |_, _, cx| cx.notify()));
        pane.content = PaneContent::Terminal(view);
        self.panes.insert(index + 1, pane);
        self.layout_epoch += 1;
        self.active = if focus {
            index + 1
        } else {
            self.panes
                .iter()
                .position(|pane| pane.id == active)
                .unwrap_or(0)
        };
        self.quicklook = false;
        self.help = false;
        self.load_preview(cx);
        if focus {
            self.focus_active(window, cx);
        }
        self.save();
        self.message(
            format!(
                "Terminal opened · Ctrl-C interrupts · {} closes pane",
                self.binding_hint(crate::Command::Close)
            ),
            false,
            cx,
        );
        true
    }
    pub(crate) fn invoke_plugin_command_line(
        &mut self,
        index: usize,
        args: Vec<String>,
        origin: Origin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = self.plugin_commands.get(index) else {
            return;
        };
        let descriptor = command.descriptor.clone();
        let plugin = command.plugin.clone();
        let title = command.title.clone();
        let registry = self.plugins.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.command_line.pending = Some(cancel.clone());
        let worker_cancel = cancel.clone();
        let context = buckets_plugin::CommandContext {
            location: origin.location.display(),
            selected: origin.selection.clone(),
        };
        self.operations += 1;
        self.message(format!("Preparing {title}…"), false, cx);
        let task = cx.background_executor().spawn(async move {
            if descriptor.command_line.is_some() {
                registry.invoke_command_line_result(
                    &plugin,
                    &descriptor,
                    context,
                    args,
                    &worker_cancel,
                )
            } else {
                registry.invoke_command_result_cancellable(
                    &plugin,
                    &descriptor.id,
                    serde_json::json!({"location":context.location,"selected":context.selected}),
                    &worker_cancel,
                )
            }
        });
        let plugin = self.plugin_commands[index].plugin.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.operations = this.operations.saturating_sub(1);
                    let current = this
                        .command_line
                        .pending
                        .as_ref()
                        .is_some_and(|pending| Arc::ptr_eq(pending, &cancel));
                    if !current
                        || cancel.load(Ordering::Acquire)
                        || !this.command_origin_valid(&origin)
                    {
                        if current {
                            this.command_line.pending = None;
                            this.command_line.pending_history = None;
                        }
                        cx.notify();
                        return;
                    }
                    match result {
                        Ok(buckets_plugin::CommandResult::Terminal(request)) => {
                            this.launch_terminal_request(request, origin, cancel, window, cx);
                        }
                        Ok(buckets_plugin::CommandResult::Agent(intent)) => {
                            this.command_line.pending = None;
                                if this.panes[this.active].id != origin.pane {
                                    this.command_line.pending_history=None;
                                    this.close_command_line(false,window,cx);
                                    this.message("Agent pane was not opened because focus moved. Run the command again.",true,cx);
                                    return;
                                }
                            this.finish_command_history(cx);
                            this.close_command_line(false, window, cx);
                            this.open_agent(plugin, title, intent, origin.pane, window, cx);
                        }
                        Ok(buckets_plugin::CommandResult::Value(value)) => {
                            this.command_line.pending = None;
                            this.finish_command_history(cx);
                            this.close_command_line(false, window, cx);
                            for pane in &mut this.panes {
                                if pane.is_files() {
                                    pane.refresh_pending = true;
                                }
                            }
                            this.message(plugin_message(&value), false, cx);
                        }
                        Err(error) => {
                            this.command_line.pending = None;
                            this.command_line.pending_history = None;
                            this.command_line.error = Some(format!("Command failed: {error:#}"));
                            this.message(format!("Command failed: {error:#}"), true, cx);
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}
