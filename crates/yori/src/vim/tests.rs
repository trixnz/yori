use super::*;
use yori_document::editing::EditHistory;

/// Stands in for the OS clipboard, which is the only register.
#[derive(Default)]
struct TestClipboard {
    text: Option<String>,
    writes: usize,
}

impl Clipboard for TestClipboard {
    fn read(&mut self) -> Option<String> {
        self.text.clone()
    }

    fn write(&mut self, text: String) {
        self.text = Some(text);
        self.writes += 1;
    }
}

struct Editor {
    vim: Vim,
    document: Document,
    history: EditHistory,
    selection: TextSelection,
    clipboard: TestClipboard,
    writable: bool,
}

impl Editor {
    fn new(text: &str) -> Self {
        Self {
            vim: Vim::default(),
            document: Document::from_bytes(text.as_bytes().to_vec()).unwrap(),
            history: EditHistory::default(),
            selection: TextSelection::caret(0),
            clipboard: TestClipboard::default(),
            writable: true,
        }
    }

    fn key(&mut self, key: &str) {
        let outcome = self
            .vim
            .handle(
                key,
                if self.writable {
                    EditTarget::Document(&mut self.document, &mut self.history)
                } else {
                    EditTarget::ReadOnly(&self.document)
                },
                self.selection,
                &mut self.clipboard,
            )
            .unwrap();

        self.selection = outcome.selection;
        if !outcome.consumed {
            // Native typing uses the same constrained source target.
            let edit = EditTarget::Document(&mut self.document, &mut self.history)
                .replace(self.selection, self.selection.range(), key, true)
                .unwrap();
            self.selection = edit.selection;
        }
    }

    fn keys(&mut self, keys: &str) {
        for key in keys.chars() {
            self.key(&key.to_string());
        }
    }

    fn text(&self) -> &str {
        self.document.text()
    }

    fn copied(&self) -> &str {
        self.clipboard.text.as_deref().unwrap_or_default()
    }
}

#[test]
fn change_inner_word_and_insert_session_are_one_undo_step() {
    let mut editor = Editor::new("let old_name = 1;\r\n");
    editor.keys("wllciw");
    assert_eq!(editor.vim.mode(), Mode::Insert);
    assert_eq!(editor.text(), "let  = 1;\r\n");

    editor.keys("new_name");
    editor.key("escape");
    assert_eq!(editor.text(), "let new_name = 1;\r\n");

    editor.key("u");
    assert_eq!(editor.text(), "let old_name = 1;\r\n");

    editor.key("ctrl-r");
    assert_eq!(editor.text(), "let new_name = 1;\r\n");
}

#[test]
fn counted_delete_and_change_word_have_distinct_whitespace_semantics() {
    let mut editor = Editor::new("one two three four\ntail\n");
    editor.keys("3dw");
    assert_eq!(editor.text(), "four\ntail\n");

    editor.key("u");
    editor.keys("cw");
    editor.keys("1");
    editor.key("escape");
    assert_eq!(editor.text(), "1 two three four\ntail\n");

    editor.key("u");
    editor.keys("2d2w");
    assert_eq!(editor.text(), "\ntail\n");
}

#[test]
fn linewise_yank_paste_delete_and_undo_preserve_line_endings() {
    for (text, doubled) in [("a\r\nb\r\n", "a\r\na\r\nb\r\n"), ("a\nb", "a\na\nb")] {
        let mut editor = Editor::new(text);
        editor.keys("yyp");
        assert_eq!(editor.text(), doubled);

        editor.key("u");
        assert_eq!(editor.text(), text);

        editor.keys("Gdd");
        assert_eq!(
            editor.text(),
            if text.ends_with('\n') { "a\r\n" } else { "a" }
        );

        editor.key("u");
        assert_eq!(editor.text(), text);
    }
}

#[test]
fn visual_ranges_are_inclusive_and_linewise_ranges_cover_full_source_lines() {
    let mut editor = Editor::new("abc\ndef\nghi");
    editor.keys("vl");
    assert_eq!(editor.document.copy_range(editor.selection.range()), "ab");

    editor.key("d");
    assert_eq!(editor.text(), "c\ndef\nghi");

    editor.key("u");
    editor.keys("Vjy");
    assert_eq!(editor.copied(), "abc\ndef\n");

    editor.key("G");
    editor.key("p");
    assert_eq!(editor.text(), "abc\ndef\nghi\nabc\ndef");
}

#[test]
fn backwards_visual_selection_includes_both_endpoints() {
    let mut editor = Editor::new("abc\n");
    editor.keys("$vhy");
    assert_eq!(editor.copied(), "bc");
}

#[test]
fn grapheme_movement_and_deletion_never_split_unicode_or_crlf() {
    let mut editor = Editor::new("e\u{301}👩‍💻界\r\nlast");
    editor.key("l");
    assert_eq!(editor.selection.head, "e\u{301}".len());

    editor.key("x");
    assert_eq!(editor.text(), "e\u{301}界\r\nlast");

    editor.key("u");
    editor.keys("$x");
    assert_eq!(editor.text(), "e\u{301}👩‍💻\r\nlast");
}

#[test]
fn insert_commands_and_escape_keep_cursor_on_the_correct_line() {
    let cases = [
        ("i", "Xabc\n"),
        ("a", "aXbc\n"),
        ("I", "Xabc\n"),
        ("A", "abcX\n"),
        ("o", "abc\nX\n"),
        ("O", "X\nabc\n"),
    ];

    for (command, expected) in cases {
        let mut editor = Editor::new("abc\n");
        editor.key(command);
        editor.key("X");
        editor.key("escape");
        assert_eq!(editor.text(), expected, "{command}");

        editor.key("u");
        assert_eq!(editor.text(), "abc\n", "{command}");
    }
}

#[test]
fn empty_documents_blank_lines_and_unterminated_last_lines_are_editable() {
    for original in ["", "\n", "\r\n", "last"] {
        let mut editor = Editor::new(original);
        editor.keys("G$i");
        editor.key("X");
        editor.key("escape");
        editor.key("u");
        assert_eq!(editor.text(), original);
    }
}

#[test]
fn read_only_pane_allows_navigation_and_yank_but_no_mutation() {
    let mut editor = Editor::new("left\nright\n");
    editor.writable = false;

    editor.keys("jyy");
    assert_eq!(editor.copied(), "right\n");

    editor.keys("ddciwpioxu");
    editor.key("ctrl-r");
    assert_eq!(editor.text(), "left\nright\n");
    assert_eq!(editor.vim.mode(), Mode::Normal);
}

#[test]
fn cancelling_pending_commands_and_disabling_insert_preserves_history() {
    let mut editor = Editor::new("abc\n");
    editor.key("d");
    editor.vim.cancel(
        EditTarget::Document(&mut editor.document, &mut editor.history),
        editor.selection,
    );
    editor.key("w");
    assert_eq!(editor.text(), "abc\n");

    editor.key("i");
    editor.key("X");
    editor.vim.cancel(
        EditTarget::Document(&mut editor.document, &mut editor.history),
        editor.selection,
    );
    let edit = editor
        .history
        .replace(
            &mut editor.document,
            editor.selection,
            editor.selection.range(),
            "Y",
        )
        .unwrap();
    editor.selection = edit.selection;

    editor.key("u");
    assert_eq!(editor.text(), "abXc\n");

    editor.key("u");
    assert_eq!(editor.text(), "abc\n");
}

#[test]
fn mouse_selection_and_insert_repositioning_use_existing_history() {
    let mut editor = Editor::new("abc def\n");
    editor.selection = TextSelection { anchor: 0, head: 3 };
    editor.vim.select(&editor.document, editor.selection);

    editor.key("c");
    editor.key("X");
    editor.vim.reposition(
        EditTarget::Document(&mut editor.document, &mut editor.history),
        editor.selection,
    );
    assert_eq!(editor.vim.mode(), Mode::Insert);

    editor.selection = TextSelection::caret(0);
    editor.key("Y");
    editor.key("escape");
    assert_eq!(editor.text(), "YX def\n");

    editor.key("u");
    assert_eq!(editor.text(), "X def\n");

    editor.key("u");
    assert_eq!(editor.text(), "abc def\n");
}

#[test]
fn clipboard_lines_cross_documents_with_different_line_endings() {
    let mut source = Editor::new("copied\r\n");
    source.keys("yy");

    let mut target = Editor::new("unterminated");
    target.clipboard = source.clipboard;
    target.key("p");
    assert_eq!(target.text(), "unterminated\ncopied");

    target.key("u");
    assert_eq!(target.text(), "unterminated");
}

#[test]
fn counted_inner_word_and_vertical_motions_respect_source_lines() {
    let mut editor = Editor::new("one two three\nx\nlonger line\n");
    editor.keys("c2iw");
    assert_eq!(editor.text(), " three\nx\nlonger line\n");

    editor.key("escape");
    editor.key("u");
    editor.keys("4ljj");
    assert_eq!(editor.selection.head, "one two three\nx\nlong".len());

    editor.keys("2gg");
    assert_eq!(editor.selection.head, "one two three\n".len());
}

#[test]
fn word_navigation_stops_at_blank_lines_and_dollar_preserves_end_column() {
    for newline in ["\n", "\r\n"] {
        let text = format!("first{newline}{newline}last longer{newline}");
        let mut editor = Editor::new(&text);
        editor.key("w");
        assert_eq!(editor.selection.head, "first".len() + newline.len());

        editor.key("w");
        assert_eq!(editor.selection.head, "first".len() + 2 * newline.len());

        editor.key("b");
        assert_eq!(editor.selection.head, "first".len() + newline.len());

        editor.keys("gg$jj");
        assert_eq!(editor.selection.head, text.len() - newline.len() - 1);
    }
}

#[test]
fn native_composition_and_typing_commit_as_one_modal_transaction() {
    let mut editor = Editor::new("old\r\n");
    editor.keys("ciw");

    for text in ["e", "e\u{301}"] {
        let range = editor
            .history
            .marked_range()
            .unwrap_or(editor.selection.range());
        let edit = EditTarget::Document(&mut editor.document, &mut editor.history)
            .replace_marked(editor.selection, range, text, None, true)
            .unwrap();
        editor.selection = edit.selection;
    }
    EditTarget::Document(&mut editor.document, &mut editor.history).unmark(editor.selection);
    editor.key("!");
    editor.key("escape");
    assert_eq!(editor.text(), "e\u{301}!\r\n");

    editor.key("u");
    assert_eq!(editor.text(), "old\r\n");

    editor.key("ctrl-r");
    assert_eq!(editor.text(), "e\u{301}!\r\n");
}

#[test]
fn unknown_commands_do_not_insert_text_or_execute_a_partial_operator() {
    let mut editor = Editor::new("one two\n");
    editor.keys("dqwl");
    editor.keys("dcw");
    assert_eq!(editor.text(), "one two\n");

    editor.keys("gqi");
    assert_eq!(editor.vim.mode(), Mode::Insert);
}

#[test]
fn merge_vim_groups_changes_and_open_lines_without_consuming_status_history() {
    use yori_diff::merge::{ConflictId, MergeSession};

    for command in ["ciw", "o", "O"] {
        let doc = |text: &str| Document::from_bytes(text.as_bytes().to_vec()).unwrap();
        let mut merge =
            MergeSession::new(doc("base\r\n"), doc("old\r\n"), doc("incoming\r\n")).unwrap();
        let mut vim = Vim::default();
        let mut clipboard = TestClipboard::default();
        let mut selection = TextSelection::caret(0);
        merge.set_resolved(ConflictId(0), true, selection).unwrap();

        for key in command.chars() {
            selection = vim
                .handle(
                    &key.to_string(),
                    EditTarget::Merge(&mut merge),
                    selection,
                    &mut clipboard,
                )
                .unwrap()
                .selection;
        }
        assert_eq!(vim.mode(), Mode::Insert);
        selection = EditTarget::Merge(&mut merge)
            .replace_marked(selection, selection.range(), "é", None, true)
            .unwrap()
            .selection;
        let marked = merge.marked_range().unwrap();
        selection = EditTarget::Merge(&mut merge)
            .replace(selection, marked, "界", true)
            .unwrap()
            .selection;
        selection = EditTarget::Merge(&mut merge)
            .replace(selection, selection.range(), "!", true)
            .unwrap()
            .selection;
        selection = vim
            .handle(
                "escape",
                EditTarget::Merge(&mut merge),
                selection,
                &mut clipboard,
            )
            .unwrap()
            .selection;
        let edited = merge.result().text().to_owned();

        selection = vim
            .handle(
                "u",
                EditTarget::Merge(&mut merge),
                selection,
                &mut clipboard,
            )
            .unwrap()
            .selection;
        assert_eq!(merge.result().text(), "old\r\n", "{command}");
        assert!(merge.state(ConflictId(0)).unwrap().resolved);

        let status = vim
            .handle(
                "u",
                EditTarget::Merge(&mut merge),
                selection,
                &mut clipboard,
            )
            .unwrap();
        assert!(status.edit.is_none());
        assert!(!merge.state(ConflictId(0)).unwrap().resolved);
        assert_eq!(merge.result().text(), "old\r\n");

        selection = vim
            .handle(
                "ctrl-r",
                EditTarget::Merge(&mut merge),
                status.selection,
                &mut clipboard,
            )
            .unwrap()
            .selection;
        vim.handle(
            "ctrl-r",
            EditTarget::Merge(&mut merge),
            selection,
            &mut clipboard,
        )
        .unwrap();
        assert_eq!(merge.result().text(), edited);
        assert!(merge.state(ConflictId(0)).unwrap().resolved);
    }
}

#[test]
fn merge_vim_reposition_and_cancel_commit_separate_typing_runs() {
    use yori_diff::merge::MergeSession;

    let doc = |text: &str| Document::from_bytes(text.as_bytes().to_vec()).unwrap();
    let mut merge = MergeSession::new(doc("base\n"), doc("old\n"), doc("incoming\n")).unwrap();
    let mut vim = Vim::default();
    let mut clipboard = TestClipboard::default();
    let mut selection = TextSelection::caret(0);

    for key in ["c", "i", "w"] {
        selection = vim
            .handle(
                key,
                EditTarget::Merge(&mut merge),
                selection,
                &mut clipboard,
            )
            .unwrap()
            .selection;
    }
    selection = EditTarget::Merge(&mut merge)
        .replace(selection, selection.range(), "X", true)
        .unwrap()
        .selection;
    vim.reposition(EditTarget::Merge(&mut merge), selection);
    assert_eq!(vim.mode(), Mode::Insert);

    selection = TextSelection::caret(0);
    selection = EditTarget::Merge(&mut merge)
        .replace(selection, selection.range(), "Y", true)
        .unwrap()
        .selection;
    vim.cancel(EditTarget::Merge(&mut merge), selection);
    assert_eq!(merge.result().text(), "YX\n");

    selection = vim
        .handle(
            "u",
            EditTarget::Merge(&mut merge),
            selection,
            &mut clipboard,
        )
        .unwrap()
        .selection;
    assert_eq!(merge.result().text(), "X\n");

    vim.handle(
        "u",
        EditTarget::Merge(&mut merge),
        selection,
        &mut clipboard,
    )
    .unwrap();
    assert_eq!(merge.result().text(), "old\n");
    assert!(merge.undo(selection).unwrap().is_none());
}

#[test]
fn shift_y_yanks_counted_lines_without_moving_the_cursor() {
    let mut editor = Editor::new("one two\nthree\nfour\n");
    editor.keys("wY");
    assert_eq!(editor.copied(), "one two\n");
    assert_eq!(editor.selection.head, "one ".len());

    editor.keys("j3Y");
    assert_eq!(editor.copied(), "three\nfour\n");
    assert_eq!(editor.text(), "one two\nthree\nfour\n");

    editor.key("p");
    assert_eq!(editor.text(), "one two\nthree\nthree\nfour\nfour\n");
}

#[test]
fn visual_shift_y_yanks_whole_selected_lines() {
    let mut editor = Editor::new("abc\ndef\nghi\n");
    editor.keys("lvjY");
    assert_eq!(editor.copied(), "abc\ndef\n");
    assert_eq!(editor.vim.mode(), Mode::Normal);
    assert_eq!(editor.selection, TextSelection::caret(1));
}

#[test]
fn big_word_motions_skip_punctuation_inside_whitespace_delimited_words() {
    let mut editor = Editor::new("foo.bar(baz) next-one\nlast\n");
    editor.key("W");
    assert_eq!(editor.selection.head, "foo.bar(baz) ".len());

    editor.key("W");
    assert_eq!(editor.selection.head, "foo.bar(baz) next-one\n".len());

    editor.key("B");
    assert_eq!(editor.selection.head, "foo.bar(baz) ".len());

    editor.key("B");
    assert_eq!(editor.selection.head, 0);

    editor.key("E");
    assert_eq!(editor.selection.head, "foo.bar(baz".len());

    editor.keys("2E");
    assert_eq!(editor.selection.head, "foo.bar(baz) next-one\nlas".len());
}

#[test]
fn yank_and_delete_big_word_cover_punctuation() {
    let mut editor = Editor::new("foo.bar baz\n");
    editor.keys("yW");
    assert_eq!(editor.copied(), "foo.bar ");
    assert_eq!(editor.selection.head, 0);

    editor.keys("yw");
    assert_eq!(editor.copied(), "foo");

    editor.keys("dW");
    assert_eq!(editor.text(), "baz\n");
    assert_eq!(editor.copied(), "foo.bar ");
}

#[test]
fn change_big_word_keeps_trailing_whitespace_like_cw() {
    let mut editor = Editor::new("foo.bar baz qux\n");
    editor.keys("cW");
    assert_eq!(editor.text(), " baz qux\n");

    editor.key("escape");
    editor.key("u");
    editor.keys("c2W");
    assert_eq!(editor.text(), " qux\n");
}

#[test]
fn visual_yank_returns_the_cursor_to_the_selection_start() {
    let mut editor = Editor::new("abc def\nghi\n");
    editor.keys("wlvhhy");
    assert_eq!(editor.copied(), " de");
    assert_eq!(editor.vim.mode(), Mode::Normal);
    assert_eq!(editor.selection, TextSelection::caret("abc".len()));

    editor.keys("lVjy");
    assert_eq!(editor.copied(), "abc def\nghi\n");
    assert_eq!(editor.vim.mode(), Mode::Normal);
    assert_eq!(editor.selection, TextSelection::caret("abc ".len()));
    assert_eq!(editor.text(), "abc def\nghi\n");
}

#[test]
fn deletes_and_changes_share_the_clipboard_with_yanks() {
    for (keys, copied) in [
        ("x", "a"),
        ("dw", "abc "),
        ("cw", "abc"),
        ("dd", "abc def\n"),
    ] {
        let mut editor = Editor::new("abc def\n");
        editor.keys(keys);
        assert_eq!(editor.copied(), copied, "{keys}");
    }

    let mut editor = Editor::new("abc def\n");
    editor.keys("yewvep");
    assert_eq!(editor.text(), "abc abc\n");
    assert_eq!(
        editor.copied(),
        "def",
        "visual paste copies what it replaced"
    );
}

#[test]
fn motions_and_empty_changes_leave_text_from_other_apps_on_the_clipboard() {
    let mut editor = Editor::new("abc\n");
    editor.clipboard.text = Some("external".to_owned());
    editor.keys("lwvlv0");
    editor.keys("c0");
    editor.key("escape");
    assert_eq!(editor.text(), "abc\n");
    assert_eq!(editor.clipboard.writes, 0);
    assert_eq!(editor.copied(), "external");
}

#[test]
fn paste_shape_follows_the_clipboard_trailing_line_ending() {
    let mut editor = Editor::new("abc\n");
    editor.clipboard.text = Some("XY".to_owned());
    editor.key("p");
    assert_eq!(editor.text(), "aXYbc\n");

    editor.clipboard.text = Some("line\n".to_owned());
    editor.key("p");
    assert_eq!(editor.text(), "aXYbc\nline\n");

    editor.keys("ggyy");
    assert_eq!(editor.copied(), "aXYbc\n");
}

#[test]
fn linewise_yank_of_an_unterminated_line_still_pastes_as_a_line() {
    let mut editor = Editor::new("a\nb");
    editor.keys("GyyggP");
    assert_eq!(editor.text(), "b\na\nb");
}

#[test]
fn paste_with_an_empty_clipboard_is_a_no_op() {
    let mut editor = Editor::new("abc\n");
    editor.keys("pP");
    assert_eq!(editor.text(), "abc\n");
    assert_eq!(editor.clipboard.writes, 0);
}

#[test]
fn rejected_visual_paste_keeps_the_clipboard() {
    let mut editor = Editor::new("abc\n");
    editor.clipboard.text = Some("x\ry".to_owned());
    editor.keys("vl");

    let result = editor.vim.handle(
        "p",
        EditTarget::Document(&mut editor.document, &mut editor.history),
        editor.selection,
        &mut editor.clipboard,
    );

    assert!(result.is_err());
    assert_eq!(editor.text(), "abc\n");
    assert_eq!(editor.copied(), "x\ry");
    assert_eq!(editor.clipboard.writes, 0);
}
