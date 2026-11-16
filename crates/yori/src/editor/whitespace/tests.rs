//! Whitespace hints must not change text, selection geometry, or comparison behavior.

use super::*;
use crate::editor::{GUTTER_WIDTH, PaneDocument};
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Entity, TestAppContext, VisualTestContext};
use yori_document::Document;

#[test]
fn whitespace_inventory_preserves_tab_spans_unicode_and_source_mapping() {
    let source = "\t界 x \t";
    let display = DisplayLine::from_source(source, 30, 4);
    let original = display.clone();
    let marks = display.whitespace_marks(source);

    assert_eq!(marks.len(), 4);
    assert_eq!(marks[0].kind, WhitespaceKind::Tab);
    assert_eq!(marks[0].display, 0..4);
    assert!(!marks[0].trailing);
    assert_eq!(marks[1].kind, WhitespaceKind::Space);
    assert!(!marks[1].trailing);
    assert!(marks[2].trailing);
    assert_eq!(marks[3].kind, WhitespaceKind::Tab);
    assert!(marks[3].trailing);
    assert_eq!(display, original);

    for mark in marks {
        let start = display.source_offset(mark.display.start);
        let end = display.source_offset(mark.display.end);
        assert_eq!(
            end - start,
            1,
            "an expanded tab still owns exactly one byte"
        );
        assert!(matches!(&source[start - 30..end - 30], " " | "\t"));
    }
}

#[test]
fn blank_lines_and_whitespace_only_lines_do_not_invent_source_content() {
    let empty = DisplayLine::from_source("", 12, 4);
    assert_eq!(empty.whitespace_marks(""), []);
    assert_eq!(empty.source_offset(0), 12);

    let whitespace = DisplayLine::from_source(" \t ", 12, 4);
    let marks = whitespace.whitespace_marks(" \t ");
    assert_eq!(marks.len(), 3);
    assert!(marks.iter().all(|mark| mark.trailing));
    assert_eq!(whitespace.source_offset(whitespace.text.len()), 15);
}

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
        let view = cx.new(|cx| {
            AlignedEditor::new(
                pane("\told\r\n\n"),
                pane("\t界 x \t\r\n\r\nlast"),
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
fn options_menu_preserves_exact_copy_hit_testing_and_diff_then_allows_edit_and_undo(
    cx: &mut TestAppContext,
) {
    let (editor, cx) = harness(cx);
    let (original, rows, selection, hit) = cx.update(|window, cx| {
        let bounds = window.find("rows-viewport").bounds();
        let position = point(bounds.size.width / 2.0 + px(GUTTER_WIDTH + 18.0), px(11.0));
        window.click_at("rows-viewport", position, cx);
        window.press("ctrl-a", cx);
        window.press("ctrl-c", cx);

        let original = cx.read_from_clipboard().unwrap().text().unwrap();
        let view = editor.read(cx);
        let rows = view.alignment.rows().to_vec();
        let selection = view.right_selection().unwrap();
        let hit = view.source_offset_at(bounds.origin + position, window, cx);

        window.click("editor-options", cx);
        (original, rows, selection, hit)
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.press("down", cx);
        window.press("enter", cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        let bounds = window.find("rows-viewport").bounds();
        let position =
            bounds.origin + point(bounds.size.width / 2.0 + px(GUTTER_WIDTH + 18.0), px(11.0));
        let view = editor.read(cx);
        assert!(view.show_whitespace);
        assert_eq!(view.right.document.text(), original);
        assert_eq!(view.alignment.rows(), rows);
        assert_eq!(view.right_selection(), Some(selection));
        assert_eq!(view.source_offset_at(position, window, cx), hit);
        assert!(!view.is_dirty());

        window.press("ctrl-c", cx);
        assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), original);

        window.press("home", cx);
        window.input("Z", cx);
        assert!(editor.read(cx).is_dirty());

        window.press("ctrl-z", cx);
        assert_eq!(editor.read(cx).right.document.text(), original);
        assert_eq!(editor.read(cx).alignment.rows(), rows);
    });
}

#[gpui_kit::test]
fn hiding_markers_clamps_only_the_extra_horizontal_travel(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        let bounds = window.find("rows-viewport").bounds();
        window.click_at(
            "rows-viewport",
            point(bounds.size.width / 2.0 + px(GUTTER_WIDTH), px(11.0)),
            cx,
        );
        window.input(&"\t".repeat(200), cx);

        editor.update(cx, |view, cx| {
            let plain_limit = view.max_horizontal_scroll(window, cx);
            let original = view.right.document.text().to_owned();
            let selection = view.right_selection();

            view.set_whitespace(true, window, cx);
            let marked_limit = view.max_horizontal_scroll(window, cx);
            assert!(marked_limit > plain_limit);
            view.horizontal_scroll = marked_limit;

            view.set_whitespace(false, window, cx);
            assert!((view.horizontal_scroll - plain_limit).abs() < f32::EPSILON);
            assert_eq!(view.right.document.text(), original);
            assert_eq!(view.right_selection(), selection);
        });
    });
}
