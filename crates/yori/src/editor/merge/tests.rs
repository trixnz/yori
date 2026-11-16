//! Behavioral coverage for the in-memory merge surface, using native input routing.

mod display;
mod line_taking;

use std::fmt::Write as _;

use super::*;
use crate::editor::{CopySelected, EntityInputHandler, Motion};
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Entity, TestAppContext, VisualTestContext, point, px};
use yori::vim::Mode;

fn harness(cx: &mut TestAppContext) -> (Entity<AlignedEditor>, &mut VisualTestContext) {
    harness_with(cx, None)
}

fn harness_with(
    cx: &mut TestAppContext,
    session: Option<MergeSession>,
) -> (Entity<AlignedEditor>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::appearance::init(cx);
        crate::editor::init(cx);
    });

    let mut editor = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            if let Some(session) = session {
                AlignedEditor::from_merge_session(session, window, cx)
            } else {
                AlignedEditor::merge_fixture(window, cx)
            }
        });
        editor = Some(view.clone());

        Root::new(view, window, cx)
    });
    cx.update(TestWindowExt::render_frame);
    cx.run_until_parked();

    (editor.unwrap(), cx)
}

fn conflict_menu(window: &mut Window, cx: &mut gpui_kit::App, id: ConflictId, item: usize) {
    window.click(("merge-conflict-options", id.0), cx);
    window.render_frame(cx);
    for _ in 0..=item {
        window.press("down", cx);
    }
    window.press("enter", cx);
}

fn assert_synchronized(editor: &AlignedEditor) {
    let merge = editor.merge.as_ref().unwrap();
    assert_eq!(editor.right.document.text(), merge.session.result().text());
    assert_eq!(
        editor.right.max_display_columns,
        crate::editor::max_display_columns(&editor.right.document, crate::editor::TAB_WIDTH)
    );
    assert_eq!(
        editor.right.line_endings,
        yori::document_info::LineEndings::from_document(&editor.right.document)
    );
    if let Some(highlighter) = &editor.right.highlighter {
        assert_eq!(highlighter.text().to_string(), editor.right.document.text());
    }

    assert_eq!(editor.left.document.text(), merge.session.local().text());
    assert_eq!(
        merge.incoming.document.text(),
        merge.session.incoming().text()
    );

    for side in [Side::Left, Side::Right, Side::Incoming] {
        let document = &editor.document(side).document;
        let text: String = (0..merge.display.rows().len())
            .filter_map(|row| editor.line_for_row(side, row))
            .map(|line| document.copy_range(document.lines()[line].full.clone()))
            .collect();
        assert_eq!(text, document.text());
    }
}

#[gpui_kit::test]
fn choices_and_resolution_status_undo_together_without_touching_inputs(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            let merge = editor.merge.as_ref().unwrap();
            assert_eq!(merge.session.unresolved().count(), 3);
            assert!(merge.session.result().text().contains("45"));
            assert!(merge.session.result().text().contains("5\n"));
            assert!(!editor.is_dirty());
            let original = merge.session.result().text().to_owned();
            let id = ConflictId(0);

            editor.merge_take(id, Take::Incoming, window, cx);
            assert!(
                editor
                    .merge
                    .as_ref()
                    .unwrap()
                    .session
                    .state(id)
                    .unwrap()
                    .resolved
            );
            assert!(editor.right.document.text().contains("Welcome back"));
            assert!(editor.is_dirty());
            assert_synchronized(editor);

            editor.travel_history(false, window, cx);
            assert_eq!(editor.right.document.text(), original);
            assert!(
                !editor
                    .merge
                    .as_ref()
                    .unwrap()
                    .session
                    .state(id)
                    .unwrap()
                    .resolved
            );
            assert!(!editor.is_dirty());

            editor.travel_history(true, window, cx);
            assert!(editor.right.document.text().contains("Welcome back"));
            editor.merge_mark(id, false, window, cx);
            let accepted = editor.right.document.text().to_owned();
            editor.travel_history(false, window, cx);
            assert_eq!(editor.right.document.text(), accepted);
            assert!(
                editor
                    .merge
                    .as_ref()
                    .unwrap()
                    .session
                    .state(id)
                    .unwrap()
                    .resolved
            );
            assert_synchronized(editor);
        });
    });
}

#[gpui_kit::test]
fn native_typing_and_composition_keep_resolution_explicit(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            let original = editor.right.document.text().to_owned();
            editor.replace_and_mark_text_in_range(None, "e", Some(1..1), window, cx);
            editor.replace_and_mark_text_in_range(None, "é", Some(1..1), window, cx);
            editor.replace_text_in_range(None, "é", window, cx);
            editor.unmark_text(window, cx);

            assert_eq!(
                editor.merge.as_ref().unwrap().session.unresolved().count(),
                3
            );
            assert!(editor.marked_range().is_none());
            assert_synchronized(editor);

            editor.travel_history(false, window, cx);
            assert_eq!(editor.right.document.text(), original);
            editor.travel_history(true, window, cx);
            assert!(editor.right.document.text().contains('é'));

            editor.merge_mark(ConflictId(0), true, window, cx);
            editor.replace_text_in_range(None, "manual", window, cx);
            assert!(
                editor
                    .merge
                    .as_ref()
                    .unwrap()
                    .session
                    .state(ConflictId(0))
                    .unwrap()
                    .resolved
            );
            assert_synchronized(editor);
        });
    });
}

#[gpui_kit::test]
fn incoming_and_local_allow_navigation_copy_but_reject_mutation(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            for side in [Side::Left, Side::Incoming] {
                let original = editor.document(side).document.text().to_owned();
                editor.selection = Some(Selection {
                    side,
                    anchor: 0,
                    head: original.len(),
                });
                editor.copy_selected(&CopySelected, window, cx);
                assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), original);

                editor.replace_text_in_range(None, "cannot write", window, cx);
                editor.move_cursor(Motion::Right, false, window, cx);
                assert_eq!(editor.document(side).document.text(), original);
                assert!(editor.selection_restore().is_none());
                assert_synchronized(editor);
            }
        });
    });
}

#[gpui_kit::test]
fn base_expansion_and_three_pane_hit_testing_preserve_source_identity(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.toggle_merge_base(window, cx);
            let display = &editor.merge.as_ref().unwrap().display;
            let base_rows: Vec<_> = display
                .rows()
                .iter()
                .enumerate()
                .filter(|(_, row)| matches!(row.kind, RowKind::Base(_)))
                .map(|(index, _)| index)
                .collect();
            assert_ne!(base_rows, Vec::<usize>::new());
            assert!(base_rows.iter().all(|&row| {
                [Side::Left, Side::Right, Side::Incoming]
                    .iter()
                    .all(|side| editor.line_for_row(*side, row).is_none())
            }));
            editor.copy_merge_base(ConflictId(0), cx);
            let merge = editor.merge.as_ref().unwrap();
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().unwrap(),
                merge
                    .session
                    .base()
                    .copy_range(merge.session.conflicts()[0].base.clone())
            );
            assert_synchronized(editor);

            let origin = editor.content_bounds.get().origin;
            for side in [Side::Left, Side::Right, Side::Incoming] {
                let row = editor.row_for_source(side, 0);
                let point = point(
                    origin.x + px(editor.pane_left(side) + super::super::GUTTER_WIDTH),
                    origin.y
                        + px(
                            crate::editor::HEADER_HEIGHT + display_units(row) * LINE_HEIGHT
                                - editor.vertical_scroll
                                + 1.0,
                        ),
                );
                assert_eq!(editor.source_offset_at(point, window, cx), (side, 0));
            }

            editor.toggle_merge_base(window, cx);
            assert_synchronized(editor);
        });
    });
}

#[gpui_kit::test]
fn clicking_each_pane_and_conflict_controls_keeps_input_on_the_right_document(
    cx: &mut TestAppContext,
) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        for side in [Side::Left, Side::Incoming, Side::Right] {
            let pane = editor.read(cx).pane_left(side);
            window.click_at(
                "rows-viewport",
                point(px(pane + super::super::GUTTER_WIDTH + 1.0), px(11.0)),
                cx,
            );
            assert_eq!(editor.read(cx).selection.as_ref().unwrap().side, side);
            let original = editor.read(cx).right.document.text().to_owned();
            window.input("x", cx);

            if side == Side::Right {
                assert_ne!(editor.read(cx).right.document.text(), original);
                window.press("ctrl-z", cx);
                assert_eq!(editor.read(cx).right.document.text(), original);
            } else {
                assert_eq!(editor.read(cx).right.document.text(), original);
            }
            assert_synchronized(editor.read(cx));
        }

        window.click(("merge-local-button", 0usize), cx);
        assert_eq!(
            editor
                .read(cx)
                .merge
                .as_ref()
                .unwrap()
                .session
                .unresolved()
                .count(),
            2
        );
        window.press("alt-down", cx);
        assert_eq!(
            editor.read(cx).merge.as_ref().unwrap().current,
            Some(ConflictId(1))
        );
        window.click(("merge-incoming-button", 1usize), cx);
        assert!(
            editor
                .read(cx)
                .right
                .document
                .text()
                .contains("legacy_mode")
        );
        assert_eq!(
            editor
                .read(cx)
                .merge
                .as_ref()
                .unwrap()
                .session
                .unresolved()
                .count(),
            1
        );

        conflict_menu(window, cx, ConflictId(1), 4);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        assert!(editor.read(cx).merge.as_ref().unwrap().show_base);
        let before = editor.read(cx).right.document.text().to_owned();
        conflict_menu(window, cx, ConflictId(1), 5);
        assert!(
            cx.read_from_clipboard()
                .unwrap()
                .text()
                .unwrap()
                .contains("legacy_mode")
        );
        assert_eq!(editor.read(cx).right.document.text(), before);
        assert_synchronized(editor.read(cx));
    });
}

#[gpui_kit::test]
fn hover_previews_do_not_change_selection_text_or_conflict_status(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        let original = editor.read(cx).right.document.text().to_owned();
        let selection = editor.read(cx).right_selection().unwrap();
        let current = editor.read(cx).merge.as_ref().unwrap().current;

        window.hover(("merge-incoming-button", 1usize), cx);
        assert_eq!(
            editor.read(cx).merge.as_ref().unwrap().hovered,
            Some((ConflictId(1), Take::Incoming))
        );
        assert_eq!(editor.read(cx).merge.as_ref().unwrap().current, current);
        assert_eq!(editor.read(cx).right_selection(), Some(selection));
        assert_eq!(editor.read(cx).right.document.text(), original);
        assert!(!editor.read(cx).is_dirty());

        window.hover("next-conflict", cx);
        assert_eq!(editor.read(cx).merge.as_ref().unwrap().hovered, None);
        window.press("ctrl-a", cx);
        window.press("ctrl-c", cx);
        assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), original);
    });
}

#[gpui_kit::test]
fn gutter_action_remains_usable_when_a_tall_conflict_header_scrolls_away(cx: &mut TestAppContext) {
    let source = |prefix: &str| {
        let mut text = String::new();
        for index in 0..80 {
            writeln!(text, "{prefix} {index}").unwrap();
        }

        Document::from_bytes(text.into_bytes()).unwrap()
    };
    let session = MergeSession::new(source("base"), source("local"), source("incoming")).unwrap();
    let original = session.result().text().to_owned();
    let expected = session.incoming().text().to_owned();
    let (editor, cx) = harness_with(cx, Some(session));

    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.vertical_scroll = 20.0 * LINE_HEIGHT;
            cx.notify();
        });
        window.render_frame(cx);
        window.click(("merge-incoming-button", 0usize), cx);

        assert_eq!(editor.read(cx).right.document.text(), expected);
        assert!(
            editor
                .read(cx)
                .merge
                .as_ref()
                .unwrap()
                .session
                .state(ConflictId(0))
                .unwrap()
                .resolved
        );
        window.press("ctrl-z", cx);
        assert_eq!(editor.read(cx).right.document.text(), original);
        assert_synchronized(editor.read(cx));
    });
}

#[gpui_kit::test]
fn local_controls_target_their_conflict_instead_of_the_active_one(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        let original = editor.read(cx).right.document.text().to_owned();
        assert_eq!(
            editor.read(cx).merge.as_ref().unwrap().current,
            Some(ConflictId(0))
        );

        window.click(("merge-incoming-button", 1usize), cx);
        let merge = editor.read(cx).merge.as_ref().unwrap();
        assert!(merge.session.state(ConflictId(1)).unwrap().resolved);
        assert!(!merge.session.state(ConflictId(0)).unwrap().resolved);
        assert_eq!(merge.current, Some(ConflictId(1)));
        assert!(merge.session.result().text().contains("legacy_mode"));

        window.press("ctrl-z", cx);
        assert_eq!(editor.read(cx).right.document.text(), original);
        assert_eq!(
            editor
                .read(cx)
                .merge
                .as_ref()
                .unwrap()
                .session
                .unresolved()
                .count(),
            3
        );

        // The menu belongs to conflict 3 even while conflict 2 is active.
        let merge = editor.read(cx).merge.as_ref().unwrap();
        let conflict = &merge.session.conflicts()[2];
        let both = format!(
            "{}{}",
            merge
                .session
                .incoming()
                .copy_range(conflict.incoming.clone()),
            merge.session.local().copy_range(conflict.local.clone())
        );
        conflict_menu(window, cx, ConflictId(2), 1);

        let merge = editor.read(cx).merge.as_ref().unwrap();
        let state = merge.session.state(ConflictId(2)).unwrap();
        assert!(state.resolved);
        assert_eq!(
            merge.session.result().copy_range(state.result.clone()),
            both
        );
        assert!(!merge.session.state(ConflictId(1)).unwrap().resolved);
        assert_synchronized(editor.read(cx));

        window.press("ctrl-z", cx);
        assert_eq!(editor.read(cx).right.document.text(), original);
        conflict_menu(window, cx, ConflictId(1), 2);
        assert_eq!(editor.read(cx).right.document.text(), original);
        assert!(
            editor
                .read(cx)
                .merge
                .as_ref()
                .unwrap()
                .session
                .state(ConflictId(1))
                .unwrap()
                .resolved
        );

        window.press("ctrl-z", cx);
        assert_eq!(
            editor
                .read(cx)
                .merge
                .as_ref()
                .unwrap()
                .session
                .unresolved()
                .count(),
            3
        );
    });
}

#[gpui_kit::test]
fn header_navigation_does_not_shift_when_conflict_status_changes(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        let previous = window.find("previous-conflict").bounds();
        let next = window.find("next-conflict").bounds();
        let original = editor.read(cx).right.document.text().to_owned();

        window.click(("merge-local-button", 0usize), cx);
        window.click("next-conflict", cx);
        assert_eq!(
            editor.read(cx).merge.as_ref().unwrap().current,
            Some(ConflictId(1))
        );
        assert_eq!(editor.read(cx).right.document.text(), original);
        assert_eq!(window.find("previous-conflict").bounds(), previous);
        assert_eq!(window.find("next-conflict").bounds(), next);

        window.press("ctrl-z", cx);
        assert_eq!(
            editor
                .read(cx)
                .merge
                .as_ref()
                .unwrap()
                .session
                .unresolved()
                .count(),
            3
        );
        assert_eq!(window.find("previous-conflict").bounds(), previous);
        assert_eq!(window.find("next-conflict").bounds(), next);
    });
}

#[gpui_kit::test]
fn reset_menu_is_conflict_local_and_undoable(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    let accepted = cx.update(|window, cx| {
        conflict_menu(window, cx, ConflictId(0), 1);
        window.click(("merge-incoming-button", 1usize), cx);
        editor.read(cx).right.document.text().to_owned()
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        // Reset conflict 1 after making a later, independent decision in conflict 2.
        conflict_menu(window, cx, ConflictId(0), 3);
        let merge = editor.read(cx).merge.as_ref().unwrap();
        let first = merge.session.state(ConflictId(0)).unwrap();
        assert!(!first.resolved);
        assert_eq!(
            merge.session.result().copy_range(first.result.clone()),
            merge
                .session
                .local()
                .copy_range(merge.session.conflicts()[0].local.clone())
        );
        assert!(merge.session.state(ConflictId(1)).unwrap().resolved);
        assert!(merge.session.result().text().contains("legacy_mode"));
        assert_eq!(merge.current, Some(ConflictId(0)));
        let reset = merge.session.result().text().to_owned();

        window.press("ctrl-z", cx);
        assert_eq!(editor.read(cx).right.document.text(), accepted);
        assert!(
            editor
                .read(cx)
                .merge
                .as_ref()
                .unwrap()
                .session
                .state(ConflictId(0))
                .unwrap()
                .resolved
        );
        window.press("ctrl-shift-z", cx);
        assert_eq!(editor.read(cx).right.document.text(), reset);
        assert_synchronized(editor.read(cx));
    });
}

#[gpui_kit::test]
fn vim_changes_and_status_only_undo_use_the_same_merge_history(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| editor.toggle_vim(true, window, cx));
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let original = editor.read(cx).right.document.text().to_owned();
        for key in ["i", "x", "y", "escape"] {
            window.press(key, cx);
        }
        assert_eq!(editor.read(cx).vim.mode(), Mode::Normal);
        assert_ne!(editor.read(cx).right.document.text(), original);
        assert_synchronized(editor.read(cx));

        window.press("u", cx);
        assert_eq!(editor.read(cx).right.document.text(), original);
        window.press("ctrl-r", cx);
        let edited = editor.read(cx).right.document.text().to_owned();
        assert_ne!(edited, original);

        editor.update(cx, |editor, cx| {
            editor.merge_mark(ConflictId(0), true, window, cx);
        });
        window.press("u", cx);
        assert_eq!(editor.read(cx).right.document.text(), edited);
        assert!(
            !editor
                .read(cx)
                .merge
                .as_ref()
                .unwrap()
                .session
                .state(ConflictId(0))
                .unwrap()
                .resolved
        );
        window.press("u", cx);
        assert_eq!(editor.read(cx).right.document.text(), original);
        assert_synchronized(editor.read(cx));
    });
}

#[gpui_kit::test]
fn unmark_during_insert_preserves_merge_grouping(cx: &mut TestAppContext) {
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
        assert_eq!(editor.read(cx).right.document.text(), composed);
        window.press("u", cx);
        assert_eq!(editor.read(cx).right.document.text(), original);

        window.press("ctrl-r", cx);
        assert_eq!(editor.read(cx).right.document.text(), composed);
        window.press("ctrl-r", cx);
        assert_eq!(editor.read(cx).right.document.text(), typed);
    });
}

#[gpui_kit::test]
fn retiring_a_net_zero_composition_restores_conflict_gutter_positions(cx: &mut TestAppContext) {
    for retirement in ["unmark", "escape", "reposition", "cancel"] {
        let source = |text: &str| Document::from_bytes(text.as_bytes().to_vec()).unwrap();
        let session = MergeSession::new(
            source("base one\nseparator\nbase two\n"),
            source("local one\nseparator\nlocal two\n"),
            source("incoming one\nseparator\nincoming two\n"),
        )
        .unwrap();
        let (editor, view) = harness_with(cx, Some(session));
        view.update(|window, cx| {
            editor.update(cx, |editor, cx| editor.toggle_vim(true, window, cx));
        });
        view.run_until_parked();
        view.update(|window, cx| {
            window.press("i", cx);
            editor.update(cx, |editor, cx| {
                let original = editor.right.document.text().to_owned();
                let conflicts = editor.merge.as_ref().unwrap().display.conflicts().to_vec();
                editor.replace_and_mark_text_in_range(
                    Some(0..original.len()),
                    "",
                    None,
                    window,
                    cx,
                );
                editor.replace_and_mark_text_in_range(None, &original, None, window, cx);

                match retirement {
                    "unmark" => editor.unmark_text(window, cx),
                    "escape" => editor.vim_key(
                        &gpui_kit::KeyDownEvent {
                            keystroke: gpui_kit::Keystroke::parse("escape").unwrap(),
                            is_held: false,
                            prefer_character_input: false,
                        },
                        window,
                        cx,
                    ),
                    "reposition" => editor.reposition_vim(),
                    "cancel" => editor.cancel_vim(),
                    _ => unreachable!(),
                }

                let merge = editor.merge.as_ref().unwrap();
                assert_eq!(editor.right.document.text(), original);
                assert_eq!(merge.session.state(ConflictId(1)).unwrap().result, 20..30);
                assert_eq!(
                    merge.display.conflicts(),
                    conflicts,
                    "{retirement}: conflict controls and gutter ranges must return to owner truth"
                );
                assert!(!editor.is_dirty());
                assert_synchronized(editor);
            });
        });
    }
}

#[gpui_kit::test]
fn deleting_a_conflict_keeps_its_explicit_identity_with_expanded_base(cx: &mut TestAppContext) {
    let source = |text: &str| Document::from_bytes(text.as_bytes().to_vec()).unwrap();
    let session = MergeSession::new(
        source("base one\nseparator\nbase two\n"),
        source("local one\nseparator\nlocal two\n"),
        source("separator\nincoming two\n"),
    )
    .unwrap();
    let (editor, cx) = harness_with(cx, Some(session));
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.navigate_merge(false, window, cx);
            editor.toggle_merge_base(window, cx);
            assert_eq!(editor.merge.as_ref().unwrap().current, Some(ConflictId(1)));

            editor.merge_take(ConflictId(0), Take::Incoming, window, cx);

            let merge = editor.merge.as_ref().unwrap();
            assert_eq!(merge.current, Some(ConflictId(0)));
            assert_eq!(merge.session.state(ConflictId(0)).unwrap().result, 0..0);
            assert!(merge.session.state(ConflictId(0)).unwrap().resolved);
            assert!(!merge.session.state(ConflictId(1)).unwrap().resolved);
            let caption = merge.display.conflicts()[0].base_caption.unwrap();
            assert_eq!(
                merge.display.rows()[caption].kind,
                RowKind::Base(BaseRow::Caption)
            );
            assert_eq!(
                merge.display.rows()[caption + 1].kind,
                RowKind::Base(BaseRow::SourceLine(0))
            );
            assert_eq!(editor.right.document.text(), "separator\nlocal two\n");
            let caret = editor.right_selection().unwrap();
            assert_eq!(caret, TextSelection::caret(0));
            let y = display_units(editor.row_for_source(Side::Right, caret.head)) * LINE_HEIGHT;
            assert!(y >= editor.vertical_scroll);
            assert!(
                y + LINE_HEIGHT
                    <= editor.vertical_scroll + editor.geometry().rows_viewport_height()
            );
            assert!(editor.focus.is_focused(window));
            assert_synchronized(editor);
        });
    });
}

#[gpui_kit::test]
fn status_only_history_retains_input_placement_and_does_not_reveal_result(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            let original = editor.current_checkpoint();
            editor.merge_mark(ConflictId(0), true, window, cx);
            editor.selection = Some(Selection {
                side: Side::Incoming,
                anchor: 9,
                head: 2,
            });
            editor.preferred_column = Some(17);
            editor.vertical_scroll = 22.0;
            editor.horizontal_scroll = 7.0;

            editor.travel_history(false, window, cx);

            let selected = editor.selection.as_ref().unwrap();
            assert_eq!(
                (selected.side, selected.anchor, selected.head),
                (Side::Incoming, 9, 2)
            );
            assert_eq!(editor.preferred_column, Some(17));
            assert_eq!(
                (editor.vertical_scroll, editor.horizontal_scroll),
                (22.0, 7.0)
            );
            assert!(editor.focus.is_focused(window));
            assert!(editor.current_checkpoint() == original);
            assert_synchronized(editor);

            editor.travel_history(true, window, cx);

            assert_eq!(editor.selection.as_ref().unwrap().side, Side::Incoming);
            assert_eq!(editor.preferred_column, Some(17));
            assert_eq!(
                (editor.vertical_scroll, editor.horizontal_scroll),
                (22.0, 7.0)
            );
            assert_eq!(editor.right.document.text(), original.text);
            assert_eq!(editor.unresolved_count(), 2);
        });
    });
}

#[gpui_kit::test]
fn utf16_composition_completes_source_syntax_metadata_and_checkpoint_together(
    cx: &mut TestAppContext,
) {
    let source = |text: &str| Document::from_bytes(text.as_bytes().to_vec()).unwrap();
    let session = MergeSession::new(
        source("base\r\n"),
        source("a😀z\r\n"),
        source("incoming\r\n"),
    )
    .unwrap();
    let (editor, cx) = harness_with(cx, Some(session));
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.replace_and_mark_text_in_range(Some(1..3), "界😀", Some(1..3), window, cx);

            assert_eq!(editor.right.document.text(), "a界😀z\r\n");
            assert_eq!(
                editor.selected_text_range(false, window, cx).unwrap().range,
                2..4
            );
            assert_eq!(editor.marked_text_range(window, cx), Some(1..4));
            assert_eq!(editor.current_checkpoint().text, "a界😀z\r\n");
            assert_synchronized(editor);

            editor.replace_text_in_range(None, "é", window, cx);
            editor.unmark_text(window, cx);

            assert_eq!(editor.right.document.text(), "aéz\r\n");
            assert_eq!(editor.current_checkpoint().text, "aéz\r\n");
            assert_synchronized(editor);

            editor.travel_history(false, window, cx);
            assert_eq!(editor.right.document.text(), "a😀z\r\n");
            assert!(!editor.is_dirty());
            assert_synchronized(editor);
        });
    });
}
