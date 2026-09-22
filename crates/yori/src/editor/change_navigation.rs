//! Wire change navigation into the existing caret, focus and viewport model.

use super::{AlignedEditor, NextChange, PreviousChange, Selection, Side};
use gpui_kit::{Context, Pixels, Point, Window};
use yori::navigation::{ChangeDirection, ChangeTarget};

impl AlignedEditor {
    pub(super) fn locate_pointer_change(&mut self, position: Point<Pixels>) {
        let row = self
            .geometry()
            .hit(
                f32::from(position.x),
                f32::from(position.y),
                self.vertical_scroll,
                self.horizontal_scroll,
            )
            .row;
        if self.merge.is_some() {
            self.locate_merge_row(row);
        } else {
            self.navigation.locate(row);
        }
    }

    pub(super) fn locate_caret_change(&mut self) {
        if let Some(selection) = &self.selection {
            self.locate_source_change(selection.side, selection.head);
        }
    }

    pub(super) fn locate_source_change(&mut self, side: Side, offset: usize) {
        let row = self.row_for_source(side, offset);
        if self.merge.is_some() {
            self.locate_merge_row(row);
        } else {
            self.navigation.locate(row);
        }
    }

    pub(super) fn previous_change(
        &mut self,
        _: &PreviousChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_change(ChangeDirection::Previous, window, cx);
    }

    pub(super) fn next_change(
        &mut self,
        _: &NextChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_change(ChangeDirection::Next, window, cx);
    }

    pub(super) fn initialize_change_navigation(&mut self) {
        let Some(target) = self
            .navigation
            .advance(&self.alignment, ChangeDirection::Next)
        else {
            return;
        };

        self.pending_initial_change_row = Some(target.rows.start);
        self.apply_change_target(&target);
    }

    pub(super) fn resolve_initial_change_viewport(&mut self) {
        let geometry = self.geometry();
        if geometry.rows_viewport_height() <= 0.0 {
            return;
        }
        let Some(first_row) = self.pending_initial_change_row.take() else {
            return;
        };

        self.vertical_scroll = geometry.change_scroll_top(first_row, self.alignment.rows().len());
    }

    fn navigate_change(
        &mut self,
        direction: ChangeDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.merge.is_some() {
            self.navigate_merge(matches!(direction, ChangeDirection::Previous), window, cx);
            return;
        }

        self.cancel_vim();
        self.finish_composition();
        let Some(target) = self.navigation.advance(&self.alignment, direction) else {
            return;
        };

        self.pending_initial_change_row = None;
        self.apply_change_target(&target);
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn apply_change_target(&mut self, target: &ChangeTarget) {
        self.selection = Some(Selection {
            side: Side::Right,
            anchor: target.right_offset,
            head: target.right_offset,
        });
        self.preferred_column = None;
        self.horizontal_scroll = 0.0;
        self.vertical_scroll = self
            .geometry()
            .change_scroll_top(target.rows.start, self.alignment.rows().len());
    }
}

#[cfg(test)]
mod tests {
    use std::{fmt::Write as _, path::PathBuf};

    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        AppContext, ParentElement, Render, Styled, TestAppContext, component::Root, div, px,
    };
    use yori::geometry::display_units;
    use yori_document::Document;

    use super::*;
    use crate::editor::{LINE_HEIGHT, PaneDocument};

    struct ConstrainedEditor {
        editor: gpui_kit::Entity<AlignedEditor>,
    }

    impl Render for ConstrainedEditor {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui_kit::IntoElement {
            div().w_full().h(px(220.0)).child(self.editor.clone())
        }
    }

    fn initialize(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::appearance::init(cx);
            crate::editor::init(cx);
        });
    }

    fn pane(name: &str, text: &str) -> PaneDocument {
        PaneDocument::new(
            PathBuf::from(name),
            Document::from_bytes(text.as_bytes().to_vec()).unwrap(),
        )
    }

    fn comparison(change: &str) -> (String, String) {
        let mut prefix = String::new();
        let mut suffix = String::new();
        for index in 0..20 {
            writeln!(prefix, "prefix {index}").unwrap();
        }
        for index in 0..40 {
            writeln!(suffix, "suffix {index}").unwrap();
        }

        match change {
            "insertion" => (
                format!("{prefix}{suffix}"),
                format!("{prefix}inserted\n{suffix}"),
            ),
            "deletion" => (
                format!("{prefix}deleted\n{suffix}"),
                format!("{prefix}{suffix}"),
            ),
            "replacement" => (
                format!("{prefix}before\n{suffix}"),
                format!("{prefix}after\n{suffix}"),
            ),
            _ => unreachable!("unknown comparison case"),
        }
    }

    fn assert_first_change(editor: &AlignedEditor, case: &str) {
        let block = &editor.alignment.blocks()[0];
        let selection = editor.selection.as_ref().unwrap();

        assert_eq!(
            editor.navigation.current(&editor.alignment),
            Some(0),
            "{case}"
        );
        assert_eq!(selection.side, Side::Right, "{case}");
        assert_eq!(selection.anchor, block.right.start, "{case}");
        assert_eq!(selection.head, block.right.start, "{case}");
        assert!(
            editor
                .right
                .document
                .text()
                .is_char_boundary(selection.head)
        );

        let expected_scroll = editor
            .geometry()
            .change_scroll_top(block.rows.start, editor.alignment.rows().len());
        assert!(
            (editor.vertical_scroll - expected_scroll).abs() < f32::EPSILON,
            "{case}"
        );
        assert!(editor.vertical_scroll > 0.0, "{case}");

        let change_top = display_units(block.rows.start) * LINE_HEIGHT;
        let viewport_bottom = editor.vertical_scroll + editor.geometry().rows_viewport_height();
        assert!(change_top >= editor.vertical_scroll, "{case}");
        assert!(change_top + LINE_HEIGHT <= viewport_bottom, "{case}");
    }

    #[gpui_kit::test]
    fn two_way_construction_initializes_each_change_shape_before_render(cx: &mut TestAppContext) {
        initialize(cx);

        let (_, _) = cx.add_window_view(|window, cx| {
            let mut root = None;
            for case in ["insertion", "deletion", "replacement"] {
                let (baseline, local) = comparison(case);
                let editor = cx.new(|cx| {
                    AlignedEditor::new(
                        pane("baseline.txt", &baseline),
                        pane("local.txt", &local),
                        window,
                        cx,
                    )
                });

                assert_first_change(editor.read(cx), case);
                root = Some(editor);
            }

            Root::new(root.unwrap(), window, cx)
        });
    }

    #[gpui_kit::test]
    fn eof_change_uses_constrained_geometry_on_the_first_frame(cx: &mut TestAppContext) {
        initialize(cx);

        let mut baseline = String::new();
        let mut local = String::new();
        for index in 0..100 {
            writeln!(baseline, "unchanged {index}").unwrap();
            writeln!(local, "unchanged {index}").unwrap();
        }
        baseline.push_str("before\n");
        local.push_str("after\n");

        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                AlignedEditor::new(
                    pane("baseline.txt", &baseline),
                    pane("local.txt", &local),
                    window,
                    cx,
                )
            });

            assert_eq!(
                view.read(cx).navigation.current(&view.read(cx).alignment),
                Some(0)
            );
            assert_eq!(
                view.read(cx).selection.as_ref().unwrap().head,
                view.read(cx).alignment.blocks()[0].right.start
            );
            editor = Some(view.clone());

            let constrained = cx.new(|_| ConstrainedEditor { editor: view });
            Root::new(constrained, window, cx)
        });
        let editor = editor.unwrap();

        cx.update(TestWindowExt::render_frame);
        let first_frame_scroll = cx.update(|window, cx| {
            let editor = editor.read(cx);
            let geometry = editor.geometry();
            let block = &editor.alignment.blocks()[0];
            let expected =
                geometry.change_scroll_top(block.rows.start, editor.alignment.rows().len());

            assert!(
                f32::from(editor.content_bounds.get().size.height)
                    < f32::from(window.viewport_size().height) / 2.0
            );
            assert!((editor.vertical_scroll - expected).abs() < f32::EPSILON);

            let change_top = display_units(block.rows.start) * LINE_HEIGHT;
            assert!(change_top >= editor.vertical_scroll);
            assert!(
                change_top + LINE_HEIGHT
                    <= editor.vertical_scroll + geometry.rows_viewport_height()
            );

            editor.vertical_scroll
        });

        cx.update(TestWindowExt::render_frame);
        cx.update(|_, cx| {
            assert!((editor.read(cx).vertical_scroll - first_frame_scroll).abs() < f32::EPSILON);
        });
    }

    #[gpui_kit::test]
    fn next_change_after_construction_advances_to_the_second_change(cx: &mut TestAppContext) {
        initialize(cx);

        let (_, _) = cx.add_window_view(|window, cx| {
            let baseline = "old first\nkeep\nold second\n";
            let local = "new first\nkeep\nnew second\n";
            let editor = cx.new(|cx| {
                AlignedEditor::new(
                    pane("baseline.txt", baseline),
                    pane("local.txt", local),
                    window,
                    cx,
                )
            });

            assert_eq!(
                editor
                    .read(cx)
                    .navigation
                    .current(&editor.read(cx).alignment),
                Some(0)
            );
            editor.update(cx, |editor, cx| editor.next_change(&NextChange, window, cx));

            let state = editor.read(cx);
            let second = &state.alignment.blocks()[1];
            assert_eq!(state.navigation.current(&state.alignment), Some(1));
            assert_eq!(state.selection.as_ref().unwrap().head, second.right.start);
            assert!(state.pending_initial_change_row.is_none());

            Root::new(editor, window, cx)
        });
    }

    #[gpui_kit::test]
    fn equal_comparison_retains_a_neutral_initial_state(cx: &mut TestAppContext) {
        initialize(cx);

        let (_, _) = cx.add_window_view(|window, cx| {
            let editor = cx.new(|cx| {
                AlignedEditor::new(
                    pane("baseline.txt", "same\n"),
                    pane("local.txt", "same\n"),
                    window,
                    cx,
                )
            });

            let state = editor.read(cx);
            assert_eq!(state.navigation.current(&state.alignment), None);
            assert!(state.selection.is_none());
            assert!(state.vertical_scroll.abs() < f32::EPSILON);
            assert!(state.horizontal_scroll.abs() < f32::EPSILON);

            Root::new(editor, window, cx)
        });
    }

    #[gpui_kit::test]
    fn review_construction_initializes_without_taking_navigator_focus(cx: &mut TestAppContext) {
        initialize(cx);

        let mut editor = None;
        let mut navigator = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let navigator_focus = cx.focus_handle();
            navigator_focus.focus(window, cx);
            let (baseline, local) = comparison("replacement");
            let view = cx.new(|cx| {
                AlignedEditor::new_review_diff(
                    pane("baseline.txt", &baseline),
                    pane("local.txt", &local),
                    true,
                    true,
                    window,
                    cx,
                )
            });

            assert_first_change(view.read(cx), "review replacement");
            assert!(navigator_focus.is_focused(window));
            assert!(!view.read(cx).focus.is_focused(window));
            editor = Some(view.clone());
            navigator = Some(navigator_focus);

            let constrained = cx.new(|_| ConstrainedEditor { editor: view });
            Root::new(constrained, window, cx)
        });
        let editor = editor.unwrap();
        let navigator = navigator.unwrap();

        cx.update(TestWindowExt::render_frame);
        cx.update(|window, cx| {
            assert!(navigator.is_focused(window));
            assert!(!editor.read(cx).focus.is_focused(window));
        });
    }
}
