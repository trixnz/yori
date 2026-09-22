//! Presentation-only expansion of aligned rows into wrapped continuations.

use std::ops::Range;

use gpui_kit::component::ActiveTheme;
use gpui_kit::{App, Font, TextRun, Window};
use yori::display::DisplayLine;

use super::{AlignedEditor, Side, TAB_WIDTH};

#[derive(Clone, Debug)]
pub(super) struct ProjectedRow {
    pub visual_start: usize,
    pub height: usize,
    left: Vec<Range<usize>>,
    right: Vec<Range<usize>>,
    incoming: Vec<Range<usize>>,
    base: Vec<Range<usize>>,
}

impl ProjectedRow {
    pub fn segments(&self, side: Side) -> &[Range<usize>] {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
            Side::Incoming => &self.incoming,
        }
    }

    pub fn base_segments(&self) -> &[Range<usize>] {
        &self.base
    }

    pub fn visual_end(&self) -> usize {
        self.visual_start + self.height
    }
}

#[derive(Clone, Debug)]
pub(super) struct WrapProjection {
    rows: Vec<ProjectedRow>,
    visual_rows: usize,
}

impl WrapProjection {
    pub fn build(editor: &AlignedEditor, columns: usize) -> Self {
        let mut rows = Vec::with_capacity(editor.alignment.rows().len());
        let mut visual_rows = 0;

        for row in 0..editor.alignment.rows().len() {
            let left = editor.wrapped_segments(Side::Left, row, columns);
            let right = editor.wrapped_segments(Side::Right, row, columns);
            let incoming = editor.wrapped_segments(Side::Incoming, row, columns);
            let base = editor.wrapped_base_segments(row, columns);
            let height = left
                .len()
                .max(right.len())
                .max(incoming.len())
                .max(base.len())
                .max(1);

            rows.push(ProjectedRow {
                visual_start: visual_rows,
                height,
                left,
                right,
                incoming,
                base,
            });
            visual_rows += height;
        }

        Self { rows, visual_rows }
    }

    pub fn rows(&self) -> &[ProjectedRow] {
        &self.rows
    }

    pub fn row(&self, logical: usize) -> Option<&ProjectedRow> {
        self.rows.get(logical)
    }

    pub fn visual_rows(&self) -> usize {
        self.visual_rows
    }

    pub fn visual_location(&self, visual: usize) -> (usize, usize) {
        let logical = self.rows.partition_point(|row| row.visual_end() <= visual);
        let continuation = self
            .rows
            .get(logical)
            .map_or(0, |row| visual.saturating_sub(row.visual_start));

        (logical, continuation)
    }

    pub fn visual_range(&self, logical: Range<usize>) -> Range<usize> {
        let start = self
            .rows
            .get(logical.start)
            .map_or(self.visual_rows, |row| row.visual_start);
        let end = self
            .rows
            .get(logical.end)
            .map_or(self.visual_rows, |row| row.visual_start);

        start..end
    }

    pub fn visible_logical_rows(&self, visual: Range<usize>) -> Range<usize> {
        let (start, _) = self.visual_location(visual.start);
        let (mut end, continuation) = self.visual_location(visual.end);
        if continuation > 0 || end < self.rows.len() {
            end += 1;
        }

        start.min(self.rows.len())..end.min(self.rows.len())
    }
}

impl AlignedEditor {
    pub(super) fn wrap_projection(&self, window: &mut Window, cx: &App) -> WrapProjection {
        WrapProjection::build(self, self.wrap_columns(window, cx))
    }

    fn wrap_columns(&self, window: &mut Window, cx: &App) -> usize {
        if !self.word_wrap.enabled() {
            return usize::MAX;
        }

        let theme = cx.theme();
        let run = TextRun {
            len: 1,
            font: Font {
                family: theme.mono_font_family.clone(),
                ..Font::default()
            },
            color: theme.foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let cell_width = f32::from(
            window
                .text_system()
                .shape_line(" ".into(), theme.mono_font_size, &[run], None)
                .width(),
        )
        .max(f32::EPSILON);
        let width = self.geometry().text_viewport_width().max(cell_width);

        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the positive pane width is intentionally floored to a whole display-column count"
        )]
        let columns = (width / cell_width).floor() as usize;

        columns.max(1)
    }

    fn wrapped_segments(&self, side: Side, row: usize, columns: usize) -> Vec<Range<usize>> {
        if side == Side::Incoming && self.merge.is_none() {
            return Vec::new();
        }
        let Some(line) = self.line_for_row(side, row) else {
            return Vec::new();
        };
        let pane = self.document(side);
        let source = &pane.document.lines()[line];
        let display =
            DisplayLine::from_source(pane.document.content(line), source.content.start, TAB_WIDTH);

        if self.word_wrap.enabled() {
            display.wrapped_ranges(columns)
        } else {
            std::iter::once(0..display.text.len()).collect()
        }
    }

    fn wrapped_base_segments(&self, row: usize, columns: usize) -> Vec<Range<usize>> {
        let Some(merge) = &self.merge else {
            return Vec::new();
        };
        let Some(super::merge::RowKind::Base(super::merge::BaseRow::SourceLine(line))) =
            merge.display.rows().get(row).map(|row| &row.kind)
        else {
            return Vec::new();
        };
        let display = DisplayLine::from_source(merge.session.base().content(*line), 0, TAB_WIDTH);

        if self.word_wrap.enabled() {
            display.wrapped_ranges(columns)
        } else {
            std::iter::once(0..display.text.len()).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::Root;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext, TestAppContext, point, px};
    use yori::geometry::display_units;
    use yori_diff::merge::MergeSession;
    use yori_document::{Document, editing::Motion};

    use super::*;
    use crate::editor::{GUTTER_WIDTH, HEADER_HEIGHT, LINE_HEIGHT, PaneDocument};

    fn document(text: &str) -> Document {
        Document::from_bytes(text.as_bytes().to_vec()).unwrap()
    }

    fn pane(name: &str, text: &str) -> PaneDocument {
        PaneDocument::new(name.into(), document(text))
    }

    fn initialize(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::appearance::init(cx);
            crate::editor::init(cx);
        });
    }

    #[gpui_kit::test]
    fn two_way_rows_use_the_tallest_wrapped_pane_without_inventing_source(cx: &mut TestAppContext) {
        initialize(cx);

        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                AlignedEditor::new(
                    pane("left.txt", "short\nnext\n"),
                    pane("right.txt", "alpha beta gamma delta\nnext\n"),
                    window,
                    cx,
                )
            });
            view.update(cx, |editor, cx| editor.set_word_wrap(true, cx));
            editor = Some(view.clone());

            Root::new(view, window, cx)
        });
        let editor = editor.unwrap();

        cx.update(|window, cx| {
            let view = editor.read(cx);
            let projection = WrapProjection::build(view, 6);
            let first = projection.row(0).unwrap();
            let second = projection.row(1).unwrap();

            assert_eq!(first.segments(Side::Left).len(), 1);
            assert_eq!(first.segments(Side::Right).len(), 4);
            assert_eq!(first.height, 4);
            assert_eq!(second.visual_start, 4);
            assert_eq!(view.left.document.text(), "short\nnext\n");
            assert_eq!(view.right.document.text(), "alpha beta gamma delta\nnext\n");

            let _ = view;
            window.render_frame(cx);
            assert!(window.try_find("horizontal-scrollbar").is_none());
            assert!(editor.read(cx).max_horizontal_scroll(window, cx).abs() < f32::EPSILON);
        });
    }

    #[gpui_kit::test]
    fn merge_projection_wraps_local_result_and_incoming_as_one_aligned_row(
        cx: &mut TestAppContext,
    ) {
        initialize(cx);

        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let session = MergeSession::new(
                document("base words here\n"),
                document("local words are substantially longer\n"),
                document("incoming tokenwithoutbreaks\n"),
            )
            .unwrap();
            let view = cx.new(|cx| AlignedEditor::from_merge_session(session, window, cx));
            view.update(cx, |editor, cx| editor.set_word_wrap(true, cx));
            editor = Some(view.clone());

            Root::new(view, window, cx)
        });
        let editor = editor.unwrap();

        cx.update(|_, cx| {
            let view = editor.read(cx);
            let projection = WrapProjection::build(view, 8);
            let row = view.row_for_source(Side::Right, 0);
            assert_eq!(view.row_for_source(Side::Left, 0), row);
            assert_eq!(view.row_for_source(Side::Incoming, 0), row);

            let projected = projection.row(row).unwrap();
            let counts = [Side::Left, Side::Right, Side::Incoming]
                .map(|side| projected.segments(side).len());
            assert!(counts.iter().all(|count| *count > 1));
            assert_eq!(projected.height, *counts.iter().max().unwrap());
        });
    }

    #[gpui_kit::test]
    fn wrapped_hit_testing_caret_geometry_and_horizontal_scroll_round_trip(
        cx: &mut TestAppContext,
    ) {
        initialize(cx);

        let source = format!("{}\n", "alpha beta\t界 gamma delta epsilon ".repeat(8));
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                AlignedEditor::new(
                    pane("left.txt", "old\n"),
                    pane("right.txt", &source),
                    window,
                    cx,
                )
            });
            editor = Some(view.clone());

            Root::new(view, window, cx)
        });
        let editor = editor.unwrap();
        cx.update(TestWindowExt::render_frame);

        cx.update(|window, cx| {
            let unwrapped_limit = editor.read(cx).max_horizontal_scroll(window, cx);
            assert!(unwrapped_limit > 0.0);

            editor.update(cx, |editor, cx| editor.set_word_wrap(true, cx));
            window.render_frame(cx);

            let offset = source.match_indices("gamma").nth(4).unwrap().0;
            let view = editor.read(cx);
            let logical = view.row_for_source(Side::Right, offset);
            let projection = view.wrap_projection(window, cx);
            let (visual, x) = view.source_position(Side::Right, offset, window, cx);
            assert!(visual > projection.row(logical).unwrap().visual_start);

            let origin = view.content_bounds.get().origin;
            let position = point(
                origin.x + px(view.geometry().right_pane_left() + GUTTER_WIDTH + x),
                origin.y
                    + px(HEADER_HEIGHT + display_units(visual) * LINE_HEIGHT
                        - view.vertical_scroll
                        + 1.0),
            );
            assert_eq!(
                view.source_offset_at(position, window, cx),
                (Side::Right, offset)
            );
            assert!(view.horizontal_scroll.abs() < f32::EPSILON);
            assert!(window.try_find("horizontal-scrollbar").is_none());
            assert_eq!(view.right.document.text(), source);

            let _ = view;
            editor.update(cx, |editor, cx| {
                editor.mouse_down(
                    &gpui_kit::MouseDownEvent {
                        button: gpui_kit::MouseButton::Left,
                        position,
                        ..Default::default()
                    },
                    window,
                    cx,
                );
            });
            editor.update(cx, |editor, cx| {
                editor.move_cursor(Motion::Down, false, window, cx);
            });
            let moved = editor.read(cx).right_selection().unwrap().head;
            assert_eq!(
                editor
                    .read(cx)
                    .source_position(Side::Right, moved, window, cx)
                    .0,
                visual + 1
            );
            editor.update(cx, |editor, cx| {
                editor.move_cursor(Motion::Up, false, window, cx);
            });
            assert_eq!(editor.read(cx).right_selection().unwrap().head, offset);

            editor.update(cx, |editor, cx| {
                editor.mouse_down(
                    &gpui_kit::MouseDownEvent {
                        button: gpui_kit::MouseButton::Left,
                        position,
                        ..Default::default()
                    },
                    window,
                    cx,
                );
            });
            window.input("Z", cx);
            assert_eq!(
                editor.read(cx).right.document.text(),
                format!("{}Z{}", &source[..offset], &source[offset..])
            );
            window.press("ctrl-z", cx);
            assert_eq!(editor.read(cx).right.document.text(), source);

            editor.update(cx, |editor, cx| editor.set_word_wrap(false, cx));
            window.render_frame(cx);
            assert!(window.try_find("horizontal-scrollbar").is_some());
            assert!(editor.read(cx).max_horizontal_scroll(window, cx) > 0.0);
            assert_eq!(editor.read(cx).right.document.text(), source);
        });
    }
}
