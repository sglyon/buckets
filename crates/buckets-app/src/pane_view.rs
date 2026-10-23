//! Pure pane projection. Sort keys are allocated once per reorder, never per row.
use crate::named_workspaces::{PaneView, SortKey};
use buckets_core::Entry;
use std::cmp::Ordering;

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Value {
    Text(String),
    Number(Option<u64>),
}
pub fn sort(entries: &mut Vec<Entry>, view: &PaneView) {
    let mut keyed = entries
        .drain(..)
        .map(|entry| {
            let name = entry.name.to_lowercase();
            let value = match view.sort {
                SortKey::Name => Value::Text(name.clone()),
                SortKey::Size => Value::Number(Some(entry.size)),
                SortKey::Modified => Value::Number(entry.modified),
                SortKey::Type => Value::Text(extension(&entry).to_lowercase()),
            };
            (entry, name, value)
        })
        .collect::<Vec<_>>();
    keyed.sort_by(|(a, an, av), (b, bn, bv)| {
        let directories = if view.directories_first {
            b.is_dir.cmp(&a.is_dir)
        } else {
            Ordering::Equal
        };
        directories.then_with(|| {
            if matches!(av, Value::Number(None)) != matches!(bv, Value::Number(None)) {
                return if matches!(av, Value::Number(None)) {
                    Ordering::Greater
                } else {
                    Ordering::Less
                };
            }
            let order = av
                .cmp(bv)
                .then_with(|| an.cmp(bn))
                .then_with(|| a.name.cmp(&b.name));
            if view.descending {
                order.reverse()
            } else {
                order
            }
        })
    });
    entries.extend(keyed.into_iter().map(|(entry, _, _)| entry));
}
pub fn extension(entry: &Entry) -> &str {
    if entry.is_symlink {
        "LINK"
    } else if entry.is_dir {
        "DIR"
    } else {
        entry
            .name
            .rsplit_once('.')
            .map(|(_, ext)| ext)
            .filter(|ext| !ext.is_empty())
            .unwrap_or("FILE")
    }
}
/// UTC date, avoiding locale/platform dependencies in provider-independent rows.
pub fn modified(value: Option<u64>) -> String {
    let Some(seconds) = value else {
        return "—".into();
    };
    let days = (seconds / 86400) as i64;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}
/// Persist the requested columns, but leave room for filenames in narrow panes.
pub fn visible_columns(view: &PaneView, width_units: f32) -> Vec<crate::named_workspaces::Column> {
    use crate::named_workspaces::Column;
    let mut available = (width_units - 140.).max(0.);
    let mut columns = vec![Column::Name];
    for column in view
        .columns
        .iter()
        .copied()
        .filter(|column| *column != Column::Name)
    {
        let width = match column {
            Column::Size => 64.,
            Column::Modified => 108.,
            Column::Type => 72.,
            Column::Name => 0.,
        };
        if available >= width {
            columns.push(column);
            available -= width;
        }
    }
    columns
}
