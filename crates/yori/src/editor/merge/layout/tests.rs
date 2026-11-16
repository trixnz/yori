use super::*;
use yori_document::{Document, editing::TextSelection};

fn merge_session(base: &str, local: &str, incoming: &str) -> MergeSession {
    let source = |text: &str| Document::from_bytes(text.as_bytes().to_vec()).unwrap();

    MergeSession::new(source(base), source(local), source(incoming)).unwrap()
}

fn assert_sources(display: &MergeDisplay, session: &MergeSession) {
    for (pane, document) in [session.local(), session.result(), session.incoming()]
        .into_iter()
        .enumerate()
    {
        let lines: Vec<_> = display
            .rows()
            .iter()
            .filter_map(|row| [row.sources.local, row.sources.result, row.sources.incoming][pane])
            .collect();
        assert_eq!(lines, (0..document.lines().len()).collect::<Vec<_>>());
        let text: String = lines
            .iter()
            .map(|&line| document.copy_range(document.lines()[line].full.clone()))
            .collect();
        assert_eq!(text, document.text());
    }

    let projection = session.alignment();
    let ordinary: Vec<_> = display
        .rows()
        .iter()
        .filter(|row| row.kind == RowKind::Aligned)
        .map(|row| row.sources.clone())
        .collect();
    assert_eq!(ordinary, projection.rows());
    for row in display.rows() {
        if row.kind != RowKind::Aligned {
            assert_eq!(
                row.sources,
                MergeRow {
                    local: None,
                    result: None,
                    incoming: None
                }
            );
        }
    }
}

#[test]
fn populated_base_classifies_ancestor_lines_without_participating_source() {
    let session = merge_session(
        "head\r\nbase one\r\nbase two\r\ntail",
        "head\r\nlocal\r\ntail",
        "head\r\nincoming\r\ntail",
    );
    let collapsed = MergeDisplay::build(&session, None);
    let expanded = MergeDisplay::build(&session, Some(ConflictId(0)));

    assert_eq!(
        collapsed.conflicts(),
        &[ConflictDisplay {
            id: ConflictId(0),
            header_row: 1,
            source_span: 2..3,
            control_span: 1..3,
            base_caption: None,
        }]
    );
    assert_eq!(
        expanded.conflicts(),
        &[ConflictDisplay {
            id: ConflictId(0),
            header_row: 1,
            source_span: 5..6,
            control_span: 1..6,
            base_caption: Some(2),
        }]
    );
    let kinds: Vec<_> = expanded.rows().iter().map(|row| &row.kind).collect();
    assert_eq!(
        kinds,
        vec![
            &RowKind::Aligned,
            &RowKind::ConflictHeader(ConflictId(0)),
            &RowKind::Base(BaseRow::Caption),
            &RowKind::Base(BaseRow::SourceLine(1)),
            &RowKind::Base(BaseRow::SourceLine(2)),
            &RowKind::Aligned,
            &RowKind::Aligned,
        ]
    );
    assert!(expanded.rows().get(expanded.rows().len()).is_none());
    assert_sources(&collapsed, &session);
    assert_sources(&expanded, &session);
}

#[test]
fn empty_ancestor_at_eof_has_caption_and_empty_row_not_source_text() {
    let session = merge_session("head\n", "head\nlocal\n", "head\nincoming\nextra");
    let display = MergeDisplay::build(&session, Some(ConflictId(0)));

    assert_eq!(
        display.conflicts(),
        &[ConflictDisplay {
            id: ConflictId(0),
            header_row: 1,
            source_span: 4..6,
            control_span: 1..6,
            base_caption: Some(2),
        }]
    );
    assert_eq!(display.rows()[2].kind, RowKind::Base(BaseRow::Caption));
    assert_eq!(
        display.rows()[3].kind,
        RowKind::Base(BaseRow::EmptyAncestor)
    );
    assert_eq!(display.rows()[5].kind, RowKind::Aligned);
    assert_eq!(
        display.rows()[5].sources,
        MergeRow {
            local: None,
            result: None,
            incoming: Some(2)
        }
    );
    assert_sources(&display, &session);
}

#[test]
fn empty_result_keeps_input_only_rows_ordinary_and_empty_documents_stay_empty() {
    let mut session = merge_session("old\n", "", "new\nsecond\n");
    let display = MergeDisplay::build(&session, Some(ConflictId(0)));

    assert_eq!(display.conflicts()[0].source_span, 3..5);
    assert_eq!(display.conflicts()[0].control_span, 0..5);
    assert!(
        display.rows()[3..]
            .iter()
            .all(|row| row.kind == RowKind::Aligned && row.sources.result.is_none())
    );
    assert_sources(&display, &session);

    session
        .replace(TextSelection::caret(0), 0..0, "result only\nmore\n")
        .unwrap();
    let display = MergeDisplay::build(&session, None);
    assert_sources(&display, &session);

    let empty = merge_session("", "", "");
    let display = MergeDisplay::build(&empty, None);
    assert_eq!(display.rows(), []);
    assert_eq!(display.conflicts(), []);
    assert_sources(&display, &empty);
}

#[test]
fn translated_conflict_endpoints_include_intervening_header_and_base_rows() {
    let mut session = merge_session("A\nS\nB\n", "L\nS\nM\n", "I\nS\nJ\n");
    session
        .replace(TextSelection::caret(0), 0..6, "M\nS\nL\n")
        .unwrap();
    let display = MergeDisplay::build(&session, Some(ConflictId(1)));

    assert_eq!(display.conflicts()[0].source_span, 1..7);
    assert_eq!(display.conflicts()[0].control_span, 0..7);
    assert_eq!(display.conflicts()[1].source_span, 6..9);
    assert_eq!(display.conflicts()[1].control_span, 3..9);
    assert_eq!(
        display.rows()[3].kind,
        RowKind::ConflictHeader(ConflictId(1))
    );
    assert_eq!(display.rows()[4].kind, RowKind::Base(BaseRow::Caption));
    assert_eq!(
        display.rows()[5].kind,
        RowKind::Base(BaseRow::SourceLine(2))
    );
    assert_sources(&display, &session);
}

#[test]
fn coincident_conflict_starts_insert_headers_in_identity_order() {
    let mut session = merge_session("A\nS\nB\n", "L\nS\nM\n", "I\nS\nJ\n");
    // Joining conflict text onto one result line makes both spans start there.
    session.replace(TextSelection::caret(1), 1..4, "").unwrap();
    let display = MergeDisplay::build(&session, Some(ConflictId(0)));

    assert_eq!(session.result().text(), "LM\n");
    assert_eq!(display.conflicts()[0].header_row, 0);
    assert_eq!(display.conflicts()[1].header_row, 3);
    assert_eq!(
        display.rows()[0].kind,
        RowKind::ConflictHeader(ConflictId(0))
    );
    assert_eq!(
        display.rows()[3].kind,
        RowKind::ConflictHeader(ConflictId(1))
    );
    assert_eq!(display.conflicts()[0].source_span, 4..5);
    assert_eq!(display.conflicts()[1].source_span, 4..7);
    assert_eq!(display.conflicts()[0].control_span, 0..5);
    assert_eq!(display.conflicts()[1].control_span, 3..7);
    assert_sources(&display, &session);
}
