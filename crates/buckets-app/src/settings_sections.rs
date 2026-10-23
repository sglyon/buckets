//! Settings navigation from visible rows and measured content coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Appearance,
    Browsing,
    Places,
    Plugins,
    Shortcuts,
    Advanced,
}

impl Section {
    pub const ALL: [Self; 6] = [
        Self::Appearance,
        Self::Browsing,
        Self::Places,
        Self::Plugins,
        Self::Shortcuts,
        Self::Advanced,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Browsing => "Browsing",
            Self::Places => "Places",
            Self::Plugins => "Plugins",
            Self::Shortcuts => "Shortcuts",
            Self::Advanced => "Advanced",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|section| section.label() == label)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionRange {
    pub section: Section,
    pub first_row: usize,
    pub end_row: usize,
}

/// Rows are already sorted by Section order and filtered by the global query.
/// Bounds refer to the visible row vector, independent of rendered headings.
pub fn ranges<'a>(labels: impl IntoIterator<Item = &'a str>) -> Vec<SectionRange> {
    let mut result: Vec<SectionRange> = vec![];
    for (index, label) in labels.into_iter().enumerate() {
        let Some(section) = Section::from_label(label) else {
            continue;
        };
        if let Some(previous) = result.last_mut()
            && previous.section == section
            && previous.end_row == index
        {
            previous.end_row = index + 1;
        } else {
            result.push(SectionRange {
                section,
                first_row: index,
                end_row: index + 1,
            });
        }
    }
    result
}

/// Heading tops are measured in content coordinates. scroll_top is positive,
/// the opposite sign from GPUI ScrollHandle::offset().y.
pub fn active_section(
    sections: &[(Section, f32)],
    scroll_top: f32,
    viewport_height: f32,
    content_height: f32,
) -> Option<Section> {
    let first = sections.first()?.0;
    let max_scroll = (content_height - viewport_height).max(0.);
    let top = scroll_top.clamp(0., max_scroll);
    // A final section shorter than the viewport cannot reach its top. At the
    // actual bottom it still owns the navigation highlight.
    if max_scroll > 1. && top >= max_scroll - 1. {
        return sections.last().map(|(section, _)| *section);
    }
    Some(
        sections
            .iter()
            .rev()
            .find(|(_, heading_top)| *heading_top <= top + 1.)
            .map(|(section, _)| *section)
            .unwrap_or(first),
    )
}

/// Exact top alignment, clamped at the content bottom. GPUI scroll offsets are
/// negative when scrolled down; merely making a heading visible is insufficient.
pub fn jump_offset(heading_top: f32, viewport_height: f32, content_height: f32) -> f32 {
    -heading_top.clamp(0., (content_height - viewport_height).max(0.))
}

pub fn cycle(available: &[Section], current: Option<Section>, delta: isize) -> Option<Section> {
    if available.is_empty() {
        return None;
    }
    let Some(index) = current.and_then(|current| available.iter().position(|s| *s == current))
    else {
        return if delta < 0 {
            available.last().copied()
        } else {
            available.first().copied()
        };
    };
    let step = delta.rem_euclid(available.len() as isize) as usize;
    Some(available[(index + step) % available.len()])
}
