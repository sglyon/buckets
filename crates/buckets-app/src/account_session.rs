//! Account workflows run separately from model requests. Only explicit sign-in
//! may open a browser; the UI receives public connection state, never tokens.
use crate::{Browser, PaneContent, account_state, session_path};
use anyhow::{Result, anyhow, ensure};
use buckets_plugin::{
    AccountAction, AccountEvent, AccountRequest, AccountState, Capability, CredentialBroker,
};
use gpui::{prelude::*, *};
use serde_json::Value;
use std::sync::{atomic::Ordering, mpsc};

enum JobEvent {
    Progress(AccountEvent),
    OpenBrowser {
        url: String,
        reply: mpsc::SyncSender<bool>,
    },
    Finished(Result<AccountState, String>),
}
impl Browser {
    fn invalidate_account_agents(&mut self, plugin: &str, message: &str, cx: &mut Context<Self>) {
        for pane in &self.panes {
            if let PaneContent::Agent(view) = &pane.content
                && view.read(cx).plugin() == plugin
            {
                view.update(cx, |view, cx| view.invalidate(message, cx));
            }
        }
    }
    pub(super) fn sync_account_views(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = &self.settings_view {
            let accounts = self.account_sessions.public.clone();
            view.update(cx, |view, cx| view.accounts(accounts, cx));
        }
        for pane in &self.panes {
            if let PaneContent::Agent(view) = &pane.content {
                let state = self
                    .account_sessions
                    .public
                    .get(view.read(cx).plugin())
                    .and_then(|status| status.state.clone());
                view.update(cx, |view, cx| view.account_state(state, cx));
            }
        }
    }
    pub(super) fn refresh_account_states(&mut self, cx: &mut Context<Self>) {
        let plugins = self
            .plugins
            .plugins()
            .iter()
            .filter(|plugin| {
                plugin.supports(Capability::Accounts) && self.plugins.is_enabled(&plugin.id)
            })
            .filter(|plugin| {
                !self.account_sessions.busy(&plugin.id)
                    && !self.account_sessions.public.contains_key(&plugin.id)
            })
            .map(|plugin| plugin.id.clone())
            .collect::<Vec<_>>();
        for plugin in plugins {
            self.run_account(
                plugin,
                AccountRequest {
                    action: AccountAction::Status,
                    account_id: None,
                },
                cx,
            );
        }
    }
    pub(super) fn open_account_settings(
        &mut self,
        plugin: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings(window, cx);
        if let Some(view) = &self.settings_view {
            let plugin = plugin.to_owned();
            view.update(cx, |view, cx| view.focus_plugin(&plugin, window, cx));
        }
    }
    pub(super) fn run_account(
        &mut self,
        plugin: String,
        request: AccountRequest,
        cx: &mut Context<Self>,
    ) {
        if !self.plugins.is_enabled(&plugin) {
            return;
        }
        if matches!(
            request.action,
            AccountAction::SignIn | AccountAction::SignOut | AccountAction::Select
        ) {
            self.invalidate_account_agents(
                &plugin,
                "Account changed. Submit a new question after the connection completes.",
                cx,
            );
        }
        let (generation, cancel) = self.account_sessions.begin(&plugin);
        self.sync_account_views(cx);
        let epoch = self.plugin_generation;
        let registry = self.plugins.clone();
        let root = session_path().parent().unwrap().join("accounts");
        let task_plugin = plugin.clone();
        let (sender, receiver) = mpsc::sync_channel(32);
        let action = request.action;
        cx.background_executor()
            .spawn(async move {
                let result = (|| -> Result<AccountState> {
                    ensure!(!cancel.load(Ordering::Acquire), "Account request cancelled");
                    let mut credentials = CredentialBroker::new(&root, &task_plugin)?;
                    let mut service = registry.start_accounts(&task_plugin)?;
                    let state = service.invoke(
                        request,
                        |method, params| {
                            ensure!(!cancel.load(Ordering::Acquire), "Account request cancelled");
                            if method == "browser.open" {
                                ensure!(
                                    matches!(action, AccountAction::SignIn),
                                    "Browser sign-in requires an explicit action"
                                );
                                let url = account_state::external_url(
                                    params
                                        .get("url")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default(),
                                )?;
                                let (reply, acknowledgement) = mpsc::sync_channel(1);
                                sender
                                    .send(JobEvent::OpenBrowser { url, reply })
                                    .map_err(|_| anyhow!("Account view closed"))?;
                                loop {
                                    ensure!(
                                        !cancel.load(Ordering::Acquire),
                                        "Account request cancelled"
                                    );
                                    match acknowledgement
                                        .recv_timeout(std::time::Duration::from_millis(50))
                                    {
                                        Ok(true) => return Ok(serde_json::json!({"opened":true})),
                                        Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                                            anyhow::bail!(
                                                "Account view closed before opening the browser"
                                            )
                                        }
                                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                                    }
                                }
                            }
                            credentials.dispatch(method, params)
                        },
                        |event| {
                            let _ = sender.try_send(JobEvent::Progress(event));
                        },
                        &cancel,
                    )?;
                    ensure!(!cancel.load(Ordering::Acquire), "Account request cancelled");
                    Ok(state)
                })()
                .map_err(|error| format!("{error:#}"));
                let _ = sender.send(JobEvent::Finished(result));
            })
            .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
                let mut events = vec![];
                let mut disconnected = false;
                loop {
                    match receiver.try_recv() {
                        Ok(event) => events.push(event),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => { disconnected = true; break; }
                    }
                }
                if events.is_empty() && !disconnected { continue; }
                let mut finished = false;
                let updated = this.update(cx, |this, cx| {
                    if this.plugin_generation != epoch || !this.plugins.is_enabled(&plugin) || !this.account_sessions.accepts(&plugin, generation) {
                        for event in events { if let JobEvent::OpenBrowser { reply, .. } = event { let _ = reply.send(false); } }
                        return false;
                    }
                    for event in events {
                        match event {
                            JobEvent::Progress(event) => this.account_sessions.progress(&plugin, generation, event.message),
                            JobEvent::OpenBrowser { url, reply } => {
                                cx.open_url(&url);
                                let _ = reply.send(true);
                            }
                            JobEvent::Finished(result) => {
                                finished = true;
                                let changed = matches!(action, AccountAction::SignIn | AccountAction::SignOut | AccountAction::Select);
                                this.account_sessions.complete(&plugin, generation, result);
                                if changed { this.invalidate_account_agents(&plugin, "Account changed. Submit a new question to use this account.", cx); }
                            }
                        }
                    }
                    if disconnected && !finished {
                        finished = true;
                        this.account_sessions.complete(&plugin, generation, Err("Account service stopped. Refresh account status and retry.".into()));
                    }
                    this.sync_account_views(cx);
                    true
                });
                if !matches!(updated, Ok(true)) || finished { break; }
            }
        }).detach();
    }
}
