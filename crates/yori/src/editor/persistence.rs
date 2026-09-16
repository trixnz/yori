//! Saved checkpoints and reloads retain editor ownership of source and history.

use gpui_kit::Context;
use yori::navigation::ChangeNavigation;
use yori_diff::Alignment;
use yori_document::Document;
use yori_document::editing::EditHistory;

use super::{AlignedEditor, DirtyChanged, DirtyState, PaneDocument};

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

    pub(crate) fn reload_diff(
        &mut self,
        baseline: bool,
        document: Document,
        cx: &mut Context<Self>,
    ) {
        self.deactivate(cx);
        let old = if baseline { &self.left } else { &self.right };
        let mut replacement = PaneDocument::new(old.path.clone(), document);
        replacement.editable = old.editable;
        replacement.saveable = old.saveable;
        replacement.set_language(old.language_override);
        if baseline {
            self.left = replacement;
        } else {
            self.right = replacement;
            self.history = EditHistory::default();
            self.dirty = DirtyState::new(self.right.document.text());
        }

        self.alignment = Alignment::between(&self.left.document, &self.right.document);
        self.navigation = ChangeNavigation::default();
        self.selection = None;
        self.preferred_column = None;
        self.hovered_connection = None;
        self.sync_vim_selection(cx);

        cx.emit(DirtyChanged);
        cx.notify();
    }
}
