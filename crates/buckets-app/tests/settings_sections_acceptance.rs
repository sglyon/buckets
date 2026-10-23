//! The model consumes measured geometry; actual GPUI scrolling needs native QA.
#[path = "../src/settings_sections.rs"]
mod settings_sections;

use settings_sections::{Section, SectionRange, active_section, cycle, jump_offset, ranges};

#[test]
fn canonical_grouping_keeps_visible_row_indices_independent_of_heading_count() {
    let mut rows = [
        (Section::Plugins, "enabled"),
        (Section::Appearance, "font"),
        (Section::Browsing, "hidden"),
        (Section::Appearance, "color"),
        (Section::Advanced, "json"),
        (Section::Places, "home"),
        (Section::Shortcuts, "split"),
    ];
    rows.sort_by_key(|(section, _)| *section);
    assert_eq!(
        rows.map(|(section, _)| section),
        [
            Section::Appearance,
            Section::Appearance,
            Section::Browsing,
            Section::Places,
            Section::Plugins,
            Section::Shortcuts,
            Section::Advanced
        ]
    );
    let grouped = ranges(rows.iter().map(|(section, _)| section.label()));
    assert_eq!(
        grouped
            .iter()
            .map(|range| range.section)
            .collect::<Vec<_>>(),
        Section::ALL
    );
    assert_eq!(
        grouped[0],
        SectionRange {
            section: Section::Appearance,
            first_row: 0,
            end_row: 2
        }
    );
    for range in grouped {
        assert!(
            rows[range.first_row..range.end_row]
                .iter()
                .all(|(section, _)| *section == range.section)
        );
    }
    assert_eq!(rows[2].1, "hidden");
    assert_eq!(Section::from_label("All"), None);
}

#[test]
fn variable_height_geometry_activates_crossed_headings_and_the_short_final_section() {
    // Inline editor/description wrapping produce deliberately uneven spacing.
    let headings = [
        (Section::Appearance, 24.),
        (Section::Browsing, 317.),
        (Section::Places, 391.),
        (Section::Plugins, 816.),
        (Section::Advanced, 1050.),
    ];
    for (top, expected) in [
        (0., Section::Appearance),
        (250., Section::Appearance),
        (317., Section::Browsing),
        (390.5, Section::Places),
        (675., Section::Places),
        (817., Section::Plugins),
        (859., Section::Advanced),
    ] {
        assert_eq!(
            active_section(&headings, top, 240., 1100.),
            Some(expected),
            "wrong section at scroll top {top}"
        );
    }
    assert_eq!(
        active_section(&headings, 500., 600., 1100.),
        Some(Section::Advanced)
    );
    assert_eq!(
        active_section(&headings, 0., 1200., 1100.),
        Some(Section::Appearance)
    );
}

#[test]
fn click_jump_aligns_a_visible_heading_to_top_and_clamps_only_at_real_bounds() {
    // Heading 170 is already visible at scroll=100 in a 240px viewport. A nav
    // click must still align it to top, rather than retain minimal visibility.
    assert_eq!(jump_offset(170., 240., 1200.), -170.);
    assert_eq!(
        active_section(
            &[
                (Section::Appearance, 0.),
                (Section::Places, 170.),
                (Section::Advanced, 1050.)
            ],
            -jump_offset(170., 240., 1200.),
            240.,
            1200.
        ),
        Some(Section::Places)
    );
    assert_eq!(jump_offset(1050., 240., 1200.), -960.);
    assert_eq!(jump_offset(0., 240., 1200.), 0.);
    assert_eq!(jump_offset(150., 500., 300.), 0.);
}

#[test]
fn global_filter_missing_sections_no_results_and_requery_rebuild_navigation() {
    let all = [
        "Appearance",
        "Appearance",
        "Browsing",
        "Places",
        "Plugins",
        "Shortcuts",
        "Advanced",
    ];
    let visible = ranges(["Places", "Shortcuts", "Advanced"]);
    assert_eq!(
        visible,
        [
            SectionRange {
                section: Section::Places,
                first_row: 0,
                end_row: 1
            },
            SectionRange {
                section: Section::Shortcuts,
                first_row: 1,
                end_row: 2
            },
            SectionRange {
                section: Section::Advanced,
                first_row: 2,
                end_row: 3
            }
        ]
    );
    let available = visible
        .iter()
        .map(|range| range.section)
        .collect::<Vec<_>>();
    assert_eq!(
        cycle(&available, Some(Section::Browsing), 1),
        Some(Section::Places)
    );
    assert_eq!(
        cycle(&available, Some(Section::Advanced), 1),
        Some(Section::Places)
    );
    assert_eq!(
        cycle(&available, Some(Section::Places), -1),
        Some(Section::Advanced)
    );
    assert!(ranges([]).is_empty());
    assert_eq!(active_section(&[], 0., 300., 0.), None);
    assert_eq!(cycle(&[], Some(Section::Places), 1), None);
    assert_eq!(
        ranges(all)
            .iter()
            .map(|range| range.section)
            .collect::<Vec<_>>(),
        Section::ALL
    );
}

#[test]
fn dynamic_rows_recompute_first_row_mapping_and_geometry_without_stale_indices() {
    let before = ranges(["Appearance", "Browsing", "Plugins", "Advanced"]);
    let after = ranges([
        "Appearance",
        "Appearance",
        "Browsing",
        "Plugins",
        "Plugins",
        "Advanced",
    ]);
    assert_eq!(before[2].first_row, 2);
    assert_eq!(after[2].first_row, 3);
    assert_eq!(after[2].end_row, 5);
    assert_eq!(after[3].first_row, 5);
    let old = [
        (Section::Appearance, 0.),
        (Section::Browsing, 80.),
        (Section::Plugins, 160.),
        (Section::Advanced, 900.),
    ];
    let new = [
        (Section::Appearance, 0.),
        (Section::Browsing, 360.),
        (Section::Plugins, 440.),
        (Section::Advanced, 1300.),
    ];
    assert_eq!(
        active_section(&old, 200., 200., 1000.),
        Some(Section::Plugins)
    );
    assert_eq!(
        active_section(&new, 200., 200., 1400.),
        Some(Section::Appearance)
    );
    assert_eq!(jump_offset(new[2].1, 200., 1400.), -440.);
}
