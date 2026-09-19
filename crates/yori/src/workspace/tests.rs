//! Input-level regressions on GPUI's headless test platform; no pixel captures.

mod merging;
mod review;
mod saving;

use super::*;
use crate::comparison::ComparisonDocument;
use crate::review::GitCommitSummary;
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, point};
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

fn home_shortcut() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-shift-h"
    } else {
        "ctrl-shift-h"
    }
}

fn show_preferences(cx: &mut VisualTestContext) {
    cx.simulate_keystrokes(preferences_shortcut());
    cx.update(TestWindowExt::render_frame);
}

fn active_editor(workspace: &Entity<Workspace>, cx: &App) -> Entity<AlignedEditor> {
    let workspace = workspace.read(cx);
    let id = workspace.tabs.active.unwrap();

    workspace
        .tabs
        .get(id)
        .unwrap()
        .content
        .comparison()
        .expect("active test tab is a comparison")
        .editor
        .clone()
}

#[gpui_kit::test]
fn home_is_fixed_outside_work_tabs_and_preserves_the_last_session(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let temporary = tempfile::tempdir().unwrap();
    let left = temporary.path().join("left.txt");
    let right = temporary.path().join("right.txt");
    std::fs::write(&left, "baseline\n").unwrap();
    std::fs::write(&right, "local\n").unwrap();

    let (active, editor) = cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_paths(&left, &right, window, cx);
        });
        window.render_frame(cx);

        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at("rows-viewport", point(width * 0.75, px(11.0)), cx);
        window.input("changed ", cx);

        let active = workspace.read(cx).tabs.active.unwrap();
        (active, active_editor(&workspace, cx))
    });

    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("home-control", cx);
        window.render_frame(cx);

        let workspace = workspace.read(cx);
        assert_eq!(workspace.selection, WorkspaceSelection::Home);
        assert_eq!(workspace.tabs.entries.len(), 2);
        assert_eq!(workspace.tabs.active, Some(active));
        assert!(editor.read(cx).needs_save());
        assert_eq!(
            window.within("home-control").find(0usize).selected(),
            Some(true)
        );
        assert_eq!(window.find("home").focused(), Some(true));
    });

    cx.simulate_keystrokes("ctrl-w");
    cx.update(|_, cx| assert_eq!(workspace.read(cx).tabs.entries.len(), 2));

    cx.simulate_keystrokes("ctrl-tab");
    cx.update(|window, cx| {
        assert_eq!(workspace.read(cx).selection, WorkspaceSelection::Work);
        assert_eq!(workspace.read(cx).tabs.active, Some(active));
        assert!(editor.focus_handle(cx).is_focused(window));
        assert!(editor.read(cx).needs_save());
    });

    cx.simulate_keystrokes(home_shortcut());
    cx.update(|window, cx| {
        assert_eq!(workspace.read(cx).selection, WorkspaceSelection::Home);
        assert_eq!(window.find("home").focused(), Some(true));
    });
}

#[gpui_kit::test]
fn cycling_still_moves_only_between_work_tabs(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let temporary = tempfile::tempdir().unwrap();
    let left = temporary.path().join("left.txt");
    let right = temporary.path().join("right.txt");
    std::fs::write(&left, "baseline\n").unwrap();
    std::fs::write(&right, "local\n").unwrap();

    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_paths(&left, &right, window, cx);
            workspace.activate(0, window, cx);
        });
    });

    cx.simulate_keystrokes("ctrl-tab");
    cx.update(|_, cx| {
        assert_eq!(workspace.read(cx).selection, WorkspaceSelection::Work);
        assert_eq!(workspace.read(cx).tabs.active, Some(1));
    });

    cx.simulate_keystrokes("ctrl-shift-tab");
    cx.update(|_, cx| {
        assert_eq!(workspace.read(cx).selection, WorkspaceSelection::Work);
        assert_eq!(workspace.read(cx).tabs.active, Some(0));
    });
}

#[gpui_kit::test]
fn closing_the_final_work_tab_reveals_and_focuses_home(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);

    cx.update(|window, cx| window.click(("tab-close-target", 0usize), cx));
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.render_frame(cx);

        let workspace = workspace.read(cx);
        assert!(workspace.tabs.entries.is_empty());
        assert_eq!(workspace.tabs.active, None);
        assert_eq!(workspace.selection, WorkspaceSelection::Home);
        assert_eq!(
            window.within("home-control").find(0usize).selected(),
            Some(true)
        );
        assert_eq!(window.find("home").focused(), Some(true));
    });
}

#[gpui_kit::test]
fn home_exposes_exactly_the_initial_actions_and_keyboard_activation(cx: &mut TestAppContext) {
    let (workspace, cx) = empty_harness(cx);

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("home").focused(), Some(true));

        window.press("down", cx);
        assert_eq!(
            workspace.read(cx).home.read(cx).selected_action(),
            HomeAction::ReviewPerforceChangelist
        );
        window.press("j", cx);
        assert_eq!(
            workspace.read(cx).home.read(cx).selected_action(),
            HomeAction::CompareFiles
        );
        window.press("up", cx);
        window.press("k", cx);
        assert_eq!(
            workspace.read(cx).home.read(cx).selected_action(),
            HomeAction::ReviewGitChange
        );

        for _ in 0..4 {
            window.press("down", cx);
        }
        window.press("space", cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(workspace.read(cx).preferences.is_some());
        window.press("escape", cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("home").focused(), Some(true));
        window.press("enter", cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        assert!(window.has_active_dialog(cx));
        window.press("escape", cx);
    });
}

#[gpui_kit::test]
fn editor_keys_reach_the_editor_without_moving_the_hidden_home_selection(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let editor = cx.update(|_, cx| active_editor(&workspace, cx));

    cx.update(|window, cx| {
        let before = editor.read(cx).current_checkpoint().text;
        assert!(editor.focus_handle(cx).is_focused(window));
        let width = window.find("rows-viewport").bounds().size.width;
        window.click_at("rows-viewport", point(width * 0.75, px(11.0)), cx);

        window.input("j", cx);

        let after = editor.read(cx).current_checkpoint().text;
        assert_eq!(after.matches('j').count(), before.matches('j').count() + 1);
        assert_eq!(after.replacen('j', "", 1), before);
        assert_eq!(
            workspace.read(cx).home.read(cx).selected_action(),
            HomeAction::ReviewGitChange
        );
    });
}

#[gpui_kit::test]
fn home_git_action_opens_the_existing_source_chooser_and_restores_focus(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let repository = tempfile::tempdir().unwrap();
    gix::init(repository.path()).unwrap();
    std::fs::write(repository.path().join("change.txt"), "changed\n").unwrap();

    cx.update(|window, cx| {
        workspace.update(cx, |workspace, _| {
            workspace.invocation_directory = Some(repository.path().to_owned());
        });
        window.render_frame(cx);
        window.click("home-control", cx);
        window.render_frame(cx);
        window.click(("home-action", 0usize), cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(workspace.read(cx).git_source_chooser.is_some());
        assert_eq!(workspace.read(cx).tabs.entries.len(), 1);
        assert_eq!(window.find("git-source-chooser").focused(), Some(true));
        window.press("escape", cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(workspace.read(cx).git_source_chooser.is_none());
        assert_eq!(window.find("home").focused(), Some(true));
    });
}

#[gpui_kit::test]
fn home_perforce_action_opens_the_existing_changelist_chooser(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);

    cx.update(|window, cx| {
        workspace.update(cx, |workspace, _| {
            workspace.test_perforce_context = Some(review::perforce_context());
        });
        window.render_frame(cx);
        window.click("home-control", cx);
        window.render_frame(cx);
        window.click(("home-action", 1usize), cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.has_active_dialog(cx));
        assert_eq!(window.find("perforce-source-list").focused(), Some(true));
        assert_eq!(workspace.read(cx).tabs.entries.len(), 1);
    });
}

#[gpui_kit::test]
fn home_compare_action_uses_the_existing_file_flow_and_restores_focus(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);

    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("home-control", cx);
        window.render_frame(cx);
        window.click(("home-action", 2usize), cx);
    });
    cx.run_until_parked();
    assert!(cx.cx.did_prompt_for_paths());
    cx.cx.simulate_path_prompt_response(|options| {
        assert_eq!(options.prompt.as_deref(), Some("Select baseline file"));
        None
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(!workspace.read(cx).picking_files);
        assert_eq!(workspace.read(cx).tabs.entries.len(), 1);
        assert_eq!(window.find("home").focused(), Some(true));
    });
}

#[gpui_kit::test]
fn home_merge_action_uses_the_existing_three_way_flow(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);

    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("home-control", cx);
        window.render_frame(cx);
        window.click(("home-action", 3usize), cx);
    });
    cx.run_until_parked();
    assert!(cx.cx.did_prompt_for_paths());
    cx.cx.simulate_path_prompt_response(|options| {
        assert_eq!(
            options.prompt.as_deref(),
            Some("Select common ancestor (BASE)")
        );
        None
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(!workspace.read(cx).picking_files);
        assert_eq!(workspace.read(cx).tabs.entries.len(), 1);
        assert_eq!(window.find("home").focused(), Some(true));
    });
}

#[gpui_kit::test]
fn home_preferences_action_uses_the_existing_dialog_and_restores_focus(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);

    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("home-control", cx);
        window.render_frame(cx);
        window.click(("home-action", 4usize), cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(workspace.read(cx).preferences.is_some());
        assert_eq!(workspace.read(cx).tabs.entries.len(), 1);
        window.press("escape", cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(workspace.read(cx).preferences.is_none());
        assert_eq!(window.find("home").focused(), Some(true));
    });
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
            let editor = &tab
                .content
                .comparison()
                .expect("preference test opens comparison tabs only")
                .editor;

            assert_eq!(editor.read(cx).applied_preferences(), expected);
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
fn initial_and_forwarded_invocations_share_handling_and_update_the_directory(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = harness(cx);
    let window = cx.update(|window, _| window.window_handle().downcast::<Root>().unwrap());
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let repository_a = fixtures.join("repository-a");
    let repository_b = fixtures.join("repository-b");

    let initial = InvocationRequest::new(repository_a.clone(), Vec::new());
    crate::dispatch_invocation(window, &workspace, &initial, &mut cx.cx).unwrap();
    cx.update(|_, cx| {
        assert_eq!(
            workspace.read(cx).invocation_directory.as_ref(),
            Some(&repository_a)
        );
    });

    let forwarded = InvocationRequest::new(
        repository_b.clone(),
        vec![Comparison::diff(
            fixtures.join("intraline-before.rs"),
            fixtures.join("intraline-after.rs"),
        )],
    );
    crate::dispatch_invocation(window, &workspace, &forwarded, &mut cx.cx).unwrap();
    cx.update(|_, cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.invocation_directory.as_ref(), Some(&repository_b));
        assert_eq!(
            workspace.perforce_discovery_directory().as_ref(),
            Some(&repository_b),
            "Perforce discovery must route through the latest invocation directory"
        );
        assert_eq!(workspace.tabs.entries.len(), 2);
    });

    let invalid = InvocationRequest::new(
        repository_b,
        vec![Comparison::diff(
            fixtures.join("missing.rs"),
            fixtures.join("after.rs"),
        )],
    );
    let error = crate::dispatch_invocation(window, &workspace, &invalid, &mut cx.cx).unwrap_err();
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
fn close_button_and_middle_click_remove_clean_comparisons(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let temporary = tempfile::tempdir().unwrap();
    let left = temporary.path().join("left.txt");
    let right = temporary.path().join("right.txt");
    std::fs::write(&left, "baseline\n").unwrap();
    std::fs::write(&right, "local\n").unwrap();

    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_paths(&left, &right, window, cx);
        });
        window.render_frame(cx);
        window.click(("tab-close-target", 0usize), cx);
        window.render_frame(cx);
    });
    cx.run_until_parked();

    let middle_click = cx.update(|window, _| {
        let bounds = window.within("tabs-inner").find(0usize).bounds();
        point(
            bounds.left() + bounds.size.width / 2.0,
            bounds.top() + bounds.size.height / 2.0,
        )
    });
    cx.simulate_mouse_down(middle_click, MouseButton::Middle, Modifiers::default());
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert!(
            workspace.read(cx).tabs.entries.is_empty(),
            "both close affordances must remove their clean tab"
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

#[gpui_kit::test]
fn home_shows_recent_commits_only_when_there_is_a_repository_to_read(cx: &mut TestAppContext) {
    let (workspace, cx) = empty_harness(cx);

    cx.update(|window, cx| {
        window.render_frame(cx);
        // A plain folder has nothing to list, so Home collapses to one column.
        assert!(window.try_find(("home-commit", 0usize)).is_none());
        assert!(window.try_find("home-working-changes").is_none());

        let home = workspace.read(cx).home.clone();
        home.update(cx, |home, cx| {
            home.set_recent(
                Some(RecentCommits {
                    repository: "yori".to_owned(),
                    commits: vec![
                        GitCommitSummary {
                            revision: "a".repeat(40),
                            short_id: "aaaaaaa".to_owned(),
                            title: "first".to_owned(),
                            is_merge: false,
                            time_seconds: 0,
                        },
                        GitCommitSummary {
                            revision: "b".repeat(40),
                            short_id: "bbbbbbb".to_owned(),
                            title: "second".to_owned(),
                            is_merge: true,
                            time_seconds: 0,
                        },
                    ],
                }),
                cx,
            );
        });
        window.render_frame(cx);

        let _ = window.find("home-working-changes");
        let _ = window.find(("home-commit", 0usize));
        let _ = window.find(("home-commit", 1usize));
        assert!(window.try_find(("home-commit", 2usize)).is_none());
        home.update(cx, |home, cx| home.set_recent(None, cx));
        window.render_frame(cx);
        assert!(window.try_find(("home-commit", 0usize)).is_none());
        assert!(window.try_find("home-working-changes").is_none());
    });
}
