//! Input-level regressions on GPUI's headless test platform; no pixel captures.

mod merging;
mod review;
mod saving;

use super::*;
use crate::comparison::ComparisonDocument;
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, VisualTestContext, point};
use std::path::Path;

fn merge_paths(result: &str) -> Comparison {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/merge");
    Comparison::Merge(MergePaths {
        base: fixtures.join("base.rs"),
        local: fixtures.join("local.rs"),
        incoming: fixtures.join("incoming.rs"),
        result: fixtures.join(result),
    })
}

fn harness(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::appearance::init(cx);
        crate::editor::init(cx);
        crate::review::init(cx);
        super::init(cx);

        // Dialog entrance animations use wall-clock time, not GPUI's test clock.
        // Keep pointer targets stationary between simulated input frames.
        cx.set_reduce_motion(true);
    });
    let mut workspace = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| Workspace::new(window, cx));
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    cx.update(|window, cx| {
        workspace.update(cx, |view, cx| {
            // Real watch delivery has separate coverage. Drive scans explicitly
            // here so external IO cannot reorder native input tests.
            view.monitor.take();
            view.disk_watch.take();

            let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
            view.open_paths(
                &fixtures.join("before.rs"),
                &fixtures.join("after.rs"),
                window,
                cx,
            );
        });
        window.render_frame(cx);
    });
    cx.run_until_parked();

    (workspace, cx)
}

#[gpui_kit::test]
fn startup_and_handoff_dispatch_can_read_modal_state_and_report_file_errors(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = harness(cx);
    let window = cx.update(|window, _| window.window_handle().downcast::<Root>().unwrap());
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");

    crate::dispatch_open(window, &workspace, &[], &mut cx.cx).unwrap();
    crate::dispatch_open(
        window,
        &workspace,
        &[Comparison::diff(
            fixtures.join("intraline-before.rs"),
            fixtures.join("intraline-after.rs"),
        )],
        &mut cx.cx,
    )
    .unwrap();
    cx.update(|_, cx| assert_eq!(workspace.read(cx).tabs.entries.len(), 2));

    let error = crate::dispatch_open(
        window,
        &workspace,
        &[Comparison::diff(
            fixtures.join("missing.rs"),
            fixtures.join("after.rs"),
        )],
        &mut cx.cx,
    )
    .unwrap_err();
    assert!(error.contains("missing.rs"));
    cx.update(|_, cx| assert_eq!(workspace.read(cx).tabs.entries.len(), 2));
}

#[gpui_kit::test]
fn read_only_in_memory_comparison_uses_logical_paths_and_rejects_edits(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let baseline = directory.path().join("history/original.rs");
    let local = directory.path().join("history/current.rs");
    let comparison = Comparison::two_way(
        ComparisonDocument::read_only_memory(baseline.clone(), b"old\n".to_vec()),
        ComparisonDocument::read_only_memory(local.clone(), b"new\n".to_vec()),
    );

    cx.update(|window, cx| {
        workspace
            .update(cx, |view, cx| {
                view.open_comparisons(&[comparison], window, cx)
            })
            .unwrap();
        window.render_frame(cx);

        let id = workspace.read(cx).tabs.active.unwrap();
        let editor = workspace
            .read(cx)
            .tabs
            .get(id)
            .unwrap()
            .content
            .comparison()
            .unwrap()
            .editor
            .clone();

        assert!(!editor.read(cx).can_edit());
        assert!(!editor.read(cx).can_save());
        assert_eq!(
            window.find(("pane-filename", 0usize)).label(),
            baseline.to_str()
        );
        assert_eq!(
            window.find(("pane-filename", 1usize)).label(),
            local.to_str()
        );
        assert!(window.try_find("save-document").is_none());

        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at("rows-viewport", point(width * 0.75, px(11.0)), cx);
        window.input("rejected", cx);
        window.press("ctrl-s", cx);

        assert_eq!(editor.read(cx).current_checkpoint().text, "new\n");
        assert!(!editor.read(cx).needs_save());
    });
    cx.run_until_parked();

    assert!(directory.path().read_dir().unwrap().next().is_none());
}

#[gpui_kit::test]
fn editable_in_memory_document_without_destination_edits_but_exposes_no_save_action(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let comparison = Comparison::two_way(
        ComparisonDocument::read_only_memory(
            directory.path().join("baseline.rs"),
            b"old\n".to_vec(),
        ),
        ComparisonDocument::editable_memory(
            directory.path().join("scratch.rs"),
            b"new\n".to_vec(),
            None,
        ),
    );

    cx.update(|window, cx| {
        workspace
            .update(cx, |view, cx| {
                view.open_comparisons(&[comparison], window, cx)
            })
            .unwrap();
        window.render_frame(cx);

        let id = workspace.read(cx).tabs.active.unwrap();
        let editor = workspace
            .read(cx)
            .tabs
            .get(id)
            .unwrap()
            .content
            .comparison()
            .unwrap()
            .editor
            .clone();

        assert!(editor.read(cx).can_edit());
        assert!(!editor.read(cx).can_save());
        assert!(window.try_find("save-document").is_none());

        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at("rows-viewport", point(width * 0.75, px(11.0)), cx);
        window.input("changed ", cx);

        assert!(editor.read(cx).needs_save());

        window.click(("tab-close-target", id), cx);
        assert!(window.has_active_dialog(cx));
        assert!(window.try_find("save-and-close").is_none());
        window.press("escape", cx);
    });

    assert!(directory.path().read_dir().unwrap().next().is_none());
}

#[gpui_kit::test]
fn in_memory_baseline_with_editable_file_saves_to_the_backing_file(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let local = directory.path().join("local.rs");
    std::fs::write(&local, "new\n").unwrap();

    let comparison = Comparison::two_way(
        ComparisonDocument::read_only_memory(
            directory.path().join("historical.rs"),
            b"old\n".to_vec(),
        ),
        ComparisonDocument::editable_file(local.clone()),
    );

    cx.update(|window, cx| {
        workspace
            .update(cx, |view, cx| {
                view.open_comparisons(&[comparison], window, cx)
            })
            .unwrap();
        window.render_frame(cx);

        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at("rows-viewport", point(width * 0.75, px(11.0)), cx);
        window.press("home", cx);
        window.input("saved ", cx);
        window.press("ctrl-s", cx);
    });
    cx.run_until_parked();

    assert_eq!(std::fs::read_to_string(local).unwrap(), "saved new\n");
    assert!(!directory.path().join("historical.rs").exists());
}

#[gpui_kit::test]
fn editable_in_memory_document_saves_only_to_its_explicit_destination(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let logical = directory.path().join("review/current.rs");
    let destination = directory.path().join("saved.rs");
    let comparison = Comparison::two_way(
        ComparisonDocument::read_only_memory(
            directory.path().join("review/original.rs"),
            b"old\n".to_vec(),
        ),
        ComparisonDocument::editable_memory(
            logical.clone(),
            b"new\n".to_vec(),
            Some(destination.clone()),
        ),
    );

    cx.update(|window, cx| {
        workspace
            .update(cx, |view, cx| {
                view.open_comparisons(&[comparison], window, cx)
            })
            .unwrap();
        window.render_frame(cx);

        assert!(window.try_find("save-document").is_some());

        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at("rows-viewport", point(width * 0.75, px(11.0)), cx);
        window.press("home", cx);
        window.input("saved ", cx);
        window.press("ctrl-s", cx);
    });
    cx.run_until_parked();

    assert_eq!(std::fs::read_to_string(destination).unwrap(), "saved new\n");
    assert!(!logical.exists());
}

#[gpui_kit::test]
fn switching_tabs_uses_current_geometry_on_the_first_frame(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    cx.update(|window, cx| {
        workspace.update(cx, |view, cx| {
            let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
            view.open_paths(
                &fixtures.join("intraline-before.rs"),
                &fixtures.join("intraline-after.rs"),
                window,
                cx,
            );
        });
        window.render_frame(cx);
    });
    cx.run_until_parked();

    // The hidden editor retains its old measurement while the window changes.
    cx.simulate_resize(gpui_kit::size(px(900.0), px(700.0)));
    cx.run_until_parked();

    cx.update(|window, cx| {
        workspace.update(cx, |view, cx| view.activate(0, window, cx));
        window.render_frame(cx);

        let editor = window.find("aligned-editor").bounds();
        let rows = window.find("rows-viewport").bounds();
        let footer = window.find("editor-footer").bounds();
        let scrollbar = window.find("diff-scrollbar").bounds();

        assert_eq!(
            rows.right(),
            scrollbar.left(),
            "pane widths must be correct immediately after activation"
        );
        assert_eq!(scrollbar.right(), editor.right());
        assert_eq!(
            rows.bottom(),
            footer.top(),
            "rows must meet the footer on the first frame"
        );
        assert_eq!(
            footer.bottom(),
            editor.bottom(),
            "the footer must reach the current viewport bottom on the first frame"
        );

        window.render_frame(cx);
        assert_eq!(
            window.find("rows-viewport").bounds(),
            rows,
            "a later frame must not reposition the panes"
        );
    });
}

#[gpui_kit::test]
fn reopening_a_merge_preserves_its_tab_and_independent_history(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let paths = merge_paths("result.rs");
    let id = cx.update(|window, cx| {
        workspace
            .update(cx, |view, cx| {
                view.open_comparisons(std::slice::from_ref(&paths), window, cx)
            })
            .unwrap();
        let id = workspace.read(cx).tabs.active.unwrap();
        assert_eq!(workspace.read(cx).tabs.entries.len(), 2);
        assert_eq!(workspace.read(cx).tabs.active, Some(id));

        window.click(("merge-incoming-button", 0usize), cx);
        let editor = workspace
            .read(cx)
            .tabs
            .get(id)
            .unwrap()
            .content
            .comparison()
            .unwrap()
            .editor
            .clone();
        assert!(editor.read(cx).is_dirty());

        workspace.update(cx, |workspace, cx| workspace.activate(0, window, cx));
        workspace
            .update(cx, |view, cx| {
                view.open_comparisons(std::slice::from_ref(&paths), window, cx)
            })
            .unwrap();
        assert_eq!(workspace.read(cx).tabs.active, Some(id));
        assert_eq!(workspace.read(cx).tabs.entries.len(), 2);
        assert!(editor.read(cx).is_dirty());

        window.press("ctrl-z", cx);
        assert!(!editor.read(cx).is_dirty());
        window.press("ctrl-shift-z", cx);
        assert!(editor.read(cx).is_dirty());

        window.press("ctrl-w", cx);
        window.click("cancel", cx);
        id
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        assert!(workspace.read(cx).tabs.get(id).is_some());
        window.press("ctrl-w", cx);
        window.click("ok", cx);
    });
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert!(workspace.read(cx).tabs.get(id).is_none());
        assert_eq!(workspace.read(cx).tabs.active, Some(0));
    });
}

#[gpui_kit::test]
fn input_pane_undo_is_scoped_to_the_active_tab(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    cx.update(|window, cx| {
        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at("rows-viewport", point(width * 0.75, px(11.0)), cx);
        window.input("X", cx);
        let diff = workspace
            .read(cx)
            .tabs
            .get(0)
            .unwrap()
            .content
            .comparison()
            .unwrap()
            .editor
            .clone();
        assert!(diff.read(cx).is_dirty());

        workspace
            .update(cx, |view, cx| {
                view.open_comparisons(&[merge_paths("result.rs")], window, cx)
            })
            .unwrap();
        let id = workspace.read(cx).tabs.active.unwrap();
        let merge = workspace
            .read(cx)
            .tabs
            .get(id)
            .unwrap()
            .content
            .comparison()
            .unwrap()
            .editor
            .clone();
        window.click(("merge-incoming-button", 0usize), cx);
        assert!(merge.read(cx).is_dirty());

        workspace.update(cx, |workspace, cx| workspace.activate(0, window, cx));
        window.render_frame(cx);
        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at("rows-viewport", point(width * 0.25, px(11.0)), cx);
        window.press("ctrl-z", cx);
        assert!(!diff.read(cx).is_dirty());
        assert!(merge.read(cx).is_dirty());

        workspace.update(cx, |workspace, cx| workspace.activate(id, window, cx));
        window.render_frame(cx);
        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at("rows-viewport", point(width * 0.9, px(11.0)), cx);
        window.press("ctrl-z", cx);
        assert!(!merge.read(cx).is_dirty());
        assert!(!diff.read(cx).is_dirty());
    });
}

#[gpui_kit::test]
fn close_button_removes_a_clean_comparison(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);

    cx.update(|window, cx| window.click(("tab-close-target", 0usize), cx));
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert!(
            workspace.read(cx).tabs.entries.is_empty(),
            "clicking close must remove the clean tab"
        );
    });
}

#[gpui_kit::test]
fn native_close_accepts_a_clean_window(cx: &mut TestAppContext) {
    let (_, cx) = harness(cx);

    assert!(
        cx.simulate_close(),
        "the compositor close request must be accepted for a clean window"
    );
}

fn copy_active_text(window: &mut Window, cx: &mut App) -> String {
    let command = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    window.press(&format!("{command}-a"), cx);
    window.press(&format!("{command}-c"), cx);

    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .expect("the editor should copy its selection")
}

#[gpui_kit::test]
fn vim_preference_is_shared_but_typing_history_and_pending_commands_are_tab_local(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = harness(cx);
    let original =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/after.rs"))
            .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let left = temporary.path().join("left.txt");
    let right = temporary.path().join("right.txt");
    std::fs::write(&left, "baseline\n").unwrap();
    std::fs::write(&right, "local\n").unwrap();

    cx.update(|window, cx| {
        window.click("editor-options", cx);
        window.render_frame(cx);

        window.press("down", cx);
        window.press("down", cx);
        window.press("down", cx);
        window.press("enter", cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.press("i", cx);
        window.input("prefix", cx);

        workspace.update(cx, |view, cx| {
            view.open_paths(&left, &right, window, cx);
        });
        window.render_frame(cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        // A newly opened tab inherits Vim mode, not the other tab's Insert state.
        window.press("i", cx);
        window.input("new", cx);
        window.press("escape", cx);
        assert_eq!(copy_active_text(window, cx), "newlocal\n");

        window.press("escape", cx);
        window.press("d", cx);
        workspace.update(cx, |view, cx| view.activate(0, window, cx));
        window.render_frame(cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.press("u", cx);
        assert_eq!(copy_active_text(window, cx), original);

        workspace.update(cx, |view, cx| view.activate(1, window, cx));
        window.render_frame(cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.press("w", cx);
        assert_eq!(copy_active_text(window, cx), "newlocal\n");

        window.press("escape", cx);
        window.press("u", cx);
        assert_eq!(copy_active_text(window, cx), "local\n");
    });
}

#[gpui_kit::test]
fn closing_a_modified_tab_can_keep_then_discard_its_edits(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let edited_text = cx.update(|window, cx| {
        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at("rows-viewport", point(width * 0.75, px(11.0)), cx);
        window.input("changed", cx);
        copy_active_text(window, cx)
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        assert!(workspace.read(cx).has_modified_tabs(cx));
        window.click(("tab-close-target", 0usize), cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(workspace.read(cx).tabs.entries.len(), 1);
        window.click("cancel", cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        assert!(!window.has_active_dialog(cx));
        assert_eq!(workspace.read(cx).tabs.entries.len(), 1);
        assert!(workspace.read(cx).has_modified_tabs(cx));
        assert_eq!(copy_active_text(window, cx), edited_text);

        window.click(("tab-close-target", 0usize), cx);
        window.click("ok", cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        assert!(!window.has_active_dialog(cx));
        assert!(workspace.read(cx).tabs.entries.is_empty());
    });
}

#[gpui_kit::test]
fn forwarded_pairs_preserve_existing_edits_and_load_new_files_before_returning(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = harness(cx);
    let edited_text = cx.update(|window, cx| {
        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at("rows-viewport", point(width * 0.75, px(11.0)), cx);
        window.input("changed", cx);
        copy_active_text(window, cx)
    });
    let temporary = tempfile::tempdir().unwrap();
    let left = temporary.path().join("baseline.rs");
    let right = temporary.path().join("local.rs");
    std::fs::write(&left, "baseline\n").unwrap();
    std::fs::write(&right, "local\n").unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");

    cx.update(|window, cx| {
        workspace.update(cx, |view, cx| {
            view.open_comparisons(&[Comparison::diff(left.clone(), right.clone())], window, cx)
                .unwrap();
        });
        assert_eq!(workspace.read(cx).tabs.entries.len(), 2);
        assert_eq!(workspace.read(cx).tabs.active, Some(1));
    });
    // Match Perforce removing temporary inputs immediately after acknowledgment.
    std::fs::remove_file(left).unwrap();
    std::fs::remove_file(right).unwrap();

    cx.update(|window, cx| {
        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at("rows-viewport", point(width * 0.75, px(11.0)), cx);
        assert_eq!(copy_active_text(window, cx), "local\n");

        workspace.update(cx, |view, cx| {
            view.open_comparisons(
                &[Comparison::diff(
                    fixtures.join("before.rs"),
                    fixtures.join("after.rs"),
                )],
                window,
                cx,
            )
            .unwrap();
        });
        assert_eq!(workspace.read(cx).tabs.entries.len(), 2);
        assert_eq!(workspace.read(cx).tabs.active, Some(0));
        assert_eq!(copy_active_text(window, cx), edited_text);
        assert!(workspace.read(cx).has_modified_tabs(cx));

        let undo = if cfg!(target_os = "macos") {
            "cmd-z"
        } else {
            "ctrl-z"
        };
        // Input simulation types one character per history entry.
        for _ in "changed".chars() {
            window.press(undo, cx);
        }
        assert!(
            !workspace.read(cx).has_modified_tabs(cx),
            "reopening an existing pair must preserve its undo history"
        );
    });
}

#[gpui_kit::test]
fn forwarded_requests_report_errors_and_do_not_interrupt_a_discard_dialog(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let missing = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/does-not-exist.rs");
    cx.update(|window, cx| {
        let error = workspace
            .update(cx, |view, cx| {
                view.open_comparisons(
                    &[Comparison::diff(missing.clone(), missing.clone())],
                    window,
                    cx,
                )
            })
            .unwrap_err();
        assert!(error.contains("does-not-exist.rs"));
        assert_eq!(workspace.read(cx).tabs.entries.len(), 1);

        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at("rows-viewport", point(width * 0.75, px(11.0)), cx);
        window.input("changed", cx);
        window.click(("tab-close-target", 0usize), cx);
        assert!(window.has_active_dialog(cx));

        let error = workspace
            .update(cx, |view, cx| {
                view.open_comparisons(
                    &[Comparison::diff(missing.clone(), missing.clone())],
                    window,
                    cx,
                )
            })
            .unwrap_err();
        assert!(error.contains("dialog open"));
        workspace
            .update(cx, |view, cx| view.open_comparisons(&[], window, cx))
            .unwrap();
        assert!(window.has_active_dialog(cx));

        // Focusing the existing window must not divert Enter/Escape from its modal.
        window.press("escape", cx);
        assert!(!window.has_active_dialog(cx));
        assert_eq!(workspace.read(cx).tabs.entries.len(), 1);
        assert!(workspace.read(cx).has_modified_tabs(cx));
    });
}

#[gpui_kit::test]
fn native_close_can_discard_edits_and_close_the_window(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    cx.update(|window, cx| {
        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at("rows-viewport", point(width * 0.75, px(11.0)), cx);
        window.input("changed", cx);
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert!(workspace.read(cx).has_modified_tabs(cx)));

    assert!(!cx.simulate_close());
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.has_active_dialog(cx));
        window.click("ok", cx);
    });
    cx.run_until_parked();

    cx.cx.update(|cx| {
        assert!(
            cx.windows().is_empty(),
            "confirming discard must close the window"
        );
    });
}
