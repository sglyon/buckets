use buckets_core::{
    Location, OperationAction as Action, OperationItemState as ItemState,
    OperationJournal as Journal, OperationPlan as Plan, OperationState as State,
};
use std::{
    fs,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tempfile::TempDir;
fn local(path: impl AsRef<Path>) -> Location {
    Location::local(path.as_ref())
}
fn store(root: &Path) -> Journal {
    Journal::open_with_trash(root.join("journal"), root.join("trash")).unwrap()
}
fn plan(label: &str, actions: Vec<Action>) -> Plan {
    Plan {
        label: label.into(),
        actions,
    }
}
fn flag() -> AtomicBool {
    AtomicBool::new(false)
}

#[test]
fn copy_is_atomic_reports_real_bytes_and_undo_uses_private_recoverable_trash() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let source = root.join("source");
    let destination = root.join("copied");
    let data = vec![37u8; 3 * 1024 * 1024 + 17];
    fs::write(&source, &data).unwrap();
    let mut journal = store(root);
    let events = Mutex::new(vec![]);
    let record = journal
        .execute(
            plan(
                "Copy",
                vec![Action::Copy {
                    source: local(&source),
                    destination: local(&destination),
                }],
            ),
            &flag(),
            |event| events.lock().unwrap().push(event),
        )
        .unwrap();
    assert_eq!(record.state, State::Complete);
    assert!(record.undo_available);
    assert_eq!(fs::read(&destination).unwrap(), data);
    let events = events.into_inner().unwrap();
    assert_eq!(events.last().unwrap().bytes_copied, data.len() as u64);
    assert!(
        events
            .iter()
            .any(|event| event.stage == "Copying" && event.bytes_copied > 0)
    );
    let recovery = Mutex::new(vec![]);
    let result = journal
        .undo(&record.id, &flag(), |event| {
            recovery.lock().unwrap().push(event)
        })
        .unwrap();
    assert_eq!(result.state, State::Undone);
    assert!(!destination.exists());
    assert_eq!(fs::read(root.join("trash/copied")).unwrap(), data);
    assert!(source.exists());
    for event in recovery.into_inner().unwrap() {
        assert!(event.completed_items <= event.total_items);
    }
}
#[test]
fn mid_copy_cancel_publishes_nothing_and_retry_reuses_the_original_destination() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let source = root.join("notes.md");
    fs::write(&source, vec![42; 4 * 1024 * 1024]).unwrap();
    let mut journal = store(root);
    let cancel = flag();
    let record = journal
        .execute(
            plan(
                "Duplicate",
                vec![Action::Duplicate {
                    source: local(&source),
                }],
            ),
            &cancel,
            |event| {
                if event.stage == "Copying" {
                    cancel.store(true, Ordering::Release);
                }
            },
        )
        .unwrap();
    assert_eq!(record.state, State::Cancelled);
    assert!(record.can_retry());
    assert!(!root.join("notes copy.md").exists());
    assert!(matches!(record.items[0].action, Action::Copy { .. }));
    let retried = journal.retry(&record.id, &flag(), |_| {}).unwrap();
    assert_eq!(retried.id, record.id);
    assert_eq!(retried.state, State::Complete);
    assert_eq!(
        fs::read(root.join("notes copy.md")).unwrap(),
        fs::read(source).unwrap()
    );
    assert!(!root.join("notes copy 2.md").exists());
    assert!(!fs::read_dir(root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".buckets-copy-")
    }));
}
#[test]
fn partial_batch_retry_never_replays_a_completed_item() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let mut journal = store(root);
    fs::write(root.join("one"), "one").unwrap();
    fs::write(root.join("two"), "two").unwrap();
    fs::write(root.join("dest-two"), "collision").unwrap();
    let actions = vec![
        Action::Copy {
            source: local(root.join("one")),
            destination: local(root.join("dest-one")),
        },
        Action::Copy {
            source: local(root.join("two")),
            destination: local(root.join("dest-two")),
        },
    ];
    let record = journal
        .execute(plan("Copy two", actions), &flag(), |_| {})
        .unwrap();
    assert_eq!(record.state, State::Failed);
    assert_eq!(record.items[0].state, ItemState::Complete);
    assert_eq!(record.items[1].state, ItemState::Failed);
    fs::write(root.join("dest-one"), "edited after copy").unwrap();
    fs::remove_file(root.join("dest-two")).unwrap();
    let retried = journal.retry(&record.id, &flag(), |_| {}).unwrap();
    assert_eq!(retried.state, State::Complete);
    assert_eq!(
        fs::read_to_string(root.join("dest-one")).unwrap(),
        "edited after copy"
    );
    assert_eq!(fs::read_to_string(root.join("dest-two")).unwrap(), "two");
    let undone = journal.undo(&record.id, &flag(), |_| {}).unwrap();
    assert!(undone.error.unwrap().contains("changed"));
    assert!(root.join("dest-one").exists());
    assert!(!root.join("dest-two").exists());
}
#[test]
fn rename_recovery_refuses_new_original_and_changed_current_contents() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let mut journal = store(root);
    let source = root.join("a");
    let destination = root.join("b");
    fs::write(&source, "original").unwrap();
    let record = journal
        .execute(
            plan(
                "Rename",
                vec![Action::Move {
                    source: local(&source),
                    destination: local(&destination),
                }],
            ),
            &flag(),
            |_| {},
        )
        .unwrap();
    fs::write(&source, "new occupant").unwrap();
    let blocked = journal.undo(&record.id, &flag(), |_| {}).unwrap();
    assert!(blocked.error.unwrap().contains("already exists"));
    assert_eq!(fs::read_to_string(&source).unwrap(), "new occupant");
    assert!(destination.exists());
    fs::remove_file(&source).unwrap();
    fs::write(&destination, "user edit").unwrap();
    let blocked = journal.undo(&record.id, &flag(), |_| {}).unwrap();
    assert!(blocked.error.unwrap().contains("changed"));
    assert_eq!(fs::read_to_string(&destination).unwrap(), "user edit");
}
#[test]
fn trash_receipt_restores_to_original_and_survives_journal_reopen() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let original = root.join("important.txt");
    fs::write(&original, "keep me").unwrap();
    let id = {
        let mut journal = store(root);
        let record = journal
            .execute(
                plan(
                    "Trash",
                    vec![Action::Trash {
                        source: local(&original),
                    }],
                ),
                &flag(),
                |_| {},
            )
            .unwrap();
        assert!(record.undo_available);
        record.id
    };
    assert!(!original.exists());
    assert_eq!(
        fs::read_to_string(root.join("trash/important.txt")).unwrap(),
        "keep me"
    );
    let mut reopened = store(root);
    let result = reopened.undo(&id, &flag(), |_| {}).unwrap();
    assert_eq!(result.state, State::Undone);
    assert_eq!(fs::read_to_string(&original).unwrap(), "keep me");
}
#[test]
fn directory_recovery_rejects_added_children_and_symlink_replacement() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let mut journal = store(root);
    let directory = root.join("new");
    let record = journal
        .execute(
            plan(
                "New folder",
                vec![Action::CreateDirectory {
                    destination: local(&directory),
                }],
            ),
            &flag(),
            |_| {},
        )
        .unwrap();
    fs::write(directory.join("unrelated"), "do not remove").unwrap();
    let result = journal.undo(&record.id, &flag(), |_| {}).unwrap();
    assert!(result.error.unwrap().contains("changed"));
    assert!(directory.join("unrelated").exists());
    #[cfg(unix)]
    {
        fs::write(root.join("target"), "private").unwrap();
        std::os::unix::fs::symlink(root.join("target"), root.join("link")).unwrap();
        let record = journal
            .execute(
                plan(
                    "Copy link",
                    vec![Action::Copy {
                        source: local(root.join("link")),
                        destination: local(root.join("other-link")),
                    }],
                ),
                &flag(),
                |_| {},
            )
            .unwrap();
        fs::remove_file(root.join("other-link")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), root.join("other-link")).unwrap();
        let result = journal.undo(&record.id, &flag(), |_| {}).unwrap();
        assert!(result.error.unwrap().contains("changed"));
        assert_eq!(fs::read_to_string(root.join("target")).unwrap(), "private");
    }
}
#[test]
fn persisted_uncertain_intent_is_interrupted_and_cannot_be_replayed() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let destination = root.join("new.txt");
    let record = store(root)
        .execute(
            plan(
                "New file",
                vec![Action::CreateFile {
                    destination: local(&destination),
                }],
            ),
            &flag(),
            |_| {},
        )
        .unwrap();
    let path = root.join("journal/journal.json");
    let mut saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    // This is the durable state at either a forward or inverse crash boundary.
    saved["records"][0]["state"] = "Running".into();
    saved["records"][0]["items"][0]["state"] = "Running".into();
    fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    let mut journal = store(root);
    let restored = journal.records().remove(0);
    assert_eq!(restored.state, State::Interrupted);
    assert!(!restored.can_retry());
    assert!(!restored.undo_available);
    assert!(journal.retry(&record.id, &flag(), |_| {}).is_err());
    assert!(journal.undo(&record.id, &flag(), |_| {}).is_err());
    assert!(destination.exists());
}
#[test]
fn undo_intent_is_persisted_before_the_inverse_and_recovery_counts_stay_bounded() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let source = root.join("a");
    let destination = root.join("b");
    fs::write(&source, "text").unwrap();
    let mut journal = store(root);
    let record = journal
        .execute(
            plan(
                "Rename",
                vec![Action::Move {
                    source: local(&source),
                    destination: local(&destination),
                }],
            ),
            &flag(),
            |_| {},
        )
        .unwrap();
    let count = Mutex::new(0);
    let result = journal
        .undo(&record.id, &flag(), |event| {
            assert!(event.completed_items <= event.total_items);
            *count.lock().unwrap() += 1;
        })
        .unwrap();
    assert_eq!(result.state, State::Undone);
    assert_eq!(fs::read_to_string(&source).unwrap(), "text");
    assert!(*count.lock().unwrap() >= 2);
}
#[test]
fn live_process_lock_prevents_other_instances_from_interrupting_or_overwriting() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::write(root.join("source"), vec![5; 512 * 1024]).unwrap();
    let mut journal = store(root);
    let attempted = AtomicBool::new(false);
    let record = journal
        .execute(
            plan(
                "Copy",
                vec![Action::Copy {
                    source: local(root.join("source")),
                    destination: local(root.join("destination")),
                }],
            ),
            &flag(),
            |_| {
                if !attempted.swap(true, Ordering::AcqRel) {
                    assert!(Journal::open(root.join("journal")).is_err());
                }
            },
        )
        .unwrap();
    assert_eq!(record.state, State::Complete);
    assert_eq!(store(root).records()[0].state, State::Complete);
}
#[test]
fn bounded_history_and_symlink_journal_are_guarded() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let mut journal = store(root);
    for index in 0..130 {
        journal
            .execute(
                plan(
                    "Create",
                    vec![Action::CreateFile {
                        destination: local(root.join(format!("file-{index}"))),
                    }],
                ),
                &flag(),
                |_| {},
            )
            .unwrap();
    }
    assert_eq!(journal.records().len(), 128);
    #[cfg(unix)]
    {
        fs::write(root.join("outside"), "unrelated").unwrap();
        fs::remove_file(root.join("journal/journal.json")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("journal/journal.json"))
            .unwrap();
        assert!(Journal::open(root.join("journal")).is_err());
        assert_eq!(
            fs::read_to_string(root.join("outside")).unwrap(),
            "unrelated"
        );
    }
}

#[test]
fn inverse_effect_without_receipt_commit_reopens_uncertain_and_never_replays() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let source = root.join("source");
    let destination = root.join("destination");
    fs::write(&source, "original").unwrap();
    let mut journal = store(root);
    let record = journal
        .execute(
            plan(
                "Rename",
                vec![Action::Move {
                    source: local(&source),
                    destination: local(&destination),
                }],
            ),
            &flag(),
            |_| {},
        )
        .unwrap();
    let moved = AtomicBool::new(false);
    let result = journal.undo(&record.id, &flag(), |event| {
        if event.stage == "Recovering" && !moved.swap(true, Ordering::AcqRel) {
            // The directory becomes unavailable exactly after durable inverse
            // intent. The move succeeds, but its completion cannot be saved.
            fs::rename(root.join("journal"), root.join("offline-journal")).unwrap();
        }
    });
    assert!(result.is_err());
    assert!(source.exists());
    assert!(!destination.exists());
    fs::rename(root.join("offline-journal"), root.join("journal")).unwrap();
    let mut reopened = store(root);
    let recovered = reopened.records().remove(0);
    assert_eq!(recovered.state, State::Interrupted);
    assert!(!recovered.can_retry());
    assert!(!recovered.undo_available);
    assert!(reopened.retry(&record.id, &flag(), |_| {}).is_err());
    assert!(reopened.undo(&record.id, &flag(), |_| {}).is_err());
    assert_eq!(fs::read_to_string(&source).unwrap(), "original");
}

#[test]
fn credentials_and_relative_paths_are_rejected_before_journal_admission() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let mut journal = store(root);
    let invalid = [
        Location::local("relative.txt"),
        Location::Ssh {
            host: "person:private-token@machine".into(),
            path: "/work".into(),
        },
        Location::S3 {
            bucket: "example".into(),
            prefix: "work".into(),
            endpoint: Some("https://person:private-token@example.com".into()),
        },
        Location::S3 {
            bucket: "example".into(),
            prefix: "work".into(),
            endpoint: Some("https://example.com?token=private-token".into()),
        },
    ];
    for destination in invalid {
        assert!(
            journal
                .execute(
                    plan("Invalid", vec![Action::CreateFile { destination }]),
                    &flag(),
                    |_| {}
                )
                .is_err()
        );
    }
    assert!(journal.records().is_empty());
    assert!(!root.join("journal/journal.json").exists());
}
#[cfg(unix)]
#[test]
fn private_state_rejects_user_controlled_symlink_ancestors() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("outside")).unwrap();
    std::os::unix::fs::symlink(root.join("outside"), root.join("linked")).unwrap();
    assert!(Journal::open(root.join("linked/state")).is_err());
    assert!(!root.join("outside/state").exists());
}

#[test]
fn cancellation_during_recovery_version_check_prevents_move_and_trash() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let mut journal = store(root);
    for (name, trash) in [("move-source", false), ("trash-source", true)] {
        let source = root.join(name);
        fs::write(&source, vec![7; 256 * 1024]).unwrap();
        let action = if trash {
            Action::Trash {
                source: local(&source),
            }
        } else {
            Action::Move {
                source: local(&source),
                destination: local(root.join("moved")),
            }
        };
        let cancel = flag();
        let result = journal
            .execute(plan("Cancel", vec![action]), &cancel, |event| {
                if event.stage == "Checking recovery version" {
                    cancel.store(true, Ordering::Release);
                }
            })
            .unwrap();
        assert_eq!(result.state, State::Cancelled);
        assert!(result.can_retry());
        assert!(source.exists());
        assert!(!root.join("moved").exists());
    }
    assert!(!root.join("trash/trash-source").exists());
}

#[test]
fn edits_after_inverse_intent_are_checked_before_recovery_effect() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let mut journal = store(root);
    let destination = root.join("new.txt");
    let record = journal
        .execute(
            plan(
                "New file",
                vec![Action::CreateFile {
                    destination: local(&destination),
                }],
            ),
            &flag(),
            |_| {},
        )
        .unwrap();
    let result = journal
        .undo(&record.id, &flag(), |event| {
            if event.stage == "Recovering" {
                fs::write(&destination, "user changes after journal sync").unwrap();
            }
        })
        .unwrap();
    assert!(result.error.unwrap().contains("changed"));
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "user changes after journal sync"
    );
    assert!(!root.join("trash/new.txt").exists());
}
