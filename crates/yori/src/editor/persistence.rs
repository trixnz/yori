//! Saved checkpoints and reloads retain editor ownership of source and history.

use gpui_kit::{Context, Window};
use yori::navigation::ChangeNavigation;
use yori_diff::Alignment;
use yori_document::Document;
use yori_document::editing::EditHistory;

use super::{AlignedEditor, DirtyChanged, DirtyState, PaneDocument};

fn normalize_offset(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }

    offset
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SaveCheckpoint {
    pub text: String,
    resolutions: Vec<bool>,
}

impl AlignedEditor {
    pub(crate) fn needs_save(&self) -> bool {
        self.can_edit() && (!self.dirty.saved_to_disk || self.is_dirty())
    }

    pub(crate) fn set_saving(&mut self, saving: bool, cx: &mut Context<Self>) {
        self.saving = saving;
        cx.notify();
    }

    pub(crate) fn unresolved_count(&self) -> usize {
        self.merge
            .as_ref()
            .map_or(0, |merge| merge.session.unresolved().count())
    }

    fn resolution_statuses(&self) -> Vec<bool> {
        self.merge.as_ref().map_or_else(Vec::new, |merge| {
            merge
                .session
                .conflicts()
                .iter()
                .map(|conflict| {
                    merge
                        .session
                        .state(conflict.id)
                        .is_some_and(|state| state.resolved)
                })
                .collect()
        })
    }

    pub(crate) fn prepare_save(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<SaveCheckpoint, String> {
        self.deactivate(cx);
        if !self.can_save() {
            return Err("This document has no save destination.".into());
        }

        let unresolved = self.unresolved_count();
        if unresolved != 0 {
            return Err(format!(
                "Resolve the remaining {unresolved} conflicts before saving RESULT."
            ));
        }

        Ok(self.current_checkpoint())
    }

    pub(crate) fn current_checkpoint(&self) -> SaveCheckpoint {
        SaveCheckpoint {
            text: self.right.document.text().to_owned(),
            resolutions: self.resolution_statuses(),
        }
    }

    pub(crate) fn mark_saved(&mut self, checkpoint: SaveCheckpoint, cx: &mut Context<Self>) {
        self.dirty.original = checkpoint.text;
        self.dirty.update(self.right.document.text());
        self.dirty.saved_resolutions = checkpoint.resolutions;
        self.dirty.saved_to_disk = true;

        cx.emit(DirtyChanged);
        cx.notify();
    }

    pub(crate) fn refresh_review_diff(
        &mut self,
        mut left: PaneDocument,
        mut right: PaneDocument,
        editable: bool,
        saveable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selection = self.selection.clone();
        let replace_local = !self.needs_save();

        left.editable = false;
        left.saveable = false;
        left.change_language(self.left.language_override);
        right.editable = editable;
        right.saveable = editable && saveable;
        right.change_language(self.right.language_override);
        self.left = left;
        if replace_local {
            self.right = right;
            self.history = EditHistory::default();
            self.dirty = DirtyState::new(self.right.document.text());
        }

        self.alignment = Alignment::between(&self.left.document, &self.right.document);
        self.invalidate_wrap_projection();
        self.navigation = ChangeNavigation::default();
        self.schedule_highlighting(super::Side::Left, window, cx);
        if replace_local {
            self.schedule_highlighting(super::Side::Right, window, cx);
        }
        self.selection = selection.map(|mut selection| {
            let text = self.document(selection.side).document.text();
            selection.anchor = normalize_offset(text, selection.anchor);
            selection.head = normalize_offset(text, selection.head);
            selection
        });
        self.preferred_column = None;
        self.preferred_visual_x = None;
        self.hovered_connection = None;
        self.sync_vim_selection(cx);

        cx.emit(DirtyChanged);
        cx.notify();
    }

    pub(crate) fn reload_diff(
        &mut self,
        baseline: bool,
        document: Document,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.deactivate(cx);
        let old = if baseline { &self.left } else { &self.right };
        let mut replacement = PaneDocument::new(old.path.clone(), document);
        replacement.editable = old.editable;
        replacement.saveable = old.saveable;
        replacement.change_language(old.language_override);
        if baseline {
            self.left = replacement;
        } else {
            self.right = replacement;
            self.history = EditHistory::default();
            self.dirty = DirtyState::new(self.right.document.text());
        }

        self.alignment = Alignment::between(&self.left.document, &self.right.document);
        self.invalidate_wrap_projection();
        self.navigation = ChangeNavigation::default();
        self.schedule_highlighting(
            if baseline {
                super::Side::Left
            } else {
                super::Side::Right
            },
            window,
            cx,
        );
        self.selection = None;
        self.preferred_column = None;
        self.preferred_visual_x = None;
        self.hovered_connection = None;
        self.sync_vim_selection(cx);

        cx.emit(DirtyChanged);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::Root;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext, TestAppContext};

    use super::*;
    use crate::editor::{Selection, Side};

    fn pane(path: &str, text: &str) -> PaneDocument {
        PaneDocument::new_highlighted(
            path.into(),
            Document::from_bytes(text.as_bytes().to_vec()).unwrap(),
        )
    }

    fn initialize(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::appearance::init(cx);
            crate::editor::init(cx);
        });
    }

    #[gpui_kit::test]
    fn clean_review_refresh_normalizes_unicode_caret(cx: &mut TestAppContext) {
        initialize(cx);
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                AlignedEditor::new(
                    pane("old.rs", "baseline\n"),
                    pane("new.rs", "abcd\n"),
                    window,
                    cx,
                )
            });
            editor = Some(view.clone());
            Root::new(view, window, cx)
        });
        let editor = editor.unwrap();

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.selection = Some(Selection {
                    side: Side::Right,
                    anchor: 1,
                    head: 1,
                });

                editor.refresh_review_diff(
                    pane("old.rs", "refreshed baseline\n"),
                    pane("new.rs", "éx\n"),
                    true,
                    true,
                    window,
                    cx,
                );

                let selection = editor.selection.as_ref().unwrap();
                assert_eq!(selection.range(), 0..0);
                assert!(
                    editor
                        .right
                        .document
                        .text()
                        .is_char_boundary(selection.head)
                );
            });
        });
    }

    #[gpui_kit::test]
    fn clean_review_refresh_normalizes_unicode_selection(cx: &mut TestAppContext) {
        initialize(cx);
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                AlignedEditor::new(
                    pane("old.rs", "baseline\n"),
                    pane("new.rs", "abcdef\n"),
                    window,
                    cx,
                )
            });
            editor = Some(view.clone());
            Root::new(view, window, cx)
        });
        let editor = editor.unwrap();

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.selection = Some(Selection {
                    side: Side::Right,
                    anchor: 1,
                    head: 3,
                });

                editor.refresh_review_diff(
                    pane("old.rs", "refreshed baseline\n"),
                    pane("new.rs", "éx\n"),
                    true,
                    true,
                    window,
                    cx,
                );

                let selection = editor.selection.as_ref().unwrap();
                assert_eq!(selection.range(), 0..3);
                assert!(
                    editor
                        .right
                        .document
                        .text()
                        .is_char_boundary(selection.anchor)
                );
                assert!(
                    editor
                        .right
                        .document
                        .text()
                        .is_char_boundary(selection.head)
                );
            });
        });
    }

    #[gpui_kit::test]
    fn review_refresh_preserves_view_options_and_dirty_local_content(cx: &mut TestAppContext) {
        initialize(cx);
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                AlignedEditor::new(
                    pane("old.rs", "baseline\n"),
                    pane("new.rs", "local\n"),
                    window,
                    cx,
                )
            });
            editor = Some(view.clone());
            Root::new(view, window, cx)
        });
        let editor = editor.unwrap();
        cx.update(TestWindowExt::render_frame);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.selection = Some(Selection {
                    side: Side::Right,
                    anchor: 1,
                    head: 4,
                });
                editor.vertical_scroll = 44.0;
                editor.horizontal_scroll = 7.0;
                editor.show_whitespace = true;
                editor.show_connections = true;
                editor
                    .left
                    .set_language(Some(yori::document_info::Language::Rust));
                editor
                    .right
                    .set_language(Some(yori::document_info::Language::Go));
                editor.dirty.modified = true;

                editor.refresh_review_diff(
                    pane("old.rs", "refreshed baseline\n"),
                    pane("new.rs", "refreshed local\n"),
                    true,
                    true,
                    window,
                    cx,
                );

                assert_eq!(editor.left.document.text(), "refreshed baseline\n");
                assert_eq!(editor.right.document.text(), "local\n");
                assert_eq!(editor.selection.as_ref().unwrap().range(), 1..4);
                assert!((editor.vertical_scroll - 44.0).abs() < f32::EPSILON);
                assert!((editor.horizontal_scroll - 7.0).abs() < f32::EPSILON);
                assert!(editor.show_whitespace);
                assert!(editor.show_connections);
                assert_eq!(editor.left.language(), yori::document_info::Language::Rust);
                assert_eq!(editor.right.language(), yori::document_info::Language::Go);
                assert!(editor.needs_save());
            });
        });
    }
}
