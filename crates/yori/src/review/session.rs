//! Native review-session surface and lifecycle.

use std::{collections::HashMap, collections::VecDeque, rc::Rc};

use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement,
};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, TestSupportExt,
    Window, div, prelude::FluentBuilder, px,
};

use crate::{
    comparison::Comparison,
    editor::{AlignedEditor, DirtyChanged, PaneDocument},
    storage::SaveError,
    workspace::files::{Files, Role},
};

use super::model::{
    ReviewFile, ReviewFileIdentity, ReviewFileKind, ReviewManifest, ReviewSource, TextComparison,
};

pub(crate) struct ReviewChanged;

impl EventEmitter<ReviewChanged> for ReviewSession {}

struct SessionEntry {
    file: ReviewFile,
    removed: bool,
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
            files.document(Role::Baseline).clone(),
        );
        let right = PaneDocument::new(
            diff.local.logical_path().to_owned(),
            files.document(Role::Local).clone(),
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
    refreshing: bool,
    saving: bool,
    message: Option<String>,
}

impl ReviewSession {
    pub(crate) fn new(source: ReviewSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files"));
        let subscription = cx.subscribe(&query, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
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
        } else {
            self.query.focus_handle(cx).focus(window, cx);
        }
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
                match result.and_then(|manifest| this.apply_manifest(manifest, window, cx)) {
                    Ok(()) => this.message = None,
                    Err(error) => this.message = Some(format!("Refresh failed: {error}")),
                }

                cx.emit(ReviewChanged);
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
    ) -> Result<(), String> {
        ReviewManifest::new(manifest.files.clone())?;

        let mut prepared = HashMap::new();
        for file in &manifest.files {
            if self.editors.contains_key(&file.identity)
                && let ReviewFileKind::Text(comparison) = &file.kind
            {
                prepared.insert(file.identity.clone(), LoadedText::load(comparison)?);
            }
        }

        let old_entries = self
            .entries
            .drain(..)
            .map(|entry| (entry.file.identity.clone(), entry))
            .collect::<HashMap<_, _>>();
        let incoming = manifest
            .files
            .iter()
            .map(|file| file.identity.clone())
            .collect::<std::collections::HashSet<_>>();

        for file in &manifest.files {
            if let Some(state) = self.editors.get_mut(&file.identity)
                && let Some(loaded) = prepared.remove(&file.identity)
            {
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
            }
        }

        self.entries = manifest
            .files
            .into_iter()
            .map(|file| SessionEntry {
                file,
                removed: false,
            })
            .collect();

        for (identity, entry) in old_entries {
            if incoming.contains(&identity) {
                continue;
            }

            let dirty = self
                .editors
                .get(&identity)
                .is_some_and(|state| state.editor.read(cx).needs_save());
            if dirty {
                self.entries.push(SessionEntry {
                    file: entry.file,
                    removed: true,
                });
            } else {
                self.editors.remove(&identity);
            }
        }

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

        Ok(())
    }

    fn ensure_editor(
        &mut self,
        identity: &ReviewFileIdentity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.editors.contains_key(identity) {
            return Ok(());
        }
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| &entry.file.identity == identity)
        else {
            return Ok(());
        };
        let ReviewFileKind::Text(comparison) = &entry.file.kind else {
            return Ok(());
        };
        let loaded = LoadedText::load(comparison)?;
        let editor = cx.new(|cx| {
            AlignedEditor::new_diff(
                loaded.left,
                loaded.right,
                loaded.editable,
                loaded.saveable,
                window,
                cx,
            )
        });
        let subscription = cx.subscribe(&editor, |_, _, _: &DirtyChanged, cx| {
            cx.emit(ReviewChanged);
            cx.notify();
        });

        self.editors.insert(
            identity.clone(),
            ReviewEditor {
                editor,
                _subscription: subscription,
                files: loaded.files,
            },
        );

        Ok(())
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

                cx.emit(ReviewChanged);
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

    fn render_file_row(
        &self,
        index: usize,
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
        let removed = entry.removed;
        let badge = entry.file.status.badge();

        div()
            .id(("review-file", index))
            .test_support()
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
            .children(removed.then(|| {
                div()
                    .text_color(crate::appearance::removed().marker)
                    .child("removed upstream")
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
        let selected = self.selected.clone();

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
            .child(div().flex_1().min_h_0().overflow_y_scrollbar().children(
                visible.into_iter().enumerate().map(|(index, entry)| {
                    self.render_file_row(index, entry, selected.as_ref(), cx)
                }),
            ))
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
                    .children((refreshing || message.is_some()).then(|| {
                        div()
                            .flex_shrink_0()
                            .px(px(12.0))
                            .py(px(6.0))
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().muted)
                            .child(message.unwrap_or_else(|| "Refreshing review…".into()))
                    }))
                    .child(div().flex_1().min_h_0().child(self.render_body(cx))),
            )
    }
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
            .is_some_and(|entry| entry.removed)
    }

    pub(crate) fn editor(&self, identity: &ReviewFileIdentity) -> Option<Entity<AlignedEditor>> {
        self.editors.get(identity).map(|state| state.editor.clone())
    }
}
