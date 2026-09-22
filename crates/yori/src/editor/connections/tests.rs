//! Connection geometry and opt-in restoration behavior through the real UI.

use super::*;
use crate::editor::{GUTTER_WIDTH, PaneDocument};
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{App, AppContext, Entity, TestAppContext, VisualTestContext};

fn document(text: &str) -> Document {
    Document::from_bytes(text.as_bytes().to_vec()).unwrap()
}

#[test]
fn connection_edges_follow_source_rows_and_collapse_missing_sides() {
    for (left, right, expected_left, expected_right) in [
        ("", "a\nb\n", 0..0, 0..2),
        ("a\nb\n", "", 0..2, 0..0),
        ("a\nb\nc\n", "x\n", 0..3, 0..1),
        ("same\na\n", "same\nx\ny\n", 1..2, 1..3),
    ] {
        let left = document(left);
        let right = document(right);
        let alignment = Alignment::between(&left, &right);
        let block = &alignment.blocks()[0];
        let connection = Connection::new(
            &alignment,
            &left,
            &right,
            block.rows.clone(),
            &block.left,
            &block.right,
        );

        assert_eq!(connection.left, expected_left);
        assert_eq!(connection.right, expected_right);
    }
}

#[test]
fn centre_channel_is_excluded_from_source_hit_coordinates() {
    for center in [0.0, WIDTH] {
        let geometry =
            EditorGeometry::new(25.0, 30.0, 1200.0, 700.0, 64.0, GUTTER_WIDTH, LINE_HEIGHT)
                .with_center_width(center);
        let right_text = 25.0 + geometry.right_pane_left() + GUTTER_WIDTH;
        let hit = geometry.hit(right_text + 10.0, 94.0, LINE_HEIGHT, 12.0);

        assert!(!hit.left_side && !hit.in_gutter);
        assert_eq!(hit.row, 1);
        assert!((hit.text_x - 22.0).abs() < f32::EPSILON);
        assert!((geometry.pane_width() * 2.0 + center - 1200.0).abs() < f32::EPSILON);
    }
}

fn harness(cx: &mut TestAppContext) -> (Entity<AlignedEditor>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::appearance::init(cx);
        crate::editor::init(cx);
    });

    let mut editor = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let left = PaneDocument::new(
            "baseline.txt".into(),
            document("head\nold one\nold two\nold three\ntail\n"),
        );
        let right = PaneDocument::new(
            "local.txt".into(),
            document("head\nnew one\nnew two\nnew three\ntail\n"),
        );
        let view = cx.new(|cx| AlignedEditor::new(left, right, window, cx));
        editor = Some(view.clone());

        Root::new(view, window, cx)
    });
    cx.update(TestWindowExt::render_frame);
    cx.run_until_parked();

    (editor.unwrap(), cx)
}

fn toggle_connections(window: &mut Window, cx: &mut App) {
    window.click("editor-options", cx);
    window.render_frame(cx);

    window.press("down", cx);
    window.press("down", cx);
    window.press("enter", cx);
    window.render_frame(cx);
}

#[gpui_kit::test]
fn opting_in_preserves_selection_and_narrows_the_same_undoable_restore(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        assert!(!editor.read(cx).show_connections);
        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at(
            "rows-viewport",
            point(width / 2.0 + px(GUTTER_WIDTH + 2.0), px(55.0)),
            cx,
        );
        window.press("home", cx);
        window.press("shift-end", cx);

        let before = editor.read(cx);
        let selection = before.right_selection();
        let plan = before.selection_restore().unwrap();
        let scroll = (before.vertical_scroll, before.horizontal_scroll);
        let current = before.navigation.current(&before.alignment);
        assert_eq!(plan.rows, 2..3);

        toggle_connections(window, cx);

        let after = editor.read(cx);
        assert!(after.show_connections);
        assert_eq!(after.right_selection(), selection);
        assert_eq!(after.selection_restore(), Some(plan));
        assert_eq!((after.vertical_scroll, after.horizontal_scroll), scroll);
        assert_eq!(after.navigation.current(&after.alignment), current);
        assert!(!after.is_dirty());
        let projection = after.wrap_projection(window, cx);
        let connections = after.visible_connections(after.geometry(), &projection);
        assert_eq!(connections.len(), 1);
        assert_eq!(connections[0].left, 2..3);
        assert_eq!(connections[0].right, 2..3);

        window.click("restore-selected-gutter", cx);
        assert_eq!(
            editor.read(cx).right.document.text(),
            "head\nnew one\nold two\nnew three\ntail\n"
        );
        window.press("ctrl-z", cx);
        assert_eq!(
            editor.read(cx).right.document.text(),
            "head\nnew one\nnew two\nnew three\ntail\n"
        );

        assert!(!editor.read(cx).is_dirty());
    });
    cx.run_until_parked();

    cx.update(toggle_connections);
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert!(!editor.read(cx).show_connections);
        assert!(!editor.read(cx).is_dirty());
    });
}

#[gpui_kit::test]
fn hover_stays_consistent_between_the_connection_and_its_restore_button(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    let (band, button, outside) = cx.update(|window, cx| {
        toggle_connections(window, cx);
        let rows = window.find("rows-viewport").bounds();
        let geometry = editor.read(cx).geometry();
        let band = rows.origin + point(px(geometry.pane_width() + WIDTH / 2.0), px(75.0));
        let button = window.find(("restore-block", 0usize)).bounds().center();
        let outside = rows.origin + point(px(10.0), px(75.0));

        (band, button, outside)
    });
    cx.run_until_parked();

    for (step, position) in [band, button, band].into_iter().enumerate() {
        cx.simulate_mouse_move(position, None, gpui_kit::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                editor.read(cx).hovered_connection,
                Some(1..4),
                "hover step {step}"
            );
            assert!(!editor.read(cx).is_dirty());
        });
    }

    cx.simulate_mouse_move(outside, None, gpui_kit::Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| assert!(editor.read(cx).hovered_connection.is_none()));
}

#[gpui_kit::test]
fn enabled_geometry_supports_right_pane_typing_and_whole_block_restore(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        toggle_connections(window, cx);
        let geometry = editor.read(cx).geometry();
        window.click_at(
            "rows-viewport",
            point(
                px(geometry.right_pane_left() + GUTTER_WIDTH + 2.0),
                px(55.0),
            ),
            cx,
        );
        window.press("home", cx);
        window.input("X", cx);
        assert!(editor.read(cx).right.document.text().contains("Xnew two"));
        assert!(!editor.read(cx).left.document.text().contains('X'));
        window.press("ctrl-z", cx);

        window.click(("restore-block", 0usize), cx);
        let view = editor.read(cx);
        assert_eq!(view.right.document.text(), view.left.document.text());
        assert!(view.alignment.blocks().is_empty());
        assert!(view.hovered_connection.is_none());

        window.press("ctrl-z", cx);
        assert_eq!(editor.read(cx).alignment.blocks().len(), 1);
    });
}
