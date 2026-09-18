//! Native review-session surface and lifecycle.

use std::{collections::HashMap, collections::VecDeque, rc::Rc};

use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement,
};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, Render, Role as AccessibilityRole, ScrollHandle,
    StatefulInteractiveElement, Styled, Subscription, TestSupportExt, Window, div,
    prelude::FluentBuilder, px,
};

use crate::{
    comparison::Comparison,
    editor::{AlignedEditor, DirtyChanged, PaneDocument},
    storage::SaveError,
    workspace::files::{Files, Role as DocumentRole},
};

use super::model::{
    ReviewFile, ReviewFileIdentity, ReviewFileKind, ReviewManifest, ReviewSource, TextComparison,
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
    RefreshCompleted { activate: bool },
    ActiveEditorChanged { transfer_focus: bool },
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
    _subscription: Subscription,
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
        let capabilities = comparison_capabilities(comparison);

        Ok(Self {
            files,
            left,
            right,
            editable: capabilities.0,
            saveable: capabilities.1,
        })
    }
}

fn comparison_capabilities(comparison: Comparison) -> (bool, bool) {
    let Comparison::Diff(diff) = comparison else {
        unreachable!("review text entries are always two-way comparisons");
    };

    (
        diff.local.editable(),
        diff.local.save_destination().is_some(),
    )
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
    query: Entity<InputState>,
    _query_subscription: Subscription,
    navigator_focus: FocusHandle,
    navigator_scroll: ScrollHandle,
    navigator_selection: Option<ReviewFileIdentity>,
    non_text_body_focus: FocusHandle,
    refreshing: bool,
    saving: bool,
    message: Option<String>,
}

impl ReviewSession {
    pub(crate) fn new(source: ReviewSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files"));
        let subscription = cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.navigator_selection = None;
                this.navigator_scroll.scroll_to_item(0);
                cx.notify();
            }
        });

        Self {
            source,
            entries: Vec::new(),
            selected: None,
            editors: HashMap::new(),
            query,
            _query_subscription: subscription,
            navigator_focus: cx.focus_handle(),
            navigator_scroll: ScrollHandle::new(),
            navigator_selection: None,
            non_text_body_focus: cx.focus_handle(),
            refreshing: false,
            saving: false,
            message: None,
        }
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

    pub(crate) fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            editor.focus_handle(cx).focus(window, cx);
        } else if self.selected_is_non_text() {
            self.non_text_body_focus.focus(window, cx);
        } else {
            self.query.focus_handle(cx).focus(window, cx);
        }
    }

    pub(crate) fn focus_navigator_or_filter(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.entries.is_empty() {
            self.query.focus_handle(cx).focus(window, cx);
        } else {
            self.navigator_focus.focus(window, cx);
        }
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
        let load = cx
            .background_executor()
            .spawn(async move { provider.load_manifest(&identity) });
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

        self.reconcile_navigator_selection(cx);

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
        let editor = cx.new(|cx| {
            AlignedEditor::new_review_diff(
                loaded.left,
                loaded.right,
                loaded.editable,
                loaded.saveable,
                window,
                cx,
            )
        });
        let subscribed_identity = identity.clone();
        let subscription = cx.subscribe_in(
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

        self.editors.insert(
            identity.clone(),
            ReviewEditor {
                editor,
                _subscription: subscription,
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
        self.navigator_selection = Some(identity.clone());
        self.selected = Some(identity);
        self.focus_active(window, cx);
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

    fn visible_identities(&self, cx: &App) -> Vec<ReviewFileIdentity> {
        let query = self.query.read(cx).text().to_string();
        self.entries
            .iter()
            .filter(|entry| entry.file.matches_query(&query))
            .map(|entry| entry.file.identity.clone())
            .collect()
    }

    fn reconcile_navigator_selection(&mut self, cx: &App) {
        let navigator_exists = self.navigator_selection.as_ref().is_some_and(|selected| {
            self.entries
                .iter()
                .any(|entry| &entry.file.identity == selected)
        });
        if !navigator_exists {
            self.navigator_selection = self.selected.clone();
        }

        self.reveal_navigator_selection(cx);
    }

    fn reveal_navigator_selection(&self, cx: &App) {
        let Some(selected) = self.navigator_selection.as_ref() else {
            return;
        };
        let Some(index) = self
            .visible_identities(cx)
            .iter()
            .position(|identity| identity == selected)
        else {
            return;
        };

        self.navigator_scroll.scroll_to_item(index);
    }

    fn move_navigator_selection(&mut self, offset: isize, cx: &mut Context<Self>) {
        let visible = self.visible_identities(cx);
        if visible.is_empty() {
            self.navigator_selection = None;
            cx.notify();
            return;
        }

        let current = self
            .navigator_selection
            .as_ref()
            .or(self.selected.as_ref())
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
        self.navigator_selection = Some(visible[next].clone());
        self.navigator_scroll.scroll_to_item(next);
        cx.notify();
    }

    fn select_previous_file(
        &mut self,
        _: &SelectPreviousFile,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_navigator_selection(-1, cx);
    }

    fn select_next_file(&mut self, _: &SelectNextFile, _: &mut Window, cx: &mut Context<Self>) {
        self.move_navigator_selection(1, cx);
    }

    fn activate_selected_file(
        &mut self,
        _: &ActivateSelectedFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(identity) = self.navigator_selection.clone() {
            self.select(identity, window, cx);
        }
    }

    fn focus_navigator_action(
        &mut self,
        _: &FocusNavigator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigator_focus.focus(window, cx);
    }

    fn render_file_row(
        &self,
        index: usize,
        set_size: usize,
        entry: &SessionEntry,
        selected: Option<&ReviewFileIdentity>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let identity = entry.file.identity.clone();
        let is_selected = selected == Some(&identity);
        let modified = self
            .editors
            .get(&identity)
            .is_some_and(|state| state.editor.read(cx).needs_save());
        let path = entry.file.path().to_string_lossy().into_owned();
        let old_path = match &entry.file.status {
            super::model::ReviewFileStatus::Renamed { from } => {
                Some(format!("from {}", from.display()))
            }
            _ => None,
        };
        let warning = entry.warning.as_ref().map(EntryWarning::label);
        let badge = entry.file.status.badge();
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
            .px(px(10.0))
            .py(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .border_b_1()
            .border_color(cx.theme().border)
            .when(is_selected, |row| row.bg(cx.theme().accent))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select(identity.clone(), window, cx);
            }))
            .child(
                div()
                    .w(px(20.0))
                    .flex_shrink_0()
                    .text_center()
                    .text_color(cx.theme().muted_foreground)
                    .child(badge),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(path)
                    .children(old_path.map(|path| {
                        div()
                            .text_size(px(11.0))
                            .text_color(cx.theme().muted_foreground)
                            .child(path)
                    })),
            )
            .children(warning.map(|warning| {
                div()
                    .text_color(crate::appearance::removed().marker)
                    .child(warning)
            }))
            .child(div().size(px(6.0)).rounded_full().bg(if modified {
                cx.theme().foreground
            } else {
                cx.theme().transparent
            }))
    }

    fn render_navigator(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.query.read(cx).text().to_string();
        let visible = self
            .entries
            .iter()
            .filter(|entry| entry.file.matches_query(&query))
            .collect::<Vec<_>>();
        let selected = self
            .navigator_selection
            .as_ref()
            .or(self.selected.as_ref())
            .cloned();
        let visible_count = visible.len();

        div()
            .w(px(280.0))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary)
            .child(
                div()
                    .p(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.query)
                                .id("review-file-filter")
                                .prefix(Icon::new(gpui_kit::assets::IconName::Search))
                                .cleanable(true)
                                .with_size(px(28.0))
                                .aria_label("Filter review files"),
                        ),
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
                            .children(visible.into_iter().enumerate().map(|(index, entry)| {
                                self.render_file_row(
                                    index,
                                    visible_count,
                                    entry,
                                    selected.as_ref(),
                                    cx,
                                )
                            }))
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
}

impl Render for ReviewSession {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let message = self.message.clone();
        let refreshing = self.refreshing && !self.entries.is_empty();
        let warning = self
            .selected_entry()
            .and_then(|entry| entry.warning.as_ref())
            .map(|warning| warning.detail().to_owned());
        let non_text_body = self.selected_is_non_text();
        let body = self.render_body(cx);

        div()
            .id("review-session")
            .size_full()
            .flex()
            .overflow_hidden()
            .child(self.render_navigator(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .children(refreshing.then(|| {
                        div()
                            .flex_shrink_0()
                            .px(px(12.0))
                            .py(px(6.0))
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().muted)
                            .child("Refreshing review…")
                    }))
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

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPreviousFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("k", SelectPreviousFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("down", SelectNextFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("j", SelectNextFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("enter", ActivateSelectedFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("space", ActivateSelectedFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("right", ActivateSelectedFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("l", ActivateSelectedFile, Some(NAVIGATOR_KEY_CONTEXT)),
        KeyBinding::new("left", FocusNavigator, Some(NON_TEXT_BODY_KEY_CONTEXT)),
        KeyBinding::new("h", FocusNavigator, Some(NON_TEXT_BODY_KEY_CONTEXT)),
    ]);
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

    pub(crate) fn focus_navigator(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigator_focus.focus(window, cx);
    }

    pub(crate) fn navigator_is_scrolled(&self) -> bool {
        self.navigator_scroll.offset().y < px(0.0)
    }

    pub(crate) fn editor(&self, identity: &ReviewFileIdentity) -> Option<Entity<AlignedEditor>> {
        self.editors.get(identity).map(|state| state.editor.clone())
    }
}
