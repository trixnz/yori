//! Find routed through real GPUI key dispatch, native text input and focus.

use super::*;
use crate::editor::PaneDocument;
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{App, Focusable, TestAppContext, VisualTestContext};
use yori::vim::Mode;
use yori_document::Document;

const LOCAL: &str = "one needle\ntwo NEEDLE\nthree needle\n";
const BASELINE: &str = "baseline needle\n";

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

fn find_shortcut() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-f"
    } else {
        "ctrl-f"
    }
}

fn label(editor: &Entity<AlignedEditor>, cx: &App) -> Option<String> {
    editor
        .read(cx)
        .open_find_bar()
        .and_then(|find| find.matches.label())
}

fn selection(editor: &Entity<AlignedEditor>, cx: &App) -> (Side, usize, usize) {
    let selection = editor.read(cx).selection.clone().unwrap();

    (selection.side, selection.anchor, selection.head)
}

fn find_input_focused(editor: &Entity<AlignedEditor>, window: &Window, cx: &App) -> bool {
    editor
        .read(cx)
        .find
        .as_ref()
        .is_some_and(|find| find.input.focus_handle(cx).is_focused(window))
}

fn highlighted_lines(editor: &Entity<AlignedEditor>, side: Side, cx: &App) -> Vec<usize> {
    let editor = editor.read(cx);
    let document = &editor.document(side).document;

    (0..document.lines().len())
        .filter(|&line| {
            let source = document.lines()[line].content.clone();
            let display = DisplayLine::from_source(document.content(line), source.start, 4);

            !editor
                .find_line_matches(side, &source, &display)
                .found
                .is_empty()
        })
        .collect()
}

/// Each step is its own update so input events reach their subscribers before assertions.
fn press(cx: &mut VisualTestContext, key: &str) {
    cx.update(|window, cx| window.press(key, cx));
}

fn type_text(cx: &mut VisualTestContext, text: &str) {
    cx.update(|window, cx| window.input(text, cx));
}

#[gpui_kit::test]
fn typing_searches_the_local_pane_and_navigation_wraps_both_ways(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);

    press(cx, find_shortcut());
    type_text(cx, "NeEdLe");

    cx.update(|window, cx| {
        assert!(find_input_focused(&editor, window, cx));
        assert!(!editor.read(cx).focus.is_focused(window));
        assert_eq!(editor.read(cx).right.document.text(), LOCAL);
        assert_eq!(label(&editor, cx).as_deref(), Some("1 of 3"));
        assert_eq!(selection(&editor, cx), (Side::Right, 4, 10));
        assert_eq!(highlighted_lines(&editor, Side::Right, cx), vec![0, 1, 2]);
        assert!(highlighted_lines(&editor, Side::Left, cx).is_empty());
    });

    press(cx, "enter");
    cx.update(|_, cx| {
        assert_eq!(label(&editor, cx).as_deref(), Some("2 of 3"));
        assert_eq!(selection(&editor, cx), (Side::Right, 15, 21));
    });

    press(cx, "shift-enter");
    press(cx, "shift-enter");
    cx.update(|_, cx| {
        assert_eq!(label(&editor, cx).as_deref(), Some("3 of 3"));
        assert_eq!(selection(&editor, cx), (Side::Right, 28, 34));
    });

    press(cx, "f3");
    cx.update(|_, cx| assert_eq!(label(&editor, cx).as_deref(), Some("1 of 3")));

    press(cx, "shift-f3");
    cx.update(|_, cx| assert_eq!(label(&editor, cx).as_deref(), Some("3 of 3")));
}

#[gpui_kit::test]
fn escape_closes_find_and_returns_typing_to_the_editor_at_the_match(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);

    press(cx, find_shortcut());
    type_text(cx, "needle");
    press(cx, "enter");
    cx.update(|_, cx| assert_eq!(selection(&editor, cx), (Side::Right, 15, 21)));

    press(cx, "escape");
    cx.update(|window, cx| {
        assert!(window.try_find("find-bar").is_none());
        assert!(editor.read(cx).focus.is_focused(window));
        assert_eq!(label(&editor, cx), None);
        assert!(highlighted_lines(&editor, Side::Right, cx).is_empty());
        assert_eq!(selection(&editor, cx), (Side::Right, 15, 21));
    });

    type_text(cx, "thread");
    cx.update(|_, cx| {
        assert_eq!(
            editor.read(cx).right.document.text(),
            "one needle\ntwo thread\nthree needle\n"
        );
    });
}

#[gpui_kit::test]
fn edits_while_find_is_open_update_matches_and_f3_works_from_the_editor(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);

    press(cx, find_shortcut());
    type_text(cx, "needle");
    cx.update(|_, cx| assert_eq!(label(&editor, cx).as_deref(), Some("1 of 3")));

    cx.update(|window, cx| editor.update(cx, |editor, cx| editor.focus.focus(window, cx)));
    press(cx, "right");
    type_text(cx, " needle");
    cx.update(|window, cx| {
        window.render_frame(cx);

        assert!(window.try_find("find-bar").is_some());
        assert_eq!(label(&editor, cx).as_deref(), Some("1 of 4"));
    });

    press(cx, "f3");
    cx.update(|_, cx| {
        assert_eq!(label(&editor, cx).as_deref(), Some("2 of 4"));
        assert_eq!(selection(&editor, cx), (Side::Right, 11, 17));
    });
}

#[gpui_kit::test]
fn a_single_line_selection_prefills_the_query_for_its_own_pane(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);

    cx.update(|_, cx| {
        editor.update(cx, |editor, _| {
            editor.selection = Some(Selection {
                side: Side::Left,
                anchor: 9,
                head: 15,
            });
        });
    });
    press(cx, find_shortcut());
    cx.update(|_, cx| {
        assert_eq!(label(&editor, cx).as_deref(), Some("1 of 1"));
        assert_eq!(selection(&editor, cx), (Side::Left, 9, 15));
        assert_eq!(highlighted_lines(&editor, Side::Left, cx), vec![0]);
        assert!(highlighted_lines(&editor, Side::Right, cx).is_empty());
    });

    press(cx, "escape");

    // A selection across lines cannot be a single-line query, so the last one stays.
    cx.update(|_, cx| {
        editor.update(cx, |editor, _| {
            editor.selection = Some(Selection {
                side: Side::Right,
                anchor: 0,
                head: 15,
            });
        });
    });
    press(cx, find_shortcut());
    cx.update(|_, cx| {
        assert_eq!(label(&editor, cx).as_deref(), Some("1 of 3"));
        assert_eq!(selection(&editor, cx), (Side::Right, 4, 10));
    });
}

#[gpui_kit::test]
fn vim_never_sees_find_typing_and_resumes_in_normal_mode_at_the_match(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| editor.toggle_vim(true, window, cx));
    });
    cx.run_until_parked();

    press(cx, "i");
    cx.update(|_, cx| assert_eq!(editor.read(cx).vim.mode(), Mode::Insert));

    press(cx, find_shortcut());
    type_text(cx, "needle");
    press(cx, "enter");
    cx.update(|_, cx| {
        assert_eq!(editor.read(cx).right.document.text(), LOCAL);
        assert_eq!(label(&editor, cx).as_deref(), Some("2 of 3"));
        assert_eq!(selection(&editor, cx), (Side::Right, 15, 15));
    });

    press(cx, "escape");
    cx.update(|window, cx| {
        assert!(editor.read(cx).focus.is_focused(window));
        assert_eq!(editor.read(cx).vim.mode(), Mode::Normal);
    });

    press(cx, "x");
    cx.update(|_, cx| {
        assert_eq!(
            editor.read(cx).right.document.text(),
            "one needle\ntwo EEDLE\nthree needle\n"
        );
    });
}

#[gpui_kit::test]
fn the_find_bar_floats_inside_the_top_right_of_the_rows(cx: &mut TestAppContext) {
    let (_, cx) = harness(cx);

    press(cx, find_shortcut());
    cx.update(|window, _| {
        let bar = window.find("find-bar").bounds();
        let rows = window.find("rows-viewport").bounds();

        assert!(bar.top() >= rows.top());
        assert!(bar.right() <= rows.right());
        assert!(bar.left() > rows.left() + rows.size.width / 2.0);
    });
}
