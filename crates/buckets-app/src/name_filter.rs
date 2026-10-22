//! Live filtering of the entries already loaded in a pane. This module never
//! touches the filesystem, changes entries, or substitutes a recursive search.

/// Return matching original indices, ranked by exact/prefix matches and fuzzy
/// subsequence quality. Every whitespace-separated token must match the name;
/// tokens may appear in any order. Ties preserve the input order.
pub fn matching_indices<'a>(names: impl IntoIterator<Item = &'a str>, query: &str) -> Vec<usize> {
    let tokens: Vec<Vec<char>> = query
        .split_whitespace()
        .map(|token| token.chars().flat_map(char::to_lowercase).collect())
        .collect();
    if tokens.is_empty() {
        return names
            .into_iter()
            .enumerate()
            .map(|(index, _)| index)
            .collect();
    }

    let mut matches = Vec::new();
    for (index, name) in names.into_iter().enumerate() {
        let (characters, boundaries) = fold_name(name);
        let mut rank = (0usize, 0usize, 0i64);
        let mut matched = true;
        for token in &tokens {
            let Some(score) = subsequence_score(&characters, &boundaries, token) else {
                matched = false;
                break;
            };
            rank.0 += usize::from(characters == *token);
            rank.1 += usize::from(characters.starts_with(token));
            rank.2 += score;
        }
        if matched {
            matches.push((index, rank));
        }
    }
    matches.sort_unstable_by(|(a_index, a_rank), (b_index, b_rank)| {
        b_rank.cmp(a_rank).then(a_index.cmp(b_index))
    });
    matches.into_iter().map(|(index, _)| index).collect()
}

fn fold_name(name: &str) -> (Vec<char>, Vec<bool>) {
    let mut characters = Vec::new();
    let mut boundaries = Vec::new();
    let mut previous: Option<char> = None;
    for character in name.chars() {
        let boundary = previous.is_none_or(|previous| {
            !previous.is_alphanumeric() || previous.is_lowercase() && character.is_uppercase()
        });
        for (offset, folded) in character.to_lowercase().enumerate() {
            characters.push(folded);
            boundaries.push(boundary && offset == 0);
        }
        previous = Some(character);
    }
    (characters, boundaries)
}

// Dynamic programming chooses the best subsequence instead of committing to
// the first possible character. A running maximum keeps it O(name * token),
// with two rows of memory, even when repeated letters create many alternatives.
fn subsequence_score(name: &[char], boundaries: &[bool], token: &[char]) -> Option<i64> {
    if token.len() > name.len() || token.is_empty() {
        return None;
    }
    const NONE: i64 = i64::MIN / 4;
    const MATCH: i64 = 16;
    const BOUNDARY: i64 = 18;
    const CONSECUTIVE: i64 = 22;
    const GAP: i64 = 2;
    let mut previous = vec![NONE; name.len()];
    let mut current = vec![NONE; name.len()];
    for (token_index, wanted) in token.iter().enumerate() {
        current.fill(NONE);
        let mut best_gap = NONE;
        for (index, character) in name.iter().enumerate() {
            if token_index > 0 && index >= 2 && previous[index - 2] != NONE {
                best_gap = best_gap.max(previous[index - 2] + GAP * (index - 2) as i64);
            }
            if character != wanted {
                continue;
            }
            let quality = MATCH + if boundaries[index] { BOUNDARY } else { 0 };
            if token_index == 0 {
                current[index] = quality - index as i64;
                continue;
            }
            let consecutive = if index > 0 && previous[index - 1] != NONE {
                previous[index - 1] + CONSECUTIVE
            } else {
                NONE
            };
            let separated = if best_gap != NONE {
                best_gap - GAP * index as i64 + GAP
            } else {
                NONE
            };
            let best = consecutive.max(separated);
            if best != NONE {
                current[index] = best + quality;
            }
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous.into_iter().max().filter(|score| *score != NONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_filter_keeps_original_indices_and_unmatched_names_drop_out() {
        let names = ["z.rs", "a.rs", "Folder"];
        assert_eq!(matching_indices(names, ""), vec![0, 1, 2]);
        assert_eq!(matching_indices(names, " \t\n"), vec![0, 1, 2]);
        assert!(matching_indices(names, "not-present").is_empty());
        assert!(matching_indices(std::iter::empty(), "a").is_empty());
        assert_eq!(matching_indices([""], ""), vec![0]);
    }

    #[test]
    fn exact_then_prefix_then_boundary_matches_outrank_scattered_letters() {
        let names = ["fooabar.txt", "foo-bar.txt", "fbar.txt", "FB"];
        assert_eq!(matching_indices(names, "fb"), vec![3, 2, 1, 0]);
        assert_eq!(
            matching_indices(["foobar.rs", "FooRepository.rs"], "fr"),
            vec![1, 0]
        );
    }

    #[test]
    fn all_tokens_must_match_but_their_order_is_not_constrained() {
        let names = ["test-report.rs", "report.rs", "test.txt", "report-test.rs"];
        let matched = matching_indices(names, "report\t test");
        assert_eq!(matched.len(), 2);
        assert!(matched.contains(&0) && matched.contains(&3));
        assert!(matching_indices(names, "report missing").is_empty());
    }

    #[test]
    fn unicode_lowercasing_and_multibyte_names_never_require_byte_slices() {
        let names = [
            "café-世界.rs",
            "CAFÉ.rs",
            "cafe.rs",
            "İstanbul.txt",
            "STRAẞE.md",
        ];
        assert_eq!(matching_indices(names, "café"), vec![0, 1]);
        assert_eq!(matching_indices(names, "界 café"), vec![0]);
        assert_eq!(matching_indices(names, "İS"), vec![3]);
        assert_eq!(matching_indices(names, "straße"), vec![4]);
        assert!(matching_indices(names, "🙂").is_empty());
    }

    #[test]
    fn equal_quality_preserves_original_order_and_repeated_characters_use_best_path() {
        assert_eq!(
            matching_indices(["foo-bar", "foo_bar", "foo.bar"], "fb"),
            vec![0, 1, 2]
        );
        let (name, boundaries) = fold_name("a___b_ab");
        let optimal = subsequence_score(&name, &boundaries, &['a', 'b']).unwrap();
        let (first, first_boundaries) = fold_name("a___b");
        assert!(optimal > subsequence_score(&first, &first_boundaries, &['a', 'b']).unwrap());
        assert!(matching_indices(["a", "ab"], "aa").is_empty());
    }
}
