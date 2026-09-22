//! Scroll interaction and geometry, not widget-presence assertions.

use super::*;
use crate::editor::{GUTTER_WIDTH, PaneDocument};
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};
use yori_document::Document;

fn harness(cx: &mut TestAppContext) -> (Entity<AlignedEditor>, &mut VisualTestContext) {
    harness_with_text(cx, String::new(), "long local line\n".repeat(1_000))
}

fn harness_with_text(
    cx: &mut TestAppContext,
    left: String,
    right: String,
) -> (Entity<AlignedEditor>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::appearance::init(cx);
        crate::editor::init(cx);
    });

    let mut editor = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let pane = |name: &str, text: String| {
            PaneDocument::new(
                name.into(),
                Document::from_bytes(text.into_bytes()).unwrap(),
            )
        };
        let view = cx.new(|cx| {
            AlignedEditor::new(
                pane("baseline.txt", left),
                pane("local.txt", right),
                window,
                cx,
            )
        });
        editor = Some(view.clone());

        Root::new(view, window, cx)
    });
    cx.update(TestWindowExt::render_frame);
    cx.run_until_parked();

    (editor.unwrap(), cx)
}

#[gpui_kit::test]
fn track_jump_and_drag_outside_the_rail_preserve_selection_and_source(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    let (rail, selection, source, current) = cx.update(|window, cx| {
        let rows = window.find("rows-viewport").bounds();
        window.click_at(
            "rows-viewport",
            point(rows.size.width / 2.0 + px(GUTTER_WIDTH), px(11.0)),
            cx,
        );
        window.press("home", cx);
        window.input("X", cx);
        window.press("shift-right", cx);
        window.render_frame(cx);

        let view = editor.read(cx);
        (
            window.find("diff-scrollbar").bounds(),
            view.right_selection(),
            view.right.document.text().to_owned(),
            view.navigation.current(&view.alignment),
        )
    });

    let middle = point(rail.center().x, rail.center().y);
    cx.simulate_click(middle, Modifiers::default());
    cx.run_until_parked();

    let grab = cx.update(|window, cx| {
        window.render_frame(cx);
        let view = editor.read(cx);
        let track = view.scroll_track();
        assert!(view.vertical_scroll > track.max_scroll() * 0.4);
        assert!(view.vertical_scroll < track.max_scroll() * 0.6);
        assert_eq!(view.right_selection(), selection);
        assert!(view.focus.is_focused(window));

        let thumb = track.thumb(view.vertical_scroll);
        point(
            rail.center().x,
            rail.top() + px(thumb.start.midpoint(thumb.end)),
        )
    });

    cx.simulate_mouse_down(grab, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);
    let outside = point(rail.left() - px(150.0), rail.bottom() + px(30.0));
    cx.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();

    cx.update(|window, cx| {
        let view = editor.read(cx);
        assert!((view.vertical_scroll - view.scroll_track().max_scroll()).abs() < f32::EPSILON);
        assert_eq!(view.right_selection(), selection);
        assert_eq!(view.right.document.text(), source);
        assert_eq!(view.navigation.current(&view.alignment), current);
        assert!(view.scrollbar_grab.is_none());

        window.press("ctrl-z", cx);
        assert_eq!(
            editor.read(cx).right.document.text(),
            "long local line\n".repeat(1_000)
        );
        assert!(!editor.read(cx).is_dirty());
    });
}

#[gpui_kit::test]
fn editor_deactivation_clears_both_drag_states_before_later_selection(cx: &mut TestAppContext) {
    let source = format!("{}\n", "0123456789".repeat(100)).repeat(100);
    let (editor, cx) = harness_with_text(cx, String::new(), source);
    let (vertical_grab, horizontal_grab, selection_start, selection_end) =
        cx.update(|window, cx| {
            window.render_frame(cx);
            let view = editor.read(cx);
            let vertical = window.find("diff-scrollbar").bounds();
            let vertical_thumb = view.scroll_track().thumb(view.vertical_scroll);
            let horizontal = window.find("horizontal-scrollbar").bounds();
            let max_horizontal = view.max_horizontal_scroll(window, cx);
            let horizontal_thumb = view
                .horizontal_scroll_track(max_horizontal)
                .thumb(view.horizontal_scroll);
            let rows = window.find("rows-viewport").bounds();
            let selection_start = point(
                rows.left() + rows.size.width / 2.0 + px(GUTTER_WIDTH + 10.0),
                rows.top() + px(11.0),
            );

            (
                point(
                    vertical.center().x,
                    vertical.top() + px(vertical_thumb.start.midpoint(vertical_thumb.end)),
                ),
                point(
                    horizontal.left() + px(horizontal_thumb.start.midpoint(horizontal_thumb.end)),
                    horizontal.center().y,
                ),
                selection_start,
                point(selection_start.x + px(60.0), selection_start.y),
            )
        });

    cx.simulate_mouse_down(vertical_grab, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_down(horizontal_grab, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let view = editor.read(cx);
        assert!(view.scrollbar_grab.is_some());
        assert!(view.horizontal_scrollbar_grab.is_some());

        editor.update(cx, AlignedEditor::deactivate);

        let view = editor.read(cx);
        assert!(view.scrollbar_grab.is_none());
        assert!(view.horizontal_scrollbar_grab.is_none());
    });

    cx.simulate_mouse_down(selection_start, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(selection_end, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(selection_end, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();

    cx.update(|_, cx| {
        let view = editor.read(cx);
        let selection = view.right_selection().expect("right pane selection");

        assert!(!selection.range().is_empty());
        assert!(view.vertical_scroll.abs() < f32::EPSILON);
        assert!(view.horizontal_scroll.abs() < f32::EPSILON);
        assert!(view.scrollbar_grab.is_none());
        assert!(view.horizontal_scrollbar_grab.is_none());
    });
}

#[gpui_kit::test]
fn resize_and_edit_keep_the_track_aligned_and_clamp_short_documents(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);

    for width in [1280.0, 700.0] {
        cx.simulate_resize(size(px(width), px(620.0)));
        cx.run_until_parked();

        cx.update(|window, cx| {
            window.render_frame(cx);
            let rail = window.find("diff-scrollbar").bounds();
            let rows = window.find("rows-viewport").bounds();
            let footer = window.find("editor-footer").bounds();
            let content = window.find("aligned-editor").bounds();

            assert_eq!(rail.left(), rows.right());
            assert_eq!(rail.right(), content.right());
            assert_eq!(rail.top(), rows.top());
            assert_eq!(rail.bottom(), rows.bottom());
            assert_eq!(rail.bottom(), footer.top());
        });
    }

    cx.update(|window, cx| {
        let rows = window.find("rows-viewport").bounds();
        window.click_at(
            "rows-viewport",
            point(rows.size.width / 2.0 + px(GUTTER_WIDTH), px(11.0)),
            cx,
        );
        window.press("ctrl-end", cx);
        assert!(editor.read(cx).vertical_scroll > 0.0);

        window.press("ctrl-a", cx);
        window.press("backspace", cx);
        window.render_frame(cx);

        let view = editor.read(cx);
        let track = view.scroll_track();
        assert!(view.vertical_scroll.abs() < f32::EPSILON);
        assert!(track.max_scroll().abs() < f32::EPSILON);
        assert!(track.bands(&view.alignment, LINE_HEIGHT).is_empty());
    });
}

#[gpui_kit::test]
fn horizontal_scrollbar_reserves_space_only_for_overflow_and_tracks_range_changes(
    cx: &mut TestAppContext,
) {
    let source = format!("{}\n", "wide ".repeat(16));
    let (editor, cx) = harness_with_text(cx, String::new(), source);

    cx.simulate_resize(size(px(700.0), px(620.0)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.render_frame(cx);
        let horizontal = window.find("horizontal-scrollbar").bounds();
        let vertical = window.find("diff-scrollbar").bounds();
        let rows = window.find("rows-viewport").bounds();
        let footer = window.find("editor-footer").bounds();
        let content = window.find("aligned-editor").bounds();

        assert_eq!(horizontal.left(), content.left());
        assert_eq!(horizontal.right(), vertical.left());
        assert_eq!(horizontal.top(), rows.bottom());
        assert_eq!(horizontal.bottom(), footer.top());
        assert_eq!(horizontal.size.height, px(HEIGHT));
        assert!(editor.read(cx).horizontal_scrollbar_visibility.is_visible());
    });

    cx.simulate_resize(size(px(2_000.0), px(620.0)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("horizontal-scrollbar").is_none());
        assert!(!editor.read(cx).horizontal_scrollbar_visibility.is_visible());
        assert!(editor.read(cx).horizontal_scroll.abs() < f32::EPSILON);
        assert_eq!(
            window.find("rows-viewport").bounds().bottom(),
            window.find("editor-footer").bounds().top()
        );
    });

    cx.simulate_resize(size(px(700.0), px(620.0)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("horizontal-scrollbar").is_some());

        editor.update(cx, |editor, cx| {
            editor.horizontal_scroll = editor.max_horizontal_scroll(window, cx);
        });
        assert!(editor.read(cx).horizontal_scroll > 0.0);

        let rows = window.find("rows-viewport").bounds();
        window.click_at(
            "rows-viewport",
            point(rows.size.width / 2.0 + px(GUTTER_WIDTH + 10.0), px(11.0)),
            cx,
        );
        window.press("ctrl-a", cx);
        window.input("short\n", cx);
        window.render_frame(cx);

        assert!(window.try_find("horizontal-scrollbar").is_none());
        assert!(!editor.read(cx).horizontal_scrollbar_visibility.is_visible());
        assert!(editor.read(cx).horizontal_scroll.abs() < f32::EPSILON);
        assert_eq!(
            window.find("rows-viewport").bounds().bottom(),
            window.find("editor-footer").bounds().top()
        );
    });
}

#[gpui_kit::test]
fn horizontal_track_jump_and_thumb_drag_reach_both_ends_without_editing(cx: &mut TestAppContext) {
    let source = format!("{}\n", "0123456789".repeat(100));
    let (editor, cx) = harness_with_text(cx, source.clone(), source.clone());
    let (bar, selection, current) = cx.update(|window, cx| {
        let rows = window.find("rows-viewport").bounds();
        window.click_at(
            "rows-viewport",
            point(rows.size.width * 0.75 + px(GUTTER_WIDTH), px(11.0)),
            cx,
        );
        window.press("shift-right", cx);
        window.render_frame(cx);

        (
            window.find("horizontal-scrollbar").bounds(),
            editor.read(cx).right_selection(),
            editor
                .read(cx)
                .navigation
                .current(&editor.read(cx).alignment),
        )
    });

    cx.simulate_click(
        point(bar.left() + bar.size.width * 0.75, bar.center().y),
        Modifiers::default(),
    );
    cx.run_until_parked();

    let grab = cx.update(|window, cx| {
        window.render_frame(cx);
        let view = editor.read(cx);
        let max_scroll = view.max_horizontal_scroll(window, cx);
        let track = view.horizontal_scroll_track(max_scroll);
        assert!(view.horizontal_scroll > max_scroll * 0.5);
        assert_eq!(view.right_selection(), selection);

        let thumb = track.thumb(view.horizontal_scroll);
        point(
            bar.left() + px(thumb.start.midpoint(thumb.end)),
            bar.center().y,
        )
    });

    cx.simulate_mouse_down(grab, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        point(bar.left() - px(100.0), bar.center().y),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_up(
        point(bar.left() - px(100.0), bar.center().y),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(editor.read(cx).horizontal_scroll.abs() < f32::EPSILON);
    });

    let start_grab = cx.update(|window, cx| {
        let view = editor.read(cx);
        let max_scroll = view.max_horizontal_scroll(window, cx);
        let thumb = view.horizontal_scroll_track(max_scroll).thumb(0.0);

        point(
            bar.left() + px(thumb.start.midpoint(thumb.end)),
            bar.center().y,
        )
    });
    cx.simulate_mouse_down(start_grab, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        point(bar.right() + px(100.0), bar.center().y),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_up(
        point(bar.right() + px(100.0), bar.center().y),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();

    cx.update(|window, cx| {
        let view = editor.read(cx);
        let max_scroll = view.max_horizontal_scroll(window, cx);
        assert!((view.horizontal_scroll - max_scroll).abs() < f32::EPSILON);
        assert_eq!(view.right_selection(), selection);
        assert_eq!(view.right.document.text(), source);
        assert_eq!(view.navigation.current(&view.alignment), current);
        assert!(view.horizontal_scrollbar_grab.is_none());
        assert!(view.focus.is_focused(window));
    });
}

#[gpui_kit::test]
fn merge_horizontal_scroll_uses_one_offset_for_every_pane_hit(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            let source = |text: String| Document::from_bytes(text.into_bytes()).unwrap();
            let session = yori_diff::merge::MergeSession::new(
                source("base\n".to_owned()),
                source(format!("{}\n", "local".repeat(200))),
                source(format!("{}\n", "incoming".repeat(200))),
            )
            .unwrap();
            *editor = AlignedEditor::from_merge_session(session, window, cx);
        });
        window.render_frame(cx);

        let bar = window.find("horizontal-scrollbar").bounds();
        window.click_at(
            "horizontal-scrollbar",
            point(bar.size.width * 0.75, bar.size.height / 2.0),
            cx,
        );
        window.render_frame(cx);

        let view = editor.read(cx);
        let geometry = view.geometry();
        assert!(view.horizontal_scroll > 0.0);

        let y = f32::from(view.content_bounds.get().origin.y) + HEADER_HEIGHT + 1.0;
        let text_inset = GUTTER_WIDTH + 12.0;
        let hits = [
            geometry.hit(
                f32::from(view.content_bounds.get().origin.x) + text_inset,
                y,
                view.vertical_scroll,
                view.horizontal_scroll,
            ),
            geometry.hit(
                f32::from(view.content_bounds.get().origin.x)
                    + geometry.right_pane_left()
                    + text_inset,
                y,
                view.vertical_scroll,
                view.horizontal_scroll,
            ),
            geometry.hit(
                f32::from(view.content_bounds.get().origin.x)
                    + geometry.incoming_pane_left()
                    + text_inset,
                y,
                view.vertical_scroll,
                view.horizontal_scroll,
            ),
        ];

        assert!(hits[0].left_side && !hits[0].incoming_side);
        assert!(!hits[1].left_side && !hits[1].incoming_side);
        assert!(!hits[2].left_side && hits[2].incoming_side);
        assert!(
            hits.iter()
                .all(|hit| { (hit.text_x - (12.0 + view.horizontal_scroll)).abs() < f32::EPSILON })
        );
    });
}

#[gpui_kit::test]
fn merge_marks_use_projected_spans_not_header_inclusive_controls(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            let source = |text: &str| Document::from_bytes(text.as_bytes().to_vec()).unwrap();
            let session = yori_diff::merge::MergeSession::new(
                source("base\nseparator\nold\n"),
                source("local\nseparator\n"),
                source("incoming\nseparator\nnew\n"),
            )
            .unwrap();
            *editor = AlignedEditor::from_merge_session(session, window, cx);
            // A large viewport keeps the expected marker positions in row units.
            let track = ScrollTrack::new(0, LINE_HEIGHT, 1000.0);
            let merge = editor.merge.as_ref().unwrap();
            let marks = merge_scrollbar_marks(merge, track);

            assert_eq!(marks.len(), 2);
            assert_eq!(marks[0].range, LINE_HEIGHT..2.0 * LINE_HEIGHT);
            assert_eq!(marks[1].range, 4.0 * LINE_HEIGHT..5.0 * LINE_HEIGHT);
            assert!(marks[0].current);
            assert!(!marks[1].current);
            assert!(marks.iter().all(|mark| !mark.resolved));

            editor.toggle_merge_base(window, cx);
            editor.merge_mark(yori_diff::merge::ConflictId(0), true, window, cx);
            let merge = editor.merge.as_ref().unwrap();
            let marks = merge_scrollbar_marks(merge, track);

            assert_eq!(marks[0].range, 3.0 * LINE_HEIGHT..4.0 * LINE_HEIGHT);
            assert_eq!(marks[1].range, 6.0 * LINE_HEIGHT..7.0 * LINE_HEIGHT);
            assert!(marks[0].current && marks[0].resolved);
            assert!(!marks[1].current && !marks[1].resolved);
        });
    });
}
