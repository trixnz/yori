//! Both real source-editing implementations obey the same rejection contract.

use yori_diff::merge::{ConflictId, MergeSession};
use yori_document::{
    Document,
    editing::{DocumentEdit, EditHistory, SourceEdit, TextSelection},
};

fn doc(text: &str) -> Document {
    Document::from_bytes(text.as_bytes().to_vec()).unwrap()
}

fn reject_input(source: &mut dyn SourceEdit) {
    let original = source.document().text().to_owned();
    let marked = source.marked_range();

    for (range, text) in [(1..2, "x"), (3..5, ""), (0..0, "\0"), (99..100, "x")] {
        assert!(source.replace(range.clone(), text).is_err());
        assert!(source.replace_marked(range, text, None).is_err());
        assert_eq!(source.document().text(), original);
        assert_eq!(source.marked_range(), marked);
    }

    assert!(source.replace_marked(0..2, "é", Some(1..1)).is_err());
    assert!(source.replace_marked(0..2, "é", Some(0..3)).is_err());
    assert_eq!(source.document().text(), original);
    assert_eq!(source.marked_range(), marked);
}

#[test]
fn document_source_rejection_preserves_composition_and_redo() {
    let mut document = doc("α\r\n");
    let mut history = EditHistory::default();
    let before = TextSelection { anchor: 2, head: 0 };
    DocumentEdit::new(&mut document, &mut history, before, false)
        .replace(0..0, "x")
        .unwrap();
    history.undo(&mut document, before).unwrap().unwrap();

    let composed = DocumentEdit::new(&mut document, &mut history, before, false)
        .replace_marked(0..2, "α", None)
        .unwrap();
    reject_input(&mut DocumentEdit::new(
        &mut document,
        &mut history,
        composed.selection,
        true,
    ));

    history
        .redo(&mut document, composed.selection)
        .unwrap()
        .unwrap();
    assert_eq!(document.text(), "xα\r\n");
    history.undo(&mut document, before).unwrap().unwrap();

    let composed = DocumentEdit::new(&mut document, &mut history, before, false)
        .replace_marked(0..2, "é", None)
        .unwrap();
    reject_input(&mut DocumentEdit::new(
        &mut document,
        &mut history,
        composed.selection,
        true,
    ));
    let committed = DocumentEdit::new(&mut document, &mut history, composed.selection, false)
        .replace(0..2, "界")
        .unwrap();

    assert_eq!(document.text(), "界\r\n");
    assert_eq!(history.marked_range(), None);
    let undone = history
        .undo(&mut document, committed.selection)
        .unwrap()
        .unwrap();
    assert_eq!(undone.selection, before);
    assert_eq!(document.text(), "α\r\n");
    assert!(history.undo(&mut document, before).unwrap().is_none());
    history.redo(&mut document, before).unwrap().unwrap();
    assert_eq!(document.text(), "界\r\n");
}

#[test]
fn merge_source_rejection_preserves_composition_redo_and_conflicts() {
    let mut merge = MergeSession::new(doc("base\r\n"), doc("α\r\n"), doc("incoming\r\n")).unwrap();
    let id = ConflictId(0);
    let state = merge.state(id).unwrap().clone();
    let before = TextSelection { anchor: 2, head: 0 };
    merge.editing(before, false).replace(0..0, "x").unwrap();
    merge.undo(before).unwrap().unwrap();

    let composed = merge
        .editing(before, false)
        .replace_marked(0..2, "α", None)
        .unwrap();
    reject_input(&mut merge.editing(composed.selection, true));
    assert_eq!(merge.state(id), Some(&state));

    merge.redo(composed.selection).unwrap().unwrap();
    assert_eq!(merge.result().text(), "xα\r\n");
    merge.undo(before).unwrap().unwrap();

    let composed = merge
        .editing(before, false)
        .replace_marked(0..2, "é", None)
        .unwrap();
    reject_input(&mut merge.editing(composed.selection, true));
    let committed = merge
        .editing(composed.selection, false)
        .replace(0..2, "界")
        .unwrap();

    assert_eq!(merge.result().text(), "界\r\n");
    assert_eq!(merge.marked_range(), None);
    assert_eq!(merge.state(id).unwrap().result, 0..5);
    assert!(!merge.state(id).unwrap().resolved);
    let undone = merge.undo(committed.selection).unwrap().unwrap();
    assert_eq!(undone.selection, before);
    assert_eq!(merge.result().text(), "α\r\n");
    assert_eq!(merge.state(id), Some(&state));
    assert!(merge.undo(before).unwrap().is_none());
    merge.redo(before).unwrap().unwrap();
    assert_eq!(merge.result().text(), "界\r\n");
}
