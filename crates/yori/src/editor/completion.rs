//! Complete source operations before returning to native callers. Presentation
//! updates never escape the editor as receipts that a caller must apply later.

use gpui_kit::{Context, Window};
use yori::geometry::{display_units, whole_rows};
use yori_diff::{
    Alignment,
    merge::{ConflictId, MergeError},
};
use yori_document::editing::EditUpdate;

use super::{AlignedEditor, DirtyChanged, LINE_HEIGHT, Selection, Side};

#[derive(Clone, Copy)]
pub(super) struct ViewAnchor {
    side: Side,
    offset: usize,
    fraction: f32,
}

/// Placement is editor implementation policy, not a source-editing command.
/// History can reveal RESULT while retaining an immutable input selection;
/// conflict actions must not derive their identity from a deletion's caret.
#[derive(Clone, Copy)]
pub(super) enum Placement {
    Edit,
    Transfer,
    Vim {
        side: Side,
        conflict_ranges_restored: bool,
    },
    History,
    Conflict(ConflictId),
}

impl AlignedEditor {
    pub(super) fn view_anchor(&self, window: &mut Window, cx: &gpui_kit::App) -> ViewAnchor {
        let visual_row = whole_rows(self.vertical_scroll / LINE_HEIGHT);
        let projection = self.wrap_projection(window, cx);
        let (row, continuation) = projection.visual_location(visual_row);
        let side = if self.line_for_row(Side::Left, row).is_some() {
            Side::Left
        } else {
            Side::Right
        };
        let display_byte = projection
            .row(self, row)
            .and_then(|row| row.segments(side).get(continuation).cloned())
            .map_or(0, |segment| segment.start);

        ViewAnchor {
            side,
            offset: self.source_offset_for(side, row, display_byte),
            fraction: self.vertical_scroll % LINE_HEIGHT,
        }
    }

    pub(super) fn complete_retirement(&mut self, conflict_ranges_restored: bool) {
        if !conflict_ranges_restored {
            return;
        }

        self.refresh_merge_projection();
        self.hovered_connection = None;
    }

    pub(super) fn complete_history(
        &mut self,
        anchor: ViewAnchor,
        result: Result<Option<EditUpdate>, MergeError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(Some(update)) => self.complete_edit(anchor, update, Placement::History, window, cx),
            other => {
                self.sync_history_input(cx);
                if let Err(error) = other {
                    eprintln!("history edit rejected: {error}");
                    window.play_system_bell();
                }

                cx.notify();
            }
        }
    }

    fn sync_history_input(&mut self, cx: &Context<Self>) {
        if self
            .selection
            .as_ref()
            .is_some_and(|selection| selection.side != Side::Right)
        {
            self.sync_vim_selection(cx);
        }
    }

    pub(super) fn complete_edit(
        &mut self,
        anchor: ViewAnchor,
        update: EditUpdate,
        placement: Placement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modal = matches!(placement, Placement::Vim { .. });
        let merge_update = self.merge.is_some() && !modal;
        let has_text_edit = update.edit.is_some();
        let conflict_ranges_restored = matches!(
            placement,
            Placement::Vim {
                conflict_ranges_restored: true,
                ..
            }
        );
        let side = if let Placement::Vim { side, .. } = placement {
            side
        } else {
            Side::Right
        };
        let input_selection = if matches!(placement, Placement::History) {
            self.selection
                .clone()
                .filter(|selection| selection.side != Side::Right)
        } else {
            None
        };
        let reveal = (has_text_edit || modal).then(|| {
            let offset = if modal {
                self.vim.cursor(update.selection)
            } else {
                update.selection.head
            };
            (side, offset)
        });

        if (has_text_edit || merge_update)
            && let Some(merge) = &self.merge
        {
            self.right.document = merge.session.result().clone();
        }
        let dirty_changed = if let Some(edit) = &update.edit {
            self.right.refresh_after_edit(edit);
            if self.right.highlighter.is_none() {
                self.schedule_highlighting(Side::Right, window, cx);
            }

            self.dirty.update(self.right.document.text())
        } else {
            false
        };

        self.selection = Some(input_selection.clone().unwrap_or(Selection {
            side,
            anchor: update.selection.anchor,
            head: update.selection.head,
        }));
        self.visual_affinity = None;
        if input_selection.is_none() && (has_text_edit || modal) {
            self.preferred_column = None;
            self.preferred_visual_x = None;
        }
        if let Placement::Conflict(id) = placement {
            self.merge.as_mut().expect("merge mode").current = Some(id);
        }

        if has_text_edit || merge_update || conflict_ranges_restored {
            if self.merge.is_some() {
                self.refresh_merge_projection();
            } else {
                self.alignment = Alignment::between(&self.left.document, &self.right.document);
                self.invalidate_wrap_projection();
            }
            self.hovered_connection = None;
        }
        if let Some(edit) = update.edit {
            let offset = if anchor.side == Side::Right {
                edit.map_anchor(anchor.offset)
            } else {
                anchor.offset
            };
            let (visual_row, _) = self.source_position(anchor.side, offset, window, cx);
            self.vertical_scroll = display_units(visual_row) * LINE_HEIGHT + anchor.fraction;
        }

        // Preserve the established modal policy: immutable-input history imports
        // its retained selection, while right-pane history stays in Normal mode.
        // Vim commands retain their own prefixes, visual endpoints and column.
        if matches!(placement, Placement::History) {
            self.sync_history_input(cx);
        }
        if let Some((side, offset)) = reveal {
            if !matches!(placement, Placement::Conflict(_)) {
                self.locate_source_change(side, update.selection.head);
            }

            let history_conflict = matches!(placement, Placement::History)
                .then(|| self.merge.as_ref().and_then(|merge| merge.current))
                .flatten();
            if let Some(id) = history_conflict {
                self.reveal_merge_conflict(id, window, cx);
            } else {
                self.reveal_source(side, offset, window, cx);
            }
        }

        if matches!(placement, Placement::Transfer | Placement::Conflict(_)) {
            self.focus.focus(window, cx);
        }
        if dirty_changed || merge_update {
            cx.emit(DirtyChanged);
        }

        cx.notify();
    }
}
