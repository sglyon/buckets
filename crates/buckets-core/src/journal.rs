//! Typed, durable file operations. The journal records intent before mutation and
//! never automatically replays an operation whose outcome is uncertain.
use crate::{Backend, Location};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const MAX_RECORDS: usize = 128;
const MAX_ACTIONS: usize = 1024;
const MAX_JOURNAL_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TREE_NODES: u64 = 100_000;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperationAction {
    Copy {
        source: Location,
        destination: Location,
    },
    Move {
        source: Location,
        destination: Location,
    },
    Duplicate {
        source: Location,
    },
    CreateFile {
        destination: Location,
    },
    CreateDirectory {
        destination: Location,
    },
    Trash {
        source: Location,
    },
}
impl OperationAction {
    fn source(&self) -> &Location {
        match self {
            Self::Copy { source, .. }
            | Self::Move { source, .. }
            | Self::Duplicate { source }
            | Self::Trash { source } => source,
            Self::CreateFile { destination } | Self::CreateDirectory { destination } => destination,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationPlan {
    pub label: String,
    pub actions: Vec<OperationAction>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationState {
    Running,
    Complete,
    Failed,
    Cancelled,
    Interrupted,
    Undone,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationItemState {
    Pending,
    Running,
    Complete,
    Failed,
    Uncertain,
    Undone,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationItem {
    pub action: OperationAction,
    pub state: OperationItemState,
    pub error: Option<String>,
    pub undo_available: bool,
    pub undo_reason: Option<String>,
    #[serde(default)]
    pub staging: Option<Location>,
    receipt: Option<UndoReceipt>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationRecord {
    pub id: String,
    pub label: String,
    pub plan: OperationPlan,
    pub state: OperationState,
    pub items: Vec<OperationItem>,
    pub error: Option<String>,
    pub undo_available: bool,
    pub undo_reason: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}
impl OperationRecord {
    /// Presentation can expose one consistent recovery decision; execution
    /// methods revalidate it under the journal lock.
    pub fn can_retry(&self) -> bool {
        self.retry_reason().is_none()
    }
    pub fn retry_reason(&self) -> Option<&'static str> {
        if !matches!(
            self.state,
            OperationState::Failed | OperationState::Cancelled | OperationState::Interrupted
        ) {
            return Some("This operation does not need retrying");
        }
        if self.items.iter().any(|item| {
            matches!(
                item.state,
                OperationItemState::Running | OperationItemState::Uncertain
            )
        }) {
            return Some(
                "An item has an uncertain outcome; inspect its paths instead of replaying it",
            );
        }
        if !self.items.iter().any(|item| {
            matches!(
                item.state,
                OperationItemState::Pending | OperationItemState::Failed
            )
        }) {
            return Some("There are no incomplete items to retry");
        }
        None
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationProgress {
    pub id: String,
    pub label: String,
    pub state: OperationState,
    pub completed_items: usize,
    pub total_items: usize,
    pub bytes_copied: u64,
    pub current: Option<Location>,
    pub stage: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UndoReceipt {
    current: Location,
    original: Option<Location>,
    fingerprint: Fingerprint,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Fingerprint {
    canonical: PathBuf,
    digest: String,
    identity: Option<(u64, u64)>,
}
struct CachedNode {
    metadata: fs::Metadata,
    content: Option<[u8; 32]>,
}
type ContentCache = BTreeMap<PathBuf, CachedNode>;
struct Applied {
    current: Option<Location>,
    original: Option<Location>,
    expected: Option<Fingerprint>,
}
#[derive(Default, Serialize, Deserialize)]
struct SavedJournal {
    version: u32,
    records: Vec<OperationRecord>,
}

pub struct OperationJournal {
    directory: PathBuf,
    records: Vec<OperationRecord>,
    trash_override: Option<PathBuf>,
}
impl OperationJournal {
    /// `directory` is an application-owned state directory, never a browsed root.
    pub fn open(directory: impl AsRef<Path>) -> Result<Self> {
        Self::open_impl(directory.as_ref(), None)
    }
    /// Explicit private trash is useful for isolated tests and portable hosts.
    pub fn open_with_trash(directory: impl AsRef<Path>, trash: impl AsRef<Path>) -> Result<Self> {
        Self::open_impl(directory.as_ref(), Some(trash.as_ref().to_owned()))
    }
    fn open_impl(directory: &Path, trash_override: Option<PathBuf>) -> Result<Self> {
        private_directory(directory)?;
        if let Some(trash) = &trash_override {
            ensure!(
                trash.is_absolute(),
                "Explicit trash directory must be absolute"
            );
        }
        let mut this = Self {
            directory: fs::canonicalize(directory)?,
            records: vec![],
            trash_override,
        };
        let _lock = this.lock()?;
        this.reload()?;
        if interrupt_records(&mut this.records) {
            this.save()?;
        }

        Ok(this)
    }
    pub fn records(&self) -> Vec<OperationRecord> {
        self.records.clone()
    }
    pub fn execute(
        &mut self,
        plan: OperationPlan,
        cancel: &AtomicBool,
        progress: impl Fn(OperationProgress),
    ) -> Result<OperationRecord> {
        for action in &plan.actions {
            validate_action(action)?;
        }
        ensure!(!plan.actions.is_empty(), "Select at least one file");
        ensure!(
            plan.actions.len() <= MAX_ACTIONS,
            "An operation supports at most {MAX_ACTIONS} selected entries"
        );
        ensure!(
            !plan.label.is_empty() && plan.label.len() <= 128,
            "Invalid operation label"
        );
        let _lock = self.lock()?;
        self.reload()?;
        if self.records.len() >= MAX_RECORDS {
            self.records.remove(0);
        }
        let now = timestamp();
        let record = OperationRecord {
            id: format!(
                "{:x}-{:x}-{:x}",
                now,
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ),
            label: plan.label.clone(),
            items: plan
                .actions
                .iter()
                .cloned()
                .map(|action| OperationItem {
                    action,
                    state: OperationItemState::Pending,
                    error: None,
                    receipt: None,
                    staging: None,
                    undo_available: false,
                    undo_reason: Some("Operation has not completed".into()),
                })
                .collect(),
            plan,
            state: OperationState::Running,
            error: None,
            undo_available: false,
            undo_reason: Some("Operation is running".into()),
            created_at_ms: now,
            updated_at_ms: now,
        };
        self.records.push(record);
        let index = self.records.len() - 1;
        self.save()?;
        self.run(index, cancel, &progress)
    }
    /// Retry only known incomplete actions. An interrupted Running item may have
    /// mutated a path before its receipt was committed and cannot be replayed.
    pub fn retry(
        &mut self,
        id: &str,
        cancel: &AtomicBool,
        progress: impl Fn(OperationProgress),
    ) -> Result<OperationRecord> {
        let _lock = self.lock()?;
        self.reload()?;
        let index = self.index(id)?;
        let record = &mut self.records[index];
        ensure!(
            record.can_retry(),
            "{}",
            record.retry_reason().unwrap_or("Retry is unavailable")
        );
        record.state = OperationState::Running;
        record.error = None;
        self.save()?;
        self.run(index, cancel, &progress)
    }
    pub fn undo(
        &mut self,
        id: &str,
        cancel: &AtomicBool,
        progress: impl Fn(OperationProgress),
    ) -> Result<OperationRecord> {
        let _lock = self.lock()?;
        self.reload()?;
        let index = self.index(id)?;
        ensure!(
            self.records[index].state != OperationState::Running,
            "Wait for the operation to finish"
        );
        ensure!(
            self.records[index].undo_available,
            "{}",
            self.records[index]
                .undo_reason
                .as_deref()
                .unwrap_or("This operation cannot be undone")
        );
        let prior_state = self.records[index].state;
        let mut event = self.progress(index, "Checking recovery");
        event.state = OperationState::Running;
        event.completed_items = 0;
        event.total_items = self.records[index]
            .items
            .iter()
            .filter(|item| item.state == OperationItemState::Complete)
            .count();
        progress(event.clone());
        for item_index in (0..self.records[index].items.len()).rev() {
            if cancel.load(Ordering::Acquire) {
                break;
            }
            let item = &self.records[index].items[item_index];
            if item.state != OperationItemState::Complete {
                continue;
            }
            let receipt = item
                .receipt
                .clone()
                .context("Recovery receipt is unavailable")?;
            let checked = (|| -> Result<()> {
                if let Some(original) = &receipt.original {
                    ensure_absent(original)?;
                }
                Ok(())
            })();
            if let Err(error) = checked {
                self.records[index].state = if cancel.load(Ordering::Acquire) {
                    OperationState::Cancelled
                } else {
                    prior_state
                };
                self.records[index].error = Some(format!("Undo stopped: {error:#}"));
                self.records[index].updated_at_ms = timestamp();
                update_undo(&mut self.records[index]);
                self.save()?;
                return Ok(self.records[index].clone());
            }
            // Commit inverse intent before moving anything. If the process dies,
            // the Running item is uncertain and cannot be silently retried.
            self.records[index].state = OperationState::Running;
            self.records[index].items[item_index].state = OperationItemState::Running;
            self.records[index].updated_at_ms = timestamp();
            self.save()?;
            event.current = Some(receipt.current.clone());
            event.stage = "Recovering".into();
            progress(event.clone());
            // Guard the operation-owned version after persisting inverse intent,
            // immediately before the effect. This avoids both a second full read
            // and accepting changes made while the journal was being synced.
            let checked = fingerprint_cancel(&receipt.current, Some(cancel)).and_then(|actual| {
                ensure!(
                    actual == receipt.fingerprint,
                    "File or directory changed after this operation; undo refused"
                );
                Ok(())
            });
            if let Err(error) = checked {
                self.records[index].items[item_index].state = OperationItemState::Complete;
                self.records[index].state = if cancel.load(Ordering::Acquire) {
                    OperationState::Cancelled
                } else {
                    prior_state
                };
                self.records[index].error = Some(format!("Undo stopped: {error:#}"));
                self.records[index].updated_at_ms = timestamp();
                update_undo(&mut self.records[index]);
                self.save()?;
                return Ok(self.records[index].clone());
            }
            let result = if let Some(original) = &receipt.original {
                Backend.move_entry(&receipt.current, original)
            } else {
                Backend
                    .trash_with_receipt_at(&receipt.current, self.trash_override.as_deref())
                    .map(|_| ())
            };
            match result {
                Ok(()) => {
                    self.records[index].items[item_index].state = OperationItemState::Undone;
                    self.records[index].items[item_index].undo_available = false;
                    self.records[index].items[item_index].error = None;
                    self.records[index].updated_at_ms = timestamp();
                    self.save()?;
                    event.completed_items += 1;
                    event.current = Some(receipt.current);
                    event.stage = "Recovered".into();
                    progress(event.clone());
                }
                Err(error) => {
                    self.records[index].items[item_index].state = OperationItemState::Uncertain;
                    self.records[index].state = OperationState::Interrupted;
                    self.records[index].error = Some(format!(
                        "Undo outcome is uncertain: {error:#}. Inspect the affected paths before further changes."
                    ));
                    self.records[index].updated_at_ms = timestamp();
                    update_undo(&mut self.records[index]);
                    self.save()?;
                    return Ok(self.records[index].clone());
                }
            }
        }
        let record = &mut self.records[index];
        let cancelled = cancel.load(Ordering::Acquire);
        record.error = cancelled.then(|| "Undo stopped; completed recoveries are retained".into());
        record.state = if !record.items.iter().any(|item| {
            matches!(
                item.state,
                OperationItemState::Complete
                    | OperationItemState::Running
                    | OperationItemState::Uncertain
            )
        }) {
            OperationState::Undone
        } else if cancelled {
            OperationState::Cancelled
        } else {
            prior_state
        };
        record.updated_at_ms = timestamp();
        update_undo(record);
        self.save()?;
        event.state = self.records[index].state;
        event.stage = if cancelled {
            "Recovery stopped"
        } else {
            "Recovery complete"
        }
        .into();
        progress(event);
        Ok(self.records[index].clone())
    }

    fn run(
        &mut self,
        index: usize,
        cancel: &AtomicBool,
        progress: &impl Fn(OperationProgress),
    ) -> Result<OperationRecord> {
        let mut event = self.progress(index, "Preparing");
        progress(event.clone());
        for item_index in 0..self.records[index].items.len() {
            if matches!(
                self.records[index].items[item_index].state,
                OperationItemState::Complete | OperationItemState::Undone
            ) {
                continue;
            }
            if cancel.load(Ordering::Acquire) {
                self.records[index].state = OperationState::Cancelled;
                break;
            }
            let action = self.records[index].items[item_index].action.clone();
            let action = match prepare_action(action) {
                Ok(action) => action,
                Err(error) => {
                    let message = format!("{error:#}");
                    self.records[index].items[item_index].state = OperationItemState::Failed;
                    self.records[index].items[item_index].error = Some(message.clone());
                    self.records[index].state = OperationState::Failed;
                    self.records[index].error = Some(message);
                    break;
                }
            };
            self.records[index].items[item_index].action = action.clone();
            self.records[index].items[item_index].state = OperationItemState::Running;
            self.records[index].items[item_index].error = None;
            self.save()?;
            event.current = Some(action.source().clone());
            event.stage = if local_action(&action) {
                "Working"
            } else {
                "Provider operation · stop after this item"
            }
            .into();
            progress(event.clone());
            let trash = self.trash_override.clone();
            let result = apply(
                &action,
                cancel,
                &mut event,
                progress,
                trash.as_deref(),
                |path| {
                    self.records[index].items[item_index].staging = Some(Location::local(path));
                    self.save()
                },
            );
            if self.records[index].items[item_index].staging.as_ref().is_some_and(|location| {
                matches!(location, Location::Local { path } if matches!(fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound))
            }) { self.records[index].items[item_index].staging = None; }
            match result {
                Ok(applied) => {
                    event.stage = "Checking recovery".into();
                    progress(event.clone());
                    let receipt =
                        applied
                            .current
                            .zip(applied.expected)
                            .map(|(current, fingerprint)| UndoReceipt {
                                current,
                                original: applied.original,
                                fingerprint,
                            });
                    let item = &mut self.records[index].items[item_index];
                    item.state = OperationItemState::Complete;
                    item.undo_available = receipt.is_some();
                    item.undo_reason = receipt
                        .is_none()
                        .then(|| "Recovery is unavailable for this provider or fingerprint".into());
                    item.receipt = receipt;
                    event.completed_items += 1;
                    self.records[index].updated_at_ms = timestamp();
                    self.save()?;
                    progress(event.clone());
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    let cancelled = cancel.load(Ordering::Acquire);
                    let item = &mut self.records[index].items[item_index];
                    item.state = if error.is::<BeforeEffectCancellation>()
                        || safely_retryable_failure(&action)
                    {
                        OperationItemState::Failed
                    } else {
                        OperationItemState::Uncertain
                    };
                    item.error = Some(message.clone());
                    self.records[index].state = if cancelled {
                        OperationState::Cancelled
                    } else {
                        OperationState::Failed
                    };
                    self.records[index].error = Some(message);
                    break;
                }
            }
        }
        let record = &mut self.records[index];
        if record.state == OperationState::Running {
            record.state = if cancel.load(Ordering::Acquire)
                && record
                    .items
                    .iter()
                    .any(|item| item.state != OperationItemState::Complete)
            {
                OperationState::Cancelled
            } else {
                OperationState::Complete
            };
        }
        record.updated_at_ms = timestamp();
        update_undo(record);
        self.save()?;
        event.state = self.records[index].state;
        event.stage = format!("{:?}", event.state);
        progress(event);
        Ok(self.records[index].clone())
    }
    fn progress(&self, index: usize, stage: &str) -> OperationProgress {
        let record = &self.records[index];
        OperationProgress {
            id: record.id.clone(),
            label: record.label.clone(),
            state: record.state,
            completed_items: record
                .items
                .iter()
                .filter(|item| item.state == OperationItemState::Complete)
                .count(),
            total_items: record.items.len(),
            bytes_copied: 0,
            current: None,
            stage: stage.into(),
        }
    }
    fn index(&self, id: &str) -> Result<usize> {
        self.records
            .iter()
            .position(|record| record.id == id)
            .context("Operation is no longer in the bounded history")
    }
    fn lock(&self) -> Result<JournalLock> {
        JournalLock::acquire(&self.directory.join("journal.lock"))
    }
    fn reload(&mut self) -> Result<()> {
        let path = self.directory.join("journal.json");
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_file() && !metadata.file_type().is_symlink(),
                    "Journal must be a regular file"
                );
                check_private_file(&metadata)?;
                ensure!(
                    metadata.len() <= MAX_JOURNAL_BYTES,
                    "Operation journal exceeds its size bound"
                );
                let file = open_read_no_follow(&path)?;
                check_private_file(&file.metadata()?)?;
                let mut bytes = Vec::new();
                file.take(MAX_JOURNAL_BYTES + 1).read_to_end(&mut bytes)?;
                ensure!(
                    bytes.len() as u64 <= MAX_JOURNAL_BYTES,
                    "Operation journal exceeds its size bound"
                );
                let saved: SavedJournal = serde_json::from_slice(&bytes)?;
                ensure!(
                    saved.version == 1 && saved.records.len() <= MAX_RECORDS,
                    "Unsupported operation journal"
                );
                for record in &saved.records {
                    for action in &record.plan.actions {
                        validate_action(action)?;
                    }
                    for item in &record.items {
                        validate_action(&item.action)?;
                        if let Some(staging) = &item.staging {
                            validate_location(staging)?;
                        }
                        if let Some(receipt) = &item.receipt {
                            validate_location(&receipt.current)?;
                            if let Some(original) = &receipt.original {
                                validate_location(original)?;
                            }
                            ensure!(
                                receipt.fingerprint.canonical.is_absolute(),
                                "Recovery path must be absolute"
                            );
                        }
                    }
                    ensure!(
                        record.items.len() <= MAX_ACTIONS
                            && record.items.len() == record.plan.actions.len(),
                        "Invalid operation journal entries"
                    );
                }
                self.records = saved.records;
                if interrupt_records(&mut self.records) {
                    self.save()?;
                }
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.records.clear();
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }
    fn save(&self) -> Result<()> {
        let bytes = serde_json::to_vec(&SavedJournal {
            version: 1,
            records: self.records.clone(),
        })?;
        ensure!(
            bytes.len() as u64 <= MAX_JOURNAL_BYTES,
            "Operation journal is full; inspect affected paths before continuing"
        );
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(self.directory.join("journal.json"))
            .map_err(|error| error.error)?;
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
}
fn update_undo(record: &mut OperationRecord) {
    let completed: Vec<_> = record
        .items
        .iter()
        .filter(|item| item.state == OperationItemState::Complete)
        .collect();
    record.undo_available = !record.items.iter().any(|item| {
        matches!(
            item.state,
            OperationItemState::Running | OperationItemState::Uncertain
        )
    }) && !completed.is_empty()
        && completed
            .iter()
            .all(|item| item.undo_available && item.receipt.is_some());
    record.undo_reason = if record.undo_available {
        None
    } else if record.items.iter().any(|item| {
        matches!(
            item.state,
            OperationItemState::Running | OperationItemState::Uncertain
        )
    }) {
        Some("An item has an uncertain outcome; inspect its paths before recovery".into())
    } else if completed.is_empty() {
        Some("No completed changes to undo".into())
    } else {
        Some("Some completed items do not support guarded recovery".into())
    };
}
fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn local_action(action: &OperationAction) -> bool {
    match action {
        OperationAction::Copy {
            source,
            destination,
        }
        | OperationAction::Move {
            source,
            destination,
        } => matches!(
            (source, destination),
            (Location::Local { .. }, Location::Local { .. })
        ),
        _ => matches!(action.source(), Location::Local { .. }),
    }
}
fn apply(
    action: &OperationAction,
    cancel: &AtomicBool,
    event: &mut OperationProgress,
    progress: &impl Fn(OperationProgress),
    trash: Option<&Path>,
    mut staging: impl FnMut(&Path) -> Result<()>,
) -> Result<Applied> {
    check_cancel(cancel)?;
    match action {
        OperationAction::Copy {
            source,
            destination,
        } => {
            let expected =
                if let (Location::Local { path: source }, Location::Local { path: destination }) =
                    (source, destination)
                {
                    copy_local(source, destination, cancel, event, progress, &mut staging)?
                } else {
                    Backend.copy(source, destination)?;
                    None
                };
            Ok(Applied {
                current: Some(destination.clone()),
                original: None,
                expected,
            })
        }
        OperationAction::Move {
            source,
            destination,
        } => {
            event.stage = "Checking recovery version".into();
            progress(event.clone());
            let expected = if local_action(action) {
                fingerprint_cancel(source, Some(cancel)).ok()
            } else {
                None
            };
            check_cancel(cancel)?;
            Backend.move_entry(source, destination)?;
            let expected = expected.and_then(|expected| rebind(expected, destination).ok());
            Ok(Applied {
                current: Some(destination.clone()),
                original: Some(source.clone()),
                expected,
            })
        }
        OperationAction::Duplicate { .. } => {
            unreachable!("Duplicate is resolved to a fixed Copy before write-ahead intent")
        }
        OperationAction::CreateFile {
            destination: Location::Local { path },
        } => {
            let expected = create_local(path, false, cancel, &mut staging)?;
            Ok(Applied {
                current: Some(Location::local(path)),
                original: None,
                expected: Some(expected),
            })
        }
        OperationAction::CreateDirectory {
            destination: Location::Local { path },
        } => {
            let expected = create_local(path, true, cancel, &mut staging)?;
            Ok(Applied {
                current: Some(Location::local(path)),
                original: None,
                expected: Some(expected),
            })
        }
        OperationAction::CreateFile { destination } => {
            Backend.create_file(destination)?;
            Ok(Applied {
                current: Some(destination.clone()),
                original: None,
                expected: None,
            })
        }
        OperationAction::CreateDirectory { destination } => {
            Backend.create_dir(destination)?;
            Ok(Applied {
                current: Some(destination.clone()),
                original: None,
                expected: None,
            })
        }
        OperationAction::Trash { source } => {
            event.stage = "Checking recovery version".into();
            progress(event.clone());
            let expected = if matches!(source, Location::Local { .. }) {
                fingerprint_cancel(source, Some(cancel)).ok()
            } else {
                None
            };
            check_cancel(cancel)?;
            let current = Backend.trash_with_receipt_at(source, trash)?;
            let expected = expected.and_then(|expected| {
                current
                    .as_ref()
                    .and_then(|current| rebind(expected, current).ok())
            });
            Ok(Applied {
                current,
                original: Some(source.clone()),
                expected,
            })
        }
    }
}
fn rebind(mut fingerprint: Fingerprint, location: &Location) -> Result<Fingerprint> {
    let Location::Local { path } = location else {
        bail!("Recovery requires local identity");
    };
    ensure!(
        fingerprint.identity.is_some()
            && fingerprint.identity == identity(&fs::symlink_metadata(path)?),
        "Cross-filesystem move recovery is unavailable"
    );
    fingerprint.canonical = canonical_entry(path)?;
    Ok(fingerprint)
}
fn create_local(
    destination: &Path,
    directory: bool,
    cancel: &AtomicBool,
    staging: &mut impl FnMut(&Path) -> Result<()>,
) -> Result<Fingerprint> {
    let parent = destination.parent().context("Destination has no parent")?;
    let stage = tempfile::Builder::new()
        .prefix(".buckets-copy-")
        .tempdir_in(parent)?;
    staging(stage.path())?;
    let staged = stage.path().join("entry");
    if directory {
        fs::create_dir(&staged)?;
    } else {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)?
            .sync_all()?;
    }
    let mut expected = fingerprint(&Location::local(&staged))?;
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Creation cancelled; destination was not published"
    );
    crate::operations::rename_exclusive(&staged, destination)?;
    expected.canonical = canonical_entry(destination)?;
    Ok(expected)
}

fn duplicate_destination(source: &Location) -> Result<Location> {
    let parent = source
        .parent()
        .context("Cannot duplicate a filesystem root")?;
    let entry = Backend.stat(source)?;
    let entries = Backend.list(&parent)?;
    let name = source.name();
    let path = Path::new(&name);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    for number in 1..=10000 {
        let suffix = if number == 1 {
            " copy".to_owned()
        } else {
            format!(" copy {number}")
        };
        let name = if entry.is_dir {
            format!("{name}{suffix}")
        } else {
            format!("{stem}{suffix}{extension}")
        };
        if !entries.iter().any(|entry| entry.name == name) {
            return parent.join(&name);
        }
    }
    bail!("No available duplicate name")
}
fn copy_local(
    source: &Path,
    destination: &Path,
    cancel: &AtomicBool,
    event: &mut OperationProgress,
    progress: &impl Fn(OperationProgress),
    staging: &mut impl FnMut(&Path) -> Result<()>,
) -> Result<Option<Fingerprint>> {
    crate::operations::validate_destination(source, destination)?;
    let parent = destination.parent().context("Destination has no parent")?;
    let stage = tempfile::Builder::new()
        .prefix(".buckets-copy-")
        .tempdir_in(parent)?;
    staging(stage.path())?;
    let staged = stage.path().join("entry");
    let mut walk = CopyWalk {
        cancel,
        event,
        progress,
        last_emit: Instant::now(),
        content: ContentCache::new(),
    };
    walk.copy_into(source, &staged, 0)?;
    let content = walk.content;
    event.stage = "Checking recovery version".into();
    progress(event.clone());
    // Version belongs to the private staging tree, never to an arbitrary editor
    // change observed after publication. Content digests reuse the copy pass.
    let expected = fingerprint_cached(&Location::local(&staged), Some(&content), Some(cancel)).ok();
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Copy cancelled; destination was not published"
    );
    crate::operations::rename_exclusive(&staged, destination)?;
    Ok(expected.and_then(|mut expected| {
        expected.canonical = canonical_entry(destination).ok()?;
        Some(expected)
    }))
}

struct CopyWalk<'a, F> {
    cancel: &'a AtomicBool,
    event: &'a mut OperationProgress,
    progress: &'a F,
    last_emit: Instant,
    content: ContentCache,
}
impl<F: Fn(OperationProgress)> CopyWalk<'_, F> {
    fn copy_into(&mut self, source: &Path, destination: &Path, depth: usize) -> Result<()> {
        ensure!(depth <= 128, "Directory nesting exceeds copy limit");
        ensure!(
            !self.cancel.load(Ordering::Acquire),
            "Copy cancelled; destination was not published"
        );
        let metadata = fs::symlink_metadata(source)?;
        let mut copied_digest = None;
        if metadata.file_type().is_symlink() {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(fs::read_link(source)?, destination)?;
            }
            #[cfg(not(unix))]
            bail!("Symlink copying is unavailable on this platform");
        } else if metadata.is_dir() {
            fs::create_dir(destination)?;
            for entry in fs::read_dir(source)? {
                let entry = entry?;
                self.copy_into(
                    &entry.path(),
                    &destination.join(entry.file_name()),
                    depth + 1,
                )?;
            }
            fs::set_permissions(destination, metadata.permissions())?;
        } else {
            ensure!(metadata.is_file(), "Special files cannot be copied");
            let mut input = open_read_no_follow(source)?;
            ensure!(
                same_identity(&metadata, &input.metadata()?),
                "Source changed while opening"
            );
            ensure!(
                !fs::symlink_metadata(source)?.file_type().is_symlink(),
                "Source changed to a symbolic link"
            );
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)?;
            let mut buffer = [0u8; 64 * 1024];
            let mut digest = Sha256::new();
            loop {
                ensure!(
                    !self.cancel.load(Ordering::Acquire),
                    "Copy cancelled; destination was not published"
                );
                let size = input.read(&mut buffer)?;
                if size == 0 {
                    break;
                }
                output.write_all(&buffer[..size])?;
                digest.update(&buffer[..size]);
                self.event.bytes_copied = self.event.bytes_copied.saturating_add(size as u64);
                self.event.stage = "Copying".into();
                if self.event.bytes_copied == size as u64
                    || self.last_emit.elapsed().as_millis() >= 40
                {
                    (self.progress)(self.event.clone());
                    self.last_emit = Instant::now();
                }
            }
            output.sync_all()?;
            copied_digest = Some(digest.finalize().into());
            fs::set_permissions(destination, metadata.permissions())?;
            let after = fs::symlink_metadata(source)?;
            ensure!(
                metadata.len() == after.len()
                    && metadata.modified().ok() == after.modified().ok()
                    && same_identity(&metadata, &after)
                    && !after.file_type().is_symlink(),
                "Source changed while copying; destination was not published"
            );
        }
        if self.content.len() < MAX_TREE_NODES as usize {
            self.content.insert(
                destination.to_owned(),
                CachedNode {
                    metadata: fs::symlink_metadata(destination)?,
                    content: copied_digest,
                },
            );
        }
        Ok(())
    }
}

fn fingerprint(location: &Location) -> Result<Fingerprint> {
    fingerprint_cancel(location, None)
}
fn fingerprint_cancel(location: &Location, cancel: Option<&AtomicBool>) -> Result<Fingerprint> {
    fingerprint_cached(location, None, cancel)
}
fn fingerprint_cached(
    location: &Location,
    content: Option<&ContentCache>,
    cancel: Option<&AtomicBool>,
) -> Result<Fingerprint> {
    let Location::Local { path } = location else {
        bail!("Provider does not expose a guarded recovery version");
    };
    let canonical = fs::canonicalize(path.parent().context("No parent directory")?)?.join(
        path.file_name()
            .context("Cannot fingerprint a filesystem root")?,
    );
    let mut digest = Sha256::new();
    let mut nodes = 0;
    hash_node(path, &mut digest, &mut nodes, 0, content, cancel)?;
    Ok(Fingerprint {
        canonical,
        digest: format!("{:x}", digest.finalize()),
        identity: identity(&fs::symlink_metadata(path)?),
    })
}
fn hash_node(
    path: &Path,
    digest: &mut Sha256,
    nodes: &mut u64,
    depth: usize,
    content: Option<&ContentCache>,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    if let Some(cancel) = cancel {
        check_cancel(cancel)?;
    }
    ensure!(
        depth <= 128,
        "Directory nesting exceeds recovery fingerprint limit"
    );
    *nodes += 1;
    ensure!(
        *nodes <= MAX_TREE_NODES,
        "Directory exceeds recovery fingerprint bound"
    );
    let metadata = fs::symlink_metadata(path)?;
    if let Some(content) = content {
        let expected = &content
            .get(path)
            .context("Copy tree changed or exceeds recovery cache bound")?
            .metadata;
        ensure!(
            same_identity(&metadata, expected)
                && metadata.len() == expected.len()
                && metadata.modified().ok() == expected.modified().ok(),
            "Destination changed before its recovery version was recorded"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        digest.update(metadata.mode().to_le_bytes());
        digest.update(metadata.ino().to_le_bytes());
        digest.update(metadata.dev().to_le_bytes());
        digest.update(metadata.mtime().to_le_bytes());
        digest.update(metadata.mtime_nsec().to_le_bytes());
    }
    if metadata.file_type().is_symlink() {
        digest.update(b"link\0");
        let target = fs::read_link(path)?;
        digest.update(target.as_os_str().as_encoded_bytes());
    } else if metadata.is_dir() {
        digest.update(b"directory\0");
        let mut entries = fs::read_dir(path)?
            .take(MAX_TREE_NODES as usize + 1)
            .collect::<std::io::Result<Vec<_>>>()?;
        ensure!(
            entries.len() as u64 <= MAX_TREE_NODES - *nodes,
            "Directory exceeds recovery fingerprint bound"
        );
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            let name = name.as_encoded_bytes();
            digest.update((name.len() as u64).to_le_bytes());
            digest.update(name);
            hash_node(&entry.path(), digest, nodes, depth + 1, content, cancel)?;
        }
    } else {
        ensure!(
            metadata.is_file(),
            "Special files do not support guarded recovery"
        );
        digest.update(b"file\0");
        digest.update(metadata.len().to_le_bytes());
        let file_digest = if let Some(content) = content {
            content
                .get(path)
                .and_then(|node| node.content)
                .context("Copy cache omitted file digest")?
        } else {
            let mut input = open_read_no_follow(path)?;
            ensure!(
                same_identity(&metadata, &input.metadata()?),
                "File changed while opening recovery version"
            );
            let mut buffer = [0u8; 64 * 1024];
            let mut file_digest = Sha256::new();
            loop {
                if let Some(cancel) = cancel {
                    check_cancel(cancel)?;
                }
                let size = input.read(&mut buffer)?;
                if size == 0 {
                    break;
                }
                file_digest.update(&buffer[..size]);
            }
            file_digest.finalize().into()
        };
        digest.update(file_digest);
        let after = fs::symlink_metadata(path)?;
        ensure!(
            metadata.len() == after.len()
                && metadata.modified().ok() == after.modified().ok()
                && same_identity(&metadata, &after),
            "File changed while computing recovery version"
        );
    }
    Ok(())
}
fn private_directory(path: &Path) -> Result<()> {
    let mut ancestor = PathBuf::new();
    for component in path.components() {
        ancestor.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&ancestor) {
            ensure!(
                !metadata.file_type().is_symlink() || system_alias(&ancestor),
                "Operation state directory must not traverse a symbolic link"
            );
        }
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        check_owner(&metadata)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Operation state must be a real directory"
        );
    }
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
struct JournalLock {
    _file: File,
}
impl JournalLock {
    fn acquire(path: &Path) -> Result<Self> {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "Operation lock must be a regular file"
            );
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(0x100);
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(0x20000);
        }
        let file = options.open(path)?;
        check_private_file(&file.metadata()?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            ensure!(
                file.metadata()?.nlink() == 1,
                "Operation lock must not have hard links"
            );
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe extern "C" {
                fn flock(fd: std::ffi::c_int, operation: std::ffi::c_int) -> std::ffi::c_int;
            }
            ensure!(
                unsafe { flock(file.as_raw_fd(), 2 | 4) } == 0,
                "Another Buckets process is executing a file operation"
            );
        }
        Ok(Self { _file: file })
    }
}

fn open_read_no_follow(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x100);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x20000);
    }
    Ok(options.open(path)?)
}
fn same_identity(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.ino() == b.ino() && a.dev() == b.dev()
    }
    #[cfg(not(unix))]
    {
        a.is_dir() == b.is_dir() && a.is_file() == b.is_file() && a.len() == b.len()
    }
}
fn prepare_action(action: OperationAction) -> Result<OperationAction> {
    let action = if let OperationAction::Duplicate { source } = action {
        let destination = duplicate_destination(&source)?;
        OperationAction::Copy {
            source,
            destination,
        }
    } else {
        action
    };
    match &action {
        OperationAction::Copy {
            source,
            destination,
        }
        | OperationAction::Move {
            source,
            destination,
        } => {
            Backend.stat(source)?;
            ensure!(source != destination, "Source and destination are the same");
            ensure_absent(destination)?;
            if let (Location::Local { path: source }, Location::Local { path: destination }) =
                (source, destination)
            {
                crate::operations::validate_destination(source, destination)?;
            }
        }
        OperationAction::CreateFile { destination }
        | OperationAction::CreateDirectory { destination } => ensure_absent(destination)?,
        OperationAction::Trash { source } => {
            Backend.stat(source)?;
        }
        OperationAction::Duplicate { .. } => unreachable!(),
    }
    Ok(action)
}
fn ensure_absent(destination: &Location) -> Result<()> {
    if let Location::Local { path } = destination {
        match fs::symlink_metadata(path) {
            Ok(_) => bail!("Destination already exists: {destination}"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    let parent = destination
        .parent()
        .context("Cannot replace a provider root")?;
    ensure!(
        !Backend
            .list(&parent)?
            .iter()
            .any(|entry| entry.location == *destination || entry.name == destination.name()),
        "Destination already exists: {destination}"
    );
    Ok(())
}
fn safely_retryable_failure(action: &OperationAction) -> bool {
    match action {
        OperationAction::Copy {
            source,
            destination,
        } => matches!(
            (source, destination),
            (Location::Local { .. }, Location::Local { .. })
        ),
        OperationAction::Move {
            source: Location::Local { path: source },
            destination: Location::Local { path: destination },
        } => {
            fs::symlink_metadata(source).is_ok()
                && matches!(fs::symlink_metadata(destination), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        }
        OperationAction::CreateFile {
            destination: Location::Local { path },
        }
        | OperationAction::CreateDirectory {
            destination: Location::Local { path },
        } => {
            matches!(fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        }
        _ => false,
    }
}

fn check_owner(metadata: &fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        unsafe extern "C" {
            fn getuid() -> u32;
        }
        ensure!(
            metadata.uid() == unsafe { getuid() },
            "Operation state must be owned by the current user"
        );
    }
    let _ = metadata;
    Ok(())
}
fn check_private_file(metadata: &fs::Metadata) -> Result<()> {
    check_owner(metadata)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.nlink() == 1 && metadata.mode() & 0o077 == 0,
            "Operation state files must be private and have no hard links"
        );
    }
    Ok(())
}

fn canonical_entry(path: &Path) -> Result<PathBuf> {
    Ok(
        fs::canonicalize(path.parent().context("No parent directory")?)?.join(
            path.file_name()
                .context("Cannot operate on a filesystem root")?,
        ),
    )
}
fn identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}
fn system_alias(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        for (alias, target) in [
            ("/var", "/private/var"),
            ("/tmp", "/private/tmp"),
            ("/etc", "/private/etc"),
        ] {
            if path == Path::new(alias)
                && fs::canonicalize(path).ok().as_deref() == Some(Path::new(target))
            {
                return true;
            }
        }
    }
    let _ = path;
    false
}
fn validate_action(action: &OperationAction) -> Result<()> {
    match action {
        OperationAction::Copy {
            source,
            destination,
        }
        | OperationAction::Move {
            source,
            destination,
        } => {
            validate_location(source)?;
            validate_location(destination)?;
        }
        _ => validate_location(action.source())?,
    }
    Ok(())
}
fn validate_location(location: &Location) -> Result<()> {
    match location {
        Location::Local { path } => ensure!(path.is_absolute(), "Operation paths must be absolute"),
        Location::Ssh { host, .. } => {
            crate::location::validate_host(host)?;
            if let Some((user, _)) = host.split_once('@') {
                ensure!(
                    !user.contains(':'),
                    "SSH passwords must not be stored in operation plans"
                );
            }
        }
        Location::S3 {
            endpoint: Some(endpoint),
            ..
        } => {
            let url = url::Url::parse(endpoint)?;
            ensure!(
                url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "S3 credentials must stay in AWS configuration, not operation plans"
            );
        }
        Location::Git { repo, .. } => {
            ensure!(repo.is_absolute(), "Git repository paths must be absolute")
        }
        _ => {}
    }
    Ok(())
}

fn interrupt_records(records: &mut [OperationRecord]) -> bool {
    let mut changed = false;
    for record in records {
        if record.state == OperationState::Running {
            record.state = OperationState::Interrupted;
            let staging = record
                .items
                .iter()
                .filter_map(|item| item.staging.as_ref())
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            record.error = Some(format!(
                "Application stopped during execution. Inspect affected paths before retrying uncertain items.{}",
                if staging.is_empty() {
                    String::new()
                } else {
                    format!(
                        " Temporary copy data may remain at: {}",
                        staging.join(" · ")
                    )
                }
            ));
            changed = true;
        }
        update_undo(record);
    }
    changed
}

#[derive(Debug)]
struct BeforeEffectCancellation;
impl std::fmt::Display for BeforeEffectCancellation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Cancelled before changing this item")
    }
}
impl std::error::Error for BeforeEffectCancellation {}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        return Err(BeforeEffectCancellation.into());
    }
    Ok(())
}
