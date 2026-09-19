//! First-party merge opening through the same workspace and native prompts as diffs.

use super::*;

fn input_paths() -> MergePaths {
    let Comparison::Merge(paths) = merge_paths("result.rs") else {
        unreachable!();
    };

    paths
}

#[gpui_kit::test]
fn merge_handoff_loads_all_inputs_and_never_reads_or_writes_the_destination(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let fixtures = input_paths();
    let mut paths = fixtures.clone();
    paths.base = directory.path().join("ancestor.rs");
    paths.local = directory.path().join("mine.rs");
    paths.incoming = directory.path().join("theirs.rs");
    paths.result = directory.path().join("result.rs");
    for (source, target) in [
        (&fixtures.base, &paths.base),
        (&fixtures.local, &paths.local),
        (&fixtures.incoming, &paths.incoming),
    ] {
        std::fs::copy(source, target).unwrap();
    }
    // Not even valid source text: RESULT must not be parsed as a fourth input.
    let existing_output = b"existing output\0must survive";
    std::fs::write(&paths.result, existing_output).unwrap();
    let expected = yori_diff::merge::MergeSession::new(
        Document::read(&paths.base).unwrap(),
        Document::read(&paths.local).unwrap(),
        Document::read(&paths.incoming).unwrap(),
    )
    .unwrap();
    let request = InvocationRequest::new(
        directory.path().to_owned(),
        vec![Comparison::Merge(paths.clone())],
    );
    let window = cx.update(|window, _| window.window_handle().downcast::<Root>().unwrap());

    crate::dispatch_invocation(window, &workspace, &request, &mut cx.cx).unwrap();
    for path in [&paths.base, &paths.local, &paths.incoming] {
        std::fs::remove_file(path).unwrap();
    }

    // A sender may remove its temporary inputs immediately after the acknowledgment.
    cx.update(|window, cx| {
        window.render_frame(cx);
        let width = window.find("rows-viewport").bounds().size.width;
        for (fraction, expected) in [
            (0.15, expected.local().text()),
            (0.5, expected.result().text()),
            (0.9, expected.incoming().text()),
        ] {
            window.click_at("rows-viewport", point(width * fraction, px(11.0)), cx);
            assert_eq!(copy_active_text(window, cx), expected);
        }

        window.click_at("rows-viewport", point(width * 0.5, px(11.0)), cx);
        let editor = active_editor(&workspace, cx);
        assert_eq!(editor.read(cx).active_pane_index(), 1);
        window.press("ctrl-h", cx);
        assert_eq!(editor.read(cx).active_pane_index(), 0);
        window.press("ctrl-l", cx);
        assert_eq!(editor.read(cx).active_pane_index(), 1);
        window.press("ctrl-l", cx);
        assert_eq!(editor.read(cx).active_pane_index(), 2);
        window.press("ctrl-h", cx);
        assert_eq!(editor.read(cx).active_pane_index(), 1);

        window.input("edited", cx);
        window.press("ctrl-z", cx);
        assert_eq!(workspace.read(cx).tabs.entries.len(), 2);
    });
    assert_eq!(std::fs::read(&paths.result).unwrap(), existing_output);
}

#[gpui_kit::test]
fn invalid_merge_input_leaves_existing_tabs_and_disk_untouched(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let mut paths = input_paths();
    paths.incoming = directory.path().join("missing.rs");
    paths.result = directory.path().join("new-result.rs");

    cx.update(|window, cx| {
        let error = workspace
            .update(cx, |view, cx| {
                view.open_comparisons(&[Comparison::Merge(paths.clone())], window, cx)
            })
            .unwrap_err();
        assert!(error.contains("missing.rs"));
        assert_eq!(workspace.read(cx).tabs.entries.len(), 1);
        assert_eq!(workspace.read(cx).tabs.active, Some(0));
    });
    assert!(!paths.result.exists());
}

#[gpui_kit::test]
fn merge_picker_opens_real_paths_and_cancellation_at_every_stage_preserves_the_workspace(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let mut paths = input_paths();
    paths.result = directory.path().join("merged.rs");
    let inputs = [&paths.base, &paths.local, &paths.incoming];

    for cancel_at in 0..4 {
        cx.update(|window, cx| window.press("ctrl-shift-m", cx));
        cx.run_until_parked();

        for (stage, input) in inputs.iter().enumerate() {
            cx.cx.simulate_path_prompt_response(|_| {
                (stage != cancel_at).then(|| vec![(*input).clone()])
            });
            cx.run_until_parked();
            if stage == cancel_at {
                break;
            }
        }
        if cancel_at == 3 {
            cx.cx.simulate_new_path_selection(|_| None);
            cx.run_until_parked();
        }

        cx.update(|_, cx| {
            assert!(!workspace.read(cx).picking_files);
            assert_eq!(workspace.read(cx).tabs.entries.len(), 1);
            assert_eq!(workspace.read(cx).tabs.active, Some(0));
        });
        assert!(!paths.result.exists());
    }

    cx.update(|window, cx| window.click("open-merge", cx));
    cx.run_until_parked();
    for input in inputs {
        cx.cx
            .simulate_path_prompt_response(|_| Some(vec![input.clone()]));
        cx.run_until_parked();
    }
    cx.cx
        .simulate_new_path_selection(|_| Some(paths.result.clone()));
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert!(!workspace.read(cx).picking_files);
        assert_eq!(workspace.read(cx).tabs.entries.len(), 2);
        let id = workspace.read(cx).tabs.active.unwrap();
        assert_eq!(
            workspace
                .read(cx)
                .tabs
                .get(id)
                .unwrap()
                .identity
                .comparison()
                .unwrap(),
            &Comparison::Merge(paths.clone()).resolve().unwrap()
        );
    });
    assert!(!paths.result.exists());
}
