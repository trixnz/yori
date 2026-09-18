//! Input-level regressions on GPUI's headless test platform; no pixel captures.

mod merging;
mod saving;

use super::*;
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{KeyDownEvent, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext, point};
use std::path::Path;

fn merge_paths(result: &str) -> ComparisonPaths {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/merge");
    ComparisonPaths::Merge(MergePaths {
        base: fixtures.join("base.rs"),
        local: fixtures.join("local.rs"),
        incoming: fixtures.join("incoming.rs"),
        result: fixtures.join(result),
    })
}

fn harness(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    harness_with_options(cx, None, true)
}

fn empty_harness(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    harness_with_options(cx, None, false)
}

fn harness_with_config(
    cx: &mut TestAppContext,
    config_path: Option<std::path::PathBuf>,
) -> (Entity<Workspace>, &mut VisualTestContext) {
    harness_with_options(cx, config_path, true)
}

fn harness_with_options(
    cx: &mut TestAppContext,
    config_path: Option<std::path::PathBuf>,
    open_comparison: bool,
) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::appearance::init(cx);
        if let Some(path) = config_path {
            crate::config::init_for_path(path, cx);
        }
        crate::editor::init(cx);
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

            if open_comparison {
                let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
                view.open_paths(
                    &fixtures.join("before.rs"),
                    &fixtures.join("after.rs"),
                    window,
                    cx,
                );
            }
        });
        window.render_frame(cx);
    });
    cx.run_until_parked();

    (workspace, cx)
}

fn preferences_shortcut() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-,"
    } else {
        "ctrl-,"
    }
}

fn show_preferences(cx: &mut VisualTestContext) {
    cx.simulate_keystrokes(preferences_shortcut());
    cx.update(TestWindowExt::render_frame);
}

fn active_editor(workspace: &Entity<Workspace>, cx: &App) -> Entity<AlignedEditor> {
    let workspace = workspace.read(cx);
    let id = workspace.tabs.active.unwrap();

    workspace.tabs.get(id).unwrap().content.editor.clone()
}

#[gpui_kit::test]
fn preferences_shortcut_opens_from_an_empty_workspace(cx: &mut TestAppContext) {
    let (workspace, cx) = empty_harness(cx);

    cx.update(|_, cx| assert!(workspace.read(cx).tabs.entries.is_empty()));
    show_preferences(cx);

    cx.update(|window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(workspace.read(cx).preferences.is_some());
        window.press("escape", cx);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn preferences_controls_expose_semantics_and_support_keyboard_navigation(cx: &mut TestAppContext) {
    let (_, cx) = harness(cx);
    show_preferences(cx);

    cx.update(|window, cx| {
        let vim = window.find("vim-keybindings");
        let whitespace = window.find("show-whitespace");
        let connections = window.find("show-change-connections");

        assert_eq!(vim.role(), Some(gpui_kit::Role::CheckBox));
        assert_eq!(vim.label(), Some("Use Vim keybindings"));
        assert_eq!(vim.checked(), Some(false));
        assert_eq!(whitespace.role(), Some(gpui_kit::Role::CheckBox));
        assert_eq!(whitespace.label(), Some("Show whitespace"));
        assert_eq!(connections.role(), Some(gpui_kit::Role::CheckBox));
        assert_eq!(connections.label(), Some("Show change connections"));

        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find("vim-keybindings").focused(), Some(true));
    });

    let space = Keystroke::parse("space").unwrap();
    cx.simulate_event(KeyDownEvent {
        keystroke: space.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(KeyUpEvent { keystroke: space });
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("vim-keybindings").checked(), Some(true));

        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find("show-whitespace").focused(), Some(true));
    });
}

#[gpui_kit::test]
fn cancel_discards_selections_and_restores_editor_focus(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("yori").join("config.toml");
    let (workspace, cx) = harness_with_config(cx, Some(path.clone()));
    let editor = cx.update(|_, cx| active_editor(&workspace, cx));

    cx.update(|window, cx| assert!(editor.focus_handle(cx).is_focused(window)));
    show_preferences(cx);
    cx.update(|window, cx| {
        window.click("vim-keybindings", cx);
        window.click("show-whitespace", cx);
        window.click("show-change-connections", cx);
        window.click("preferences-cancel", cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        assert!(!window.has_active_dialog(cx));
        assert!(editor.focus_handle(cx).is_focused(window));
        assert_eq!(
            crate::config::editor(cx),
            crate::config::EditorConfig::default()
        );
    });
    assert!(!path.exists());
}

#[gpui_kit::test]
fn repeated_preferences_shortcut_focuses_one_existing_dialog(cx: &mut TestAppContext) {
    let (_, cx) = harness(cx);
    show_preferences(cx);

    cx.update(|window, cx| window.click("vim-keybindings", cx));
    cx.simulate_keystrokes(preferences_shortcut());
    cx.update(|window, cx| {
        window.render_frame(cx);

        assert_eq!(window.find("vim-keybindings").checked(), Some(true));
        assert_eq!(
            window.find("preferences-dialog-content").focused(),
            Some(true)
        );
        assert!(window.has_active_dialog(cx));
        window.press("escape", cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        assert!(
            !window.has_active_dialog(cx),
            "one cancel must close the only preferences dialog"
        );
    });
}

#[gpui_kit::test]
fn apply_updates_every_open_editor_and_future_editor(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("yori").join("config.toml");
    let (workspace, cx) = harness_with_config(cx, Some(path.clone()));
    let temporary = tempfile::tempdir().unwrap();
    let second_left = temporary.path().join("second-left.txt");
    let second_right = temporary.path().join("second-right.txt");
    let third_left = temporary.path().join("third-left.txt");
    let third_right = temporary.path().join("third-right.txt");
    for file in [&second_left, &second_right, &third_left, &third_right] {
        std::fs::write(file, "text\n").unwrap();
    }

    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_paths(&second_left, &second_right, window, cx);
        });
    });
    let focused_editor = cx.update(|_, cx| active_editor(&workspace, cx));
    show_preferences(cx);

    std::fs::write(
        &path,
        "# external note\n[editor]\nvim_keybindings = false\nfuture_option = \"keep\"\n\n[plugin]\nenabled = true\n",
    )
    .unwrap();

    cx.update(|window, cx| {
        window.click("vim-keybindings", cx);
        window.click("show-whitespace", cx);
        window.click("show-change-connections", cx);
        window.click("preferences-apply", cx);
    });
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();

    let expected = crate::config::EditorConfig {
        vim_keybindings: true,
        show_whitespace: true,
        show_change_connections: true,
    };
    cx.update(|window, cx| {
        assert!(!window.has_active_dialog(cx));
        assert!(focused_editor.focus_handle(cx).is_focused(window));
        assert_eq!(crate::config::editor(cx), expected);
        for tab in &workspace.read(cx).tabs.entries {
            assert_eq!(tab.content.editor.read(cx).applied_preferences(), expected);
        }

        workspace.update(cx, |workspace, cx| {
            workspace.open_paths(&third_left, &third_right, window, cx);
        });
        assert_eq!(
            active_editor(&workspace, cx).read(cx).applied_preferences(),
            crate::config::editor(cx)
        );
    });

    let saved = std::fs::read_to_string(path).unwrap();
    assert!(saved.contains("# external note"));
    assert!(saved.contains("future_option = \"keep\""));
    assert!(saved.contains("[plugin]\nenabled = true"));
}

#[gpui_kit::test]
fn save_failure_keeps_dialog_selections_and_reports_an_accessible_error(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("yori").join("config.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[editor]\nvim_keybindings = false\n").unwrap();

    let (_, cx) = harness_with_config(cx, Some(path.clone()));
    show_preferences(cx);
    std::fs::write(&path, "[editor\nvim_keybindings = false").unwrap();
    cx.update(|window, cx| {
        window.click("vim-keybindings", cx);
        window.click("show-whitespace", cx);
        window.click("show-change-connections", cx);
        window.click("preferences-apply", cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.render_frame(cx);

        assert!(window.has_active_dialog(cx));
        assert_eq!(window.find("vim-keybindings").checked(), Some(true));
        assert_eq!(window.find("show-whitespace").checked(), Some(true));
        assert_eq!(window.find("show-change-connections").checked(), Some(true));
        assert!(
            crate::config::diagnostic(cx).is_some_and(|message| message.contains("invalid TOML")),
            "applying over a newly malformed configuration must retain the write error"
        );
        let error = window.find("preferences-error");
        assert_eq!(error.role(), Some(gpui_kit::Role::Alert));
        assert!(
            error
                .label()
                .is_some_and(|label| label.contains("invalid TOML"))
        );
        assert_eq!(
            crate::config::editor(cx),
            crate::config::EditorConfig::default()
        );
    });
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
        &[ComparisonPaths::diff(
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
        &[ComparisonPaths::diff(
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
            view.open_comparisons(
                &[ComparisonPaths::diff(left.clone(), right.clone())],
                window,
                cx,
            )
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
                &[ComparisonPaths::diff(
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
                    &[ComparisonPaths::diff(missing.clone(), missing.clone())],
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
                    &[ComparisonPaths::diff(missing.clone(), missing.clone())],
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
