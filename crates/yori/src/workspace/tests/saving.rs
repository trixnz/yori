//! Native save/reload flows operate on disposable files, never repository fixtures.

use super::*;
use crate::workspace::files::Role;
use std::path::{Path, PathBuf};

fn open_diff(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    directory: &Path,
) -> (usize, PathBuf) {
    let baseline = directory.join("baseline.rs");
    let local = directory.join("local.rs");
    std::fs::write(&baseline, "base\r\n").unwrap();
    std::fs::write(&local, "local\r\n").unwrap();
    let id = cx.update(|window, cx| {
        workspace.update(cx, |view, cx| {
            view.open_comparisons(&[Comparison::diff(baseline, local.clone())], window, cx)
                .unwrap();
        });
        window.render_frame(cx);
        workspace.read(cx).tabs.active.unwrap()
    });
    cx.run_until_parked();

    (id, local)
}

fn select_pane(window: &mut Window, cx: &mut App, fraction: f32) {
    window.render_frame(cx);
    let width = window.find("rows-viewport").bounds().size.width;
    window.click_at("rows-viewport", point(width * fraction, px(11.0)), cx);
}

fn select_merge_input(window: &mut Window, cx: &mut App) {
    // The one-line conflict fixture starts with a display-only action row.
    window.render_frame(cx);
    let width = window.find("rows-viewport").bounds().size.width;
    window.click_at("rows-viewport", point(width * 0.9, px(35.0)), cx);
}

fn scan(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    cx.update(|_, cx| workspace.update(cx, Workspace::scan_disk));
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);
    cx.run_until_parked();
}

fn dialog_choice(cx: &mut VisualTestContext, id: &'static str) {
    cx.update(|window, cx| window.click(id, cx));
    // Closing one dialog and opening the next is a deferred UI transition.
    // Model separate input turns, rather than clicking both inside one update.
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);
    cx.run_until_parked();
}

fn editor(workspace: &Entity<Workspace>, id: usize, cx: &App) -> Entity<AlignedEditor> {
    workspace
        .read(cx)
        .tabs
        .get(id)
        .unwrap()
        .content
        .comparison()
        .unwrap()
        .editor
        .clone()
}

#[gpui_kit::test]
fn external_changes_require_acknowledgment_before_editing_or_switching_tabs(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let (id, local) = open_diff(&workspace, cx, directory.path());
    let bounds = cx.update(|window, cx| {
        select_pane(window, cx, 0.75);
        window.find("rows-viewport").bounds()
    });
    std::fs::write(&local, "external\n").unwrap();
    scan(&workspace, cx);
    cx.update(TestWindowExt::render_frame);
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.input("blocked", cx);
        window.press("escape", cx);
        window.input("also blocked", cx);
        window.press("ctrl-tab", cx);

        assert_eq!(workspace.read(cx).tabs.active, Some(id));
        assert_eq!(
            editor(&workspace, id, cx)
                .read(cx)
                .current_checkpoint()
                .text,
            "local\r\n"
        );
        assert_eq!(window.find("rows-viewport").bounds(), bounds);
        window.click("keep-current", cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| window.input("allowed", cx));
    cx.update(|_, cx| {
        assert!(
            editor(&workspace, id, cx)
                .read(cx)
                .current_checkpoint()
                .text
                .contains("allowed")
        );
    });
    assert_eq!(std::fs::read_to_string(local).unwrap(), "external\n");
}

#[gpui_kit::test]
fn disk_dialog_acknowledges_the_latest_displayed_version(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let (id, local) = open_diff(&workspace, cx, directory.path());
    cx.update(|window, cx| select_pane(window, cx, 0.75));

    std::fs::write(&local, "external one\n").unwrap();
    scan(&workspace, cx);
    std::fs::write(&local, "external two\n").unwrap();
    scan(&workspace, cx);
    dialog_choice(cx, "keep-current");
    cx.update(|window, cx| {
        window.input("allowed", cx);
        assert!(
            editor(&workspace, id, cx)
                .read(cx)
                .current_checkpoint()
                .text
                .contains("allowed")
        );
        assert!(
            workspace
                .read(cx)
                .tabs
                .get(id)
                .unwrap()
                .content
                .comparison()
                .unwrap()
                .files
                .notice()
                .is_none()
        );
    });
    assert_eq!(std::fs::read_to_string(local).unwrap(), "external two\n");
}

#[gpui_kit::test]
fn watcher_bursts_do_not_publish_a_temporary_missing_file(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let (id, local) = open_diff(&workspace, cx, directory.path());
    let (sender, events) = async_channel::bounded(1);
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.monitor = Some(Workspace::monitor_disk(events, window, cx));
        });
    });
    cx.run_until_parked();

    std::fs::rename(&local, directory.path().join("backup")).unwrap();
    sender.try_send(()).unwrap();
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(50));
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(
            workspace
                .read(cx)
                .tabs
                .get(id)
                .unwrap()
                .content
                .comparison()
                .unwrap()
                .files
                .notice()
                .is_none()
        );
    });

    std::fs::write(&local, "replacement\n").unwrap();
    sender.try_send(()).unwrap();
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(150));
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(150));
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);
    cx.run_until_parked();
    dialog_choice(cx, "reload-disk");

    cx.update(|_, cx| {
        assert_eq!(
            editor(&workspace, id, cx)
                .read(cx)
                .current_checkpoint()
                .text,
            "replacement\n"
        );
    });
}

#[gpui_kit::test]
fn a_recreated_file_can_be_reloaded_from_the_existing_disk_dialog(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let (id, local) = open_diff(&workspace, cx, directory.path());
    let backup = directory.path().join("local.rs~");

    // Model an editor moving the original aside before writing its replacement.
    std::fs::rename(&local, &backup).unwrap();
    scan(&workspace, cx);
    std::fs::write(&local, "replacement\n").unwrap();
    scan(&workspace, cx);
    dialog_choice(cx, "reload-disk");

    cx.update(|_, cx| {
        assert_eq!(
            editor(&workspace, id, cx)
                .read(cx)
                .current_checkpoint()
                .text,
            "replacement\n"
        );
        assert!(!editor(&workspace, id, cx).read(cx).needs_save());
    });
}

#[gpui_kit::test]
fn saving_keeps_the_editor_bounds_stable_on_every_input_frame(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    open_diff(&workspace, cx, directory.path());
    let bounds = cx.update(|window, cx| {
        select_pane(window, cx, 0.75);
        window.input("X", cx);
        let bounds = window.find("rows-viewport").bounds();

        window.press("ctrl-s", cx);
        assert_eq!(window.find("rows-viewport").bounds(), bounds);
        window.render_frame(cx);
        assert_eq!(window.find("rows-viewport").bounds(), bounds);
        bounds
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("rows-viewport").bounds(), bounds);
    });
}

#[gpui_kit::test]
fn saving_from_baseline_retains_focus_and_tracks_undo_against_saved_bytes(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let (id, local) = open_diff(&workspace, cx, directory.path());
    let expected = cx.update(|window, cx| {
        select_pane(window, cx, 0.75);
        window.input("X", cx);
        let expected = copy_active_text(window, cx);
        select_pane(window, cx, 0.25);
        window.press("ctrl-s", cx);
        expected
    });
    cx.run_until_parked();

    assert_eq!(std::fs::read_to_string(&local).unwrap(), expected);
    cx.update(|window, cx| {
        let editor = editor(&workspace, id, cx);
        assert!(!editor.read(cx).needs_save());
        assert_eq!(copy_active_text(window, cx), "base\r\n");

        window.press("ctrl-z", cx);
        assert!(editor.read(cx).needs_save());
        window.press("ctrl-shift-z", cx);
        assert!(!editor.read(cx).needs_save());
    });
}

#[gpui_kit::test]
fn save_checkpoint_does_not_clear_edits_made_after_its_snapshot(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let (id, _) = open_diff(&workspace, cx, directory.path());
    cx.update(|window, cx| {
        let editor = editor(&workspace, id, cx);
        select_pane(window, cx, 0.75);
        window.input("A", cx);
        let checkpoint = editor.update(cx, AlignedEditor::prepare_save).unwrap();
        window.input("B", cx);

        editor.update(cx, |editor, cx| editor.mark_saved(checkpoint, cx));
        assert!(editor.read(cx).needs_save());
        window.press("ctrl-z", cx);
        assert!(!editor.read(cx).needs_save());
    });
}

#[gpui_kit::test]
fn keep_current_never_approves_overwrite_and_approval_is_for_one_disk_version(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let (id, local) = open_diff(&workspace, cx, directory.path());
    std::fs::write(&local, "external one\n").unwrap();
    scan(&workspace, cx);

    cx.update(|_, cx| {
        assert_eq!(
            workspace
                .read(cx)
                .tabs
                .get(id)
                .unwrap()
                .content
                .comparison()
                .unwrap()
                .files
                .notice()
                .unwrap()
                .role,
            Role::Local
        );
    });
    dialog_choice(cx, "keep-current");
    cx.update(|window, cx| window.press("ctrl-s", cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.click("cancel", cx));
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&local).unwrap(), "external one\n");

    cx.update(|window, cx| window.press("ctrl-s", cx));
    cx.run_until_parked();
    std::fs::write(&local, "external two\n").unwrap();
    cx.update(|window, cx| window.click("ok", cx));
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&local).unwrap(), "external two\n");

    cx.update(|window, cx| window.click("ok", cx));
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&local).unwrap(), "local\r\n");
}

#[gpui_kit::test]
fn reload_requires_discard_for_local_edits_and_resets_only_local_history(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let (id, local) = open_diff(&workspace, cx, directory.path());
    cx.update(|window, cx| {
        select_pane(window, cx, 0.75);
        window.input("X", cx);
    });
    std::fs::write(&local, "external\n").unwrap();
    scan(&workspace, cx);

    dialog_choice(cx, "reload-disk");
    dialog_choice(cx, "cancel");
    cx.update(|_, cx| assert!(editor(&workspace, id, cx).read(cx).needs_save()));

    dialog_choice(cx, "reload-disk");
    dialog_choice(cx, "ok");

    cx.update(|window, cx| {
        select_pane(window, cx, 0.75);
        assert_eq!(copy_active_text(window, cx), "external\n");
        window.press("ctrl-z", cx);
        assert_eq!(copy_active_text(window, cx), "external\n");
        assert!(!editor(&workspace, id, cx).read(cx).needs_save());
    });
}

#[gpui_kit::test]
fn preferences_shortcut_is_ignored_during_a_delayed_save_and_close(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let (id, local) = open_diff(&workspace, cx, directory.path());
    let release = crate::storage::delay_next_save(&local);

    cx.update(|window, cx| {
        select_pane(window, cx, 0.75);
        window.input("X", cx);
        window.press("ctrl-w", cx);
        window.click("save-and-close", cx);

        assert!(workspace.read(cx).saving);
        assert!(!window.has_active_dialog(cx));
    });

    cx.update(|window, cx| {
        window.press(preferences_shortcut(), cx);

        assert!(workspace.read(cx).saving);
        assert!(workspace.read(cx).preferences.is_none());
        assert!(!window.has_active_dialog(cx));
        assert!(workspace.read(cx).tabs.get(id).is_some());
    });

    release.try_send(()).unwrap();
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert!(!workspace.read(cx).saving);
        assert!(workspace.read(cx).tabs.get(id).is_none());
    });
    assert!(std::fs::read_to_string(local).unwrap().contains('X'));
}

#[gpui_kit::test]
fn saving_on_close_succeeds_or_preserves_the_tab_on_failure(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let (id, local) = open_diff(&workspace, cx, directory.path());
    let writable_permissions = std::fs::metadata(&local).unwrap().permissions();
    let mut read_only_permissions = writable_permissions.clone();
    read_only_permissions.set_readonly(true);
    std::fs::set_permissions(&local, read_only_permissions).unwrap();
    cx.update(|window, cx| {
        select_pane(window, cx, 0.75);
        window.input("X", cx);
        window.press("ctrl-w", cx);
        window.click("save-and-close", cx);
    });
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert!(workspace.read(cx).tabs.get(id).is_some());
        assert!(editor(&workspace, id, cx).read(cx).needs_save());
    });
    assert_eq!(std::fs::read_to_string(&local).unwrap(), "local\r\n");
    std::fs::set_permissions(&local, writable_permissions).unwrap();
    cx.update(|window, cx| {
        window.press("ctrl-w", cx);
        window.click("save-and-close", cx);
    });
    cx.run_until_parked();

    cx.update(|_, cx| assert!(workspace.read(cx).tabs.get(id).is_none()));
    assert!(std::fs::read_to_string(&local).unwrap().contains('X'));
}

#[gpui_kit::test]
fn baseline_reload_preserves_local_edits_and_their_history(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let (id, _) = open_diff(&workspace, cx, directory.path());
    cx.update(|window, cx| {
        select_pane(window, cx, 0.75);
        window.input("X", cx);
    });
    std::fs::write(directory.path().join("baseline.rs"), "new baseline\n").unwrap();
    scan(&workspace, cx);

    cx.update(|window, cx| window.click("reload-disk", cx));
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(editor(&workspace, id, cx).read(cx).needs_save());
        select_pane(window, cx, 0.75);
        window.press("ctrl-z", cx);
        assert_eq!(copy_active_text(window, cx), "local\r\n");
        assert!(!editor(&workspace, id, cx).read(cx).needs_save());

        select_pane(window, cx, 0.25);
        assert_eq!(copy_active_text(window, cx), "new baseline\n");
    });
}

#[gpui_kit::test]
fn save_all_closes_the_window_only_after_every_changed_tab_is_written(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let (_, local_a) = open_diff(&workspace, cx, first.path());
    cx.update(|window, cx| {
        select_pane(window, cx, 0.75);
        window.input("A", cx);
    });
    let (_, local_b) = open_diff(&workspace, cx, second.path());
    cx.update(|window, cx| {
        select_pane(window, cx, 0.75);
        window.input("B", cx);
    });

    assert!(!cx.simulate_close());
    cx.update(|window, cx| window.click("save-and-close", cx));
    cx.run_until_parked();

    assert!(std::fs::read_to_string(local_a).unwrap().contains('A'));
    assert!(std::fs::read_to_string(local_b).unwrap().contains('B'));
    cx.cx.update(|cx| assert!(cx.windows().is_empty()));
}

#[gpui_kit::test]
fn a_save_in_another_tab_is_an_external_change_not_implicit_overwrite_approval(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let (first, local) = open_diff(&workspace, cx, directory.path());
    let baseline = directory.path().join("other-base.rs");
    std::fs::write(&baseline, "other baseline\n").unwrap();
    let second = cx.update(|window, cx| {
        workspace
            .update(cx, |view, cx| {
                view.open_comparisons(&[Comparison::diff(baseline, local.clone())], window, cx)
            })
            .unwrap();
        let second = workspace.read(cx).tabs.active.unwrap();
        workspace.update(cx, |view, cx| view.activate(first, window, cx));
        select_pane(window, cx, 0.75);
        window.input("X", cx);
        window.press("ctrl-s", cx);
        second
    });
    cx.run_until_parked();
    scan(&workspace, cx);

    cx.update(|_, cx| {
        let files = &workspace
            .read(cx)
            .tabs
            .get(second)
            .unwrap()
            .content
            .comparison()
            .unwrap()
            .files;
        assert_eq!(files.notice().unwrap().role, Role::Local);
        assert_eq!(workspace.read(cx).tabs.active, Some(second));
    });
    dialog_choice(cx, "keep-current");

    cx.update(|window, cx| {
        select_pane(window, cx, 0.75);
        assert_eq!(copy_active_text(window, cx), "local\r\n");
        window.press("ctrl-s", cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.click("cancel", cx));
    cx.run_until_parked();
    assert!(std::fs::read_to_string(local).unwrap().contains('X'));
}

fn open_merge(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    directory: &Path,
    conflicting: bool,
) -> (usize, MergePaths) {
    let paths = MergePaths {
        base: directory.join("base.rs"),
        local: directory.join("local.rs"),
        incoming: directory.join("incoming.rs"),
        result: directory.join("result.rs"),
    };
    std::fs::write(&paths.base, "base\n").unwrap();
    std::fs::write(&paths.local, "local\n").unwrap();
    std::fs::write(
        &paths.incoming,
        if conflicting { "incoming\n" } else { "base\n" },
    )
    .unwrap();
    let id = cx.update(|window, cx| {
        workspace
            .update(cx, |view, cx| {
                view.open_comparisons(&[Comparison::Merge(paths.clone())], window, cx)
            })
            .unwrap();
        window.render_frame(cx);
        workspace.read(cx).tabs.active.unwrap()
    });
    cx.run_until_parked();

    (id, paths)
}

#[gpui_kit::test]
fn merge_save_requires_resolution_and_a_new_automatic_result_is_saveable(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let (id, paths) = open_merge(&workspace, cx, directory.path(), true);
    cx.update(|window, cx| window.press("ctrl-s", cx));
    cx.run_until_parked();
    assert!(!paths.result.exists());

    cx.update(|window, cx| {
        window.click(("merge-incoming-button", 0usize), cx);
        select_merge_input(window, cx);
        window.press("ctrl-s", cx);
    });
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(&paths.result).unwrap(),
        "incoming\n"
    );
    cx.update(|window, cx| {
        let editor = editor(&workspace, id, cx);
        assert!(!editor.read(cx).needs_save());
        window.press("ctrl-z", cx);
        assert!(editor.read(cx).needs_save());
        assert_eq!(editor.read(cx).unresolved_count(), 1);
        window.press("ctrl-shift-z", cx);
        assert!(!editor.read(cx).needs_save());
    });

    let automatic = tempfile::tempdir().unwrap();
    let (id, paths) = open_merge(&workspace, cx, automatic.path(), false);
    cx.update(|window, cx| {
        assert!(editor(&workspace, id, cx).read(cx).needs_save());
        assert!(!editor(&workspace, id, cx).read(cx).is_dirty());
        window.press("ctrl-s", cx);
    });
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&paths.result).unwrap(), "local\n");
}

#[gpui_kit::test]
fn changed_merge_inputs_restart_explicitly_but_disappearing_inputs_keep_the_session(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let (id, paths) = open_merge(&workspace, cx, directory.path(), true);
    cx.update(|window, cx| window.click(("merge-incoming-button", 0usize), cx));
    std::fs::write(&paths.incoming, "different incoming\n").unwrap();
    scan(&workspace, cx);

    dialog_choice(cx, "reload-disk");
    dialog_choice(cx, "ok");
    cx.update(|window, cx| {
        assert_eq!(editor(&workspace, id, cx).read(cx).unresolved_count(), 1);
        select_merge_input(window, cx);
        assert_eq!(copy_active_text(window, cx), "different incoming\n");
    });

    std::fs::remove_file(&paths.base).unwrap();
    std::fs::remove_file(&paths.incoming).unwrap();
    scan(&workspace, cx);
    cx.update(|window, cx| {
        assert!(
            workspace
                .read(cx)
                .tabs
                .get(id)
                .unwrap()
                .content
                .comparison()
                .unwrap()
                .files
                .notice()
                .is_none()
        );
        select_merge_input(window, cx);
        assert_eq!(copy_active_text(window, cx), "different incoming\n");
    });
}
