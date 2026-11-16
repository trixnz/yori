//! Exercise selection, restore controls, and undo through headless GPUI input.

use super::*;
use crate::editor::{GUTTER_WIDTH, PaneDocument};
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Entity, TestAppContext, VisualTestContext, point};

const BASELINE: &str = "head\nold one\nold two\nold three\ntail\n";
const LOCAL: &str = "head\nnew one\nnew two\nnew three\ntail\n";
const RESTORED: &str = "head\nnew one\nold two\nnew three\ntail\n";

fn harness(cx: &mut TestAppContext) -> (Entity<AlignedEditor>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::appearance::init(cx);
        crate::editor::init(cx);
    });
    let mut editor = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let pane = |text: &str| {
            PaneDocument::new(
                "fixture.txt".into(),
                Document::from_bytes(text.as_bytes().to_vec()).unwrap(),
            )
        };
        let view = cx.new(|cx| AlignedEditor::new(pane(BASELINE), pane(LOCAL), window, cx));
        editor = Some(view.clone());
        Root::new(view, window, cx)
    });
    cx.update(TestWindowExt::render_frame);
    cx.run_until_parked();

    (editor.unwrap(), cx)
}

fn select_middle_line(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App, left_side: bool) {
    let width = window.find("rows-viewport").bounds().size.width;
    let pane = if left_side { px(0.0) } else { width / 2.0 };
    window.click_at(
        "rows-viewport",
        point(pane + px(GUTTER_WIDTH + 2.0), px(LINE_HEIGHT * 2.0 + 11.0)),
        cx,
    );
    window.press("home", cx);
    window.press("shift-end", cx);
}

#[gpui_kit::test]
fn either_pane_selection_restores_only_the_previewed_lines_and_undoes(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    let undo = if cfg!(target_os = "macos") {
        "cmd-z"
    } else {
        "ctrl-z"
    };

    for left_side in [true, false] {
        cx.update(|window, cx| {
            select_middle_line(window, cx, left_side);
            let plan = editor.read(cx).selection_restore().unwrap();
            assert_eq!(plan.rows, 2..3);
            assert_eq!(
                editor.read(cx).left.document.copy_range(plan.baseline),
                "old two\n"
            );
            assert_eq!(
                editor.read(cx).right.document.copy_range(plan.local),
                "new two\n"
            );

            // Both the toolbar and gutter apply the preview, not the whole block.
            let control = if left_side {
                "restore-selected-lines"
            } else {
                "restore-selected-gutter"
            };
            window.click(control, cx);
            assert_eq!(editor.read(cx).right.document.text(), RESTORED);
            assert_eq!(editor.read(cx).left.document.text(), BASELINE);
            assert!(editor.read(cx).is_dirty());
            assert_eq!(editor.read(cx).alignment.blocks().len(), 2);

            window.press(undo, cx);
            assert_eq!(editor.read(cx).right.document.text(), LOCAL);
            assert!(!editor.read(cx).is_dirty());
        });
        cx.run_until_parked();
    }
}

#[gpui_kit::test]
fn clearing_a_selection_returns_to_whole_block_restoration(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        select_middle_line(window, cx, false);
        window.press("right", cx);
        window.click(("restore-block", 0usize), cx);

        assert_eq!(editor.read(cx).right.document.text(), BASELINE);
        assert_eq!(editor.read(cx).alignment.blocks(), []);

        let undo = if cfg!(target_os = "macos") {
            "cmd-z"
        } else {
            "ctrl-z"
        };
        window.press(undo, cx);
        assert_eq!(editor.read(cx).right.document.text(), LOCAL);
    });
}

#[gpui_kit::test]
fn restore_shortcut_uses_current_selection_and_rejects_an_outdated_preview(
    cx: &mut TestAppContext,
) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        select_middle_line(window, cx, false);
        let old_preview = editor.read(cx).selection_restore().unwrap();
        window.press("right", cx);

        editor.update(cx, |editor, cx| {
            editor.apply_selection_restore(&old_preview, window, cx);
        });
        window.press("alt-enter", cx);
        assert_eq!(
            editor.read(cx).right.document.text(),
            LOCAL,
            "a caret must not fall back to restoring the block"
        );

        select_middle_line(window, cx, false);
        window.press("alt-enter", cx);
        assert_eq!(editor.read(cx).right.document.text(), RESTORED);
    });
}
