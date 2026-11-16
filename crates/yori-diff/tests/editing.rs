use yori_diff::{Alignment, restore_block};
use yori_document::{Document, editing::*};

fn doc(text: &str) -> Document {
    Document::from_bytes(text.as_bytes().to_vec()).unwrap()
}

#[test]
fn edit_realign_undo_redo_keeps_source_and_baseline_intact() {
    let baseline = doc("a\nb\n");
    let mut local = doc("a\ninserted\nb\n");
    let mut history = EditHistory::default();
    let original = local.text().to_owned();
    let before = TextSelection {
        anchor: 2,
        head: 11,
    };

    let edit = history
        .replace(&mut local, before, before.range(), "")
        .unwrap();

    assert_eq!(local.text(), baseline.text());
    assert_eq!(Alignment::between(&baseline, &local).rows().len(), 2);

    let undo = history.undo(&mut local, edit.selection).unwrap().unwrap();
    assert_eq!(local.text(), original);
    assert_eq!(undo.selection, before);

    let redo = history.redo(&mut local, undo.selection).unwrap().unwrap();
    assert_eq!(local.text(), "a\nb\n");
    assert_eq!(redo.selection, TextSelection::caret(2));
    assert_eq!(baseline.text(), "a\nb\n");
}

#[test]
fn restoring_a_block_preserves_other_changes_and_is_one_undo_step() {
    let baseline = doc("keep\nold one\nold two\nseparator\noriginal\nend\n");
    let mut local = doc("keep\nnew\nseparator\nother change\nend\n");
    let original = local.text().to_owned();
    let alignment = Alignment::between(&baseline, &local);

    assert_eq!(alignment.blocks().len(), 2);
    let block = &alignment.blocks()[0];
    assert_eq!(block.rows, 1..3);
    assert_eq!(
        baseline.copy_range(block.left.clone()),
        "old one\nold two\n"
    );
    assert_eq!(local.copy_range(block.right.clone()), "new\n");

    let before = TextSelection { anchor: 7, head: 5 };
    let mut history = EditHistory::default();
    let edit = restore_block(&mut history, &baseline, &mut local, before, block).unwrap();

    assert_eq!(
        local.text(),
        "keep\nold one\nold two\nseparator\nother change\nend\n"
    );
    assert_eq!(Alignment::between(&baseline, &local).blocks().len(), 1);

    let undone = history.undo(&mut local, edit.selection).unwrap().unwrap();
    assert_eq!(local.text(), original);
    assert_eq!(undone.selection, before);
    assert!(history.undo(&mut local, before).unwrap().is_none());

    history.redo(&mut local, before).unwrap().unwrap();
    assert_eq!(
        local.text(),
        "keep\nold one\nold two\nseparator\nother change\nend\n"
    );
}

#[test]
fn block_restoration_round_trips_small_source_pairs_exactly() {
    // Exercise leading/middle/trailing gaps, all-added/all-deleted files,
    // blank lines, CRLF, Unicode, tabs, and missing final newlines.
    let mut sources = vec![String::new()];
    for first in ["a\n", "b\r\n", "\n", "\t界\n"] {
        for last in ["", "a\n", "b\r\n", "\n", "界"] {
            sources.push(format!("{first}{last}"));
        }
    }
    sources.extend(["a".into(), "界".into()]);

    for left in &sources {
        for right in &sources {
            let baseline = doc(left);
            let mut local = doc(right);
            let mut history = EditHistory::default();
            let mut selection = TextSelection::caret(0);
            let mut count = 0;

            loop {
                let alignment = Alignment::between(&baseline, &local);
                let Some(block) = alignment.blocks().first() else {
                    break;
                };

                let original = local.text().to_owned();
                let edit =
                    restore_block(&mut history, &baseline, &mut local, selection, block).unwrap();
                let restored = local.text().to_owned();

                assert_eq!(
                    &restored[..block.right.start],
                    &original[..block.right.start]
                );
                assert_eq!(
                    &restored[block.right.start + edit.inserted_len..],
                    &original[block.right.end..]
                );

                let undone = history.undo(&mut local, edit.selection).unwrap().unwrap();
                assert_eq!(local.text(), original);

                selection = history
                    .redo(&mut local, undone.selection)
                    .unwrap()
                    .unwrap()
                    .selection;
                assert_eq!(local.text(), restored);

                count += 1;
                assert!(
                    count <= 4,
                    "restoration must converge: {left:?} vs {right:?}"
                );
            }

            assert_eq!(local.text(), left);
            assert_eq!(baseline.text(), left);

            for _ in 0..count {
                selection = history
                    .undo(&mut local, selection)
                    .unwrap()
                    .unwrap()
                    .selection;
            }

            assert_eq!(local.text(), right);
            assert!(history.undo(&mut local, selection).unwrap().is_none());
        }
    }
}

#[test]
fn restoring_after_typing_uses_current_ranges_and_keeps_typing_in_history() {
    let baseline = doc("one\ntwo\n");
    let mut local = doc("one\nchanged\n");
    let mut history = EditHistory::default();

    let typed = history
        .replace(&mut local, TextSelection::caret(0), 0..0, "note\n")
        .unwrap();
    let alignment = Alignment::between(&baseline, &local);
    assert_eq!(alignment.blocks().len(), 2);

    let restored = restore_block(
        &mut history,
        &baseline,
        &mut local,
        typed.selection,
        &alignment.blocks()[1],
    )
    .unwrap();

    assert_eq!(local.text(), "note\none\ntwo\n");

    let undone = history
        .undo(&mut local, restored.selection)
        .unwrap()
        .unwrap();
    assert_eq!(local.text(), "note\none\nchanged\n");

    history.undo(&mut local, undone.selection).unwrap().unwrap();
    assert_eq!(local.text(), "one\nchanged\n");
}

#[test]
fn deleting_everything_then_typing_and_undoing_supports_empty_files() {
    let mut local = doc("hello\r\n");
    let mut history = EditHistory::default();
    let original = TextSelection {
        anchor: 0,
        head: local.text().len(),
    };

    let empty = history
        .replace(&mut local, original, original.range(), "")
        .unwrap();
    assert_eq!(local.lines(), []);

    let added = history
        .replace(&mut local, empty.selection, 0..0, "界")
        .unwrap();
    assert_eq!(added.selection.head, 3);

    history.undo(&mut local, added.selection).unwrap();
    history.undo(&mut local, empty.selection).unwrap();
    assert_eq!(local.text(), "hello\r\n");
}

#[test]
fn composing_text_is_one_undoable_edit_with_relative_selection() {
    let mut local = doc("a😀z");
    let mut history = EditHistory::default();
    let before = TextSelection { anchor: 1, head: 5 };

    let first = history
        .replace_marked(&mut local, before, 1..5, "ni", Some(1..2))
        .unwrap();
    assert_eq!(first.selection, TextSelection { anchor: 2, head: 3 });
    assert_eq!(local.text(), "aniz");

    let marked = history.marked_range().unwrap();
    let final_edit = history
        .replace(&mut local, first.selection, marked, "你")
        .unwrap();
    assert_eq!(local.text(), "a你z");
    assert_eq!(history.marked_range(), None);

    let undone = history
        .undo(&mut local, final_edit.selection)
        .unwrap()
        .unwrap();
    assert_eq!(local.text(), "a😀z");
    assert_eq!(undone.selection, before);
    assert!(history.undo(&mut local, before).unwrap().is_none());
}

#[test]
fn grapheme_deletion_does_not_split_crlf_or_combining_text() {
    for cluster in ["\r\n", "e\u{301}", "👩‍💻"] {
        let text = format!("a{cluster}z");
        let after = 1 + cluster.len();

        assert_eq!(previous_grapheme(&text, after), 1);
        assert_eq!(next_grapheme(&text, 1), after);
    }
}

#[test]
fn utf16_ranges_round_trip_non_bmp_text() {
    let text = "a😀界";
    for (byte, _) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        assert_eq!(from_utf16(text, to_utf16(text, byte)), byte);
    }

    assert_eq!(from_utf16(text, 2), 1);
    assert_eq!(from_utf16(text, 999), text.len());
}

#[test]
fn bad_edits_leave_document_and_history_untouched() {
    let mut local = doc("α\r\n");
    let mut history = EditHistory::default();

    assert!(
        history
            .replace(&mut local, TextSelection::caret(0), 1..2, "x")
            .is_err()
    );
    assert!(
        history
            .replace(&mut local, TextSelection::caret(0), 0..0, "\0")
            .is_err()
    );
    assert!(
        history
            .replace(&mut local, TextSelection::caret(0), 3..4, "")
            .is_err()
    );

    assert_eq!(local.text(), "α\r\n");
    assert!(
        history
            .undo(&mut local, TextSelection::caret(0))
            .unwrap()
            .is_none()
    );
}

#[test]
fn navigation_preserves_column_and_handles_final_empty_line() {
    let document = doc("abcd\r\nx\r\nabcd\r\n");
    let mut column = None;

    let first = navigate(
        &document,
        TextSelection::caret(3),
        Motion::Down,
        false,
        &mut column,
    );
    assert_eq!(first.head, 7);

    let second = navigate(&document, first, Motion::Down, true, &mut column);
    assert_eq!(
        second,
        TextSelection {
            anchor: 7,
            head: 12
        }
    );

    let eof = navigate(&document, second, Motion::Down, false, &mut column);
    assert_eq!(eof.head, document.text().len());
    assert_eq!(document.newline(), "\r\n");
}

#[test]
fn source_anchors_and_caret_survive_new_alignment_rows() {
    let baseline = doc("a\nb\n");
    let mut local = doc("a\nb\n");
    let mut history = EditHistory::default();

    let edit = history
        .replace(&mut local, TextSelection::caret(0), 0..0, "new\n")
        .unwrap();

    assert_eq!(edit.map_anchor(2), 6);
    let alignment = Alignment::between(&baseline, &local);
    assert_eq!(alignment.row_for_offset(&local, 6, false), 2);
    assert_eq!(alignment.row_for_offset(&baseline, 2, true), 2);
    assert_eq!(
        alignment.row_for_offset(&local, local.text().len(), false),
        3
    );
}
