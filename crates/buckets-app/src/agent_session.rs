//! Agent application orchestration: context admission, supervised service jobs,
//! stale completion guards and presentation events. Provider/model logic lives
//! behind the plugin protocol, not in the desktop view.
use crate::{
    Browser, Entry, Location, Pane, PaneContent, agent_context,
    agent_view::{AgentView, AgentViewEvent},
    session_path,
};
use anyhow::{Result, anyhow, ensure};
use buckets_core::AgentBroker;
use buckets_plugin::{
    AccountAction, AccountRequest, AgentEntry, AgentEvent, AgentLaunchRequest, AgentMode,
    AgentPaneContext, AgentRequest, AgentResult, Capability, CredentialBroker, WorkspaceContext,
};
use gpui::{prelude::*, *};
use std::sync::{atomic::Ordering, mpsc};

enum JobEvent {
    Progress(AgentEvent),
    Finished(Result<AgentResult, String>),
}
impl Browser {
    fn snapshot_agent_context(&self) -> (WorkspaceContext, String) {
        let mut budget = 400_000usize;
        let mut metadata = vec![];
        let mut panes = vec![];
        for pane in self.panes.iter().filter(|pane| pane.is_files()).take(32) {
            let location = pane.location.display();
            budget = budget.saturating_sub(location.len() + 128);
            let mut selected = vec![];
            let mut chosen = pane.chosen();
            chosen.sort_by_cached_key(Location::display);
            for entry in chosen.iter().take(100) {
                let value = entry.display();
                if value.len() + 32 > budget {
                    break;
                }
                budget -= value.len() + 32;
                selected.push(value);
            }
            selected.sort();
            let cursor = pane
                .entries
                .get(pane.cursor)
                .map(|entry| entry.location.display())
                .filter(|value| {
                    if value.len() + 32 > budget {
                        false
                    } else {
                        budget -= value.len() + 32;
                        true
                    }
                });
            let mut entries = vec![];
            let mut details = vec![];
            let mut canonical_entries = pane.entries.iter().collect::<Vec<_>>();
            canonical_entries.sort_by_cached_key(|entry| entry.location.display());
            for entry in canonical_entries.into_iter().take(200) {
                let location = entry.location.display();
                let bytes = location.len() + entry.name.len() + 96;
                if bytes > budget {
                    break;
                }
                budget -= bytes;
                entries.push(AgentEntry {
                    location: location.clone(),
                    name: entry.name.clone(),
                    is_dir: entry.is_dir,
                });
                details.push((location, entry.size, entry.modified));
            }
            metadata.push((pane.id, pane.loading, details));
            panes.push(AgentPaneContext {
                id: pane.id.to_string(),
                location,
                selected,
                cursor,
                entries,
                capabilities: if matches!(
                    pane.location,
                    Location::Local { .. } | Location::Ssh { .. }
                ) {
                    vec!["list".into(), "read".into(), "search".into()]
                } else {
                    vec!["list".into(), "read".into()]
                },
            });
        }
        let fingerprint =
            serde_json::to_string(&(&self.workspace_id, &panes, metadata)).unwrap_or_default();
        (
            WorkspaceContext {
                workspace_id: self.workspace_id.clone(),
                revision: self.workspace_revision,
                panes,
            },
            fingerprint,
        )
    }
    pub(super) fn open_agent(
        &mut self,
        plugin: String,
        title: String,
        intent: AgentLaunchRequest,
        origin: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.plugins.is_enabled(&plugin) {
            self.message(
                "This plugin is disabled. Enable it in Settings before opening its agent.",
                true,
                cx,
            );
            return;
        }
        if let Some(index)=self.panes.iter().position(|pane|matches!(&pane.content,PaneContent::Agent(view) if view.read(cx).plugin()==plugin)){
            self.active=index;
            if let PaneContent::Agent(view)=&self.panes[index].content{view.update(cx,|view,cx|{if view.busy() && view.mode()!=intent.mode{view.invalidate("Mode changed. Previous request cancelled.",cx);} view.configure(title,intent.mode,intent.prompt.clone(),window,cx)});}
            if matches!(intent.mode,AgentMode::Memory|AgentMode::Index){let id=self.panes[index].id;self.run_agent(id,intent.mode,intent.prompt,None,window,cx);}
            self.sync_account_views(cx);self.refresh_account_states(cx);cx.notify();return;
        }
        if self.panes.len() >= 6 {
            self.message(
                "Six panes are open. Close one before opening an agent.",
                true,
                cx,
            );
            return;
        }
        let Some(origin_index) = self.panes.iter().position(|pane| pane.id == origin) else {
            return;
        };
        let location = self.panes[origin_index].location.clone();
        let id = self.next_id;
        self.next_id += 1;
        let view = cx.new(|cx| {
            AgentView::new(
                plugin,
                title,
                intent.mode,
                intent.prompt.clone(),
                window,
                cx,
            )
        });
        let subscription = cx.subscribe_in(&view, window, move |this, _, event, window, cx| {
            this.agent_event(id, event, window, cx)
        });
        let observation = cx.observe(&view, |_, _, cx| cx.notify());
        let mut pane = Pane::new(id, location);
        pane.loading = false;
        pane.content = PaneContent::Agent(view);
        pane.agent_subscription = Some(subscription);
        pane.terminal_observation = Some(observation);
        self.panes.insert(origin_index + 1, pane);
        self.active = origin_index + 1;
        self.sync_account_views(cx);
        self.refresh_account_states(cx);
        self.quicklook = false;
        self.help = false;
        self.prompt = None;
        self.focus_active(window, cx);
        self.load_preview(cx);
        self.save();
        if matches!(intent.mode, AgentMode::Memory | AgentMode::Index) {
            self.run_agent(id, intent.mode, intent.prompt, None, window, cx);
        }
        cx.notify();
    }
    pub(super) fn clear_private_agents(&mut self, message: &str, cx: &mut Context<Self>) {
        for pane in &self.panes {
            if let PaneContent::Agent(view) = &pane.content {
                view.update(cx, |view, cx| view.clear_private(message, cx));
            }
        }
    }
    fn agent_event(
        &mut self,
        pane_id: u64,
        event: &AgentViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.panes.iter().position(|pane| pane.id == pane_id) else {
            return;
        };
        match event {
            AgentViewEvent::Run {
                mode,
                prompt,
                memory_update,
            } => self.run_agent(
                pane_id,
                *mode,
                prompt.clone(),
                memory_update.clone(),
                window,
                cx,
            ),
            AgentViewEvent::Preview { location, line } => {
                self.preview_agent_citation(pane_id, location.clone(), *line, cx)
            }
            AgentViewEvent::Open { location, line } => {
                self.open_agent_location(pane_id, location, *line, window, cx)
            }
            AgentViewEvent::Results => self.open_agent_results(pane_id, window, cx),
            AgentViewEvent::Settings => {
                if let PaneContent::Agent(view) = &self.panes[index].content {
                    let plugin = view.read(cx).plugin().to_owned();
                    self.open_account_settings(&plugin, window, cx);
                }
            }
            AgentViewEvent::ManageUsage(url) => {
                if let Ok(url) = crate::account_state::external_url(url) {
                    cx.open_url(&url);
                }
            }
            AgentViewEvent::CancelClose => {
                self.exit_after_agent = None;
            }
            AgentViewEvent::Close => {
                let exiting = self.exit_after_agent.take() == Some(pane_id);
                self.panes.remove(index);
                if self.panes.is_empty() {
                    let location = Location::parse("/").expect("root location");
                    self.panes.push(Pane::new(self.next_id, location));
                    self.next_id += 1;
                    self.reload(0, cx);
                }
                self.active = self.active.min(self.panes.len() - 1);
                self.focus_active(window, cx);
                self.load_preview(cx);
                self.save();
                if exiting {
                    self.request_quit(window, cx);
                }
                cx.notify();
            }
        }
    }
    fn run_agent(
        &mut self,
        pane_id: u64,
        mode: AgentMode,
        prompt: String,
        memory_update: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.panes.iter().position(|pane| pane.id == pane_id) else {
            return;
        };
        let PaneContent::Agent(view) = &self.panes[index].content else {
            return;
        };
        let view = view.clone();
        if view.read(cx).busy() {
            self.message(
                "This agent is already working. Cancel it before starting another request.",
                true,
                cx,
            );
            return;
        }
        let plugin = view.read(cx).plugin().to_owned();
        if matches!(mode, AgentMode::Ask) && self.account_sessions.busy(&plugin) {
            view.update(cx, |view, cx| {
                view.invalidate(
                    "An account operation is in progress. Finish or cancel it in AI settings.",
                    cx,
                )
            });
            self.open_account_settings(&plugin, window, cx);
            return;
        }
        if matches!(mode, AgentMode::Ask)
            && self
                .account_sessions
                .public
                .get(&plugin)
                .and_then(|status| status.state.as_ref())
                .is_some_and(|state| state.sign_in_required)
        {
            view.update(cx, |view, cx| {
                view.invalidate(
                    "Sign in from AI settings, or choose an API key or local provider.",
                    cx,
                )
            });
            self.open_account_settings(&plugin, window, cx);
            return;
        }
        let Some(manifest) = self
            .plugins
            .plugins()
            .iter()
            .find(|manifest| manifest.id == plugin && self.plugins.is_enabled(&plugin))
            .cloned()
        else {
            view.update(cx, |view, cx| {
                view.invalidate(
                    "Plugin disabled or unavailable. Enable it in Settings and try again.",
                    cx,
                )
            });
            return;
        };
        self.workspace_revision = self.workspace_revision.saturating_add(1);
        let (context, fingerprint) = self.snapshot_agent_context();
        let (generation, cancel, service) = view.update(cx, |view, cx| {
            view.begin(mode, &prompt, memory_update.clone(), window, cx)
        });
        let epoch = self.plugin_generation;
        let registry = self.plugins.clone();
        let storage = agent_context::storage_directory(
            session_path().parent().unwrap(),
            &plugin,
            &self.workspace_id,
        );
        let account_root = session_path().parent().unwrap().join("accounts");
        let request_id = agent_context::admission_id(
            &self.workspace_id,
            pane_id,
            generation,
            self.workspace_revision,
        );
        let (sender, receiver) = mpsc::sync_channel(64);
        cx.background_executor()
            .spawn(async move {
                let result = (|| -> Result<AgentResult> {
                    ensure!(!cancel.load(Ordering::SeqCst), "Request cancelled");
                    let (context, roots, search_roots) = agent_context::admit_context(context, &manifest)?;
                    ensure!(matches!(mode, AgentMode::Memory) || !roots.is_empty(),
                        "No open or configured file root is within this plugin's read grants. Open an allowed directory or adjust the installed plugin permissions.");
                    let broker = AgentBroker::new(context.clone(), roots, manifest.permissions.network)?;
                    let search_broker = search_roots.map(|roots| {
                        AgentBroker::new(WorkspaceContext { panes: vec![], ..context.clone() }, roots, manifest.permissions.network)
                    }).transpose()?;
                    let request = AgentRequest {
                        request_id: request_id.clone(), mode, prompt, context,
                        storage_dir: storage, memory_update,
                    };
                    let mut slot = service.lock().map_err(|_| anyhow!("Agent service state is unavailable"))?;
                    if slot.as_mut().is_none_or(|service| !service.is_running()) {
                        *slot = Some(registry.start_agent(&plugin)?);
                    }
                    let service = slot.as_mut().ok_or_else(|| anyhow!("Agent service did not start"))?;
                    let mut credentials = manifest.supports(Capability::Accounts).then(|| CredentialBroker::new(&account_root, &plugin)).transpose()?;
                    let result = service.invoke(
                        request,
                        |method, params| {
                            if method.starts_with("credentials.") {
                                return credentials.as_mut().ok_or_else(|| anyhow!("This plugin has no account capability"))?.dispatch(method, params);
                            }
                            agent_context::dispatch_tool(mode,&broker,search_broker.as_ref(),method,&params)
                        },
                        |event| {
                            if event.request_id == request_id {
                                let _ = sender.try_send(JobEvent::Progress(event));
                            }
                        },
                        &cancel,
                    )?;
                    ensure!(!cancel.load(Ordering::SeqCst), "Request cancelled");
                    broker.validate_result(&result)?;
                    ensure!(!matches!(mode, AgentMode::Memory) || result.memory.is_some(),
                        "Agent did not acknowledge workspace memory; edits remain unsaved.");
                    Ok(result)
                })().map_err(|error| format!("{error:#}"));
                let _ = sender.send(JobEvent::Finished(result));
            })
            .detach();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
                let mut events = vec![];
                loop {
                    match receiver.try_recv() {
                        Ok(event) => events.push(event),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            if !events.iter().any(|event| matches!(event, JobEvent::Finished(_))) {
                                events.push(JobEvent::Finished(Err("Agent worker stopped before completing the request.".into())));
                            }
                            break;
                        }
                    }
                }
                let mut finished = false;
                let mut missing = false;
                if cx.update(|window, cx| {
                    let _ = this.update(cx, |this, cx| {
                        let Some(pane) = this.panes.iter().find(|pane| pane.id == pane_id) else { missing = true; return; };
                        let PaneContent::Agent(view) = &pane.content else { missing = true; return; };
                        let view = view.clone();
                        if !view.read(cx).accepts(generation) { missing = true; return; }
                        for event in events {
                            match event {
                                JobEvent::Progress(event) => view.update(cx, |view, cx| view.progress(generation, event, cx)),
                                JobEvent::Finished(mut result) => {
                                    finished = true;
                                    if this.plugin_generation != epoch || !this.plugins.is_enabled(view.read(cx).plugin()) {
                                        result = Err("Plugin settings changed while this request was running. Run it again.".into());
                                    } else if !matches!(mode, AgentMode::Memory) && this.snapshot_agent_context().1 != fingerprint {
                                        result = Err("File pane context changed while this request was running. Run it again for the current panes.".into());
                                    }
                                    let refresh_account = matches!(mode, AgentMode::Ask) && result.is_err();
                                    view.update(cx, |view, cx| view.complete(generation, mode, result, window, cx));
                                    if refresh_account {
                                        let plugin = view.read(cx).plugin().to_owned();
                                        if this.plugins.plugins().iter().any(|manifest| manifest.id == plugin && manifest.supports(Capability::Accounts)) {
                                            this.run_account(plugin, AccountRequest { action: AccountAction::Status, account_id: None }, cx);
                                        }
                                    }
                                }
                            }
                        }
                    });
                }).is_err() { break; }
                if finished || missing { break; }
            }
        }).detach();
    }
    fn preview_agent_citation(
        &mut self,
        pane_id: u64,
        location: String,
        line: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let parsed = match Location::parse(&location) {
            Ok(location) => location,
            Err(error) => {
                self.message(error.to_string(), true, cx);
                return;
            }
        };
        let Some(PaneContent::Agent(view)) = self
            .panes
            .iter()
            .find(|pane| pane.id == pane_id)
            .map(|pane| &pane.content)
        else {
            return;
        };
        let preview_token = view.update(cx, |view, cx| view.begin_preview(cx));
        let task = cx.background_executor().spawn(async move {
            buckets_core::Backend
                .preview(&parsed, 32768)
                .map(|preview| preview.text)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(pane) = this.panes.iter().find(|pane| pane.id == pane_id) else {
                    return;
                };
                if let PaneContent::Agent(view) = &pane.content {
                    let label = format!(
                        "{}{}",
                        location,
                        line.map(|line| format!(":{line}")).unwrap_or_default()
                    );
                    view.update(cx, |view, cx| {
                        view.set_preview(
                            preview_token,
                            label,
                            result.unwrap_or_else(|error| format!("Preview failed: {error:#}")),
                            cx,
                        )
                    });
                }
            });
        })
        .detach();
    }
    fn open_agent_location(
        &mut self,
        origin: u64,
        location: &str,
        line: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = match Location::parse(location) {
            Ok(location) => location,
            Err(error) => {
                self.message(error.to_string(), true, cx);
                return;
            }
        };
        if self.panes.len() >= 6 {
            self.message("Close a pane before opening a citation.", true, cx);
            return;
        }
        let Some(index) = self.panes.iter().position(|pane| pane.id == origin) else {
            return;
        };
        let root = target.parent().unwrap_or_else(|| target.clone());
        let mut pane = Pane::new(self.next_id, root);
        self.next_id += 1;
        pane.pending_reveal = Some(target);
        self.panes.insert(index + 1, pane);
        self.active = index + 1;
        self.reload(self.active, cx);
        self.focus_active(window, cx);
        self.save();
        self.message(
            format!(
                "Citation opened{}",
                line.map(|line| format!(" · line {line}"))
                    .unwrap_or_default()
            ),
            false,
            cx,
        );
    }
    fn open_agent_results(&mut self, origin: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.panes.len() >= 6 {
            self.message("Close a pane before opening results.", true, cx);
            return;
        }
        let Some(index) = self.panes.iter().position(|pane| pane.id == origin) else {
            return;
        };
        let PaneContent::Agent(view) = &self.panes[index].content else {
            return;
        };
        let Some(result) = view.read(cx).result() else {
            self.message("No completed results to open yet.", false, cx);
            return;
        };
        if result.results.is_empty() {
            self.message(
                "This response has no file results. Use its cited files instead.",
                false,
                cx,
            );
            return;
        }
        let details = result
            .results
            .iter()
            .map(|item| {
                let mut parts = vec![];
                if let Some(line) = item.line {
                    parts.push(format!("Line {line}"));
                }
                if let Some(reason) = &item.reason {
                    parts.push(reason.clone());
                }
                if let Some(snippet) = &item.snippet {
                    parts.push(snippet.clone());
                }
                (item.location.clone(), parts.join("\n\n"))
            })
            .collect();
        let entries = result
            .results
            .iter()
            .filter_map(|item| {
                Location::parse(&item.location).ok().map(|location| Entry {
                    name: if item.name.is_empty() {
                        location.name()
                    } else {
                        item.name.clone()
                    },
                    location,
                    is_dir: item.is_dir,
                    is_symlink: false,
                    size: 0,
                    modified: None,
                })
            })
            .collect();
        let mut pane = Pane::new(self.next_id, self.panes[index].location.clone());
        self.next_id += 1;
        pane.loading = false;
        pane.entries = entries;
        pane.result_details = details;
        pane.search = Some("Agent results".into());
        self.panes.insert(index + 1, pane);
        self.active = index + 1;
        self.focus_active(window, cx);
        self.load_preview(cx);
        self.save();
        cx.notify();
    }
}
