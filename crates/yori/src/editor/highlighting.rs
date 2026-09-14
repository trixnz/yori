//! Own syntax-query composition and paint-layer projection.

use gpui_kit::component::highlighter::{HighlightTheme, LanguageRegistry, SyntaxHighlighter};
use gpui_kit::{HighlightStyle, Hsla, UnderlineStyle, px};
use std::{
    collections::BTreeMap,
    ops::{Range, RangeInclusive},
    rc::Rc,
    sync::{Arc, OnceLock},
};
use yori::{display::DisplayLine, document_info::Language};
use yori_document::Document;

const CPP_GRAMMAR: &str = "yori-cpp";
const MAX_CACHED_LINES: usize = 512;
const CACHE_RADIUS: usize = MAX_CACHED_LINES / 2;
static REGISTERED_CPP_GRAMMAR: OnceLock<&'static str> = OnceLock::new();

pub(super) fn grammar_for(language: Language) -> Option<&'static str> {
    if language != Language::Cpp {
        return language.grammar();
    }

    Some(REGISTERED_CPP_GRAMMAR.get_or_init(|| {
        let registry = LanguageRegistry::singleton();
        let (Some(c), Some(mut cpp)) = (registry.language("c"), registry.language("cpp")) else {
            return "cpp";
        };

        cpp.name = CPP_GRAMMAR.into();
        cpp.highlights = format!("{}\n{}", c.highlights, cpp.highlights).into();
        registry.register(CPP_GRAMMAR, &cpp);

        CPP_GRAMMAR
    }))
}

type LineStyles = Rc<[(Range<usize>, HighlightStyle)]>;

#[derive(Default)]
pub(super) struct VisibleSyntax {
    first_line: usize,
    lines: Vec<LineStyles>,
}

impl VisibleSyntax {
    pub fn line(&self, line_index: usize) -> &[(Range<usize>, HighlightStyle)] {
        line_index
            .checked_sub(self.first_line)
            .and_then(|index| self.lines.get(index))
            .map_or(&[], AsRef::as_ref)
    }
}

#[derive(Default)]
pub(super) struct SyntaxCache {
    theme: Option<Arc<HighlightTheme>>,
    lines: BTreeMap<usize, LineStyles>,
}

impl SyntaxCache {
    pub fn clear(&mut self) {
        self.lines.clear();
    }

    pub fn visible(
        &mut self,
        highlighter: Option<&SyntaxHighlighter>,
        document: &Document,
        lines: Option<RangeInclusive<usize>>,
        theme: &Arc<HighlightTheme>,
        tab_width: usize,
    ) -> VisibleSyntax {
        let (Some(highlighter), Some(lines)) = (highlighter, lines) else {
            return VisibleSyntax::default();
        };
        let first_line = *lines.start();
        let last_line = *lines.end();
        if document.lines().get(first_line).is_none() || document.lines().get(last_line).is_none() {
            return VisibleSyntax::default();
        }

        if !self
            .theme
            .as_ref()
            .is_some_and(|cached| Arc::ptr_eq(cached, theme))
        {
            self.lines.clear();
            self.theme = Some(theme.clone());
        }

        let missing = self.missing_ranges(first_line..=last_line);
        for range in missing {
            self.populate(highlighter, document, range, theme.as_ref(), tab_width);
        }

        if self.lines.len() > MAX_CACHED_LINES {
            let keep =
                first_line.saturating_sub(CACHE_RADIUS)..=last_line.saturating_add(CACHE_RADIUS);
            self.lines.retain(|line, _| keep.contains(line));
        }

        VisibleSyntax {
            first_line,
            lines: (first_line..=last_line)
                .map(|line| self.lines.get(&line).cloned().unwrap_or_default())
                .collect(),
        }
    }

    fn missing_ranges(&self, lines: RangeInclusive<usize>) -> Vec<RangeInclusive<usize>> {
        let last_line = *lines.end();
        let mut ranges = Vec::new();
        let mut start = None;

        for line in lines {
            if self.lines.contains_key(&line) {
                if let Some(first) = start.take() {
                    ranges.push(first..=line - 1);
                }
            } else {
                start.get_or_insert(line);
            }
        }

        if let Some(first) = start {
            ranges.push(first..=last_line);
        }

        ranges
    }

    fn populate(
        &mut self,
        highlighter: &SyntaxHighlighter,
        document: &Document,
        lines: RangeInclusive<usize>,
        theme: &HighlightTheme,
        tab_width: usize,
    ) {
        let first_line = *lines.start();
        let last_line = *lines.end();
        let source_lines = document.lines();
        let first = &source_lines[first_line];
        let last = &source_lines[last_line];
        let styles = highlighter.styles(&(first.content.start..last.content.end), theme);
        let mut style_start = 0;

        for line_index in lines {
            let line = &source_lines[line_index];
            while styles
                .get(style_start)
                .is_some_and(|(range, _)| range.end <= line.content.start)
            {
                style_start += 1;
            }

            let display = DisplayLine::from_source(
                document.content(line_index),
                line.content.start,
                tab_width,
            );
            let line_styles = styles[style_start..]
                .iter()
                .take_while(|(range, _)| range.start < line.content.end)
                .filter_map(|(range, style)| {
                    let overlap =
                        range.start.max(line.content.start)..range.end.min(line.content.end);
                    if overlap.is_empty() {
                        return None;
                    }

                    let display_range = display.display_range(overlap);
                    (!display_range.is_empty()).then_some((display_range, *style))
                })
                .collect::<Vec<_>>();
            self.lines.insert(line_index, Rc::from(line_styles));
        }
    }
}

pub(super) struct OverlayColors {
    pub changed: Hsla,
    pub selected: Hsla,
    pub foreground: Hsla,
}

pub(super) fn compose(
    text_len: usize,
    syntax: &[(Range<usize>, HighlightStyle)],
    changed: &[Range<usize>],
    selected: Option<&Range<usize>>,
    marked: Option<&Range<usize>>,
    colors: &OverlayColors,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut boundaries = vec![0, text_len];
    for (range, _) in syntax {
        boundaries.extend([range.start, range.end]);
    }
    for range in changed.iter().chain(selected).chain(marked) {
        boundaries.extend([range.start, range.end]);
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    boundaries
        .windows(2)
        .filter_map(|pair| {
            let range = pair[0]..pair[1];
            if range.is_empty() {
                return None;
            }

            let mut style = syntax
                .iter()
                .find(|(span, _)| span.start <= range.start && span.end >= range.end)
                .map(|(_, style)| *style)
                .unwrap_or_default();
            let overlaps = |span: &Range<usize>| span.start < range.end && span.end > range.start;

            if changed.iter().any(overlaps) {
                style.background_color = Some(colors.changed);
            }

            // A text selection must remain unambiguous on top of diff emphasis.
            if selected.is_some_and(overlaps) {
                style.background_color = Some(colors.selected);
            }
            if marked.is_some_and(overlaps) {
                style.underline = Some(UnderlineStyle {
                    color: Some(colors.foreground),
                    thickness: px(1.0),
                    wavy: false,
                });
            }

            Some((range, style))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{LineStyles, OverlayColors, SyntaxCache, compose};
    use gpui_kit::{HighlightStyle, hsla};
    use std::rc::Rc;

    #[test]
    fn missing_lines_are_queried_as_contiguous_ranges() {
        let mut cache = SyntaxCache::default();

        assert_eq!(cache.missing_ranges(10..=49), vec![10..=49]);

        for line in 10..50 {
            cache.lines.insert(line, Rc::from([]) as LineStyles);
        }

        assert_eq!(cache.missing_ranges(11..=50), vec![50..=50]);
        assert_eq!(cache.missing_ranges(100..=139), vec![100..=139]);
    }

    #[test]
    fn selection_wins_over_diff_while_syntax_and_composition_remain_visible() {
        let foreground = hsla(0.6, 0.5, 0.8, 1.0);
        let colors = OverlayColors {
            changed: hsla(0.1, 0.5, 0.3, 1.0),
            selected: hsla(0.6, 0.5, 0.3, 1.0),
            foreground,
        };
        let syntax = vec![(
            0..6,
            HighlightStyle {
                color: Some(foreground),
                ..HighlightStyle::default()
            },
        )];
        let changes = vec![1..5, 7..8];

        let runs = compose(8, &syntax, &changes, Some(&(2..4)), Some(&(3..5)), &colors);

        for byte in 0..8 {
            let (_, style) = runs
                .iter()
                .find(|(range, _)| range.contains(&byte))
                .unwrap();

            assert_eq!(style.color, (byte < 6).then_some(foreground));
            assert_eq!(
                style.background_color,
                if (2..4).contains(&byte) {
                    Some(colors.selected)
                } else if (1..5).contains(&byte) || byte == 7 {
                    Some(colors.changed)
                } else {
                    None
                }
            );
            assert_eq!(style.underline.is_some(), (3..5).contains(&byte));
        }

        assert_eq!(runs.first().unwrap().0.start, 0);
        assert_eq!(runs.last().unwrap().0.end, 8);
        assert!(runs.windows(2).all(|pair| pair[0].0.end == pair[1].0.start));
    }
}
