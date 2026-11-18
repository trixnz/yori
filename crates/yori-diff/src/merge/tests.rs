use super::*;
use yori_document::editing::SourceEdit;

fn doc(text: &str) -> Document {
    Document::from_bytes(text.as_bytes().to_vec()).unwrap()
}

fn session(base: &str, local: &str, incoming: &str) -> MergeSession {
    MergeSession::new(doc(base), doc(local), doc(incoming)).unwrap()
}

fn conflicted() -> MergeSession {
    session(
        "head\nbase\ntail\n",
        "head\nlocal\ntail\n",
        "head\nincoming\ntail\n",
    )
}

fn caret() -> TextSelection {
    TextSelection::caret(0)
}

#[test]
fn cr_conflict_takes_do_not_add_blank_lines() {
    for (choice, expected) in [
        (Take::Incoming, "head\rincoming\rtail\r"),
        (Take::LocalThenIncoming, "head\rlocal\rincoming\rtail\r"),
        (Take::IncomingThenLocal, "head\rincoming\rlocal\rtail\r"),
    ] {
        let mut merge = session(
            "head\rbase\rtail\r",
            "head\rlocal\rtail\r",
            "head\rincoming\rtail\r",
        );
        let id = merge.conflicts()[0].id;

        merge.take(id, choice, caret()).unwrap();

        assert_eq!(merge.result().text(), expected);
    }
}

#[test]
fn independent_and_identical_changes_merge_without_conflicts() {
    let merge = session(
        "one\ntwo\nthree\n",
        "ONE\ntwo\nthree\n",
        "one\ntwo\nTHREE\n",
    );
    assert_eq!(merge.result().text(), "ONE\ntwo\nTHREE\n");
    assert_eq!(merge.unresolved().count(), 0);

    let merge = session("old\n", "same change\n", "same change\n");
    assert_eq!(merge.result().text(), "same change\n");
    assert_eq!(merge.conflicts(), []);
}

#[test]
fn conflicts_start_with_local_text_but_remain_explicitly_unresolved() {
    let merge = conflicted();
    let conflict = &merge.conflicts()[0];

    assert_eq!(merge.result().text(), merge.local().text());
    assert_eq!(merge.base().copy_range(conflict.base.clone()), "base\n");
    assert_eq!(
        merge.incoming().copy_range(conflict.incoming.clone()),
        "incoming\n"
    );
    assert_eq!(merge.unresolved().collect::<Vec<_>>(), vec![conflict.id]);
    assert!(!merge.result().text().contains("<<<<<<<"));
}

#[test]
fn each_take_order_is_one_undo_step_including_resolution_status() {
    for (choice, expected) in [
        (Take::Local, "head\nlocal\ntail\n"),
        (Take::Incoming, "head\nincoming\ntail\n"),
        (Take::LocalThenIncoming, "head\nlocal\nincoming\ntail\n"),
        (Take::IncomingThenLocal, "head\nincoming\nlocal\ntail\n"),
    ] {
        let mut merge = conflicted();
        let id = merge.conflicts()[0].id;
        let original = merge.result().text().to_owned();
        let update = merge.take(id, choice, caret()).unwrap();

        assert_eq!(merge.result().text(), expected);
        assert!(merge.state(id).unwrap().resolved);
        assert_eq!(merge.local().text(), "head\nlocal\ntail\n");
        assert_eq!(merge.incoming().text(), "head\nincoming\ntail\n");

        merge.undo(update.selection).unwrap().unwrap();
        assert_eq!(merge.result().text(), original);
        assert!(!merge.state(id).unwrap().resolved);
        assert!(merge.undo(caret()).unwrap().is_none());

        merge.redo(caret()).unwrap().unwrap();
        assert_eq!(merge.result().text(), expected);
        assert!(merge.state(id).unwrap().resolved);
    }
}

#[test]
fn reset_restores_starting_text_and_status_in_one_undo_step() {
    for choice in [
        Take::Local,
        Take::Incoming,
        Take::LocalThenIncoming,
        Take::IncomingThenLocal,
    ] {
        let mut merge = conflicted();
        let id = ConflictId(0);
        let initial = merge.result().text().to_owned();
        merge.take(id, choice, caret()).unwrap();
        let accepted = merge.result().text().to_owned();

        merge.reset(id, caret()).unwrap();
        assert_eq!(merge.result().text(), initial);
        assert!(!merge.state(id).unwrap().resolved);

        merge.undo(caret()).unwrap().unwrap();
        assert_eq!(merge.result().text(), accepted);
        assert!(merge.state(id).unwrap().resolved);

        merge.redo(caret()).unwrap().unwrap();
        assert_eq!(merge.result().text(), initial);
        assert!(!merge.state(id).unwrap().resolved);
    }

    let mut merge = conflicted();
    merge.reset(ConflictId(0), caret()).unwrap();
    assert!(
        merge.undo(caret()).unwrap().is_none(),
        "an untouched reset is a no-op"
    );
}

#[test]
fn manual_edits_keep_status_and_status_only_actions_share_history() {
    let mut merge = conflicted();
    let id = merge.conflicts()[0].id;
    let range = merge.state(id).unwrap().result.clone();
    merge.replace(caret(), range, "manual\n").unwrap();
    assert!(!merge.state(id).unwrap().resolved);

    merge.set_resolved(id, true, caret()).unwrap();
    let end = merge.state(id).unwrap().result.end - 1;
    merge.replace(caret(), end..end, "!").unwrap();
    assert!(merge.state(id).unwrap().resolved);
    assert_eq!(merge.result().text(), "head\nmanual!\ntail\n");

    merge.undo(caret()).unwrap();
    assert!(merge.state(id).unwrap().resolved);
    assert_eq!(merge.result().text(), "head\nmanual\ntail\n");
    let update = merge.undo(caret()).unwrap().unwrap();
    assert!(update.edit.is_none());
    assert!(!merge.state(id).unwrap().resolved);
    merge.undo(caret()).unwrap();
    assert_eq!(merge.result().text(), "head\nlocal\ntail\n");
}

#[test]
fn deleted_conflicts_keep_ordered_anchors_for_later_resolution_and_undo() {
    let mut merge = session(
        "base one\nseparator\nbase two\n",
        "local one\nseparator\nlocal two\n",
        "incoming one\nseparator\nincoming two\n",
    );
    assert_eq!(merge.conflicts().len(), 2);
    let first = merge.conflicts()[0].id;
    let second = merge.conflicts()[1].id;
    let original_states = merge.states.clone();
    let original = merge.result().text().to_owned();

    merge.replace(caret(), 0..original.len(), "").unwrap();
    assert_eq!(merge.state(first).unwrap().result, 0..0);
    assert_eq!(merge.state(second).unwrap().result, 0..0);
    assert_eq!(merge.unresolved().count(), 2);

    merge.take(second, Take::Incoming, caret()).unwrap();
    merge.take(first, Take::Local, caret()).unwrap();
    assert_eq!(merge.result().text(), "local one\nincoming two\n");
    assert_eq!(merge.unresolved().count(), 0);

    for _ in 0..3 {
        merge.undo(caret()).unwrap().unwrap();
    }
    assert_eq!(merge.result().text(), original);
    assert_eq!(merge.states, original_states);
}

#[test]
fn typing_transaction_restores_conflict_ranges_in_one_undo_step() {
    let mut merge = conflicted();
    let original = merge.result().text().to_owned();
    let states = merge.states.clone();
    merge.begin_transaction(caret());
    let a = merge.replace(caret(), 0..0, "α").unwrap();
    let b = merge.replace(a.selection, 2..2, "β").unwrap();
    merge.finish_transaction(b.selection);

    assert_eq!(merge.result().text(), format!("αβ{original}"));
    merge.undo(b.selection).unwrap().unwrap();
    assert_eq!(merge.result().text(), original);
    assert_eq!(merge.states, states);
    assert!(merge.undo(caret()).unwrap().is_none());
}

#[test]
fn invalid_edits_do_not_change_result_metadata_or_history() {
    let mut merge = conflicted();
    let original = merge.result().text().to_owned();
    let states = merge.states.clone();
    assert!(merge.replace(caret(), 0..1, "\0").is_err());
    assert!(merge.take(ConflictId(99), Take::Incoming, caret()).is_err());
    assert!(merge.set_resolved(ConflictId(99), true, caret()).is_err());

    assert_eq!(merge.result().text(), original);
    assert_eq!(merge.states, states);
    assert!(merge.undo(caret()).unwrap().is_none());
}

#[test]
fn empty_inputs_deletion_conflicts_and_final_newlines_remain_source_faithful() {
    let merge = session("", "", "new\r\n");
    assert_eq!(merge.result().text(), "new\r\n");
    assert_eq!(merge.conflicts(), []);

    let mut merge = session("old\r\n", "", "changed\r\n");
    let id = merge.conflicts()[0].id;
    assert_eq!(merge.result().text(), "");
    assert_eq!(merge.state(id).unwrap().result, 0..0);
    merge.take(id, Take::Incoming, caret()).unwrap();
    assert_eq!(merge.result().text(), "changed\r\n");
    merge.undo(caret()).unwrap();
    assert_eq!(merge.result().text(), "");

    let mut merge = session("old", "本地", "incoming");
    let id = merge.conflicts()[0].id;
    merge.take(id, Take::LocalThenIncoming, caret()).unwrap();
    assert_eq!(merge.result().text(), "本地\nincoming");
    merge.undo(caret()).unwrap();
    assert_eq!(merge.result().text(), "本地");
}

#[test]
fn result_ranges_remain_ordered_after_cross_conflict_replacements() {
    let mut merge = session(
        "base one\nseparator\nbase two\nend\n",
        "local one\nseparator\nlocal two\nend\n",
        "incoming one\nseparator\nincoming two\nend\n",
    );
    let original = merge.result().text().to_owned();
    let states = merge.states.clone();
    let mut seed = 19_u64;
    let mut steps = 0;

    for _ in 0..80 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let boundaries: Vec<_> = merge
            .result()
            .text()
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(merge.result().text().len()))
            .collect();
        let modulus = u64::try_from(boundaries.len()).unwrap();
        let a = boundaries[usize::try_from(seed % modulus).unwrap()];
        let b = boundaries[usize::try_from((seed >> 16) % modulus).unwrap()];
        let replacement = if seed & 1 == 0 { "λ\n" } else { "" };
        let old = merge.result().text().to_owned();
        let metadata = merge.states.clone();

        merge
            .replace(caret(), a.min(b)..a.max(b), replacement)
            .unwrap();
        if merge.result().text() != old || merge.states != metadata {
            steps += 1;
        }

        let mut end = 0;
        for state in &merge.states {
            assert!(state.result.start >= end);
            assert!(state.result.start <= state.result.end);
            assert!(merge.result().text().get(state.result.clone()).is_some());
            assert!(!state.resolved);
            end = state.result.end;
        }
    }

    for _ in 0..steps {
        merge.undo(caret()).unwrap().unwrap();
    }
    assert_eq!(merge.result().text(), original);
    assert_eq!(merge.states, states);
    assert!(merge.undo(caret()).unwrap().is_none());
}

#[test]
fn independent_eof_changes_do_not_join_separate_source_lines() {
    let merge = session("a\n", "A", "a\nb\n");
    assert_ne!(merge.result().text(), "Ab\n");
}

#[test]
fn a_new_status_decision_after_undo_discards_the_old_redo_branch() {
    let mut merge = conflicted();
    let id = merge.conflicts()[0].id;
    merge.take(id, Take::Incoming, caret()).unwrap();
    merge.undo(caret()).unwrap();
    merge.set_resolved(id, true, caret()).unwrap();

    assert!(merge.redo(caret()).unwrap().is_none());
    merge.undo(caret()).unwrap();
    merge.redo(caret()).unwrap();
    assert_eq!(merge.result().text(), merge.local().text());
    assert!(merge.state(id).unwrap().resolved);
}

#[test]
fn source_edit_maintains_conflicts_and_history_without_a_caller_receipt() {
    let mut merge = conflicted();
    let id = ConflictId(0);
    let original = merge.state(id).unwrap().clone();

    merge.editing(caret(), false).replace(0..0, "α").unwrap();

    assert_eq!(
        merge.state(id).unwrap().result,
        original.result.start + 2..original.result.end + 2
    );
    merge.undo(caret()).unwrap().unwrap();
    assert_eq!(merge.result().text(), merge.local().text());
    assert_eq!(merge.state(id), Some(&original));
}

#[test]
fn source_group_returning_to_original_text_restores_conflict_ranges() {
    let mut merge = session(
        "base one\nseparator\nbase two\n",
        "local one\nseparator\nlocal two\n",
        "incoming one\nseparator\nincoming two\n",
    );
    let original = merge.result().text().to_owned();
    let states = merge.states.clone();

    let mut source = merge.editing(caret(), true);
    source.replace(0..original.len(), "").unwrap();
    let update = source.replace(0..0, &original).unwrap();
    assert!(merge.finish_transaction(update.selection));
    assert!(!merge.finish_transaction(update.selection));

    assert_eq!(merge.result().text(), original);
    assert_eq!(merge.states, states);
    assert!(merge.undo(caret()).unwrap().is_none());
}
