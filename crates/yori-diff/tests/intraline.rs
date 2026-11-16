use yori_diff::{Alignment, DiffKind, IntralineDiff};
use yori_document::Document;

fn doc(text: &str) -> Document {
    Document::from_bytes(text.as_bytes().to_vec()).unwrap()
}

fn assert_changes(left: &str, right: &str, old: &[&str], new: &[&str]) {
    // A shared prefix makes these nonzero document offsets, not line-local indices.
    let left = doc(&format!("unchanged\r\n{left}\r\n"));
    let right = doc(&format!("unchanged\r\n{right}\r\n"));
    let alignment = Alignment::between(&left, &right);
    let changes = alignment.intraline(&left, &right, 1);

    assert_eq!(
        changes
            .left
            .iter()
            .map(|r| left.copy_range(r.clone()))
            .collect::<Vec<_>>(),
        old
    );
    assert_eq!(
        changes
            .right
            .iter()
            .map(|r| right.copy_range(r.clone()))
            .collect::<Vec<_>>(),
        new
    );

    for (document, spans) in [(&left, &changes.left), (&right, &changes.right)] {
        let content = &document.lines()[1].content;
        assert!(
            spans
                .iter()
                .all(|r| content.start <= r.start && r.start < r.end && r.end <= content.end)
        );
        assert!(spans.windows(2).all(|r| r[0].end < r[1].start));
    }
}

#[test]
fn keeps_identifiers_and_numbers_whole_instead_of_highlighting_character_fragments() {
    assert_changes(
        "let customer_count = 1000;",
        "let customer_total = 1000;",
        &["customer_count"],
        &["customer_total"],
    );

    assert_changes(
        "fooBar42(value)",
        "fooBar43(value)",
        &["fooBar42"],
        &["fooBar43"],
    );

    assert_changes("return 1000;", "return 1001;", &["1000"], &["1001"]);
}

#[test]
fn distinguishes_punctuation_and_argument_insertions_from_unchanged_words() {
    assert_changes("if count > limit {", "if count < limit {", &[">"], &["<"]);
    assert_changes("if count >= limit {", "if count > limit {", &["="], &[]);

    assert_changes(
        "call(first, second);",
        "call(first, second, extra);",
        &[],
        &[", extra"],
    );

    assert_changes(
        "// temporary explanation",
        "// permanent explanation",
        &["temporary"],
        &["permanent"],
    );

    assert_changes(
        "let message = \"hello there\";",
        "let message = \"hello world\";",
        &["there"],
        &["world"],
    );
}

#[test]
fn uses_grapheme_boundaries_for_unicode_and_tracks_whitespace_exactly() {
    assert_changes(
        "let cafe\u{301} = 値;",
        "let cafe\u{301}_total = 値;",
        &["cafe\u{301}"],
        &["cafe\u{301}_total"],
    );

    assert_changes("\"👩‍💻\"", "\"👨‍💻\"", &["👩‍💻"], &["👨‍💻"]);

    assert_changes("\tlet count = 1;", "    let count = 1;", &["\t"], &["    "]);
    assert_changes("let count = 1;  ", "let count = 1;", &["  "], &[]);
}

#[test]
fn line_endings_gaps_equal_rows_and_empty_content_do_not_invent_text_spans() {
    for (left, right) in [
        ("", "new\n"),
        ("old\n", ""),
        ("same\n", "same\n"),
        ("same\r\n", "same\n"),
        ("same\n", "same"),
        ("\n", "\r\n"),
    ] {
        let (left, right) = (doc(left), doc(right));
        let alignment = Alignment::between(&left, &right);

        for row in 0..=alignment.rows().len() {
            assert_eq!(
                alignment.intraline(&left, &right, row),
                IntralineDiff::default()
            );
        }
    }

    assert_changes("", "word", &[], &["word"]);
    assert_changes("word", "", &["word"], &[]);
}

#[test]
fn does_not_change_line_pairing_or_create_emphasis_on_unpaired_rows() {
    let left = doc("start\nold\nend\n");
    let right = doc("start\nnew\nadditional\nend\n");
    let alignment = Alignment::between(&left, &right);

    assert_eq!(alignment.rows()[1].kind, DiffKind::Modified);
    assert_eq!(alignment.rows()[2].kind, DiffKind::Added);

    let original = alignment.rows().to_vec();
    assert_ne!(
        alignment.intraline(&left, &right, 1).left,
        Vec::<std::ops::Range<usize>>::new()
    );
    assert_eq!(
        alignment.intraline(&left, &right, 2),
        IntralineDiff::default()
    );
    assert_eq!(alignment.rows(), original);
    assert_eq!(left.text(), "start\nold\nend\n");
    assert_eq!(right.text(), "start\nnew\nadditional\nend\n");
}

#[test]
fn bounds_work_for_pathological_lines_by_using_whole_content_emphasis() {
    for prefix in ["x".repeat(20_000), ";".repeat(600)] {
        let left = doc(&format!("{prefix}old\n"));
        let right = doc(&format!("{prefix}new\n"));
        let diff = Alignment::between(&left, &right).intraline(&left, &right, 0);

        assert_eq!(
            diff.left,
            std::iter::once(0..left.lines()[0].content.end).collect::<Vec<_>>()
        );
        assert_eq!(
            diff.right,
            std::iter::once(0..right.lines()[0].content.end).collect::<Vec<_>>()
        );
    }
}
