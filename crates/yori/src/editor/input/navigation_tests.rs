//! Horizontal pane focus preserves comparison position across different source shapes.

use std::fmt::Write as _;

use super::*;
use crate::editor::PaneDocument;
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Entity, TestAppContext, VisualTestContext};
use yori::geometry::display_units;
use yori_document::Document;

fn harness<'a>(
    cx: &'a mut TestAppContext,
    baseline: &str,
    local: &str,
) -> (Entity<AlignedEditor>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::appearance::init(cx);
        crate::editor::init(cx);
    });

    let baseline = baseline.to_owned();
    let local = local.to_owned();
    let mut editor = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let pane = |text: &str| {
            PaneDocument::new(
                "fixture.txt".into(),
                Document::from_bytes(text.as_bytes().to_vec()).unwrap(),
            )
        };
        let view = cx.new(|cx| AlignedEditor::new(pane(&baseline), pane(&local), window, cx));
        editor = Some(view.clone());

        Root::new(view, window, cx)
    });
    cx.update(TestWindowExt::render_frame);
    cx.run_until_parked();

    (editor.unwrap(), cx)
}

fn set_caret(
    editor: &Entity<AlignedEditor>,
    side: Side,
    offset: usize,
    window: &mut Window,
    cx: &mut App,
) {
    editor.update(cx, |editor, cx| {
        editor.selection = Some(Selection {
            side,
            anchor: offset,
            head: offset,
        });
        editor.focus.focus(window, cx);
        cx.notify();
    });
    window.render_frame(cx);
}

#[gpui_kit::test]
fn pane_switching_preserves_shaped_columns_across_unicode_and_tabs(cx: &mut TestAppContext) {
    let (editor, cx) = harness(cx, "éxyz\n\tfoo\n", "abcd\n    foo\n");

    cx.update(|window, cx| {
        set_caret(&editor, Side::Right, 1, window, cx);
        window.press("ctrl-h", cx);
        assert_eq!(editor.read(cx).selection.as_ref().unwrap().head, 2);
        window.press("ctrl-l", cx);
        assert_eq!(editor.read(cx).selection.as_ref().unwrap().head, 1);

        set_caret(&editor, Side::Right, 9, window, cx);
        window.press("ctrl-h", cx);
        assert_eq!(editor.read(cx).selection.as_ref().unwrap().head, 7);
        window.press("ctrl-l", cx);
        assert_eq!(editor.read(cx).selection.as_ref().unwrap().head, 9);
    });
}

#[gpui_kit::test]
fn pane_switching_reveals_the_nearest_caret_across_a_long_alignment_gap(cx: &mut TestAppContext) {
    let mut local = String::from("start\n");
    for line in 0..100 {
        writeln!(local, "inserted {line}").unwrap();
    }
    local.push_str("end\n");
    let (editor, cx) = harness(cx, "start\nend\n", &local);

    cx.update(|window, cx| {
        set_caret(&editor, Side::Right, "start\n".len(), window, cx);
        editor.update(cx, |editor, _| editor.vertical_scroll = 0.0);

        window.press("ctrl-h", cx);

        let editor = editor.read(cx);
        let selection = editor.selection.as_ref().unwrap();
        assert_eq!(selection.side, Side::Left);
        let row = editor.row_for_source(Side::Left, selection.head);
        assert!(
            row > 50,
            "the gap should map to the distant baseline boundary"
        );

        let caret_top = display_units(row) * LINE_HEIGHT;
        let viewport_bottom = editor.vertical_scroll + editor.geometry().rows_viewport_height();
        assert!(caret_top >= editor.vertical_scroll);
        assert!(caret_top + LINE_HEIGHT <= viewport_bottom + f32::EPSILON);
    });
}
