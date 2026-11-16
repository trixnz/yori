//! Behavioral routing tests: real GPUI key dispatch, native typing, and history.

use super::*;
use crate::editor::{EntityInputHandler, PaneDocument};
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Entity, TestAppContext, VisualTestContext};
use yori_document::Document;

fn harness(cx: &mut TestAppContext) -> (Entity<AlignedEditor>, &mut VisualTestContext) {
    harness_with(cx, "let old = 1;\n")
}

fn harness_with<'a>(
    cx: &'a mut TestAppContext,
    text: &str,
) -> (Entity<AlignedEditor>, &'a mut VisualTestContext) {
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
        let view = cx.new(|cx| AlignedEditor::new(pane("baseline\n"), pane(text), window, cx));
        editor = Some(view.clone());

        Root::new(view, window, cx)
    });

    cx.update(TestWindowExt::render_frame);
    cx.run_until_parked();

    (editor.unwrap(), cx)
}

#[gpui_kit::test]
fn modal_commands_route_before_native_text_and_undo_the_whole_change(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| editor.toggle_vim(true, window, cx));
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        for key in ["w", "c", "i", "w"] {
            window.press(key, cx);
        }

        assert_eq!(editor.read(cx).right.document.text(), "let  = 1;\n");
        assert_eq!(editor.read(cx).vim.mode(), Mode::Insert);

        for key in ["n", "e", "w", "escape"] {
            window.press(key, cx);
        }

        assert_eq!(editor.read(cx).right.document.text(), "let new = 1;\n");
        assert_eq!(editor.read(cx).vim.mode(), Mode::Normal);

        window.press("u", cx);
        assert_eq!(editor.read(cx).right.document.text(), "let old = 1;\n");

        window.press("ctrl-r", cx);
        assert_eq!(editor.read(cx).right.document.text(), "let new = 1;\n");
    });
}

fn toggle_vim_from_options(window: &mut Window, cx: &mut App) {
    window.click("editor-options", cx);
    window.render_frame(cx);

    window.press("down", cx);
    window.press("down", cx);
    window.press("down", cx);
    window.press("enter", cx);
}

#[gpui_kit::test]
fn toggling_off_restores_conventional_typing_without_losing_undo(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(toggle_vim_from_options);
    cx.run_until_parked();
    cx.update(|window, cx| {
        for key in ["shift-a", "x", "y"] {
            window.press(key, cx);
        }

        assert_eq!(editor.read(cx).right.document.text(), "let old = 1;xy\n");

        toggle_vim_from_options(window, cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.press("z", cx);
        assert_eq!(editor.read(cx).right.document.text(), "let old = 1;xyz\n");

        window.press("ctrl-z", cx);
        assert_eq!(editor.read(cx).right.document.text(), "let old = 1;xy\n");

        window.press("ctrl-z", cx);
        assert_eq!(editor.read(cx).right.document.text(), "let old = 1;\n");
    });
}

#[gpui_kit::test]
fn baseline_is_navigable_and_copyable_but_cannot_enter_insert(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.toggle_vim(true, window, cx);
            editor.selection = Some(Selection {
                side: Side::Left,
                anchor: 0,
                head: 0,
            });
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        for key in ["l", "y", "y", "d", "d", "i", "x"] {
            window.press(key, cx);
        }

        assert_eq!(editor.read(cx).left.document.text(), "baseline\n");
        assert_eq!(editor.read(cx).right.document.text(), "let old = 1;\n");
        assert_eq!(editor.read(cx).vim.mode(), Mode::Normal);
        assert_eq!(editor.read(cx).selection.as_ref().unwrap().side, Side::Left);
    });
}

#[gpui_kit::test]
fn unmark_during_insert_preserves_document_grouping(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| editor.toggle_vim(true, window, cx));
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let original = editor.read(cx).right.document.text().to_owned();
        window.press("i", cx);
        editor.update(cx, |editor, cx| {
            editor.replace_and_mark_text_in_range(None, "é", None, window, cx);
            editor.unmark_text(window, cx);
        });
        let composed = editor.read(cx).right.document.text().to_owned();
        window.press("x", cx);
        window.press("escape", cx);
        let typed = editor.read(cx).right.document.text().to_owned();

        window.press("u", cx);
        assert_ne!(composed, original);
        assert_eq!(editor.read(cx).right.document.text(), original);

        window.press("ctrl-r", cx);
        assert_eq!(editor.read(cx).right.document.text(), typed);
    });
}

#[gpui_kit::test]
fn vim_paste_reveals_only_its_final_cursor(cx: &mut TestAppContext) {
    let text = "source line\n".repeat(600);
    let (editor, cx) = harness_with(cx, &text);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| editor.toggle_vim(true, window, cx));
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        for key in ["4", "0", "0", "y", "y", "j", "j"] {
            window.press(key, cx);
        }
        assert!(editor.read(cx).vertical_scroll.abs() < f32::EPSILON);

        window.press("p", cx);

        let editor = editor.read(cx);
        assert_eq!(editor.right.document.text(), "source line\n".repeat(1000));
        assert_eq!(
            editor.right_selection().unwrap().head,
            "source line\n".len() * 3
        );
        assert!(
            editor.vertical_scroll.abs() < f32::EPSILON,
            "the final cursor was already visible"
        );
        assert!(editor.is_dirty());
    });
}

#[gpui_kit::test]
fn right_history_retains_its_existing_normal_mode_with_a_restored_selection(
    cx: &mut TestAppContext,
) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| editor.toggle_vim(true, window, cx));
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        for key in ["v", "l", "d", "u"] {
            window.press(key, cx);
        }

        let editor = editor.read(cx);
        assert_eq!(editor.right.document.text(), "let old = 1;\n");
        assert_eq!(editor.right_selection().unwrap().range(), 0..2);
        assert_eq!(editor.vim.mode(), Mode::Normal);
    });
}

#[gpui_kit::test]
fn shift_y_and_visual_yank_write_the_system_clipboard(cx: &mut TestAppContext) {
    let (editor, cx) = harness_with(cx, "let old = 1;\nnext line\n");
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| editor.toggle_vim(true, window, cx));
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        for key in ["w", "shift-y"] {
            window.press(key, cx);
        }

        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "let old = 1;\n"
        );
        assert_eq!(editor.read(cx).right_selection().unwrap().head, 4);

        for key in ["v", "e", "y"] {
            window.press(key, cx);
        }

        assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), "old");
        assert_eq!(editor.read(cx).right_selection().unwrap().range(), 4..4);
        assert_eq!(editor.read(cx).vim.mode(), Mode::Normal);
    });
}

#[gpui_kit::test]
fn paste_reads_text_copied_by_other_apps(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| editor.toggle_vim(true, window, cx));
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("external\n".to_owned()));
        window.press("shift-p", cx);
        assert_eq!(
            editor.read(cx).right.document.text(),
            "external\nlet old = 1;\n"
        );

        cx.write_to_clipboard(ClipboardItem::new_string("X".to_owned()));
        window.press("p", cx);
        assert_eq!(
            editor.read(cx).right.document.text(),
            "eXxternal\nlet old = 1;\n"
        );
    });
}

#[gpui_kit::test]
fn baseline_yank_reaches_the_system_clipboard(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.toggle_vim(true, window, cx);
            editor.selection = Some(Selection {
                side: Side::Left,
                anchor: 0,
                head: 0,
            });
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        for key in ["y", "shift-w"] {
            window.press(key, cx);
        }

        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "baseline"
        );
        assert_eq!(editor.read(cx).left.document.text(), "baseline\n");
    });
}
