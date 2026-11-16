//! Case-insensitive literal find over one document's text.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindDirection {
    Next,
    Previous,
}

/// Matches for one query in one document snapshot, with a current match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindMatches {
    query: String,
    searched: String,
    ranges: Vec<Range<usize>>,
    current: Option<usize>,
}

impl FindMatches {
    /// Searches `text` and makes the first match at or after `origin` current,
    /// wrapping to the first match when none follows it.
    #[must_use]
    pub fn search(text: &str, query: &str, origin: usize) -> Self {
        let ranges = find_all(text, query);
        let current = first_from(&ranges, origin);

        Self {
            query: query.to_owned(),
            searched: text.to_owned(),
            ranges,
            current,
        }
    }

    #[must_use]
    pub fn ranges(&self) -> &[Range<usize>] {
        &self.ranges
    }

    #[must_use]
    pub fn current(&self) -> Option<Range<usize>> {
        self.current.map(|index| self.ranges[index].clone())
    }

    /// Matches that intersect `range`, such as one source line.
    #[must_use]
    pub fn overlapping(&self, range: Range<usize>) -> &[Range<usize>] {
        let start = self
            .ranges
            .partition_point(|candidate| candidate.end <= range.start);
        let count = self.ranges[start..].partition_point(|candidate| candidate.start < range.end);

        &self.ranges[start..start + count]
    }

    /// Moves to the adjacent match, wrapping at either end.
    pub fn advance(&mut self, direction: FindDirection) -> Option<Range<usize>> {
        let current = self.current?;
        let count = self.ranges.len();

        self.current = Some(match direction {
            FindDirection::Next => (current + 1) % count,
            FindDirection::Previous => (current + count - 1) % count,
        });

        self.current()
    }

    /// Searches an edited document again, returning whether its text changed.
    ///
    /// The current match keeps its index while the match count is unchanged,
    /// because an edit elsewhere shifts offsets without changing which
    /// occurrence the user was looking at. Otherwise the nearest match wins.
    pub fn refresh(&mut self, text: &str) -> bool {
        if text == self.searched {
            return false;
        }

        let previous = self.current();
        let ranges = find_all(text, &self.query);

        self.current = match previous {
            Some(_) if ranges.len() == self.ranges.len() => self.current,
            Some(previous) => nearest(&ranges, previous.start),
            None => first_from(&ranges, 0),
        };
        self.ranges = ranges;
        text.clone_into(&mut self.searched);

        true
    }

    /// The match position for display, or `None` before anything was typed.
    #[must_use]
    pub fn label(&self) -> Option<String> {
        if self.query.is_empty() {
            return None;
        }

        Some(self.current.map_or_else(
            || "No results".to_owned(),
            |index| format!("{} of {}", index + 1, self.ranges.len()),
        ))
    }
}

/// Non-overlapping, case-insensitive occurrences of `query` as byte ranges.
#[must_use]
pub fn find_all(text: &str, query: &str) -> Vec<Range<usize>> {
    let needle = query.chars().collect::<Vec<_>>();
    let Some(&first) = needle.first() else {
        return Vec::new();
    };

    let mut ranges = Vec::new();
    let mut resume = 0;
    for (start, character) in text.char_indices() {
        if start < resume || !same_letter(character, first) {
            continue;
        }

        if let Some(length) = match_length(&text[start..], &needle) {
            ranges.push(start..start + length);
            resume = start + length;
        }
    }

    ranges
}

fn match_length(haystack: &str, needle: &[char]) -> Option<usize> {
    let mut characters = haystack.char_indices();
    for &expected in needle {
        let (_, actual) = characters.next()?;
        if !same_letter(actual, expected) {
            return None;
        }
    }

    Some(
        characters
            .next()
            .map_or(haystack.len(), |(offset, _)| offset),
    )
}

/// Compares one character at a time so every match keeps the source's byte
/// boundaries, even where lowercasing would change a character's length.
fn same_letter(left: char, right: char) -> bool {
    if left.is_ascii() && right.is_ascii() {
        return left.eq_ignore_ascii_case(&right);
    }

    left.to_lowercase().eq(right.to_lowercase())
}

fn first_from(ranges: &[Range<usize>], origin: usize) -> Option<usize> {
    if ranges.is_empty() {
        return None;
    }

    let index = ranges.partition_point(|range| range.start < origin);
    Some(if index == ranges.len() { 0 } else { index })
}

fn nearest(ranges: &[Range<usize>], offset: usize) -> Option<usize> {
    ranges
        .iter()
        .enumerate()
        .min_by_key(|(_, range)| range.start.abs_diff(offset))
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_are_literal_case_insensitive_and_non_overlapping() {
        assert_eq!(find_all("Foo foo FOO", "fOo"), vec![0..3, 4..7, 8..11]);
        assert_eq!(find_all("aaaa", "aa"), vec![0..2, 2..4]);
        assert_eq!(find_all("axb a.b a.b", "a.b"), vec![4..7, 8..11]);
        assert_eq!(find_all("fn main()", "(["), Vec::<Range<usize>>::new());
    }

    #[test]
    fn multibyte_text_yields_char_boundary_ranges() {
        let text = "Größe GRÖẞE größe";
        assert_eq!(find_all(text, "größe"), vec![0..7, 8..16, 17..24]);
        assert!(find_all(text, "GRÖSSE").is_empty());

        // Lowercasing İ yields two chars, so it must not shift or split offsets.
        let text = "İstanbul istanbul";
        let matches = find_all(text, "istanbul");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0], 10..18);
        assert_eq!(&text[matches[0].clone()], "istanbul");
    }

    #[test]
    fn empty_query_matches_nothing_and_shows_no_label() {
        let matches = FindMatches::search("text", "", 0);

        assert!(matches.ranges().is_empty());
        assert_eq!(matches.current(), None);
        assert_eq!(matches.label(), None);
    }

    #[test]
    fn search_starts_at_the_origin_and_wraps_past_the_last_match() {
        let text = "x x x";

        assert_eq!(FindMatches::search(text, "x", 0).current(), Some(0..1));
        assert_eq!(FindMatches::search(text, "x", 1).current(), Some(2..3));
        assert_eq!(FindMatches::search(text, "x", 4).current(), Some(4..5));
        assert_eq!(FindMatches::search(text, "x", 5).current(), Some(0..1));
    }

    #[test]
    fn navigation_wraps_in_both_directions_and_updates_the_label() {
        let mut matches = FindMatches::search("a b a b a", "A", 0);
        assert_eq!(matches.label().as_deref(), Some("1 of 3"));

        assert_eq!(matches.advance(FindDirection::Previous), Some(8..9));
        assert_eq!(matches.label().as_deref(), Some("3 of 3"));

        assert_eq!(matches.advance(FindDirection::Next), Some(0..1));
        assert_eq!(matches.advance(FindDirection::Next), Some(4..5));
        assert_eq!(matches.label().as_deref(), Some("2 of 3"));

        let mut missing = FindMatches::search("a b", "z", 0);
        assert_eq!(missing.advance(FindDirection::Next), None);
        assert_eq!(missing.label().as_deref(), Some("No results"));
    }

    #[test]
    fn refresh_follows_edits_and_keeps_the_current_match() {
        let mut matches = FindMatches::search("foo bar foo", "foo", 5);
        assert_eq!(matches.current(), Some(8..11));
        assert!(!matches.refresh("foo bar foo"));

        // Editing elsewhere keeps the same match current even though it moved.
        assert!(matches.refresh("XXXXfoo bar foo"));
        assert_eq!(matches.current(), Some(12..15));

        // Removing a match settles on the nearest survivor.
        assert!(matches.refresh("XXXXfoo bar fo"));
        assert_eq!(matches.ranges(), std::slice::from_ref(&(4..7)));
        assert_eq!(matches.current(), Some(4..7));

        assert!(matches.refresh("nothing left"));
        assert_eq!(matches.current(), None);
        assert_eq!(matches.label().as_deref(), Some("No results"));
    }

    #[test]
    fn overlapping_selects_only_matches_that_touch_the_range() {
        let matches = FindMatches::search("ab\nab ab\nab", "ab", 0);

        assert_eq!(matches.overlapping(3..8), &[3..5, 6..8]);
        assert_eq!(matches.overlapping(4..7), &[3..5, 6..8]);
        assert_eq!(matches.overlapping(5..6), &[] as &[Range<usize>]);
        assert_eq!(matches.overlapping(9..11), std::slice::from_ref(&(9..11)));
    }
}
