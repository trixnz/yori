//! Presentation-only tab expansion and aligned hit mapping.

use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use yori_diff::Alignment;
use yori_document::Document;

/// Maps an aligned row and shaped display byte to an authoritative source offset.
/// A row below the complete alignment is EOF, making a final line terminator selectable.
#[must_use]
pub fn source_offset_at(
    alignment: &Alignment,
    document: &Document,
    row: usize,
    left_side: bool,
    display_byte: usize,
    tab_width: usize,
) -> usize {
    let Some(alignment_row) = alignment.rows().get(row) else {
        return document.text().len();
    };

    let line = if left_side {
        alignment_row.left
    } else {
        alignment_row.right
    };
    let Some(line) = line else {
        return alignment.gap_offset(document, row, left_side);
    };

    let source_line = &document.lines()[line];
    DisplayLine::from_source(document.content(line), source_line.content.start, tab_width)
        .source_offset(display_byte)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhitespaceKind {
    Space,
    Tab,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhitespaceMark {
    pub display: Range<usize>,
    pub kind: WhitespaceKind,
    pub trailing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayLine {
    pub text: String,
    columns: usize,
    // Character boundaries in display bytes mapped to authoritative source bytes.
    boundaries: Vec<(usize, usize)>,
    // One span per source character; tabs own their complete visual expansion.
    spans: Vec<(Range<usize>, Range<usize>)>,
}

impl DisplayLine {
    #[must_use]
    pub fn from_source(text: &str, source_start: usize, tab_width: usize) -> Self {
        let mut display = String::new();
        let mut boundaries = vec![(0, source_start)];
        let mut spans = Vec::new();
        let mut column = 0;

        for (relative, character) in text.char_indices() {
            let source = source_start + relative;
            let display_start = display.len();

            if character == '\t' {
                let width = tab_width - (column % tab_width);
                for step in 0..width {
                    display.push(' ');
                    let mapped = if (step + 1) * 2 <= width {
                        source
                    } else {
                        source + 1
                    };
                    boundaries.push((display.len(), mapped));
                }
                column += width;
            } else {
                display.push(character);
                column += unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
                boundaries.push((display.len(), source + character.len_utf8()));
            }

            spans.push((
                source..source + character.len_utf8(),
                display_start..display.len(),
            ));
        }

        Self {
            text: display,
            columns: column,
            boundaries,
            spans,
        }
    }

    /// Describe overlays for the same source used to build this display line.
    /// Markers never replace the shaped text or participate in source mapping.
    #[must_use]
    pub fn whitespace_marks(&self, source: &str) -> Vec<WhitespaceMark> {
        let trailing_start = source.trim_end_matches([' ', '\t']).len();
        let source_start = self.boundaries[0].1;

        self.spans
            .iter()
            .zip(source.chars())
            .filter_map(|((bytes, display), character)| {
                let kind = match character {
                    ' ' => WhitespaceKind::Space,
                    '\t' => WhitespaceKind::Tab,
                    _ => return None,
                };

                Some(WhitespaceMark {
                    display: display.clone(),
                    kind,
                    trailing: bytes.start - source_start >= trailing_start,
                })
            })
            .collect()
    }

    #[must_use]
    pub fn columns(&self) -> usize {
        self.columns
    }

    #[must_use]
    pub fn source_offset(&self, display_byte: usize) -> usize {
        match self
            .boundaries
            .binary_search_by_key(&display_byte, |&(display, _)| display)
        {
            Ok(index) => self.boundaries[index].1,
            Err(index) => self.boundaries[index.saturating_sub(1)].1,
        }
    }

    #[must_use]
    pub fn display_offset(&self, source: usize) -> usize {
        self.spans
            .iter()
            .find(|(span, _)| source < span.end)
            .map_or(self.text.len(), |(_, display)| display.start)
    }

    #[must_use]
    pub fn display_range(&self, source: Range<usize>) -> Range<usize> {
        let mut overlapping = self.spans.iter().filter(|(source_span, _)| {
            source_span.start < source.end && source_span.end > source.start
        });
        let Some((_, first)) = overlapping.next() else {
            return 0..0;
        };

        let mut range = first.clone();
        for (_, display_span) in overlapping {
            range.end = display_span.end;
        }

        range
    }

    /// Split presentation text into byte ranges that fit the requested columns.
    /// Whitespace is preferred as a boundary; a single grapheme is the indivisible fallback.
    #[must_use]
    pub fn wrapped_ranges(&self, max_columns: usize) -> Vec<Range<usize>> {
        if self.text.is_empty() {
            return std::iter::once(0..0).collect();
        }

        let max_columns = max_columns.max(1);
        let mut ranges = Vec::new();
        let mut start = 0;

        while start < self.text.len() {
            let mut columns = 0;
            let mut fitting_end = start;
            let mut whitespace_end = None;
            let mut overflowed = false;

            for (relative, grapheme) in self.text[start..].grapheme_indices(true) {
                let grapheme_start = start + relative;
                let grapheme_end = grapheme_start + grapheme.len();
                let width = UnicodeWidthStr::width(grapheme);

                if fitting_end > start && columns + width > max_columns {
                    overflowed = true;
                    break;
                }

                fitting_end = grapheme_end;
                columns += width;
                if grapheme.chars().all(char::is_whitespace) {
                    whitespace_end = Some(grapheme_end);
                }
            }

            let end = if overflowed {
                whitespace_end
                    .filter(|end| *end > start)
                    .unwrap_or(fitting_end)
            } else {
                fitting_end
            };
            ranges.push(start..end);
            start = end;
        }

        ranges
    }
}

#[must_use]
pub fn max_display_columns(document: &Document, tab_width: usize) -> usize {
    document
        .lines()
        .iter()
        .map(|line| {
            document
                .copy_range(line.content.clone())
                .chars()
                .fold(0, |column, ch| {
                    column
                        + if ch == '\t' {
                            tab_width - column % tab_width
                        } else {
                            unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0)
                        }
                })
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::DisplayLine;

    #[test]
    fn wrapping_prefers_whitespace_without_changing_display_bytes() {
        let display = DisplayLine::from_source("alpha beta gamma", 0, 4);
        let ranges = display.wrapped_ranges(7);
        let pieces = ranges
            .iter()
            .map(|range| &display.text[range.clone()])
            .collect::<Vec<_>>();

        assert_eq!(pieces, ["alpha ", "beta ", "gamma"]);
        assert_eq!(pieces.concat(), display.text);
    }

    #[test]
    fn wrapping_keeps_short_indented_and_exact_fit_lines_whole() {
        for (text, columns) in [("short line", 20), ("    indented", 20), ("exact fit", 9)] {
            let display = DisplayLine::from_source(text, 0, 4);

            assert_eq!(
                display.wrapped_ranges(columns),
                std::iter::once(0..display.text.len()).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn wrapping_only_prefers_whitespace_after_overflow() {
        let display = DisplayLine::from_source("alpha beta", 0, 4);
        let ranges = display.wrapped_ranges(10);

        assert_eq!(
            ranges,
            std::iter::once(0..display.text.len()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn wrapping_falls_back_to_graphemes_and_keeps_tabs_expanded() {
        let display = DisplayLine::from_source("\t界e\u{301}abcdef", 20, 4);
        let ranges = display.wrapped_ranges(4);
        let pieces = ranges
            .iter()
            .map(|range| &display.text[range.clone()])
            .collect::<Vec<_>>();

        assert_eq!(pieces, ["    ", "界e\u{301}a", "bcde", "f"]);
        assert_eq!(pieces.concat(), display.text);
        assert!(
            ranges
                .iter()
                .all(|range| display.text.is_char_boundary(range.start)
                    && display.text.is_char_boundary(range.end))
        );
        assert_eq!(display.source_offset(ranges[1].start), 21);
    }

    #[test]
    fn wrapping_never_splits_a_wide_grapheme_even_below_its_width() {
        let display = DisplayLine::from_source("👨‍👩‍👧‍👦x", 0, 4);
        let ranges = display.wrapped_ranges(1);

        assert_eq!(&display.text[ranges[0].clone()], "👨‍👩‍👧‍👦");
        assert_eq!(&display.text[ranges[1].clone()], "x");
    }
}
