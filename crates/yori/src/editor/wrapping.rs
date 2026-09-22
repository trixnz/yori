//! Presentation-only expansion of aligned rows into wrapped continuations.

use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};

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
    identity: u64,
    rows: Option<Vec<ProjectedRow>>,
    logical_rows: usize,
    visual_rows: usize,
}

impl WrapProjection {
    #[cfg(test)]
    pub fn build(editor: &AlignedEditor, columns: usize) -> Self {
        Self::build_with_identity(editor, columns, 0)
    }

    fn build_with_identity(editor: &AlignedEditor, columns: usize, identity: u64) -> Self {
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

        Self {
            identity,
            rows: Some(rows),
            logical_rows: editor.alignment.rows().len(),
            visual_rows,
        }
    }

    fn unwrapped(logical_rows: usize) -> Self {
        Self {
            identity: 0,
            rows: None,
            logical_rows,
            visual_rows: logical_rows,
        }
    }

    pub fn identity(&self) -> u64 {
        self.identity
    }

    pub fn row<'a>(
        &'a self,
        editor: &AlignedEditor,
        logical: usize,
    ) -> Option<Cow<'a, ProjectedRow>> {
        if logical >= self.logical_rows {
            return None;
        }
        if let Some(rows) = &self.rows {
            return rows.get(logical).map(Cow::Borrowed);
        }

        Some(Cow::Owned(ProjectedRow {
            visual_start: logical,
            height: 1,
            left: editor.wrapped_segments(Side::Left, logical, usize::MAX),
            right: editor.wrapped_segments(Side::Right, logical, usize::MAX),
            incoming: editor.wrapped_segments(Side::Incoming, logical, usize::MAX),
            base: editor.wrapped_base_segments(logical, usize::MAX),
        }))
    }

    pub fn visual_rows(&self) -> usize {
        self.visual_rows
    }

    pub fn visual_location(&self, visual: usize) -> (usize, usize) {
        let Some(rows) = &self.rows else {
            return (visual.min(self.logical_rows), 0);
        };

        let logical = rows.partition_point(|row| row.visual_end() <= visual);
        let continuation = rows
            .get(logical)
            .map_or(0, |row| visual.saturating_sub(row.visual_start));

        (logical, continuation)
    }

    pub fn visual_range(&self, logical: Range<usize>) -> Range<usize> {
        let Some(rows) = &self.rows else {
            return logical.start.min(self.logical_rows)..logical.end.min(self.logical_rows);
        };

        let start = rows
            .get(logical.start)
            .map_or(self.visual_rows, |row| row.visual_start);
        let end = rows
            .get(logical.end)
            .map_or(self.visual_rows, |row| row.visual_start);

        start..end
    }

    pub fn visible_logical_rows(&self, visual: Range<usize>) -> Range<usize> {
        let (start, _) = self.visual_location(visual.start);
        let (mut end, continuation) = self.visual_location(visual.end);
        if continuation > 0 || end < self.logical_rows {
            end += 1;
        }

        start.min(self.logical_rows)..end.min(self.logical_rows)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProjectionKey {
    revision: u64,
    columns: usize,
    viewport_width: u32,
    cell_width: u32,
    font_size: u32,
    font_family: String,
}

#[derive(Clone, Debug)]
pub(super) struct ProjectionCache {
    revision: u64,
    next_identity: Cell<u64>,
    cached: RefCell<Option<(ProjectionKey, Rc<WrapProjection>)>>,
    #[cfg(test)]
    builds: Cell<usize>,
}

impl Default for ProjectionCache {
    fn default() -> Self {
        Self {
            revision: 0,
            next_identity: Cell::new(1),
            cached: RefCell::new(None),
            #[cfg(test)]
            builds: Cell::new(0),
        }
    }
}

impl ProjectionCache {
    fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.cached.get_mut().take();
    }
}

impl AlignedEditor {
    pub(super) fn wrap_projection(&self, window: &mut Window, cx: &App) -> Rc<WrapProjection> {
        if !self.word_wrap.enabled() {
            return Rc::new(WrapProjection::unwrapped(self.alignment.rows().len()));
        }

        let (key, columns) = self.wrap_projection_key(window, cx);
        if let Some((cached_key, projection)) = self.wrap_projection_cache.cached.borrow().as_ref()
            && cached_key == &key
        {
            return Rc::clone(projection);
        }

        let identity = self.wrap_projection_cache.next_identity.get();
        self.wrap_projection_cache
            .next_identity
            .set(identity.wrapping_add(1));
        let projection = Rc::new(WrapProjection::build_with_identity(self, columns, identity));
        #[cfg(test)]
        self.wrap_projection_cache
            .builds
            .set(self.wrap_projection_cache.builds.get() + 1);
        *self.wrap_projection_cache.cached.borrow_mut() = Some((key, Rc::clone(&projection)));

        projection
    }

    pub(super) fn invalidate_wrap_projection(&mut self) {
        self.wrap_projection_cache.invalidate();
        self.visual_affinity = None;
    }

    #[cfg(test)]
    fn wrap_projection_build_count(&self) -> usize {
        self.wrap_projection_cache.builds.get()
    }

    fn wrap_projection_key(&self, window: &mut Window, cx: &App) -> (ProjectionKey, usize) {
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
        let columns = columns.max(1);
        let key = ProjectionKey {
            revision: self.wrap_projection_cache.revision,
            columns,
            viewport_width: width.to_bits(),
            cell_width: cell_width.to_bits(),
            font_size: f32::from(theme.mono_font_size).to_bits(),
            font_family: theme.mono_font_family.to_string(),
        };

        (key, columns)
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
    use gpui_kit::{AppContext, Bounds, EntityInputHandler, TestAppContext, point, px, size};
    use yori::geometry::display_units;
    use yori_diff::merge::MergeSession;
    use yori_document::{
        Document,
        editing::{self, Motion},
    };

    use super::*;
    use crate::editor::{GUTTER_WIDTH, HEADER_HEIGHT, LINE_HEIGHT, PaneDocument, VisualAffinity};

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

    fn caret_visual_row(
        editor: &gpui_kit::Entity<AlignedEditor>,
        side: Side,
        window: &mut Window,
        cx: &App,
    ) -> usize {
        let view = editor.read(cx);
        let offset = view.selection.as_ref().unwrap().head;

        view.source_position(side, offset, window, cx).0
    }

    fn click(
        editor: &gpui_kit::Entity<AlignedEditor>,
        position: gpui_kit::Point<gpui_kit::Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
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
            let first = projection.row(view, 0).unwrap();
            let second = projection.row(view, 1).unwrap();

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

            let projected = projection.row(view, row).unwrap();
            let counts = [Side::Left, Side::Right, Side::Incoming]
                .map(|side| projected.segments(side).len());
            assert!(counts.iter().all(|count| *count > 1));
            assert_eq!(projected.height, *counts.iter().max().unwrap());
        });
    }

    #[gpui_kit::test]
    fn wrap_boundaries_keep_their_visual_row_through_hit_testing_and_navigation(
        cx: &mut TestAppContext,
    ) {
        initialize(cx);

        let left = format!("{}\nnext\n", "left words ".repeat(15));
        let right = format!("{}\nnext\n", "right words ".repeat(40));
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                AlignedEditor::new(
                    pane("left.txt", &left),
                    pane("right.txt", &right),
                    window,
                    cx,
                )
            });
            view.update(cx, |editor, cx| editor.set_word_wrap(true, cx));
            editor = Some(view.clone());

            Root::new(view, window, cx)
        });
        let editor = editor.unwrap();
        cx.update(TestWindowExt::render_frame);

        cx.update(|window, cx| {
            let view = editor.read(cx);
            let projection = view.wrap_projection(window, cx);
            let first = projection.row(view, 0).unwrap();
            let left_continuations = first.segments(Side::Left).len();
            let row_height = first.height;
            assert!(left_continuations >= 2);
            assert!(first.segments(Side::Right).len() > left_continuations);

            let origin = view.content_bounds.get().origin;
            let text_edge = view.geometry().text_viewport_width() - 1.0;
            let left_edge_position = point(
                origin.x + px(GUTTER_WIDTH + text_edge),
                origin.y + px(HEADER_HEIGHT + 1.0),
            );
            let right_edge_position = point(
                origin.x + px(view.geometry().right_pane_left() + GUTTER_WIDTH + text_edge),
                origin.y + px(HEADER_HEIGHT + 1.0),
            );
            let first_visual = first.visual_start;
            let _ = view;

            click(&editor, right_edge_position, window, cx);
            editor.update(cx, |editor, cx| {
                editor.move_cursor(Motion::End, false, window, cx);
            });
            let right_end = editor.read(cx).selection.as_ref().unwrap().head;
            assert_eq!(
                caret_visual_row(&editor, Side::Right, window, cx),
                first_visual
            );

            let utf16 = editing::to_utf16(&right, right_end);
            let ime_bounds = editor
                .update(cx, |editor, cx| {
                    editor.bounds_for_range(
                        utf16..utf16,
                        Bounds::new(point(px(0.0), px(0.0)), size(px(0.0), px(0.0))),
                        window,
                        cx,
                    )
                })
                .unwrap();
            assert_eq!(ime_bounds.origin.y, origin.y + px(HEADER_HEIGHT));

            click(&editor, left_edge_position, window, cx);
            editor.update(cx, |editor, cx| {
                editor.move_cursor(Motion::End, false, window, cx);
            });
            assert_eq!(
                caret_visual_row(&editor, Side::Left, window, cx),
                first_visual
            );

            editor.update(cx, |editor, cx| {
                editor.move_cursor(Motion::Down, false, window, cx);
                editor.move_cursor(Motion::Home, false, window, cx);
            });
            assert_eq!(
                caret_visual_row(&editor, Side::Left, window, cx),
                first_visual + 1
            );

            editor.update(cx, |editor, cx| {
                editor.move_cursor(Motion::Up, false, window, cx);
                for _ in 1..left_continuations {
                    editor.move_cursor(Motion::Down, false, window, cx);
                }
            });
            assert_eq!(
                caret_visual_row(&editor, Side::Left, window, cx),
                first_visual + left_continuations - 1
            );

            editor.update(cx, |editor, cx| {
                editor.move_cursor(Motion::Down, false, window, cx);
            });
            assert_eq!(
                caret_visual_row(&editor, Side::Left, window, cx),
                first_visual + row_height
            );
        });
    }

    #[gpui_kit::test]
    fn expanded_tab_boundaries_keep_exact_continuation_affinity(cx: &mut TestAppContext) {
        initialize(cx);

        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                AlignedEditor::new(
                    pane("left.txt", "\tword\n"),
                    pane("right.txt", "\tword\n"),
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
            let projection = WrapProjection::build(editor.read(cx), 1);
            editor.update(cx, |editor, _| {
                editor.visual_affinity = Some(VisualAffinity {
                    projection: projection.identity(),
                    side: Side::Right,
                    offset: 0,
                    logical_row: 0,
                    continuation: 1,
                });
            });

            assert_eq!(
                editor
                    .read(cx)
                    .source_position_in(Side::Right, 0, &projection, window, cx,),
                (1, 0.0)
            );
        });
    }

    #[gpui_kit::test]
    fn unchanged_coordinate_queries_reuse_wrapped_projection_and_skip_it_unwrapped(
        cx: &mut TestAppContext,
    ) {
        initialize(cx);

        let source = format!("{}\n", "alpha beta gamma ".repeat(20));
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

        cx.update(|window, cx| {
            let view = editor.read(cx);
            let _ = view.source_position(Side::Right, 0, window, cx);
            let _ = view.source_position(Side::Right, 0, window, cx);
            assert_eq!(view.wrap_projection_build_count(), 0);
            let _ = view;

            editor.update(cx, |editor, cx| editor.set_word_wrap(true, cx));
            let view = editor.read(cx);
            let _ = view.source_position(Side::Right, 0, window, cx);
            let builds = view.wrap_projection_build_count();
            assert_eq!(builds, 1);

            for _ in 0..4 {
                let _ = view.source_position(Side::Right, 0, window, cx);
            }
            assert_eq!(view.wrap_projection_build_count(), builds);
            let _ = view;

            window.input("Z", cx);
            let view = editor.read(cx);
            assert!(view.right.document.text().contains('Z'));
            let _ = view.source_position(Side::Right, 0, window, cx);
            assert!(view.wrap_projection_build_count() > builds);
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
            assert!(visual > projection.row(view, logical).unwrap().visual_start);

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
