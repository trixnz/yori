//! Three-way merging on the shared native editor. The session owns
//! canonical result text/history; `PaneDocument` is its synchronized rendering cache.

mod chrome;
mod controls;
mod layout;
mod selection;
#[cfg(test)]
mod tests;

use gpui_kit::{ClipboardItem, Context, Window};
use yori::geometry::display_units;
use yori_diff::{
    Alignment,
    merge::{ConflictId, MergeError, MergeInput, MergeRow, MergeSession, MergeUpdate, Take},
};
#[cfg(test)]
use yori_document::Document;
use yori_document::editing::TextSelection;

use super::completion::{Placement, ViewAnchor};
use super::{AlignedEditor, LINE_HEIGHT, PaneDocument, Selection, Side};

pub(super) use layout::{BaseRow, MergeDisplay, RowKind};

pub(super) struct MergeState {
    pub session: MergeSession,
    pub incoming: PaneDocument,
    pub incoming_alignment: Alignment,
    pub revision: u64,
    pub hovered_lines: Option<MergeInput>,
    pub base_columns: usize,
    pub display: MergeDisplay,
    pub hovered: Option<(ConflictId, Take)>,
    pub current: Option<ConflictId>,
    pub show_base: bool,
}

impl AlignedEditor {
    #[cfg(test)]
    pub(crate) fn merge_fixture(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let source = |text: &str| {
            let text = text.replace("\r\n", "\n");

            Document::from_bytes(text.into_bytes()).expect("valid merge fixture")
        };
        let session = MergeSession::new(
            source(include_str!("../../fixtures/merge/base.rs")),
            source(include_str!("../../fixtures/merge/local.rs")),
            source(include_str!("../../fixtures/merge/incoming.rs")),
        )
        .expect("valid merge fixture");

        Self::from_merge_session(session, window, cx)
    }

    #[cfg(test)]
    pub(super) fn from_merge_session(
        session: MergeSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let paths = crate::comparison::MergePaths {
            base: "base.rs".into(),
            local: "local.rs".into(),
            incoming: "incoming.rs".into(),
            result: "result.rs".into(),
        };

        Self::new_merge(&paths, session, window, cx)
    }

    pub(crate) fn new_merge(
        paths: &crate::comparison::MergePaths,
        session: MergeSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let local = PaneDocument::new(paths.local.clone(), session.local().clone());
        let result = PaneDocument::new(paths.result.clone(), session.result().clone());
        let incoming = PaneDocument::new(paths.incoming.clone(), session.incoming().clone());
        let incoming_alignment = Alignment::between(session.incoming(), session.result());
        let base_columns = super::max_display_columns(session.base(), super::TAB_WIDTH);

        let current = session.conflicts().first().map(|conflict| conflict.id);
        let offset = current
            .and_then(|id| session.state(id))
            .map_or(0, |state| state.result.start);

        let mut editor = Self::new_merge_base(local, result, window, cx);
        editor.merge = Some(MergeState {
            session,
            incoming,
            incoming_alignment,
            revision: 0,
            hovered_lines: None,
            base_columns,
            display: MergeDisplay::default(),
            hovered: None,
            current,
            show_base: false,
        });
        editor.dirty.saved_to_disk = false;
        editor.selection = Some(Selection {
            side: Side::Right,
            anchor: offset,
            head: offset,
        });
        editor.refresh_merge_projection();
        editor.schedule_highlighting(Side::Incoming, window, cx);

        editor
    }

    pub(super) fn refresh_merge_projection(&mut self) {
        let Some(merge) = &mut self.merge else {
            return;
        };

        merge.display =
            MergeDisplay::build(&merge.session, merge.current.filter(|_| merge.show_base));
        merge.hovered = None;
        merge.revision += 1;
        merge.hovered_lines = None;
        merge.incoming_alignment = Alignment::from_projection(
            &merge.incoming.document,
            &self.right.document,
            merge
                .display
                .rows()
                .iter()
                .map(|row| (row.sources.incoming, row.sources.result)),
        );

        self.alignment = Alignment::from_projection(
            &self.left.document,
            &self.right.document,
            merge
                .display
                .rows()
                .iter()
                .map(|row| (row.sources.local, row.sources.result)),
        );
    }

    pub(super) fn merge_take(
        &mut self,
        id: ConflictId,
        take: Take,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_vim();
        self.finish_composition();

        let selection = self.conflict_action_selection(id);
        let anchor = self.view_anchor();
        let merge = self.merge.as_mut().expect("merge mode");
        let update = merge.session.take(id, take, selection);

        self.finish_merge_action(anchor, id, update, window, cx);
    }

    pub(super) fn merge_reset(
        &mut self,
        id: ConflictId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_vim();
        self.finish_composition();

        let selection = self.conflict_action_selection(id);
        let anchor = self.view_anchor();
        let merge = self.merge.as_mut().expect("merge mode");
        let update = merge.session.reset(id, selection);

        self.finish_merge_action(anchor, id, update, window, cx);
    }

    fn conflict_action_selection(&self, id: ConflictId) -> TextSelection {
        self.merge
            .as_ref()
            .and_then(|merge| merge.session.state(id))
            .map_or_else(
                || self.right_selection().unwrap_or(TextSelection::caret(0)),
                |state| TextSelection::caret(state.result.start),
            )
    }

    fn finish_merge_action(
        &mut self,
        anchor: ViewAnchor,
        id: ConflictId,
        update: Result<MergeUpdate, MergeError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match update {
            Ok(update) => self.complete_edit(anchor, update, Placement::Conflict(id), window, cx),
            Err(error) => {
                eprintln!("merge action rejected: {error}");
                self.merge.as_mut().expect("merge mode").current = Some(id);
                self.focus.focus(window, cx);
            }
        }
    }

    pub(super) fn merge_mark(
        &mut self,
        id: ConflictId,
        resolved: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_vim();
        self.finish_composition();
        let selection = self.right_selection().unwrap_or(TextSelection::caret(0));
        let anchor = self.view_anchor();
        let merge = self.merge.as_mut().expect("merge mode");
        let update = merge.session.set_resolved(id, resolved, selection);

        self.finish_merge_action(anchor, id, update, window, cx);
    }

    pub(super) fn merge_target(&self, previous: bool) -> Option<ConflictId> {
        let merge = self.merge.as_ref()?;

        if previous {
            merge
                .session
                .unresolved()
                .filter(|id| merge.current.is_some_and(|current| id.0 < current.0))
                .last()
        } else {
            merge
                .session
                .unresolved()
                .find(|id| merge.current.is_none_or(|current| id.0 > current.0))
        }
    }

    pub(super) fn navigate_merge(
        &mut self,
        previous: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.merge_target(previous) else {
            return;
        };

        self.cancel_vim();
        self.finish_composition();

        let merge = self.merge.as_mut().expect("merge mode");
        merge.current = Some(id);
        let offset = merge
            .session
            .state(id)
            .expect("known conflict")
            .result
            .start;
        self.selection = Some(Selection {
            side: Side::Right,
            anchor: offset,
            head: offset,
        });
        self.refresh_merge_projection();

        self.reveal_merge_conflict(id);
        self.horizontal_scroll = 0.0;

        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn reveal_merge_conflict(&mut self, id: ConflictId) {
        let row = self.merge.as_ref().expect("merge mode").display.conflicts()[id.0].header_row;
        self.vertical_scroll = self
            .geometry()
            .change_scroll_top(row, self.alignment.rows().len());
    }

    pub(super) fn locate_merge_row(&mut self, row: usize) {
        let Some(merge) = &mut self.merge else {
            return;
        };

        let current = merge
            .display
            .conflicts()
            .iter()
            .find(|conflict| conflict.source_span.contains(&row))
            .map(|conflict| conflict.id);
        if let Some(current) = current
            && Some(current) != merge.current
        {
            merge.current = Some(current);
            if merge.show_base {
                self.refresh_merge_projection();
            }
        }
    }

    pub(super) fn toggle_merge_base(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_composition();
        let merge = self.merge.as_mut().expect("merge mode");
        merge.show_base = !merge.show_base;
        let current = merge.current;
        self.refresh_merge_projection();

        if let Some(id) = current {
            let merge = self.merge.as_ref().expect("merge mode");
            let conflict = &merge.display.conflicts()[id.0];
            let top = conflict.base_caption.unwrap_or(conflict.source_span.start);
            self.vertical_scroll = (display_units(top) * LINE_HEIGHT).min(
                self.geometry()
                    .vertical_scroll_limit(self.alignment.rows().len()),
            );
        }

        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn copy_merge_base(&self, id: ConflictId, cx: &mut Context<Self>) {
        if let Some(merge) = &self.merge {
            let text = merge
                .session
                .base()
                .copy_range(merge.session.conflicts()[id.0].base.clone());
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        }
    }
}
