//! Public account presentation and job admission. Credentials never enter this state.
use anyhow::{Result, ensure};
use buckets_plugin::AccountState;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Default)]
pub struct AccountPresentation {
    pub state: Option<AccountState>,
    pub busy: bool,
    pub message: String,
    pub error: bool,
}
struct Pending {
    generation: u64,
    cancel: Arc<AtomicBool>,
}
#[derive(Default)]
pub struct AccountSessions {
    pub public: BTreeMap<String, AccountPresentation>,
    pending: BTreeMap<String, Pending>,
    sequence: u64,
}
impl AccountSessions {
    pub fn begin(&mut self, plugin: &str) -> (u64, Arc<AtomicBool>) {
        self.cancel(plugin);
        self.sequence = self.sequence.wrapping_add(1);
        let generation = self.sequence;
        let cancel = Arc::new(AtomicBool::new(false));
        self.pending.insert(
            plugin.into(),
            Pending {
                generation,
                cancel: cancel.clone(),
            },
        );
        let status = self.public.entry(plugin.into()).or_default();
        status.busy = true;
        status.error = false;
        status.message = "Connecting…".into();
        (generation, cancel)
    }
    pub fn accepts(&self, plugin: &str, generation: u64) -> bool {
        self.pending
            .get(plugin)
            .is_some_and(|job| job.generation == generation && !job.cancel.load(Ordering::Acquire))
    }
    pub fn busy(&self, plugin: &str) -> bool {
        self.pending.contains_key(plugin)
    }
    pub fn progress(&mut self, plugin: &str, generation: u64, message: String) {
        if self.accepts(plugin, generation) {
            self.public.entry(plugin.into()).or_default().message = message;
        }
    }
    pub fn complete(
        &mut self,
        plugin: &str,
        generation: u64,
        result: Result<AccountState, String>,
    ) -> bool {
        if !self.accepts(plugin, generation) {
            return false;
        }
        self.pending.remove(plugin);
        let status = self.public.entry(plugin.into()).or_default();
        status.busy = false;
        match result {
            Ok(state) => {
                status.message = state
                    .message
                    .clone()
                    .unwrap_or_else(|| "Account status refreshed.".into());
                status.error = false;
                status.state = Some(state);
            }
            Err(error) => {
                status.message = error;
                status.error = true;
            }
        }
        true
    }
    pub fn cancel(&mut self, plugin: &str) {
        if let Some(job) = self.pending.remove(plugin) {
            job.cancel.store(true, Ordering::Release);
            let status = self.public.entry(plugin.into()).or_default();
            status.busy = false;
            status.message =
                "Connection cancelled. Refresh account status to check the connection.".into();
            status.error = false;
        }
    }
    pub fn cancel_all(&mut self) {
        let plugins = self.pending.keys().cloned().collect::<Vec<_>>();
        for plugin in plugins {
            self.cancel(&plugin);
        }
    }
    pub fn clear(&mut self) {
        self.cancel_all();
        self.public.clear();
    }
}
impl Drop for AccountSessions {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

/// External navigation is an explicit account action, never a model tool.
pub fn external_url(value: &str) -> Result<String> {
    ensure!(
        value.len() <= 16384 && !value.chars().any(|c| c.is_control() || c.is_whitespace()),
        "Invalid account URL"
    );
    let url = url::Url::parse(value)?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none(),
        "Account URLs must use HTTPS without embedded credentials or fragments"
    );
    Ok(url.into())
}
