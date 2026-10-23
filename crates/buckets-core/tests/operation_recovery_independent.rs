//! Independent behavior checks at operation/publication and recovery boundaries.
use buckets_core::{
    Location, OperationAction as Action, OperationJournal as Journal, OperationPlan as Plan,
    OperationState as State,
};
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

fn local(path: impl AsRef<Path>) -> Location {
    Location::local(path.as_ref())
}
fn journal(root: &Path) -> Journal {
    Journal::open_with_trash(root.join("journal"), root.join("trash")).unwrap()
}
fn plan(action: Action) -> Plan {
    Plan {
        label: "Independent recovery".into(),
        actions: vec![action],
    }
}

#[test]
fn edits_after_publication_cannot_be_adopted_as_operation_owned_content() {
    for kind in ["copy", "move", "file", "directory", "trash"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, "original bytes").unwrap();
        let action = match kind {
            "copy" => Action::Copy {
                source: local(&source),
                destination: local(&destination),
            },
            "move" => Action::Move {
                source: local(&source),
                destination: local(&destination),
            },
            "file" => Action::CreateFile {
                destination: local(&destination),
            },
            "directory" => Action::CreateDirectory {
                destination: local(&destination),
            },
            _ => Action::Trash {
                source: local(&source),
            },
        };
        let current = if kind == "trash" {
            root.join("trash/source")
        } else {
            destination
        };
        let injected = AtomicBool::new(false);
        let cancel = AtomicBool::new(false);
        let mut service = journal(root);
        let record = service
            .execute(plan(action), &cancel, |event| {
                if event.stage == "Checking recovery" && !injected.swap(true, Ordering::AcqRel) {
                    assert!(
                        current.exists(),
                        "Progress must describe the post-publication phase"
                    );
                    if kind == "directory" {
                        fs::write(current.join("user-created-child"), "unrelated user data")
                            .unwrap();
                    } else {
                        fs::write(&current, "unrelated user edit").unwrap();
                    }
                }
            })
            .unwrap();
        assert!(
            injected.load(Ordering::Acquire),
            "Recovery phase was not observed for {kind}"
        );
        assert_eq!(record.state, State::Complete);
        if record.undo_available {
            let undone = service.undo(&record.id, &cancel, |_| {}).unwrap();
            assert!(
                undone.error.is_some(),
                "Undo accepted a post-publication edit for {kind}"
            );
        }
        assert!(
            current.exists(),
            "Recovery moved unrelated user data for {kind}"
        );
        if kind == "directory" {
            assert_eq!(
                fs::read_to_string(current.join("user-created-child")).unwrap(),
                "unrelated user data"
            );
        } else {
            assert_eq!(fs::read_to_string(&current).unwrap(), "unrelated user edit");
        }
    }
}

#[test]
fn cancelled_recovery_can_continue_after_restart_without_replaying_completed_inverse() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut service = journal(root);
    let mut actions = vec![];
    for index in 0..3 {
        let source = root.join(format!("source-{index}"));
        fs::write(&source, format!("text-{index}")).unwrap();
        actions.push(Action::Copy {
            source: local(source),
            destination: local(root.join(format!("copy-{index}"))),
        });
    }
    let fresh = AtomicBool::new(false);
    let record = service
        .execute(
            Plan {
                label: "Copy three".into(),
                actions,
            },
            &fresh,
            |_| {},
        )
        .unwrap();
    let stop = AtomicBool::new(false);
    let stopped = service
        .undo(&record.id, &stop, |event| {
            assert!(event.completed_items <= event.total_items);
            if event.completed_items == 1 {
                stop.store(true, Ordering::Release);
            }
        })
        .unwrap();
    assert_eq!(stopped.state, State::Cancelled);
    assert!(stopped.undo_available);
    assert!(!stopped.can_retry());
    assert!(!root.join("copy-2").exists());
    assert!(root.join("copy-1").exists());
    assert!(root.join("copy-0").exists());
    drop(service);
    let mut reopened = journal(root);
    let result = reopened
        .undo(&record.id, &fresh, |event| {
            assert!(event.completed_items <= event.total_items)
        })
        .unwrap();
    assert_eq!(result.state, State::Undone);
    for index in 0..3 {
        assert!(!root.join(format!("copy-{index}")).exists());
        assert_eq!(
            fs::read_to_string(root.join(format!("trash/copy-{index}"))).unwrap(),
            format!("text-{index}")
        );
        assert_eq!(
            fs::read_to_string(root.join(format!("source-{index}"))).unwrap(),
            format!("text-{index}")
        );
        assert!(
            !root.join(format!("trash/copy-{index}.1")).exists(),
            "Inverse was replayed"
        );
    }
}

#[test]
fn relative_locations_cannot_change_meaning_across_a_process_restart() {
    let temp = tempfile::tempdir().unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "relative_location_probe", "--nocapture"])
        .env("BUCKETS_RELATIVE_JOURNAL_PROBE", temp.path())
        .current_dir(temp.path())
        .status()
        .unwrap();
    assert!(status.success());
    assert!(!temp.path().join("relative-destination").exists());
}

#[test]
fn relative_location_probe() {
    let Some(root) = std::env::var_os("BUCKETS_RELATIVE_JOURNAL_PROBE") else {
        return;
    };
    let mut service = journal(Path::new(&root));
    let outcome = service.execute(
        plan(Action::CreateFile {
            destination: local("relative-destination"),
        }),
        &AtomicBool::new(false),
        |_| {},
    );
    match outcome {
        Ok(record) => {
            assert_eq!(record.state, State::Failed);
            assert!(record.error.unwrap().contains("absolute"));
        }
        Err(error) => assert!(format!("{error:#}").contains("absolute")),
    }
    assert!(!Path::new("relative-destination").exists());
}
