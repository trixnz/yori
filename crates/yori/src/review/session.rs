//! Native review-session surface and lifecycle.

use std::{
    collections::HashMap,
    collections::VecDeque,
    ffi::OsString,
    path::{Path, PathBuf},
    rc::Rc,
};

use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable,
    button::{Button, ButtonVariants},
    scroll::ScrollableElement,
    tooltip::Tooltip,
};
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, KeyBinding, MouseButton, ParentElement, Render,
    Role as AccessibilityRole, ScrollHandle, StatefulInteractiveElement, Styled, Subscription,
    TestSupportExt, Window, div, prelude::FluentBuilder, px,
};

use yori::document_info::path_labels;

use crate::{
    comparison::{Comparison, MergeComparison},
    editor::{
        AlignedEditor, DirtyChanged, PaneDocument, PaneFocusBoundary, ToggleWordWrap, WordWrap,
        WordWrapChanged,
    },
    storage::SaveError,
    workspace::files::{Files, Role as DocumentRole},
};

use super::model::{
    DiffStat, ReviewConflict, ReviewFile, ReviewFileIdentity, ReviewFileKind, ReviewManifest,
    ReviewSource, TextComparison,
};

const NAVIGATOR_KEY_CONTEXT: &str = "ReviewNavigator";
const NON_TEXT_BODY_KEY_CONTEXT: &str = "ReviewNonTextBody";

gpui_kit::actions!(
    review_navigator,
    [
        SelectPreviousFile,
        SelectNextFile,
        ActivateSelectedFile,
        FocusNavigator
    ]
);

pub(crate) enum ReviewChanged {
    State,
    RefreshCompleted {
        activate: bool,
    },
    ActiveEditorChanged {
        transfer_focus: bool,
    },
    WordWrapChanged {
        enabled: bool,
    },
    /// The user asked to reconcile a conflicted file in a three-way merge tab.
    MergeRequested(Box<MergeComparison>),
}

impl EventEmitter<ReviewChanged> for ReviewSession {}

enum EntryWarning {
    RemovedUpstream,
    KindChanged { incoming: Box<ReviewFile> },
}

impl EntryWarning {
    fn label(&self) -> &'static str {
        match self {
            Self::RemovedUpstream => "removed upstream",
            Self::KindChanged { incoming } => match &incoming.kind {
                ReviewFileKind::Binary { .. } => "now binary upstream",
                ReviewFileKind::Submodule { .. } => "now a submodule upstream",
                ReviewFileKind::Text(_) => {
                    unreachable!("kind-change warnings retain non-text files")
                }
            },
        }
    }

    fn detail(&self) -> &'static str {
        match self {
            Self::RemovedUpstream => {
                "This file was removed upstream. Unsaved text edits remain available until resolved."
            }
            Self::KindChanged { incoming } => match &incoming.kind {
                ReviewFileKind::Binary { .. } => {
                    "This file is now binary upstream. Unsaved text edits remain available until resolved."
                }
                ReviewFileKind::Submodule { .. } => {
                    "This file is now a submodule upstream. Unsaved text edits remain available until resolved."
                }
                ReviewFileKind::Text(_) => {
                    unreachable!("kind-change warnings retain non-text files")
                }
            },
        }
    }
}

struct SessionEntry {
    file: ReviewFile,
    warning: Option<EntryWarning>,
}

struct ReviewEditor {
    editor: Entity<AlignedEditor>,
    _dirty_subscription: Subscription,
    _pane_subscription: Subscription,
    _word_wrap_subscription: Subscription,
    files: Files,
}

struct LoadedText {
    files: Files,
    left: PaneDocument,
    right: PaneDocument,
    editable: bool,
    saveable: bool,
}

impl LoadedText {
    fn load(comparison: &TextComparison) -> Result<Self, String> {
        let capabilities = comparison.capabilities();
        let comparison = comparison.comparison().resolve()?;
        let Comparison::Diff(diff) = &comparison else {
            unreachable!("review text entries are always two-way comparisons");
        };
        let files = Files::load(&comparison)?;
        let left = PaneDocument::new(
            diff.baseline.logical_path().to_owned(),
            files.document(DocumentRole::Baseline).clone(),
        );
        let right = PaneDocument::new(
            diff.local.logical_path().to_owned(),
            files.document(DocumentRole::Local).clone(),
        );

        Ok(Self {
            files,
            left,
            right,
            editable: capabilities.editable,
            saveable: capabilities.saveable,
        })
    }
}

/// The longest directory every reviewed file sits under. Naming it once keeps
/// it off the front of every heading, where it would be the part of the path
/// that carries no information. `None` when the files share nothing.
fn common_directory<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Option<PathBuf> {
    let mut shared: Option<Vec<OsString>> = None;

    for path in paths {
        let parent = path
            .parent()
            .map(|parent| {
                parent
                    .components()
                    .map(|component| component.as_os_str().to_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        shared = Some(match shared {
            None => parent,
            Some(shared) => shared
                .into_iter()
                .zip(parent)
                .take_while(|(shared, parent)| shared == parent)
                .map(|(shared, _)| shared)
                .collect(),
        });
    }

    let shared = shared?;
    (!shared.is_empty()).then(|| shared.into_iter().collect())
}

/// The heading a file belongs under, relative to the review's shared base.
fn group_directory(path: &Path, base: Option<&Path>) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    base.and_then(|base| parent.strip_prefix(base).ok())
        .unwrap_or(parent)
        .to_path_buf()
}

/// Files sitting directly in the shared base need no heading — the base line
/// above them already names their directory. Without a shared base, a file at
/// the repository root still needs one, or it would look like part of the run
/// above it.
fn heading_is_shown(directory: &Path, base: Option<&Path>) -> bool {
    !directory.as_os_str().is_empty() || base.is_none()
}

/// Counts the elements that precede an entry's row but are not rows themselves.
/// Rendering and scrolling both go through this so the two cannot disagree.
fn leading_elements(entries: &[&SessionEntry], entry_index: usize) -> usize {
    let base = common_directory(entries.iter().map(|entry| entry.file.path()));
    let mut count = usize::from(base.is_some());
    let mut group: Option<PathBuf> = None;

    for entry in entries.iter().take(entry_index + 1) {
        let directory = group_directory(entry.file.path(), base.as_deref());
        if group.as_ref() != Some(&directory) {
            if heading_is_shown(&directory, base.as_deref()) {
                count += 1;
            }
            group = Some(directory);
        }
    }

    count
}

/// Names the directory the whole review sits under, once, above the first run.
fn render_base_directory(base: &Path, cx: &App) -> AnyElement {
    div()
        .id("review-base-directory")
        .test_support()
        .px(px(10.0))
        .pt(px(8.0))
        .pb(px(2.0))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis_start()
        .text_size(px(11.0))
        .text_color(cx.theme().muted_foreground)
        .child(format!("{}/", base.display()))
        .into_any_element()
}

/// Names the directory a run of files shares. Truncation drops the front of the
/// path because the trailing segments are what tell two directories apart.
fn render_directory_heading(directory: &Path, index: usize, cx: &App) -> AnyElement {
    let label = if directory.as_os_str().is_empty() {
        "/".to_owned()
    } else {
        directory.display().to_string()
    };

    div()
        .id(("review-directory-heading", index))
        .test_support()
        .px(px(10.0))
        .pt(px(10.0))
        .pb(px(2.0))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis_start()
        .text_size(px(11.0))
        .text_color(cx.theme().muted_foreground)
        .child(label)
        .into_any_element()
}

/// Everything the navigator shows about one file apart from its status badge.
struct FileRowLabels {
    index: usize,
    name: String,
    old_path: Option<String>,
    warning: Option<&'static str>,
    stat: Option<DiffStat>,
    modified: bool,
    /// The full path, shown on hover because both label lines are truncated.
    tooltip: String,
}

/// Renders a file by name alone; the directory it sits in comes from the group
/// heading above it, so a deep path costs nothing in the scan column.
fn render_file_labels(labels: FileRowLabels, cx: &App) -> impl IntoElement + use<> {
    let FileRowLabels {
        index,
        name,
        old_path,
        warning,
        stat,
        modified,
        tooltip,
    } = labels;
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(1.0))
        .child(
            div()
                .h(px(18.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .id(("review-file-name", index))
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis_middle()
                        .child(name)
                        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx)),
                )
                .children(stat.map(|stat| render_stat(stat, index)))
                .child(
                    div()
                        .size(px(6.0))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(if modified {
                            cx.theme().foreground
                        } else {
                            cx.theme().transparent
                        }),
                ),
        )
        .children(old_path.map(|path| {
            div()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis_middle()
                .text_size(px(11.0))
                .text_color(cx.theme().muted_foreground)
                .child(path)
        }))
        .children(warning.map(|warning| {
            div()
                .text_size(px(11.0))
                .text_color(crate::appearance::removed().marker)
                .child(warning)
        }))
}

/// Renders one file's line counts as `+12 -3`, dropping a side that did not
/// change so an addition-only file reads as a single number.
fn render_stat(stat: DiffStat, index: usize) -> impl IntoElement {
    div()
        .id(("review-file-stat", index))
        .test_support()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.0))
        .text_size(px(11.0))
        .children((stat.added > 0).then(|| {
            div()
                .text_color(crate::appearance::added().marker)
                .child(format!("+{}", stat.added))
        }))
        .children((stat.removed > 0).then(|| {
            div()
                .text_color(crate::appearance::removed().marker)
                .child(format!("\u{2212}{}", stat.removed))
        }))
}

pub(crate) type SaveCompletion = Rc<dyn Fn(bool, &mut Window, &mut App)>;

struct SaveRun {
    pending: VecDeque<ReviewFileIdentity>,
    completion: SaveCompletion,
}

pub(crate) struct ReviewSession {
    source: ReviewSource,
    entries: Vec<SessionEntry>,
    selected: Option<ReviewFileIdentity>,
    editors: HashMap<ReviewFileIdentity, ReviewEditor>,
    navigator_focus: FocusHandle,
    /// Whether the navigator should *look* focused, in the spirit of the web's
    /// `:focus-visible`. Holding focus and showing it are different questions:
    /// a click passes through navigator focus on its way to the editor, and
    /// drawing that is a flash with no state behind it. Only the keyboard, which
    /// can rest here and move the selection without committing, turns it on.
    navigator_focus_visible: bool,
    navigator_scroll: ScrollHandle,
    non_text_body_focus: FocusHandle,
    refreshing: bool,
    saving: bool,
    message: Option<String>,
    word_wrap: WordWrap,
}

impl ReviewSession {
    pub(crate) fn new(source: ReviewSource, word_wrap: bool, cx: &mut Context<Self>) -> Self {
        Self {
            source,
            entries: Vec::new(),
            selected: None,
            editors: HashMap::new(),
            navigator_focus: cx.focus_handle(),
            navigator_focus_visible: true,
            navigator_scroll: ScrollHandle::new(),
            non_text_body_focus: cx.focus_handle(),
            refreshing: false,
            saving: false,
            message: None,
            word_wrap: word_wrap.into(),
        }
    }

    pub(crate) fn set_word_wrap(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.word_wrap = enabled.into();
        for state in self.editors.values() {
            state
                .editor
                .update(cx, |editor, cx| editor.set_word_wrap(enabled, cx));
        }

        cx.notify();
    }

    fn toggle_word_wrap(&mut self, _: &ToggleWordWrap, _: &mut Window, cx: &mut Context<Self>) {
        let enabled = !self.word_wrap.enabled();
        self.set_word_wrap(enabled, cx);
        cx.emit(ReviewChanged::WordWrapChanged { enabled });
    }

    pub(crate) fn needs_save(&self, cx: &App) -> bool {
        self.editors
            .values()
            .any(|state| state.editor.read(cx).needs_save())
    }

    pub(crate) fn can_save_all(&self, cx: &App) -> bool {
        self.editors
            .values()
            .all(|state| !state.editor.read(cx).needs_save() || state.editor.read(cx).can_save())
    }

    pub(crate) fn deactivate(&self, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            editor.update(cx, AlignedEditor::deactivate);
        }
    }

    pub(crate) fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            editor.focus_handle(cx).focus(window, cx);
        } else if self.selected_is_non_text() {
            self.non_text_body_focus.focus(window, cx);
        } else {
            // Nothing to open, so the navigator is where focus comes to rest
            // rather than somewhere it passes through.
            self.focus_navigator(window, cx);
        }
    }

    pub(crate) fn focus_navigator(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigator_focus_visible = true;
        self.navigator_focus.focus(window, cx);
    }

    fn selected_is_non_text(&self) -> bool {
        self.selected_entry()
            .is_some_and(|entry| !entry.file.kind.is_text())
    }

    fn active_editor(&self) -> Option<Entity<AlignedEditor>> {
        self.selected
            .as_ref()
            .and_then(|identity| self.editors.get(identity))
            .map(|state| state.editor.clone())
    }

    pub(crate) fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.refreshing || self.saving {
            return;
        }

        self.refreshing = true;
        self.message = None;
        cx.notify();

        let provider = self.source.provider.clone();
        let identity = self.source.identity.clone();
        let load = cx.background_executor().spawn(async move {
            provider.load_manifest(&identity).map(|mut manifest| {
                manifest.sort();
                manifest.measure();
                manifest
            })
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = load.await;
            let _ = view.update_in(cx, |this, window, cx| {
                this.refreshing = false;
                let event =
                    match result.and_then(|manifest| this.apply_manifest(manifest, window, cx)) {
                        Ok(activate) => {
                            this.message = None;
                            ReviewChanged::RefreshCompleted { activate }
                        }
                        Err(error) => {
                            this.message = Some(format!("Refresh failed: {error}"));
                            ReviewChanged::State
                        }
                    };

                cx.emit(event);
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_manifest(
        &mut self,
        manifest: ReviewManifest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        ReviewManifest::new(manifest.files.clone())?;

        let old_selected = self.selected.clone();
        let old_active_editor = self.active_editor();
        let mut prepared = HashMap::new();
        for file in &manifest.files {
            if self.editors.contains_key(&file.identity)
                && let ReviewFileKind::Text(comparison) = &file.kind
            {
                prepared.insert(file.identity.clone(), LoadedText::load(comparison)?);
            }
        }

        let mut old_entries = self
            .entries
            .drain(..)
            .map(|entry| (entry.file.identity.clone(), entry))
            .collect::<HashMap<_, _>>();
        let mut entries = Vec::with_capacity(manifest.files.len());

        for file in manifest.files {
            let identity = file.identity.clone();
            let old_entry = old_entries.remove(&identity);

            if let Some(loaded) = prepared.remove(&identity) {
                let state = self
                    .editors
                    .get_mut(&identity)
                    .expect("prepared text belongs to an existing editor");
                let dirty = state.editor.read(cx).needs_save();
                state.editor.update(cx, |editor, cx| {
                    editor.refresh_review_diff(
                        loaded.left,
                        loaded.right,
                        loaded.editable,
                        loaded.saveable,
                        window,
                        cx,
                    );
                });
                if !dirty {
                    state.files = loaded.files;
                }

                entries.push(SessionEntry {
                    file,
                    warning: None,
                });
                continue;
            }

            if !file.kind.is_text()
                && self.editor_is_dirty(&identity, cx)
                && let Some(mut entry) = old_entry
                && entry.file.kind.is_text()
            {
                entry.file.logical_path.clone_from(&file.logical_path);
                entry.file.status = file.status.clone();
                entry.warning = Some(EntryWarning::KindChanged {
                    incoming: Box::new(file),
                });
                entries.push(entry);
                continue;
            }

            if !file.kind.is_text() {
                self.editors.remove(&identity);
            }
            entries.push(SessionEntry {
                file,
                warning: None,
            });
        }

        for (identity, entry) in old_entries {
            if self.editor_is_dirty(&identity, cx) {
                entries.push(SessionEntry {
                    file: entry.file,
                    warning: Some(EntryWarning::RemovedUpstream),
                });
            } else {
                self.editors.remove(&identity);
            }
        }

        self.entries = entries;

        let selected_exists = self.selected.as_ref().is_some_and(|selected| {
            self.entries
                .iter()
                .any(|entry| &entry.file.identity == selected)
        });
        if !selected_exists {
            self.selected = self
                .entries
                .iter()
                .find(|entry| entry.file.kind.is_text())
                .map(|entry| entry.file.identity.clone());
        }

        if let Some(selected) = self.selected.clone() {
            self.ensure_editor(&selected, window, cx)?;
        }

        self.reveal_selected();

        Ok(old_selected != self.selected || old_active_editor != self.active_editor())
    }

    fn editor_is_dirty(&self, identity: &ReviewFileIdentity, cx: &App) -> bool {
        self.editors
            .get(identity)
            .is_some_and(|state| state.editor.read(cx).needs_save())
    }

    fn resolve_clean_kind_change(
        &mut self,
        identity: &ReviewFileIdentity,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.editor_is_dirty(identity, cx) {
            return false;
        }

        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| &entry.file.identity == identity)
        else {
            return false;
        };

        let warning = entry.warning.take();
        let incoming = match warning {
            Some(EntryWarning::KindChanged { incoming }) => incoming,
            warning => {
                entry.warning = warning;
                return false;
            }
        };
        let active_editor_changed = self.selected.as_ref() == Some(identity);

        entry.file = *incoming;
        self.editors.remove(identity);

        active_editor_changed
    }

    fn ensure_editor(
        &mut self,
        identity: &ReviewFileIdentity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        if self.editors.contains_key(identity) {
            return Ok(false);
        }

        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| &entry.file.identity == identity)
        else {
            return Ok(false);
        };
        let ReviewFileKind::Text(comparison) = &entry.file.kind else {
            return Ok(false);
        };

        let loaded = LoadedText::load(comparison)?;
        let word_wrap = self.word_wrap.enabled();
        let editor = cx.new(|cx| {
            AlignedEditor::new_review_diff(
                loaded.left,
                loaded.right,
                loaded.editable,
                loaded.saveable,
                word_wrap,
                window,
                cx,
            )
        });
        let subscribed_identity = identity.clone();
        let dirty_subscription = cx.subscribe_in(
            &editor,
            window,
            move |this, editor, _: &DirtyChanged, window, cx| {
                let transfer_focus = editor.focus_handle(cx).contains_focused(window, cx);
                let active_editor_changed =
                    this.resolve_clean_kind_change(&subscribed_identity, cx);
                let event = if active_editor_changed {
                    ReviewChanged::ActiveEditorChanged { transfer_focus }
                } else {
                    ReviewChanged::State
                };

                cx.emit(event);
                cx.notify();
            },
        );
        let pane_subscription = cx.subscribe_in(
            &editor,
            window,
            |this, _, boundary: &PaneFocusBoundary, window, cx| {
                if *boundary == PaneFocusBoundary::Previous {
                    this.focus_navigator(window, cx);
                    cx.notify();
                }
            },
        );
        let word_wrap_subscription =
            cx.subscribe(&editor, |this, _, event: &WordWrapChanged, cx| {
                this.set_word_wrap(event.enabled, cx);
                cx.emit(ReviewChanged::WordWrapChanged {
                    enabled: event.enabled,
                });
            });

        self.editors.insert(
            identity.clone(),
            ReviewEditor {
                editor,
                _dirty_subscription: dirty_subscription,
                _pane_subscription: pane_subscription,
                _word_wrap_subscription: word_wrap_subscription,
                files: loaded.files,
            },
        );

        Ok(true)
    }

    fn select(
        &mut self,
        identity: ReviewFileIdentity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.saving {
            return;
        }

        self.deactivate(cx);
        if let Err(error) = self.ensure_editor(&identity, window, cx) {
            self.message = Some(format!("Cannot open file comparison: {error}"));
        }
        self.selected = Some(identity);
        self.reveal_selected();

        // Selecting keeps the keyboard in the navigator. Callers that want the
        // content instead, such as a click or Enter, move focus themselves.
        self.navigator_focus.focus(window, cx);

        cx.notify();
    }

    pub(crate) fn save_all(
        &mut self,
        completion: SaveCompletion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.saving || self.refreshing {
            self.message = Some("Wait for the review refresh to finish before saving.".into());
            Self::complete(false, completion, window, cx);
            return;
        }

        let pending = self
            .entries
            .iter()
            .filter_map(|entry| {
                self.editors.get(&entry.file.identity).and_then(|state| {
                    state
                        .editor
                        .read(cx)
                        .needs_save()
                        .then(|| entry.file.identity.clone())
                })
            })
            .collect();

        self.save_next(
            SaveRun {
                pending,
                completion,
            },
            window,
            cx,
        );
    }

    fn save_next(&mut self, mut run: SaveRun, window: &mut Window, cx: &mut Context<Self>) {
        let Some(identity) = run.pending.pop_front() else {
            Self::complete(true, run.completion, window, cx);
            return;
        };
        let Some(state) = self.editors.get_mut(&identity) else {
            self.save_next(run, window, cx);
            return;
        };

        let checkpoint = match state.editor.update(cx, AlignedEditor::prepare_save) {
            Ok(checkpoint) => checkpoint,
            Err(error) => {
                self.fail_save(identity, error, run.completion, window, cx);
                return;
            }
        };
        let Some(destination) = state.files.destination() else {
            self.fail_save(
                identity,
                "This document has no save destination.".into(),
                run.completion,
                window,
                cx,
            );
            return;
        };

        let path = destination.path.clone();
        let expected = destination.accepted.clone();
        let text = checkpoint.text.clone();
        let editor = state.editor.clone();
        self.saving = true;
        editor.update(cx, |editor, cx| editor.set_saving(true, cx));
        cx.notify();

        let save = cx
            .background_executor()
            .spawn(async move { crate::storage::save(&path, &expected, text.as_bytes()) });
        cx.spawn_in(window, async move |view, cx| {
            let result = save.await;
            let _ = view.update_in(cx, |this, window, cx| {
                this.saving = false;
                editor.update(cx, |editor, cx| editor.set_saving(false, cx));

                match result {
                    Ok(snapshot) => {
                        if let Some(state) = this.editors.get_mut(&identity) {
                            state.files.saved(&snapshot);
                        }
                        editor.update(cx, |editor, cx| editor.mark_saved(checkpoint, cx));
                        this.save_next(run, window, cx);
                    }
                    Err(SaveError::Changed(_)) => this.fail_save(
                        identity,
                        "Destination changed on disk; review it before saving again.".into(),
                        run.completion,
                        window,
                        cx,
                    ),
                    Err(SaveError::Failed(error)) => this.fail_save(
                        identity,
                        format!("Save failed: {error}"),
                        run.completion,
                        window,
                        cx,
                    ),
                }

                cx.emit(ReviewChanged::State);
                cx.notify();
            });
        })
        .detach();
    }

    fn fail_save(
        &mut self,
        identity: ReviewFileIdentity,
        error: String,
        completion: SaveCompletion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = Some(identity);
        self.message = Some(error);
        self.focus_active(window, cx);
        Self::complete(false, completion, window, cx);
    }

    fn complete(
        success: bool,
        completion: SaveCompletion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.defer(cx, move |window, cx| completion(success, window, cx));
    }

    fn selected_entry(&self) -> Option<&SessionEntry> {
        let selected = self.selected.as_ref()?;
        self.entries
            .iter()
            .find(|entry| &entry.file.identity == selected)
    }

    fn visible_identities(&self) -> Vec<ReviewFileIdentity> {
        self.entries
            .iter()
            .map(|entry| entry.file.identity.clone())
            .collect()
    }

    fn reveal_selected(&self) {
        let Some(selected) = self.selected.as_ref() else {
            return;
        };
        let Some(index) = self
            .visible_identities()
            .iter()
            .position(|identity| identity == selected)
        else {
            return;
        };

        self.navigator_scroll
            .scroll_to_item(self.element_index(index));
    }

    /// Maps an entry's position among the files to its position among the list's
    /// children, which also hold the base line and one heading per directory
    /// run. Scrolling addresses children, so the two indices only agree in a
    /// repository flat enough to need no headings at all.
    fn element_index(&self, entry_index: usize) -> usize {
        let visible = self.entries.iter().collect::<Vec<_>>();
        entry_index + leading_elements(&visible, entry_index)
    }

    fn move_file_selection(&mut self, offset: isize, window: &mut Window, cx: &mut Context<Self>) {
        let visible = self.visible_identities();
        if visible.is_empty() {
            return;
        }

        let current = self
            .selected
            .as_ref()
            .and_then(|selected| visible.iter().position(|identity| identity == selected));
        let next = current.map_or_else(
            || {
                if offset < 0 { visible.len() - 1 } else { 0 }
            },
            |current| {
                current
                    .saturating_add_signed(offset)
                    .min(visible.len().saturating_sub(1))
            },
        );

        self.navigator_focus_visible = true;
        self.select(visible[next].clone(), window, cx);
    }

    fn select_previous_file(
        &mut self,
        _: &SelectPreviousFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_file_selection(-1, window, cx);
    }

    fn select_next_file(
        &mut self,
        _: &SelectNextFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_file_selection(1, window, cx);
    }

    fn activate_selected_file(
        &mut self,
        _: &ActivateSelectedFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| editor.focus_leftmost_pane(window, cx));
        } else if self.selected_is_non_text() {
            self.non_text_body_focus.focus(window, cx);
        }
    }

    fn focus_navigator_action(
        &mut self,
        _: &FocusNavigator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_navigator(window, cx);
    }

    /// Sums the measured line counts across every file in the review.
    fn totals(&self) -> DiffStat {
        self.entries
            .iter()
            .filter_map(|entry| entry.file.stat)
            .fold(DiffStat::default(), |mut total, stat| {
                total.added += stat.added;
                total.removed += stat.removed;
                total
            })
    }

    /// Lays the navigator out as runs of files under the directory they share.
    /// Entries arrive sorted by path, so a run is simply a stretch of rows whose
    /// directory has not changed. Headings are relative to the directory the
    /// whole review shares, which is named once at the top instead of repeating
    /// at the front of every heading.
    fn render_file_groups(
        &self,
        visible: &[&SessionEntry],
        selected: Option<&ReviewFileIdentity>,
        navigator_focused: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let base = common_directory(visible.iter().map(|entry| entry.file.path()));
        let mut elements = Vec::with_capacity(visible.len() + 1);
        if let Some(base) = &base {
            elements.push(render_base_directory(base, cx));
        }

        let mut group: Option<PathBuf> = None;
        for (index, entry) in visible.iter().enumerate() {
            let directory = group_directory(entry.file.path(), base.as_deref());
            if group.as_ref() != Some(&directory) {
                if heading_is_shown(&directory, base.as_deref()) {
                    elements.push(render_directory_heading(&directory, index, cx));
                }
                group = Some(directory);
            }

            elements.push(
                self.render_file_row(index, visible.len(), entry, selected, navigator_focused, cx)
                    .into_any_element(),
            );
        }

        elements
    }

    fn render_file_row(
        &self,
        index: usize,
        set_size: usize,
        entry: &SessionEntry,
        selected: Option<&ReviewFileIdentity>,
        navigator_focused: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let identity = entry.file.identity.clone();
        let is_selected = selected == Some(&identity);
        let modified = self
            .editors
            .get(&identity)
            .is_some_and(|state| state.editor.read(cx).needs_save());
        let path = entry.file.path().to_string_lossy().into_owned();
        let (name, _) = path_labels(entry.file.path());
        let stat = entry.file.stat.filter(|stat| stat.total() > 0);
        let old_path = match &entry.file.status {
            super::model::ReviewFileStatus::Renamed { from } => {
                Some(format!("from {}", from.display()))
            }
            _ => None,
        };
        let warning = entry.warning.as_ref().map(EntryWarning::label);
        let badge = entry.file.status.badge();
        let badge_color = match &entry.file.status {
            super::model::ReviewFileStatus::Added => cx.theme().success,
            super::model::ReviewFileStatus::Modified => cx.theme().warning,
            super::model::ReviewFileStatus::Deleted
            | super::model::ReviewFileStatus::Conflicted => cx.theme().danger,
            super::model::ReviewFileStatus::Renamed { .. } => cx.theme().info,
        };
        let accessible_label = format!(
            "{}; {}; {}{}",
            entry.file.status.label(),
            path,
            if modified { "modified" } else { "unmodified" },
            warning.map_or_else(String::new, |warning| format!("; {warning}")),
        );

        div()
            .id(("review-file", index))
            .test_support()
            .role(AccessibilityRole::ListBoxOption)
            .aria_label(accessible_label)
            .aria_position_in_set(index + 1)
            .aria_size_of_set(set_size)
            .aria_selected(is_selected)
            .relative()
            .px(px(10.0))
            .py(px(6.0))
            .flex()
            .items_start()
            .gap(px(8.0))
            // Selection says which file is open; brightness says whether the
            // keyboard is in this list. One element carries both, so moving
            // focus does not light up a second part of the sidebar.
            .when(is_selected, |row| {
                row.bg(if navigator_focused {
                    cx.theme().selection
                } else {
                    cx.theme().accent
                })
            })
            .when(!is_selected, |row| {
                row.hover(|row| row.bg(cx.theme().secondary_hover))
            })
            .cursor_pointer()
            // Selection commits on press and focus moves on release. Pressing
            // focuses the list either way, so selecting on release would leave
            // a frame where the list is focused but the previous row is still
            // the selected one — lighting up the row being left behind. The
            // focus transfer waits for release because the press-time focus
            // change lands after this handler and would undo it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.navigator_focus_visible = false;
                    this.select(identity.clone(), window, cx);
                }),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.focus_active(window, cx);
            }))
            .children((is_selected && navigator_focused).then(|| {
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(2.0))
                    .bg(cx.theme().ring)
            }))
            .child(
                div()
                    .id(("review-status-badge", index))
                    .test_support()
                    .w(px(20.0))
                    .h(px(18.0))
                    .flex_shrink_0()
                    .text_center()
                    .text_color(badge_color)
                    .child(badge),
            )
            .child(render_file_labels(
                FileRowLabels {
                    index,
                    name,
                    old_path,
                    warning,
                    stat,
                    modified,
                    tooltip: path,
                },
                cx,
            ))
    }

    /// Names the change under review, then summarises how big it is. The label
    /// is whatever the provider called the source: a commit subject for Git, a
    /// changelist for Perforce.
    fn render_navigator_header(
        &self,
        visible_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let headline = self.source.headline.to_string();
        let tooltip = self.source.label.to_string();
        let totals = self.totals();

        div()
            .id("review-files-header")
            .test_support()
            .px(px(8.0))
            .py(px(6.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id("review-source-label")
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis_middle()
                            .font_weight(FontWeight::MEDIUM)
                            .child(headline)
                            .tooltip(move |window, cx| {
                                Tooltip::new(tooltip.clone()).build(window, cx)
                            }),
                    )
                    .child(
                        Button::new("refresh-review")
                            .icon(gpui_kit::assets::IconName::RefreshCw)
                            .ghost()
                            .with_size(px(28.0))
                            .accessibility_label("Refresh review")
                            .tooltip("Refresh review")
                            .disabled(self.refreshing || self.saving)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("review-source-summary")
                    .test_support()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(px(11.0))
                    .child(div().text_color(cx.theme().muted_foreground).child(
                        if visible_count == 1 {
                            format!("{} \u{b7} 1 file", self.source.kind)
                        } else {
                            format!("{} \u{b7} {visible_count} files", self.source.kind)
                        },
                    ))
                    .children((totals.added > 0).then(|| {
                        div()
                            .text_color(crate::appearance::added().marker)
                            .child(format!("+{}", totals.added))
                    }))
                    .children((totals.removed > 0).then(|| {
                        div()
                            .text_color(crate::appearance::removed().marker)
                            .child(format!("\u{2212}{}", totals.removed))
                    })),
            )
    }

    fn render_navigator(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.entries.iter().collect::<Vec<_>>();
        let selected = self.selected.clone();
        let visible_count = visible.len();
        let navigator_focused =
            self.navigator_focus.is_focused(window) && self.navigator_focus_visible;

        div()
            .w(px(280.0))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary)
            .child(self.render_navigator_header(visible_count, cx))
            .child(
                div()
                    .id("review-file-list")
                    .test_support()
                    .key_context(NAVIGATOR_KEY_CONTEXT)
                    .track_focus(&self.navigator_focus)
                    .tab_index(0)
                    .role(AccessibilityRole::ListBox)
                    .aria_label("Review files")
                    .aria_size_of_set(visible_count)
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .on_action(cx.listener(Self::select_previous_file))
                    .on_action(cx.listener(Self::select_next_file))
                    .on_action(cx.listener(Self::activate_selected_file))
                    .child(
                        div()
                            .id("review-file-scroll")
                            .size_full()
                            .track_scroll(&self.navigator_scroll)
                            .overflow_y_scroll()
                            .children(self.render_file_groups(
                                &visible,
                                selected.as_ref(),
                                navigator_focused,
                                cx,
                            ))
                            .vertical_scrollbar(&self.navigator_scroll),
                    ),
            )
    }

    fn render_placeholder(
        title: impl Into<gpui_kit::SharedString>,
        detail: impl Into<gpui_kit::SharedString>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .child(title.into())
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(detail.into()),
            )
    }

    fn render_body(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        if self.refreshing && self.entries.is_empty() {
            return Self::render_placeholder("Loading review", "Fetching the file manifest…", cx)
                .into_any_element();
        }
        if self.entries.is_empty() {
            return Self::render_placeholder("No changes", "This review source is clean.", cx)
                .into_any_element();
        }
        let Some(entry) = self.selected_entry() else {
            return Self::render_placeholder(
                "Select a file",
                "Choose a file from the navigator to inspect it.",
                cx,
            )
            .into_any_element();
        };

        match &entry.file.kind {
            ReviewFileKind::Text(_) => self.editors.get(&entry.file.identity).map_or_else(
                || {
                    Self::render_placeholder(
                        "Comparison unavailable",
                        "The file could not be opened.",
                        cx,
                    )
                    .into_any_element()
                },
                |state| state.editor.clone().into_any_element(),
            ),
            ReviewFileKind::Binary { explanation } => {
                Self::render_placeholder("Binary file", explanation.to_string(), cx)
                    .into_any_element()
            }
            ReviewFileKind::Submodule {
                old_identifier,
                new_identifier,
            } => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.0))
                .child("Submodule reference changed")
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{old_identifier} → {new_identifier}")),
                )
                .into_any_element(),
        }
    }

    fn render_conflict(
        conflict: &ReviewConflict,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let merge = conflict.merge();
        let (detail, tooltip) = match conflict {
            ReviewConflict::Mergeable(_) => (
                "This file has an unresolved merge conflict. Saving the merge result does not mark it resolved in Git.".to_owned(),
                "Open base, local and incoming versions in a three-way merge".to_owned(),
            ),
            ReviewConflict::Unmergeable { reason } => (
                format!("This file has an unresolved merge conflict. {reason}"),
                "This conflict cannot be merged as text".to_owned(),
            ),
        };

        div()
            .id("review-file-conflict")
            .test_support()
            .flex_shrink_0()
            .px(px(12.0))
            .py(px(6.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().muted)
            .aria_label(detail.clone())
            .child(div().flex_1().min_w_0().child(detail))
            .child(
                Button::new("start-conflict-merge")
                    .label("Start three-way merge")
                    .small()
                    .primary()
                    .disabled(merge.is_none())
                    .tooltip(tooltip)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        if let Some(merge) = merge.clone() {
                            cx.emit(ReviewChanged::MergeRequested(merge));
                        }
                    })),
            )
    }
}

impl Render for ReviewSession {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let message = self.message.clone();
        let warning = self
            .selected_entry()
            .and_then(|entry| entry.warning.as_ref())
            .map(|warning| warning.detail().to_owned());
        let conflict = self
            .selected_entry()
            .and_then(|entry| entry.file.conflict.clone())
            .map(|conflict| Self::render_conflict(&conflict, cx));
        let non_text_body = self.selected_is_non_text();
        let body = self.render_body(cx);

        div()
            .id("review-session")
            .size_full()
            .flex()
            .overflow_hidden()
            .on_action(cx.listener(Self::toggle_word_wrap))
            .child(self.render_navigator(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .children(message.map(|message| {
                        div()
                            .flex_shrink_0()
                            .px(px(12.0))
                            .py(px(6.0))
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().muted)
                            .child(message)
                    }))
                    .children(warning.map(|warning| {
                        div()
                            .id("review-file-warning")
                            .test_support()
                            .flex_shrink_0()
                            .px(px(12.0))
                            .py(px(6.0))
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().muted)
                            .aria_label(warning.clone())
                            .child(warning)
                    }))
                    .children(conflict)
                    .child(
                        div()
                            .id("review-file-body")
                            .test_support()
                            .flex_1()
                            .min_h_0()
                            .when(non_text_body, |body| {
                                body.key_context(NON_TEXT_BODY_KEY_CONTEXT)
                                    .track_focus(&self.non_text_body_focus)
                                    .tab_index(0)
                                    .on_action(cx.listener(Self::focus_navigator_action))
                            })
                            .child(body),
                    ),
            )
    }
}

pub(crate) fn fixed_key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("up", SelectPreviousFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("k", SelectPreviousFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("down", SelectNextFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("j", SelectNextFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("enter", ActivateSelectedFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("space", ActivateSelectedFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("ctrl-l", ActivateSelectedFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("ctrl-h", FocusNavigator, Some(NON_TEXT_BODY_KEY_CONTEXT)),
    ]
}

#[cfg(test)]
impl ReviewSession {
    pub(crate) fn selected_identity(&self) -> Option<&ReviewFileIdentity> {
        self.selected.as_ref()
    }

    pub(crate) fn editor_count(&self) -> usize {
        self.editors.len()
    }

    pub(crate) fn is_removed(&self, identity: &ReviewFileIdentity) -> bool {
        self.entries
            .iter()
            .find(|entry| &entry.file.identity == identity)
            .is_some_and(|entry| matches!(entry.warning, Some(EntryWarning::RemovedUpstream)))
    }

    pub(crate) fn warning(&self, identity: &ReviewFileIdentity) -> Option<&'static str> {
        self.entries
            .iter()
            .find(|entry| &entry.file.identity == identity)
            .and_then(|entry| entry.warning.as_ref())
            .map(EntryWarning::label)
    }

    pub(crate) fn navigator_focus_is_visible(&self) -> bool {
        self.navigator_focus_visible
    }

    pub(crate) fn navigator_is_scrolled(&self) -> bool {
        self.navigator_scroll.offset().y < px(0.0)
    }

    pub(crate) fn editor(&self, identity: &ReviewFileIdentity) -> Option<Entity<AlignedEditor>> {
        self.editors.get(identity).map(|state| state.editor.clone())
    }
}

#[cfg(test)]
mod layout_tests {
    use super::super::model::ReviewFileStatus;
    use super::{
        ReviewFile, ReviewFileIdentity, SessionEntry, common_directory, group_directory,
        heading_is_shown, leading_elements,
    };
    use std::path::{Path, PathBuf};

    fn entry(path: &str) -> SessionEntry {
        SessionEntry {
            file: ReviewFile::binary(
                ReviewFileIdentity::new(path),
                PathBuf::from(path),
                ReviewFileStatus::Modified,
                "Binary content cannot be displayed.",
            ),
            warning: None,
        }
    }

    fn base(paths: &[&str]) -> Option<PathBuf> {
        common_directory(paths.iter().map(Path::new))
    }

    #[test]
    fn the_shared_base_is_the_deepest_directory_every_file_sits_under() {
        assert_eq!(
            base(&[
                "crates/yori/src/review/git.rs",
                "crates/yori/src/editor/chrome.rs"
            ]),
            Some(PathBuf::from("crates/yori/src"))
        );
        // One file contributes its whole directory.
        assert_eq!(
            base(&["crates/yori/src/review/git.rs"]),
            Some(PathBuf::from("crates/yori/src/review"))
        );
        // A file at the root leaves nothing to share.
        assert_eq!(base(&["crates/yori/src/review/git.rs", "README.md"]), None);
        assert_eq!(base(&[]), None);
    }

    /// Scrolling addresses the list's children, so every base line and heading
    /// above a row shifts that row's index. The keyboard scroll test cannot
    /// catch a mistake here because its fixture is one flat directory.
    #[test]
    fn scroll_indices_count_the_base_line_and_every_heading_above_a_row() {
        let entries = [
            entry("crates/yori/src/lib.rs"),
            entry("crates/yori/src/editor/mod.rs"),
            entry("crates/yori/src/review/git.rs"),
            entry("crates/yori/src/review/model.rs"),
        ];
        let entries = entries.iter().collect::<Vec<_>>();

        // The base line counts for every row; a file sitting directly in the
        // base adds no heading of its own.
        assert_eq!(leading_elements(&entries, 0), 1);
        assert_eq!(leading_elements(&entries, 1), 2);
        assert_eq!(leading_elements(&entries, 2), 3);
        // The fourth file shares the third's heading, so nothing new precedes it.
        assert_eq!(leading_elements(&entries, 3), 3);
    }

    #[test]
    fn a_root_file_takes_a_heading_when_the_review_shares_no_base() {
        let entries = [entry("README.md"), entry("src/main.rs")];
        let entries = entries.iter().collect::<Vec<_>>();

        // No shared base means no base line, but the root file still needs its
        // own heading or it would read as part of the run below it.
        assert_eq!(leading_elements(&entries, 0), 1);
        assert_eq!(leading_elements(&entries, 1), 2);
    }

    #[test]
    fn headings_are_relative_to_the_base_and_vanish_inside_it() {
        let base = PathBuf::from("crates/yori/src");
        let directory = group_directory(Path::new("crates/yori/src/review/git.rs"), Some(&base));
        assert_eq!(directory, PathBuf::from("review"));
        assert!(heading_is_shown(&directory, Some(&base)));

        // A file directly in the base is already named by the base line.
        let directory = group_directory(Path::new("crates/yori/src/lib.rs"), Some(&base));
        assert_eq!(directory, PathBuf::from(""));
        assert!(!heading_is_shown(&directory, Some(&base)));

        // Without a base, a root file still needs a heading of its own.
        let directory = group_directory(Path::new("README.md"), None);
        assert_eq!(directory, PathBuf::from(""));
        assert!(heading_is_shown(&directory, None));
    }
}
