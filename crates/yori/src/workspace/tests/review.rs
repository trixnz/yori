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
        "Working changes — fixture",
        "Working changes",
        "fixture",
        provider,
    )
}

/// Home is the landing surface, so its first action opens the Git chooser that
/// an invocation used to open by itself.
fn open_git_chooser_from_home(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    });
    cx.run_until_parked();
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
fn clicking_a_file_selects_it_and_hands_focus_to_its_editor(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let second = ReviewFileIdentity::new("second");
    let manifest = ReviewManifest::new(vec![
        text_file("first", "src/first.rs", "one\n", "ONE\n"),
        text_file("second", "src/second.rs", "two\n", "TWO\n"),
    ])
    .unwrap();
    let (_, session) = open_review(
        &workspace,
        source(TestProvider::new([manifest]), "click-focus"),
        cx,
    );

    cx.update(|window, cx| {
        session.update(cx, |session, cx| session.focus_navigator(window, cx));
        window.render_frame(cx);
        assert_eq!(window.find("review-file-list").focused(), Some(true));
        // Parked here by the keyboard, so the list shows its focus.
        assert!(session.read(cx).navigator_focus_is_visible());

        // Selection commits on mouse down, so focus reaches the editor without
        // a frame in between where the list still holds both.
        window.click(("review-file", 1usize), cx);
        window.render_frame(cx);

        assert_eq!(session.read(cx).selected_identity(), Some(&second));
        let editor = session.read(cx).editor(&second).unwrap();
        assert!(editor.focus_handle(cx).contains_focused(window, cx));
        assert_ne!(window.find("review-file-list").focused(), Some(true));
        // The press never rests in the navigator, so it must not light it up
        // on the way through to the editor.
        assert!(!session.read(cx).navigator_focus_is_visible());

        // Returning by keyboard shows focus again.
        session.update(cx, |session, cx| session.focus_navigator(window, cx));
        window.render_frame(cx);
        assert!(session.read(cx).navigator_focus_is_visible());
    });
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
        // The navigator orders files by path, so src/removed.rs sits below
        // src/kept.rs; select it explicitly rather than relying on that order.
        window.click(("review-file", 1usize), cx);
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
fn active_review_refresh_does_not_steal_focus_from_home(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let manifest = ReviewManifest::new(vec![text_file(
        "file",
        "src/file.rs",
        "before\n",
        "after\n",
    )])
    .unwrap();
    let provider = TestProvider::new([manifest]);

    let session = cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_review_source(source(provider, "home-focus"), window, cx);
        });
        let review_id = workspace.read(cx).tabs.active.unwrap();
        let OpenTab::Review { session, .. } =
            &workspace.read(cx).tabs.get(review_id).unwrap().content
        else {
            panic!("new tab should be a review session");
        };
        let session = session.clone();

        workspace.update(cx, |workspace, cx| {
            workspace.show_home(&ShowHome, window, cx);
        });
        window.render_frame(cx);

        assert_eq!(workspace.read(cx).selection, WorkspaceSelection::Home);
        assert_eq!(window.find("home").focused(), Some(true));

        session
    });
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);

    cx.update(|window, cx| {
        assert_eq!(session.read(cx).editor_count(), 1);
        assert_eq!(workspace.read(cx).selection, WorkspaceSelection::Home);
        assert_eq!(window.find("home").focused(), Some(true));
    });
}

#[gpui_kit::test]
fn navigator_selection_displays_files_and_ctrl_h_l_moves_between_panes(cx: &mut TestAppContext) {
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
        assert_eq!(session.read(cx).selected_identity(), Some(&binary));
        assert_eq!(window.find(("review-file", 0usize)).selected(), Some(true));
        assert_eq!(window.find("review-file-list").focused(), Some(true));

        window.press("j", cx);
        window.render_frame(cx);
        assert_eq!(session.read(cx).selected_identity(), Some(&text));
        assert_eq!(window.find(("review-file", 1usize)).selected(), Some(true));

        window.press("k", cx);
        window.press("enter", cx);
        window.render_frame(cx);
        assert_eq!(session.read(cx).selected_identity(), Some(&binary));
        assert_eq!(window.find("review-file-body").focused(), Some(true));

        window.press("ctrl-h", cx);
        window.render_frame(cx);
        assert_eq!(window.find("review-file-list").focused(), Some(true));
        window.press("ctrl-l", cx);
        window.render_frame(cx);
        assert_eq!(window.find("review-file-body").focused(), Some(true));

        session.update(cx, |session, cx| session.focus_navigator(window, cx));
        window.press("down", cx);
        window.press("space", cx);
        window.render_frame(cx);
        assert_eq!(session.read(cx).selected_identity(), Some(&text));
        let editor = session.read(cx).editor(&text).unwrap();
        assert!(editor.focus_handle(cx).contains_focused(window, cx));
        assert_eq!(editor.read(cx).active_pane_index(), 0);

        window.press("ctrl-l", cx);
        assert_eq!(editor.read(cx).active_pane_index(), 1);
        window.press("ctrl-h", cx);
        assert_eq!(editor.read(cx).active_pane_index(), 0);
        window.press("ctrl-h", cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("review-file-list").focused(), Some(true));
    });
}

#[gpui_kit::test]
fn navigator_displays_a_fresh_file_before_syntax_highlighting_finishes(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let first = ReviewFileIdentity::new("first");
    let second = ReviewFileIdentity::new("second");
    let manifest = ReviewManifest::new(vec![
        text_file(
            "first",
            "src/first.rs",
            "fn before() {}\n",
            "fn after() {}\n",
        ),
        text_file(
            "second",
            "src/second.rs",
            "fn previous() {}\n",
            "fn current() {}\n",
        ),
    ])
    .unwrap();
    let (_, session) = open_review(
        &workspace,
        source(TestProvider::new([manifest]), "background-highlighting"),
        cx,
    );

    cx.update(|window, cx| {
        assert_eq!(session.read(cx).selected_identity(), Some(&first));
        session.update(cx, |session, cx| session.focus_navigator(window, cx));

        window.press("j", cx);

        assert_eq!(session.read(cx).selected_identity(), Some(&second));
        let editor = session.read(cx).editor(&second).unwrap();
        assert!(!editor.read(cx).highlighting_ready());
    });
    cx.run_until_parked();

    cx.update(|_, cx| {
        let editor = session.read(cx).editor(&second).unwrap();
        assert!(editor.read(cx).highlighting_ready());
    });
}

#[gpui_kit::test]
fn navigator_keyboard_selection_scrolls_beyond_one_viewport(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let files = (0..80)
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
        for _ in 0..60 {
            window.press("j", cx);
        }
        window.render_frame(cx);
        window.render_frame(cx);

        let list = window.find("review-file-list").bounds();
        let selected = window.find(("review-file", 59usize));
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
        assert_eq!(window.find(("review-file", 58usize)).selected(), Some(true));
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

pub(super) fn perforce_context() -> Arc<PerforceContext> {
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
        let tab = workspace.read(cx).tabs.get(active).unwrap();
        assert!(matches!(tab.content, OpenTab::Review { .. }));
        assert_eq!(
            tab.identity.description(),
            "Perforce: ssl:perforce.example:1666 | robin-yori | submitted 41"
        );
    });
}

#[gpui_kit::test]
fn invoking_inside_a_repository_lands_on_home_and_offers_its_working_changes(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = harness(cx);
    let repository = tempfile::tempdir().unwrap();
    gix::init(repository.path()).unwrap();
    std::fs::write(repository.path().join("working.txt"), "working\n").unwrap();
    let window = cx.update(|window, _| {
        window
            .window_handle()
            .downcast::<gpui_kit::component::Root>()
            .unwrap()
    });

    crate::dispatch_invocation(
        window,
        &workspace,
        &InvocationRequest::new(repository.path().to_owned(), Vec::new()),
        &mut cx.cx,
    )
    .unwrap();
    cx.run_until_parked();

    let before = cx.update(|window, cx| {
        window.render_frame(cx);
        // Home is the landing surface; nothing opens over it.
        assert!(workspace.read(cx).git_source_chooser.is_none());
        let _ = window.find("home");

        let count = workspace.read(cx).tabs.entries.len();
        window.click("home-working-changes", cx);
        count
    });
    cx.run_until_parked();

    cx.update(|_, cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.tabs.entries.len(), before + 1);
        let active = workspace.tabs.active.unwrap();
        assert!(
            workspace
                .tabs
                .get(active)
                .unwrap()
                .identity
                .description()
                .contains("working")
        );
    });
}

#[gpui_kit::test]
#[expect(
    clippy::too_many_lines,
    reason = "one sequence walks invocation, chooser routing, and tab reuse across two repositories; splitting it would duplicate the setup without separating the behaviour"
)]
fn invocation_routes_git_chooser_and_deduplication_to_the_invoking_repository(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = harness(cx);
    let repository_a = tempfile::tempdir().unwrap();
    let repository_b = tempfile::tempdir().unwrap();
    gix::init(repository_a.path()).unwrap();
    gix::init(repository_b.path()).unwrap();
    std::fs::write(repository_a.path().join("from-a.txt"), "a\n").unwrap();
    std::fs::write(repository_b.path().join("from-b.txt"), "b\n").unwrap();
    let window = cx.update(|window, _| {
        window
            .window_handle()
            .downcast::<gpui_kit::component::Root>()
            .unwrap()
    });

    crate::dispatch_invocation(
        window,
        &workspace,
        &InvocationRequest::new(repository_a.path().to_owned(), Vec::new()),
        &mut cx.cx,
    )
    .unwrap();
    cx.run_until_parked();
    open_git_chooser_from_home(cx);
    cx.update(|window, cx| {
        window.render_frame(cx);
        let chooser = workspace
            .read(cx)
            .git_source_chooser
            .as_ref()
            .unwrap()
            .0
            .clone();
        assert_eq!(
            chooser.read(cx).work_dir(),
            repository_a.path().canonicalize().unwrap()
        );
        assert_eq!(chooser.read(cx).selected(), 0);
        chooser.update(cx, |chooser, cx| chooser.focus(window, cx));
        assert_eq!(window.find("git-source-chooser").focused(), Some(true));

        window.within("git-source-chooser").press("down", cx);
        window.render_frame(cx);
        assert_eq!(chooser.read(cx).selected(), 1);
        window.click("git-revision-input", cx);
        window.input("k", cx);
        assert_eq!(chooser.read(cx).selected(), 1);
        assert_eq!(chooser.read(cx).revision_text(cx), "k");
    });

    crate::dispatch_invocation(
        window,
        &workspace,
        &InvocationRequest::new(repository_b.path().to_owned(), Vec::new()),
        &mut cx.cx,
    )
    .unwrap();
    cx.run_until_parked();
    open_git_chooser_from_home(cx);
    let before = cx.update(|window, cx| {
        window.render_frame(cx);
        let chooser = workspace
            .read(cx)
            .git_source_chooser
            .as_ref()
            .unwrap()
            .0
            .clone();
        assert_eq!(
            chooser.read(cx).work_dir(),
            repository_b.path().canonicalize().unwrap()
        );

        let count = workspace.read(cx).tabs.entries.len();
        window.press("enter", cx);
        count
    });
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);

    cx.update(|_, cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.tabs.entries.len(), before + 1);
        let active = workspace.tabs.active.unwrap();
        let repository_b = repository_b.path().canonicalize().unwrap();
        assert!(
            workspace
                .tabs
                .get(active)
                .unwrap()
                .identity
                .description()
                .contains(repository_b.to_str().unwrap())
        );
    });

    crate::dispatch_invocation(
        window,
        &workspace,
        &InvocationRequest::new(repository_b.path().to_owned(), Vec::new()),
        &mut cx.cx,
    )
    .unwrap();
    cx.run_until_parked();
    open_git_chooser_from_home(cx);
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(workspace.read(cx).tabs.entries.len(), before + 1));
}

#[gpui_kit::test]
fn git_review_is_unavailable_while_perforce_discovery_runs(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let repository = tempfile::tempdir().unwrap();
    gix::init(repository.path()).unwrap();

    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.invocation_directory = Some(repository.path().to_owned());
            workspace.perforce_discovery = PerforceDiscovery::Loading;
            cx.notify();
        });
        window.render_frame(cx);
        window.click("open-git-review", cx);

        assert!(workspace.read(cx).git_source_chooser.is_none());
    });
}

#[gpui_kit::test]
fn git_chooser_falls_back_to_the_active_local_comparison_repository(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let repository = tempfile::tempdir().unwrap();
    gix::init(repository.path()).unwrap();
    let baseline = repository.path().join("baseline.txt");
    let local = repository.path().join("local.txt");
    std::fs::write(&baseline, "before\n").unwrap();
    std::fs::write(&local, "after\n").unwrap();
    let outside = tempfile::tempdir().unwrap();
    let window = cx.update(|window, _| {
        window
            .window_handle()
            .downcast::<gpui_kit::component::Root>()
            .unwrap()
    });

    crate::dispatch_invocation(
        window,
        &workspace,
        &InvocationRequest::new(outside.path().to_owned(), Vec::new()),
        &mut cx.cx,
    )
    .unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_paths(&baseline, &local, window, cx);
            workspace.choose_git_review(&OpenGitReview, window, cx);
        });
        window.render_frame(cx);

        let chooser = workspace
            .read(cx)
            .git_source_chooser
            .as_ref()
            .expect("active local comparison should provide repository context")
            .0
            .clone();
        assert_eq!(
            chooser.read(cx).work_dir(),
            repository.path().canonicalize().unwrap()
        );
    });
}

#[gpui_kit::test]
fn git_working_document_edits_save_to_the_real_worktree_path(cx: &mut TestAppContext) {
    let (workspace, cx) = harness(cx);
    let repository = tempfile::tempdir().unwrap();
    gix::init(repository.path()).unwrap();
    let path = repository.path().join("working.txt");
    std::fs::write(&path, "working\n").unwrap();
    let window = cx.update(|window, _| {
        window
            .window_handle()
            .downcast::<gpui_kit::component::Root>()
            .unwrap()
    });

    crate::dispatch_invocation(
        window,
        &workspace,
        &InvocationRequest::new(repository.path().to_owned(), Vec::new()),
        &mut cx.cx,
    )
    .unwrap();
    cx.run_until_parked();
    open_git_chooser_from_home(cx);
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    });
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);

    cx.update(|window, cx| {
        edit_active(window, cx, "edited ");
        window.press("ctrl-s", cx);
    });
    cx.run_until_parked();

    assert!(std::fs::read_to_string(path).unwrap().contains("edited"));
}

#[cfg(unix)]
#[gpui_kit::test]
fn git_symlink_working_document_cannot_edit_or_save_through_its_target(cx: &mut TestAppContext) {
    use std::os::unix::fs::symlink;

    let (workspace, cx) = harness(cx);
    let repository = tempfile::tempdir().unwrap();
    gix::init(repository.path()).unwrap();
    let target = repository.path().join("target.txt");
    let link = repository.path().join("link.txt");
    std::fs::write(&target, "target contents\n").unwrap();
    symlink("target.txt", &link).unwrap();
    let window = cx.update(|window, _| {
        window
            .window_handle()
            .downcast::<gpui_kit::component::Root>()
            .unwrap()
    });

    crate::dispatch_invocation(
        window,
        &workspace,
        &InvocationRequest::new(repository.path().to_owned(), Vec::new()),
        &mut cx.cx,
    )
    .unwrap();
    cx.run_until_parked();
    open_git_chooser_from_home(cx);
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    });
    cx.run_until_parked();
    cx.update(TestWindowExt::render_frame);

    cx.update(|window, cx| {
        edit_active(window, cx, "must not reach target ");
        window.press("ctrl-s", cx);
    });
    cx.run_until_parked();

    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "target contents\n"
    );
    assert_eq!(std::fs::read_link(link).unwrap(), Path::new("target.txt"));
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
