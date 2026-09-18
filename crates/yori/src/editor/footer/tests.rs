//! Language changes, incremental syntax, and footer geometry—not widget-presence tests.

use super::*;
use crate::editor::{GUTTER_WIDTH, HEADER_HEIGHT, LINE_HEIGHT};
use gpui_kit::component::{Root, highlighter::HighlightTheme};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Entity, TestAppContext, VisualTestContext, point};
use yori_document::{
    Document,
    editing::{EditHistory, TextSelection},
};

fn pane(path: &str, text: &str) -> PaneDocument {
    PaneDocument::new_highlighted(
        path.into(),
        Document::from_bytes(text.as_bytes().to_vec()).unwrap(),
    )
}

fn assert_matches_fresh_highlighting(pane: &PaneDocument) {
    let mut fresh = PaneDocument::new_highlighted(pane.path.clone(), pane.document.clone());
    fresh.set_language(pane.language_override);
    let theme = HighlightTheme::default_dark();
    let range = 0..pane.document.text().len();
    let expected = fresh
        .highlighter
        .as_ref()
        .unwrap()
        .styles(&range, theme.as_ref());
    let actual = pane
        .highlighter
        .as_ref()
        .unwrap()
        .styles(&range, theme.as_ref());

    assert!(
        !expected.is_empty(),
        "{} grammar must produce syntax styles",
        pane.language().label()
    );
    assert_eq!(
        actual,
        expected,
        "{} incremental styles differ from fresh parse",
        pane.language().label()
    );
}

#[test]
fn cpp_highlighting_composes_c_and_cpp_queries() {
    let source = "#include \"file.hpp\"\nclass Widget {};\n";
    let pane = pane("main.cpp", source);
    let theme = HighlightTheme::default_dark();
    let styles = pane
        .highlighter
        .as_ref()
        .unwrap()
        .styles(&(0..source.len()), theme.as_ref());

    let is_styled = |target: &str| {
        let start = source.find(target).unwrap();
        let target = start..start + target.len();

        styles.iter().any(|(range, style)| {
            range.start < target.end && range.end > target.start && style.color.is_some()
        })
    };

    assert!(
        is_styled("#include"),
        "C query did not style the include directive: {styles:?}"
    );
    assert!(
        is_styled("\"file.hpp\""),
        "C query did not style the include path: {styles:?}"
    );
    assert!(
        is_styled("class"),
        "C++ query did not style the class keyword: {styles:?}"
    );
}

#[test]
fn all_four_grammars_track_edits_and_undo_without_changing_source_fidelity() {
    let sources = [
        (
            "main.go",
            "package main\nfunc meaning() int { return 42 }\n",
            Language::Go,
        ),
        (
            "main.c",
            "#include <stdio.h>\r\nint meaning(void) { return 42; }\r\n",
            Language::C,
        ),
        (
            "main.cpp",
            "template<typename T> T meaning() { return T{42}; }",
            Language::Cpp,
        ),
        ("main.rs", "fn meaning() -> u32 { 42 }\n", Language::Rust),
    ];

    for (path, source, language) in sources {
        let mut pane = pane(path, source);
        let mut history = EditHistory::default();
        assert_eq!(pane.language(), language);
        assert_matches_fresh_highlighting(&pane);

        let start = source.find("42").unwrap();
        let selection = TextSelection {
            anchor: start,
            head: start + 2,
        };
        let edit = history
            .replace(
                &mut pane.document,
                selection,
                selection.range(),
                "7 /* changed */",
            )
            .unwrap();
        pane.refresh_after_edit(&edit);

        assert_matches_fresh_highlighting(&pane);
        assert_eq!(
            pane.document.text(),
            source.replacen("42", "7 /* changed */", 1)
        );

        let undo = history
            .undo(&mut pane.document, edit.selection)
            .unwrap()
            .unwrap();
        pane.refresh_after_edit(&undo);

        assert_eq!(pane.document.text(), source);
        assert_matches_fresh_highlighting(&pane);
    }
}

#[test]
fn manual_language_and_plain_text_overrides_survive_edits_and_return_to_auto() {
    let source = "int meaning(void) { return 42; }\r\n";
    let mut pane = pane("temporary-p4-file", source);
    let mut history = EditHistory::default();
    assert_eq!(pane.language(), Language::PlainText);
    assert!(pane.highlighter.is_none());

    pane.set_language(Some(Language::C));
    assert_matches_fresh_highlighting(&pane);
    pane.set_language(Some(Language::PlainText));

    let edit = history
        .replace(
            &mut pane.document,
            TextSelection::caret(0),
            0..0,
            "// local note\n",
        )
        .unwrap();
    pane.refresh_after_edit(&edit);
    assert!(pane.highlighter.is_none());
    assert_eq!(pane.line_endings, super::super::LineEndings::Mixed);

    pane.set_language(Some(Language::Cpp));
    assert_matches_fresh_highlighting(&pane);
    let undo = history
        .undo(&mut pane.document, edit.selection)
        .unwrap()
        .unwrap();
    pane.refresh_after_edit(&undo);

    assert_eq!(pane.document.text(), source);
    assert_eq!(pane.line_endings, super::super::LineEndings::CrLf);
    assert_eq!(pane.language(), Language::Cpp);
    assert_matches_fresh_highlighting(&pane);

    pane.set_language(None);
    assert_eq!(pane.language(), Language::PlainText);
    assert!(pane.highlighter.is_none());
    assert_eq!(pane.document.text(), source);
}

fn harness(cx: &mut TestAppContext) -> (Entity<AlignedEditor>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::appearance::init(cx);
        crate::editor::init(cx);
    });

    let mut editor = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let left = pane("base.h", "int meaning(void) { return 42; }\r\n");
        let right = pane("local.cpp", "int meaning() { return 7; }\n");
        let view = cx.new(|cx| AlignedEditor::new(left, right, window, cx));
        editor = Some(view.clone());

        Root::new(view, window, cx)
    });
    cx.update(TestWindowExt::render_frame);
    cx.run_until_parked();

    (editor.unwrap(), cx)
}

#[gpui_kit::test]
fn language_menu_changes_only_its_pane_and_preserves_dirty_text_selection_and_undo(
    cx: &mut TestAppContext,
) {
    let (editor, cx) = harness(cx);
    let selection = cx.update(|window, cx| {
        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at(
            "rows-viewport",
            point(width / 2.0 + px(GUTTER_WIDTH), px(11.0)),
            cx,
        );
        window.press("home", cx);
        window.input("X", cx);

        let selection = editor.read(cx).right_selection().unwrap();
        window.click("left-language", cx);
        selection
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        // Auto, Plain text, Go: use the real menu's keyboard interaction.
        for key in ["down", "down", "down", "enter"] {
            window.press(key, cx);
        }
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        let view = editor.read(cx);
        assert_eq!(view.left.language(), Language::Go);
        assert_eq!(view.right.language(), Language::Cpp);
        assert_eq!(
            view.left.document.text(),
            "int meaning(void) { return 42; }\r\n"
        );
        assert_eq!(view.right.document.text(), "Xint meaning() { return 7; }\n");
        assert_eq!(view.right_selection(), Some(selection));
        assert!(view.is_dirty());

        window.press("ctrl-z", cx);
        assert_eq!(
            editor.read(cx).right.document.text(),
            "int meaning() { return 7; }\n"
        );
        assert!(!editor.read(cx).is_dirty());
    });
}

#[gpui_kit::test]
fn footer_reserves_viewport_space_and_eof_reveal_stays_above_it(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx);

    for width in [1280.0, 700.0] {
        cx.simulate_resize(gpui_kit::size(px(width), px(620.0)));
        cx.run_until_parked();

        cx.update(|window, cx| {
            let rows = window.find("rows-viewport").bounds();
            let footer = window.find("editor-footer").bounds();
            let content = window.find("aligned-editor").bounds();
            assert_eq!(rows.bottom(), footer.top());
            assert_eq!(footer.bottom(), content.bottom());
            assert_eq!(
                rows.size.height,
                content.size.height - px(HEADER_HEIGHT + FOOTER_HEIGHT)
            );

            window.click_at(
                "rows-viewport",
                point(rows.size.width / 2.0 + px(GUTTER_WIDTH), px(11.0)),
                cx,
            );
            window.input(&"line\n".repeat(100), cx);
            window.press("ctrl-end", cx);

            let view = editor.read(cx);
            let offset = view.right_selection().unwrap().head;
            let row = view
                .alignment
                .row_for_offset(&view.right.document, offset, false);
            let cursor_bottom = yori::geometry::display_units(row) * LINE_HEIGHT + LINE_HEIGHT
                - view.vertical_scroll;
            assert!(cursor_bottom <= view.geometry().rows_viewport_height() + f32::EPSILON);
        });
    }
}
