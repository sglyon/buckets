#[allow(dead_code)]
#[path = "../src/named_workspaces.rs"]
mod named_workspaces;
#[path = "../src/pane_view.rs"]
mod pane_view;
use buckets_core::{Entry, Location};
use named_workspaces::{Column, PaneView, SortKey};
fn entry(name: &str, size: u64, modified: Option<u64>, is_dir: bool) -> Entry {
    Entry {
        name: name.into(),
        location: Location::local(format!("/fixture/{name}")),
        size,
        modified,
        is_dir,
        is_symlink: false,
    }
}
#[test]
fn user_sort_preserves_directories_first_in_both_directions() {
    let entries = vec![
        entry("z.rs", 1, Some(10), false),
        entry("a.rs", 5, Some(20), false),
        entry("B-dir", 0, None, true),
    ];
    for descending in [false, true] {
        let mut sorted = entries.clone();
        pane_view::sort(
            &mut sorted,
            &PaneView {
                sort: SortKey::Size,
                descending,
                ..Default::default()
            },
        );
        assert_eq!(sorted[0].name, "B-dir");
        assert_eq!(sorted[1].name, if descending { "a.rs" } else { "z.rs" });
    }
}
#[test]
fn missing_dates_stay_last_and_name_ties_are_deterministic() {
    for descending in [false, true] {
        let mut entries = vec![
            entry("Unknown", 0, None, false),
            entry("é", 0, Some(0), false),
            entry("A", 0, Some(3), false),
            entry("a", 0, Some(3), false),
        ];
        pane_view::sort(
            &mut entries,
            &PaneView {
                sort: SortKey::Modified,
                descending,
                directories_first: false,
                ..Default::default()
            },
        );
        assert_eq!(entries.last().unwrap().name, "Unknown");
        assert_eq!(entries[0].name, if descending { "a" } else { "é" });
    }
}
#[test]
fn optional_metadata_never_consumes_filename_space_and_restores_when_wider() {
    let view = PaneView {
        columns: vec![Column::Name, Column::Size, Column::Modified, Column::Type],
        ..Default::default()
    };
    assert_eq!(pane_view::visible_columns(&view, 130.), vec![Column::Name]);
    assert_eq!(
        pane_view::visible_columns(&view, 220.),
        vec![Column::Name, Column::Size]
    );
    assert_eq!(pane_view::visible_columns(&view, 500.), view.columns);
    assert_eq!(view.columns.len(), 4);
}
#[test]
fn modified_dates_are_stable_utc_and_missing_metadata_is_explicit() {
    assert_eq!(pane_view::modified(Some(0)), "1970-01-01");
    assert_eq!(pane_view::modified(Some(1709164800)), "2024-02-29");
    assert_eq!(pane_view::modified(None), "—");
}
