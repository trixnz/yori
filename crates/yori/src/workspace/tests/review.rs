//! Provider-neutral review-session behavior through the native workspace.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use gpui_kit::test::TestWindowExt;
use gpui_kit::{App, Entity, Role, VisualTestContext, point, px};

use super::*;
use crate::{
    comparison::ComparisonDocument,
    review::{
        ReviewSession,
        model::{
            ReviewFile, ReviewFileIdentity, ReviewFileStatus, ReviewManifest, ReviewProvider,
            ReviewSource, ReviewSourceIdentity, TextComparison,
        },
    },
};

struct TestProvider {
    manifests: Mutex<VecDeque<Result<ReviewManifest, String>>>,
    loads: AtomicUsize,
}

impl TestProvider {
    fn new(manifests: impl IntoIterator<Item = ReviewManifest>) -> Arc<Self> {
        Arc::new(Self {
            manifests: Mutex::new(manifests.into_iter().map(Ok).collect()),
            loads: AtomicUsize::new(0),
        })
    }

    fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }
}

impl ReviewProvider for TestProvider {
    fn load_manifest(&self, _: &ReviewSourceIdentity) -> Result<ReviewManifest, String> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        self.manifests
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err("unexpected provider load".into()))
    }
}

fn source(provider: Arc<TestProvider>, key: &str) -> ReviewSource {
    ReviewSource::new(
        ReviewSourceIdentity::new("test", key),
        "Working changes",
        provider,
    )
}

fn text_file(identity: &str, path: &str, baseline: &str, local: &str) -> ReviewFile {
    let path = PathBuf::from(path);
    let comparison = TextComparison::new(
        ComparisonDocument::read_only_memory(path.clone(), baseline.as_bytes().to_vec()),
        ComparisonDocument::editable_memory(path.clone(), local.as_bytes().to_vec(), None),
    )
    .unwrap();

    ReviewFile::text(
        ReviewFileIdentity::new(identity),
        path,
        ReviewFileStatus::Modified,
        comparison,
    )
}

fn saveable_text_file(
    identity: &str,
    logical_path: &str,
    baseline: &str,
    local: &str,
    destination: &Path,
) -> ReviewFile {
    let logical_path = PathBuf::from(logical_path);
    let comparison = TextComparison::new(
        ComparisonDocument::read_only_memory(logical_path.clone(), baseline.as_bytes().to_vec()),
        ComparisonDocument::editable_memory(
            logical_path.clone(),
            local.as_bytes().to_vec(),
            Some(destination.to_owned()),
        ),
    )
    .unwrap();

    ReviewFile::text(
        ReviewFileIdentity::new(identity),
        logical_path,
        ReviewFileStatus::Modified,
        comparison,
    )
}

fn open_review(
    workspace: &Entity<Workspace>,
    source: ReviewSource,
    cx: &mut VisualTestContext,
) -> (usize, Entity<ReviewSession>) {
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_review_source(source, window, cx);
        });
        window.render_frame(cx);
    });
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);

    cx.update(|_, cx| {
        let id = workspace.read(cx).tabs.active.unwrap();
        let OpenTab::Review { session, .. } = &workspace.read(cx).tabs.get(id).unwrap().content
        else {
            panic!("active tab should be a review session");
        };

        (id, session.clone())
    })
}

fn edit_active(window: &mut Window, cx: &mut App, text: &str) {
    window.render_frame(cx);
    let width = window.find("rows-viewport").bounds().size.width;
    window.click_at("rows-viewport", point(width * 0.75, px(11.0)), cx);
    window.input(text, cx);
}

#[gpui_kit::test]
fn source_identity_deduplicates_and_navigation_lazily_retains_editors(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let first = ReviewFileIdentity::new("first");
    let second = ReviewFileIdentity::new("second");
    let manifest = ReviewManifest::new(vec![
        ReviewFile::binary(
            ReviewFileIdentity::new("binary"),
            "assets/image.png".into(),
            ReviewFileStatus::Added,
            "Binary image content cannot be displayed.",
        ),
        text_file("first", "src/first.rs", "old\n", "new\n"),
        text_file("second", "src/second.rs", "before\n", "after\n"),
    ])
    .unwrap();
    let provider = TestProvider::new([manifest]);
    let review_source = source(provider.clone(), "dedupe");
    let (id, session) = open_review(&workspace, review_source.clone(), cx);

    cx.update(|window, cx| {
        assert_eq!(session.read(cx).selected_identity(), Some(&first));
        assert_eq!(session.read(cx).editor_count(), 1);
        let first_editor = session.read(cx).editor(&first).unwrap();

        assert!(window.try_find("review-file-filter").is_none());
        assert!(window.try_find("review-files-header").is_some());
        assert!(window.try_find("refresh-review").is_some());

        window.click(("review-file", 2usize), cx);
        assert_eq!(session.read(cx).selected_identity(), Some(&second));
        assert_eq!(session.read(cx).editor_count(), 2);

        window.click(("review-file", 1usize), cx);
        assert_eq!(session.read(cx).editor(&first).unwrap(), first_editor);

        workspace.update(cx, |workspace, cx| {
            workspace.open_review_source(review_source, window, cx);
        });
        assert_eq!(workspace.read(cx).tabs.active, Some(id));
        assert_eq!(workspace.read(cx).tabs.entries.len(), 2);
    });
    cx.run_until_parked();

    assert_eq!(provider.loads(), 1, "duplicate opening must not reload");
}

#[gpui_kit::test]
fn refresh_keeps_dirty_removed_files_and_drops_clean_removed_files(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let kept = ReviewFileIdentity::new("kept");
    let removed = ReviewFileIdentity::new("removed");
    let initial = ReviewManifest::new(vec![
        text_file("removed", "src/removed.rs", "old\n", "local\n"),
        text_file("kept", "src/kept.rs", "before\n", "after\n"),
    ])
    .unwrap();
    let refreshed = ReviewManifest::new(vec![text_file(
        "kept",
        "src/kept.rs",
        "new baseline\n",
        "after\n",
    )])
    .unwrap();
    let provider = TestProvider::new([initial, refreshed]);
    let (_, session) = open_review(&workspace, source(provider.clone(), "dirty-removal"), cx);

    let removed_editor = cx.update(|window, cx| {
        edit_active(window, cx, "dirty ");
        let editor = session.read(cx).editor(&removed).unwrap();
        window.click("refresh-review", cx);
        editor
    });
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);

    cx.update(|window, cx| {
        assert!(session.read(cx).is_removed(&removed));
        assert_eq!(session.read(cx).editor(&removed).unwrap(), removed_editor);
        assert!(removed_editor.read(cx).needs_save());
        assert_eq!(session.read(cx).selected_identity(), Some(&removed));

        window.click(("review-file", 1usize), cx);
        assert_eq!(session.read(cx).selected_identity(), Some(&removed));
        window.click(("review-file", 0usize), cx);
        assert_eq!(session.read(cx).selected_identity(), Some(&kept));
    });
    assert_eq!(provider.loads(), 2);

    let clean_initial = ReviewManifest::new(vec![
        text_file("removed", "src/removed.rs", "old\n", "local\n"),
        text_file("kept", "src/kept.rs", "before\n", "after\n"),
    ])
    .unwrap();
    let clean_refreshed = ReviewManifest::new(vec![text_file(
        "kept",
        "src/kept.rs",
        "before\n",
        "after\n",
    )])
    .unwrap();
    let clean_provider = TestProvider::new([clean_initial, clean_refreshed]);
    let (_, clean_session) = open_review(&workspace, source(clean_provider, "clean-removal"), cx);

    cx.update(|window, cx| window.click("refresh-review", cx));
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(clean_session.read(cx).selected_identity(), Some(&kept));
        assert_eq!(clean_session.read(cx).editor_count(), 1);
        assert!(clean_session.read(cx).editor(&removed).is_none());
    });
}

#[gpui_kit::test]
fn dirty_text_to_binary_refresh_retains_then_resolves_with_navigator_focus(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = harness(cx);
    let identity = ReviewFileIdentity::new("changed-kind");
    let initial = ReviewManifest::new(vec![text_file(
        "changed-kind",
        "src/data.txt",
        "before\n",
        "after\n",
    )])
    .unwrap();
    let refreshed = ReviewManifest::new(vec![ReviewFile::binary(
        identity.clone(),
        "src/data.txt".into(),
        ReviewFileStatus::Modified,
        "Binary content cannot be displayed.",
    )])
    .unwrap();
    let provider = TestProvider::new([initial, refreshed]);
    let (_, session) = open_review(&workspace, source(provider, "dirty-kind-change"), cx);

    let editor = cx.update(|window, cx| {
        edit_active(window, cx, "dirty ");
        let editor = session.read(cx).editor(&identity).unwrap();
        window.click("refresh-review", cx);
        editor
    });
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);

    cx.update(|window, cx| {
        assert_eq!(session.read(cx).editor(&identity), Some(editor.clone()));
        assert!(editor.read(cx).needs_save());
        assert_eq!(
            session.read(cx).warning(&identity),
            Some("now binary upstream")
        );
        assert_eq!(session.read(cx).selected_identity(), Some(&identity));
        assert!(
            window
                .find("review-file-warning")
                .label()
                .unwrap()
                .contains("binary")
        );
        assert!(editor.focus_handle(cx).contains_focused(window, cx));

        let checkpoint = editor.read(cx).current_checkpoint();
        editor.update(cx, |editor, cx| editor.mark_saved(checkpoint, cx));
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(session.read(cx).editor(&identity).is_none());
        assert_eq!(session.read(cx).warning(&identity), None);
        assert_eq!(window.find("review-file-list").focused(), Some(true));
    });
}

#[gpui_kit::test]
fn hidden_kind_change_resolution_does_not_steal_comparison_focus(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let identity = ReviewFileIdentity::new("changed-kind");
    let initial = ReviewManifest::new(vec![text_file(
        "changed-kind",
        "vendor/library",
        "old\n",
        "new\n",
    )])
    .unwrap();
    let refreshed = ReviewManifest::new(vec![ReviewFile::submodule(
        identity.clone(),
        "vendor/library".into(),
        ReviewFileStatus::Modified,
        "1111111",
        "2222222",
    )])
    .unwrap();
    let provider = TestProvider::new([initial, refreshed]);
    let (_, session) = open_review(&workspace, source(provider, "hidden-kind-resolution"), cx);

    let (editor, comparison) = cx.update(|window, cx| {
        edit_active(window, cx, "dirty ");
        let editor = session.read(cx).editor(&identity).unwrap();
        window.click("refresh-review", cx);

        let comparison = workspace
            .read(cx)
            .tabs
            .get(0)
            .unwrap()
            .content
            .comparison()
            .unwrap()
            .editor
            .clone();
        (editor, comparison)
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| workspace.activate(0, window, cx));
        assert!(comparison.focus_handle(cx).contains_focused(window, cx));

        let checkpoint = editor.read(cx).current_checkpoint();
        editor.update(cx, |editor, cx| editor.mark_saved(checkpoint, cx));
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(session.read(cx).editor(&identity).is_none());
        assert!(comparison.focus_handle(cx).contains_focused(window, cx));
    });
}

#[gpui_kit::test]
fn clean_text_to_submodule_refresh_evicts_the_incompatible_editor(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let identity = ReviewFileIdentity::new("changed-kind");
    let initial = ReviewManifest::new(vec![text_file(
        "changed-kind",
        "vendor/library",
        "old\n",
        "new\n",
    )])
    .unwrap();
    let refreshed = ReviewManifest::new(vec![ReviewFile::submodule(
        identity.clone(),
        "vendor/library".into(),
        ReviewFileStatus::Modified,
        "1111111",
        "2222222",
    )])
    .unwrap();
    let provider = TestProvider::new([initial, refreshed]);
    let (_, session) = open_review(&workspace, source(provider, "clean-kind-change"), cx);

    cx.update(|window, cx| window.click("refresh-review", cx));
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);

    cx.update(|window, cx| {
        assert_eq!(session.read(cx).selected_identity(), Some(&identity));
        assert_eq!(session.read(cx).editor_count(), 0);
        assert!(session.read(cx).editor(&identity).is_none());
        assert_eq!(session.read(cx).warning(&identity), None);
        assert_eq!(window.find("review-file-list").role(), Some(Role::ListBox));
    });
}

#[gpui_kit::test]
fn background_review_load_does_not_steal_focus_from_the_active_comparison(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let manifest = ReviewManifest::new(vec![text_file(
        "file",
        "src/file.rs",
        "before\n",
        "after\n",
    )])
    .unwrap();
    let provider = TestProvider::new([manifest]);

    let (session, comparison) = cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_review_source(source(provider, "background-focus"), window, cx);
        });
        let review_id = workspace.read(cx).tabs.active.unwrap();
        let OpenTab::Review { session, .. } =
            &workspace.read(cx).tabs.get(review_id).unwrap().content
        else {
            panic!("new tab should be a review session");
        };
        let session = session.clone();
        let comparison = workspace
            .read(cx)
            .tabs
            .get(0)
            .unwrap()
            .content
            .comparison()
            .unwrap()
            .editor
            .clone();

        workspace.update(cx, |workspace, cx| workspace.activate(0, window, cx));
        assert!(comparison.focus_handle(cx).is_focused(window));

        (session, comparison)
    });
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);

    cx.update(|window, cx| {
        assert_eq!(session.read(cx).editor_count(), 1);
        assert_eq!(workspace.read(cx).tabs.active, Some(0));
        assert!(comparison.focus_handle(cx).is_focused(window));
    });
}

#[gpui_kit::test]
fn navigator_exposes_list_selection_and_activates_from_the_keyboard(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let binary = ReviewFileIdentity::new("binary");
    let text = ReviewFileIdentity::new("text");
    let manifest = ReviewManifest::new(vec![
        ReviewFile::binary(
            binary.clone(),
            "assets/data.bin".into(),
            ReviewFileStatus::Modified,
            "Binary content cannot be displayed.",
        ),
        text_file("text", "src/file.rs", "before\n", "after\n"),
    ])
    .unwrap();
    let (_, session) = open_review(
        &workspace,
        source(TestProvider::new([manifest]), "keyboard-navigation"),
        cx,
    );

    cx.update(|window, cx| {
        session.update(cx, |session, cx| session.focus_navigator(window, cx));
        window.render_frame(cx);

        assert_eq!(window.find("review-file-list").role(), Some(Role::ListBox));
        assert_eq!(window.find("review-file-list").focused(), Some(true));
        assert_eq!(
            window.find(("review-file", 1usize)).role(),
            Some(Role::ListBoxOption)
        );
        assert_eq!(window.find(("review-file", 1usize)).selected(), Some(true));

        window.press("up", cx);
        window.render_frame(cx);
        assert_eq!(window.find(("review-file", 0usize)).selected(), Some(true));
        assert_eq!(session.read(cx).selected_identity(), Some(&text));

        window.press("j", cx);
        window.render_frame(cx);
        assert_eq!(window.find(("review-file", 1usize)).selected(), Some(true));
        window.press("k", cx);
        window.render_frame(cx);
        assert_eq!(window.find(("review-file", 0usize)).selected(), Some(true));

        window.press("enter", cx);
        window.render_frame(cx);
        assert_eq!(session.read(cx).selected_identity(), Some(&binary));
        assert_eq!(window.find("review-file-body").focused(), Some(true));

        window.press("h", cx);
        window.render_frame(cx);
        assert_eq!(window.find("review-file-list").focused(), Some(true));
        window.press("l", cx);
        window.render_frame(cx);
        assert_eq!(window.find("review-file-body").focused(), Some(true));
        window.press("left", cx);
        window.render_frame(cx);
        assert_eq!(window.find("review-file-list").focused(), Some(true));
        window.press("right", cx);
        window.render_frame(cx);
        assert_eq!(window.find("review-file-body").focused(), Some(true));

        session.update(cx, |session, cx| session.focus_navigator(window, cx));
        window.press("down", cx);
        window.press("space", cx);
        window.render_frame(cx);
        assert_eq!(session.read(cx).selected_identity(), Some(&text));
        let editor = session.read(cx).editor(&text).unwrap();
        assert!(editor.focus_handle(cx).contains_focused(window, cx));

        window.press("h", cx);
        assert!(editor.focus_handle(cx).contains_focused(window, cx));
        assert_eq!(window.find("review-file-list").focused(), Some(false));
    });
}

#[gpui_kit::test]
fn navigator_keyboard_selection_scrolls_beyond_one_viewport(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let files = (0..40)
        .map(|index| {
            ReviewFile::binary(
                ReviewFileIdentity::new(format!("binary-{index}")),
                format!("assets/file-{index:02}.bin").into(),
                ReviewFileStatus::Modified,
                "Binary content cannot be displayed.",
            )
        })
        .collect();
    let manifest = ReviewManifest::new(files).unwrap();
    let (_, session) = open_review(
        &workspace,
        source(TestProvider::new([manifest]), "scroll-navigation"),
        cx,
    );

    cx.update(|window, cx| {
        session.update(cx, |session, cx| session.focus_navigator(window, cx));
        for _ in 0..30 {
            window.press("j", cx);
        }
        window.render_frame(cx);
        window.render_frame(cx);

        let list = window.find("review-file-list").bounds();
        let selected = window.find(("review-file", 29usize));
        assert_eq!(selected.selected(), Some(true));
        assert!(selected.bounds().bottom() > list.top());
        assert!(
            selected.bounds().top() < list.bottom(),
            "selected {:?} must intersect list {:?}; scrolled={}",
            selected.bounds(),
            list,
            session.read(cx).navigator_is_scrolled(),
        );
        assert!(session.read(cx).navigator_is_scrolled());

        window.press("k", cx);
        window.render_frame(cx);
        assert_eq!(window.find(("review-file", 28usize)).selected(), Some(true));
    });
}

#[gpui_kit::test]
fn navigator_header_and_status_badges_render_without_a_filter(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let manifest = ReviewManifest::new(vec![
        ReviewFile::binary(
            ReviewFileIdentity::new("added"),
            "added.bin".into(),
            ReviewFileStatus::Added,
            "Binary content cannot be displayed.",
        ),
        ReviewFile::binary(
            ReviewFileIdentity::new("modified"),
            "modified.bin".into(),
            ReviewFileStatus::Modified,
            "Binary content cannot be displayed.",
        ),
        ReviewFile::binary(
            ReviewFileIdentity::new("deleted"),
            "deleted.bin".into(),
            ReviewFileStatus::Deleted,
            "Binary content cannot be displayed.",
        ),
        ReviewFile::binary(
            ReviewFileIdentity::new("renamed"),
            "renamed.bin".into(),
            ReviewFileStatus::Renamed {
                from: "old.bin".into(),
            },
            "Binary content cannot be displayed.",
        ),
    ])
    .unwrap();
    open_review(
        &workspace,
        source(TestProvider::new([manifest]), "status-header"),
        cx,
    );

    cx.update(|window, _| {
        assert!(window.try_find("review-file-filter").is_none());
        let header = window.find("review-files-header").bounds();
        let refresh = window.find("refresh-review").bounds();
        assert!(refresh.top() >= header.top());
        assert!(refresh.bottom() <= header.bottom());

        for (index, status) in ["Added", "Modified", "Deleted", "Renamed"]
            .into_iter()
            .enumerate()
        {
            assert!(window.try_find(("review-status-badge", index)).is_some());
            assert!(
                window
                    .find(("review-file", index))
                    .label()
                    .unwrap()
                    .starts_with(status)
            );
        }
    });
}

#[gpui_kit::test]
fn refresh_updates_in_place_without_transient_layout_or_focus_change(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let identity = ReviewFileIdentity::new("file");
    let initial = ReviewManifest::new(vec![text_file(
        "file",
        "src/file.rs",
        "before\n",
        "after\n",
    )])
    .unwrap();
    let refreshed = ReviewManifest::new(vec![text_file(
        "file",
        "src/file.rs",
        "new baseline\n",
        "updated\n",
    )])
    .unwrap();
    let provider = TestProvider::new([initial, refreshed]);
    let (_, session) = open_review(&workspace, source(provider, "in-place-refresh"), cx);
    let editor = cx.update(|window, cx| {
        session.update(cx, |session, cx| session.focus_navigator(window, cx));
        window.render_frame(cx);

        let editor = session.read(cx).editor(&identity).unwrap();
        let header = window.find("review-files-header").bounds();
        let list = window.find("review-file-list").bounds();
        let body = window.find("review-file-body").bounds();

        window.click("refresh-review", cx);
        window.render_frame(cx);

        assert_eq!(window.find("review-files-header").bounds(), header);
        assert_eq!(window.find("review-file-list").bounds(), list);
        assert_eq!(window.find("review-file-body").bounds(), body);
        assert_eq!(window.find("review-file-list").focused(), Some(true));
        editor
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(editor.read(cx).current_checkpoint().text, "updated\n");
        assert_eq!(window.find("review-file-list").focused(), Some(true));
    });
}

#[gpui_kit::test]
fn activation_refreshes_once_without_provider_polling(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let first = ReviewManifest::new(vec![text_file(
        "file",
        "src/file.rs",
        "before\n",
        "after\n",
    )])
    .unwrap();
    let second = first.clone();
    let provider = TestProvider::new([first, second]);
    open_review(&workspace, source(provider.clone(), "activation"), cx);

    cx.executor()
        .advance_clock(std::time::Duration::from_secs(60));
    cx.run_until_parked();
    assert_eq!(provider.loads(), 1, "review sources must never be polled");

    cx.deactivate_window();
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();

    assert_eq!(provider.loads(), 2);
}

#[gpui_kit::test]
fn empty_binary_submodule_and_rename_states_remain_navigable(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let empty_provider = TestProvider::new([ReviewManifest::default()]);
    let (_, empty) = open_review(&workspace, source(empty_provider, "empty"), cx);
    cx.update(|window, cx| {
        assert!(empty.read(cx).selected_identity().is_none());
        assert!(window.try_find("rows-viewport").is_none());
    });

    let binary = ReviewFileIdentity::new("binary");
    let submodule = ReviewFileIdentity::new("submodule");
    let manifest = ReviewManifest::new(vec![
        ReviewFile::binary(
            binary.clone(),
            "assets/data.bin".into(),
            ReviewFileStatus::Renamed {
                from: "assets/old-data.bin".into(),
            },
            "Binary content cannot be displayed.",
        ),
        ReviewFile::submodule(
            submodule.clone(),
            "vendor/library".into(),
            ReviewFileStatus::Modified,
            "1111111",
            "2222222",
        ),
    ])
    .unwrap();
    let (_, session) = open_review(
        &workspace,
        source(TestProvider::new([manifest]), "non-text"),
        cx,
    );

    cx.update(|window, cx| {
        assert!(session.read(cx).selected_identity().is_none());
        window.click(("review-file", 0usize), cx);
        assert_eq!(session.read(cx).selected_identity(), Some(&binary));
        assert_eq!(session.read(cx).editor_count(), 0);

        window.click(("review-file", 1usize), cx);
        assert_eq!(session.read(cx).selected_identity(), Some(&submodule));
        assert_eq!(session.read(cx).editor_count(), 0);
    });
}

fn perforce_context() -> Arc<PerforceContext> {
    let root = PathBuf::from("/work/robin-yori");
    let info = yori_p4::ClientInfo {
        server_address: "ssl:perforce.example:1666".into(),
        server_version: "P4D/test".into(),
        user_name: "robin".into(),
        client_name: "robin-yori".into(),
        client_root: Some(root.clone()),
        current_directory: root,
        case_handling: Some("sensitive".into()),
        unicode_enabled: true,
    };
    let default = yori_p4::ChangelistSummary {
        id: yori_p4::ChangelistId::Default,
        status: yori_p4::ChangelistStatus::Pending,
        description: "Default changelist".into(),
        user: "robin".into(),
        client: "robin-yori".into(),
        modified_unix_seconds: None,
    };
    let pending_number = std::num::NonZeroU32::new(42).unwrap();
    let pending = yori_p4::ChangelistSummary {
        id: yori_p4::ChangelistId::Number(pending_number),
        status: yori_p4::ChangelistStatus::Pending,
        description: "Pending work".into(),
        user: "robin".into(),
        client: "robin-yori".into(),
        modified_unix_seconds: None,
    };
    let mut numbered = vec![pending];
    numbered.extend((1..=40).map(|number| yori_p4::ChangelistSummary {
        id: yori_p4::ChangelistId::Number(std::num::NonZeroU32::new(number).unwrap()),
        status: yori_p4::ChangelistStatus::Pending,
        description: format!("Pending work {number}"),
        user: "robin".into(),
        client: "robin-yori".into(),
        modified_unix_seconds: None,
    }));
    let submitted_number = std::num::NonZeroU32::new(41).unwrap();
    let submitted = yori_p4::ChangelistSummary {
        id: yori_p4::ChangelistId::Number(submitted_number),
        status: yori_p4::ChangelistStatus::Submitted,
        description: "Submitted work".into(),
        user: "robin".into(),
        client: "robin-yori".into(),
        modified_unix_seconds: None,
    };
    let descriptions = std::collections::HashMap::from([(
        submitted_number,
        yori_p4::ChangelistDescription {
            summary: submitted.clone(),
            files: Vec::new(),
        },
    )]);

    Arc::new(PerforceContext::for_test(
        info,
        yori_p4::PendingChangelists { default, numbered },
        vec![submitted],
        descriptions,
    ))
}

#[gpui_kit::test]
fn perforce_source_chooser_is_keyboard_first_without_stealing_input_keys(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let chooser = cx.update(|window, cx| {
        let chooser = workspace.update(cx, |_, cx| {
            Workspace::open_perforce_chooser(perforce_context(), window, cx)
        });
        window.render_frame(cx);
        chooser
    });
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);

    cx.update(|window, cx| {
        assert_eq!(window.find("perforce-source-list").focused(), Some(true));
        assert_eq!(
            window.find(("perforce-source", 0usize)).selected(),
            Some(true)
        );

        window.press("down", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find(("perforce-source", 1usize)).selected(),
            Some(true)
        );
        window.press("k", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find(("perforce-source", 0usize)).selected(),
            Some(true)
        );

        for _ in 0..30 {
            window.press("j", cx);
        }
        window.render_frame(cx);
        window.render_frame(cx);
        assert_eq!(
            window.find(("perforce-source", 30usize)).selected(),
            Some(true)
        );
        assert!(chooser.read(cx).list_is_scrolled());

        window.click("perforce-recent", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find(("perforce-source", 0usize)).selected(),
            Some(true)
        );

        window.click("perforce-number", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("perforce-changelist-number").focused(),
            Some(true)
        );
        window.press("j", cx);
        assert_eq!(chooser.read(cx).number_text(cx), "j");

        window.press(
            if cfg!(target_os = "macos") {
                "cmd-a"
            } else {
                "ctrl-a"
            },
            cx,
        );
        window.press("backspace", cx);
        window.input("41", cx);
        window.press("enter", cx);
    });
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);

    cx.update(|window, cx| {
        assert!(!window.has_active_dialog(cx));
        let active = workspace.read(cx).tabs.active.unwrap();
        assert!(matches!(
            workspace.read(cx).tabs.get(active).unwrap().content,
            OpenTab::Review { .. }
        ));
    });
}

#[gpui_kit::test]
fn aggregate_close_cancel_discard_and_failed_save_preserve_the_session(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("first.rs");
    let second_path = directory.path().join("second.rs");
    std::fs::write(&first_path, "first\n").unwrap();
    std::fs::write(&second_path, "second\n").unwrap();
    let writable = std::fs::metadata(&second_path).unwrap().permissions();
    let mut read_only = writable.clone();
    read_only.set_readonly(true);

    let manifest = ReviewManifest::new(vec![
        saveable_text_file("first", "src/first.rs", "base\n", "first\n", &first_path),
        saveable_text_file(
            "second",
            "src/second.rs",
            "base\n",
            "second\n",
            &second_path,
        ),
    ])
    .unwrap();
    let (id, session) = open_review(
        &workspace,
        source(TestProvider::new([manifest]), "aggregate-save"),
        cx,
    );

    cx.update(|window, cx| {
        edit_active(window, cx, "A");
        window.click(("review-file", 1usize), cx);
        edit_active(window, cx, "B");

        window.click(("tab-close-target", id), cx);
        window.click("cancel", cx);
        assert!(workspace.read(cx).tabs.get(id).is_some());
        assert!(session.read(cx).needs_save(cx));

        window.click(("tab-close-target", id), cx);
        std::fs::set_permissions(&second_path, read_only).unwrap();
        window.click("save-and-close", cx);
    });
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert!(workspace.read(cx).tabs.get(id).is_some());
        assert!(session.read(cx).needs_save(cx));
        assert!(std::fs::read_to_string(&first_path).unwrap().contains('A'));
        assert_eq!(std::fs::read_to_string(&second_path).unwrap(), "second\n");
    });

    std::fs::set_permissions(&second_path, writable).unwrap();
    cx.update(|window, cx| {
        window.click(("tab-close-target", id), cx);
        window.click("ok", cx);
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert!(workspace.read(cx).tabs.get(id).is_none()));
}
