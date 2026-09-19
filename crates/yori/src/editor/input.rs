//! GPUI input integration for the same aligned surface used by the read-only checkpoint.

use super::completion::Placement;
use super::{
    ActiveTheme, AlignedEditor, App, Backspace, Bounds, Context, CopySelected, CutSelected, Delete,
    DisplayLine, EntityInputHandler, FocusNextPane, FocusPreviousPane, Font, GUTTER_WIDTH,
    HEADER_HEIGHT, InsertTab, KEY_CONTEXT, KeyBinding, LINE_HEIGHT, Motion, MoveDown, MoveEnd,
    MoveFinish, MoveHome, MoveLeft, MoveRight, MoveStart, MoveUp, Newline, NextChange, Paste,
    Pixels, PreviousChange, Range, Redo, RestoreSelectedLines, SelectAll, SelectDown, SelectEnd,
    SelectHome, SelectLeft, SelectRight, SelectUp, Selection, Side, TAB_WIDTH, TextRun,
    UTF16Selection, Undo, Window, point, px,
};
use yori::geometry::display_units;
use yori::vim::EditTarget;
use yori_document::editing::{self, EditUpdate, TextSelection};

#[cfg(test)]
mod history_tests;
#[cfg(test)]
mod navigation_tests;

impl AlignedEditor {
    pub(super) fn right_selection(&self) -> Option<TextSelection> {
        self.selection
            .as_ref()
            .filter(|s| s.side == Side::Right)
            .map(|s| TextSelection {
                anchor: s.anchor,
                head: s.head,
            })
    }

    pub(super) fn marked_range(&self) -> Option<Range<usize>> {
        self.merge.as_ref().map_or_else(
            || self.history.marked_range(),
            |merge| merge.session.marked_range(),
        )
    }

    fn edit_target(&mut self) -> EditTarget<'_> {
        if !self.right.editable {
            return EditTarget::ReadOnly(&self.right.document);
        }

        if let Some(merge) = &mut self.merge {
            EditTarget::Merge(&mut merge.session)
        } else {
            EditTarget::Document(&mut self.right.document, &mut self.history)
        }
    }

    pub(super) fn finish_composition(&mut self) {
        if let Some(selection) = self.right_selection() {
            let conflict_ranges_restored = self.edit_target().unmark(selection);
            self.complete_retirement(conflict_ranges_restored);
        }
    }

    pub(super) fn restore_block(
        &mut self,
        index: usize,
        expected: &yori_diff::ChangeBlock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_edit() {
            return;
        }

        // Never apply coordinates from a button rendered for a different alignment.
        if self.alignment.blocks().get(index) != Some(expected) {
            return;
        }

        self.cancel_vim();
        self.finish_composition();
        let selection = self
            .right_selection()
            .unwrap_or(TextSelection::caret(expected.right.start));
        let anchor = self.view_anchor();

        match yori_diff::restore_block(
            &mut self.history,
            &self.left.document,
            &mut self.right.document,
            selection,
            expected,
        ) {
            Ok(edit) => {
                self.complete_edit(
                    anchor,
                    EditUpdate {
                        selection: edit.selection,
                        edit: Some(edit),
                    },
                    Placement::Transfer,
                    window,
                    cx,
                );
            }
            Err(error) => {
                eprintln!("block restoration rejected: {error}");
                window.play_system_bell();
            }
        }
    }

    pub(super) fn restore_selected_lines(
        &mut self,
        _: &RestoreSelectedLines,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(plan) = self.selection_restore() {
            self.apply_selection_restore(&plan, window, cx);
        }
    }

    pub(super) fn apply_selection_restore(
        &mut self,
        expected: &yori_diff::SelectionRestore,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The preview must still describe the current selection and alignment.
        if self.selection_restore().as_ref() != Some(expected) {
            return;
        }

        self.cancel_vim();
        self.finish_composition();
        let selection = self
            .right_selection()
            .unwrap_or(TextSelection::caret(expected.local.start));
        let anchor = self.view_anchor();

        match yori_diff::restore_selection(
            &mut self.history,
            &self.left.document,
            &mut self.right.document,
            selection,
            expected,
        ) {
            Ok(edit) => {
                self.complete_edit(
                    anchor,
                    EditUpdate {
                        selection: edit.selection,
                        edit: Some(edit),
                    },
                    Placement::Transfer,
                    window,
                    cx,
                );
            }
            Err(error) => {
                eprintln!("selected-line restoration rejected: {error}");
                window.play_system_bell();
            }
        }
    }

    fn replace(
        &mut self,
        range: Range<usize>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_edit() {
            window.play_system_bell();
            return;
        }

        let Some(selection) = self.right_selection() else {
            return;
        };

        let inserting = Self::vim_enabled(cx) && self.vim.mode() == yori::vim::Mode::Insert;
        let anchor = self.view_anchor();
        match self
            .edit_target()
            .replace(selection, range, text, inserting)
        {
            Ok(update) => self.complete_edit(anchor, update, Placement::Edit, window, cx),
            Err(error) => {
                eprintln!("edit rejected: {error}");
                window.play_system_bell();
            }
        }
    }

    pub(super) fn cursor_position(
        &self,
        offset: usize,
        window: &mut Window,
        cx: &App,
    ) -> (usize, f32) {
        let side = self
            .selection
            .as_ref()
            .map_or(Side::Right, |selection| selection.side);
        self.source_position(side, offset, window, cx)
    }

    pub(super) fn source_position(
        &self,
        side: Side,
        offset: usize,
        window: &mut Window,
        cx: &App,
    ) -> (usize, f32) {
        let document = &self.document(side).document;
        let row = self.row_for_source(side, offset);
        let range = document.line_content_range(document.line_at_offset(offset));
        let display =
            DisplayLine::from_source(&document.text()[range.clone()], range.start, TAB_WIDTH);
        if display.text.is_empty() {
            return (row, 0.0);
        }

        let display_offset = display.display_offset(offset);
        let run = TextRun {
            len: display.text.len(),
            font: Font {
                family: cx.theme().mono_font_family.clone(),
                ..Font::default()
            },
            color: cx.theme().foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(
            display.text.into(),
            cx.theme().mono_font_size,
            &[run],
            None,
        );

        (row, f32::from(line.x_for_index(display_offset)))
    }

    pub(super) fn reveal_cursor(&mut self, window: &mut Window, cx: &App) {
        let Some(selection) = &self.selection else {
            return;
        };

        let cursor = if Self::vim_enabled(cx) {
            self.vim.cursor(TextSelection {
                anchor: selection.anchor,
                head: selection.head,
            })
        } else {
            selection.head
        };
        self.reveal_source(selection.side, cursor, window, cx);
    }

    pub(super) fn reveal_source(
        &mut self,
        side: Side,
        offset: usize,
        window: &mut Window,
        cx: &App,
    ) {
        let (row, x) = self.source_position(side, offset, window, cx);
        let geometry = self.geometry();
        let y = display_units(row) * LINE_HEIGHT;
        let height = geometry.rows_viewport_height();
        if y < self.vertical_scroll {
            self.vertical_scroll = y;
        } else if y + LINE_HEIGHT > self.vertical_scroll + height {
            self.vertical_scroll = (y + LINE_HEIGHT - height).max(0.0);
        }
        self.vertical_scroll = self
            .vertical_scroll
            .min(geometry.vertical_scroll_limit(self.alignment.rows().len()));

        let width = geometry.text_viewport_width();
        if x < self.horizontal_scroll {
            self.horizontal_scroll = x;
        } else if x + 2.0 > self.horizontal_scroll + width {
            self.horizontal_scroll = (x + 2.0 - width).max(0.0);
        }
    }

    pub(super) fn move_cursor(
        &mut self,
        motion: Motion,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(selection) = self.right_selection() {
            self.history
                .finish_transaction(&self.right.document, selection);
        }
        self.finish_composition();
        let Some(selection) = self.selection.as_ref() else {
            return;
        };

        let side = selection.side;
        let old = TextSelection {
            anchor: selection.anchor,
            head: selection.head,
        };
        let document = &self.document(side).document;
        let mut column = self.preferred_column;
        let next = editing::navigate(document, old, motion, extend, &mut column);
        self.preferred_column = column;

        self.selection = Some(Selection {
            side,
            anchor: next.anchor,
            head: next.head,
        });
        self.locate_caret_change();
        self.reveal_cursor(window, cx);

        cx.notify();
    }

    pub(super) fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_composition();
        let Some(side) = self.selection.as_ref().map(|s| s.side) else {
            return;
        };

        self.selection = Some(Selection {
            side,
            anchor: 0,
            head: self.document(side).document.text().len(),
        });
        self.sync_vim_selection(cx);
        self.locate_caret_change();
        self.preferred_column = None;

        cx.notify();
    }

    pub(super) fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selection) = self.right_selection() else {
            return;
        };

        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let range = self.marked_range().unwrap_or(selection.range());
            self.replace(range, &text, window, cx);
        }
    }

    pub(super) fn cut(&mut self, _: &CutSelected, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selection) = self.right_selection() else {
            return;
        };

        if !selection.range().is_empty() {
            self.copy_selected(&CopySelected, window, cx);
            self.replace(selection.range(), "", window, cx);
        }
    }

    pub(super) fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_at_cursor(true, window, cx);
    }

    pub(super) fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_at_cursor(false, window, cx);
    }

    fn delete_at_cursor(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selection) = self.right_selection() else {
            return;
        };

        let mut range = selection.range();
        if range.is_empty() {
            if backwards {
                range.start = editing::previous_grapheme(self.right.document.text(), range.start);
            } else {
                range.end = editing::next_grapheme(self.right.document.text(), range.end);
            }
        }

        if !range.is_empty() {
            self.replace(range, "", window, cx);
        }
    }

    pub(super) fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selection) = self.right_selection() else {
            return;
        };

        self.replace(selection.range(), self.right.document.newline(), window, cx);
    }

    pub(super) fn insert_tab(
        &mut self,
        _: &InsertTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.right_selection() else {
            return;
        };

        self.replace(selection.range(), "\t", window, cx);
    }

    pub(super) fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        self.travel_history(false, window, cx);
    }

    pub(super) fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        self.travel_history(true, window, cx);
    }

    pub(super) fn travel_history(
        &mut self,
        redo: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Input fields and dialogs own their history. Only the focused source
        // surface may route these commands into the active comparison's history.
        if !self.focus.is_focused(window) {
            cx.propagate();
            return;
        }
        if !self.can_edit() {
            window.play_system_bell();
            return;
        }

        self.cancel_vim();
        let selection = self.right_selection().unwrap_or(TextSelection::caret(0));
        let anchor = self.view_anchor();
        let result = if let Some(merge) = &mut self.merge {
            if redo {
                merge.session.redo(selection)
            } else {
                merge.session.undo(selection)
            }
        } else {
            let result = if redo {
                self.history.redo(&mut self.right.document, selection)
            } else {
                self.history.undo(&mut self.right.document, selection)
            };
            result
                .map(|edit| {
                    edit.map(|edit| EditUpdate {
                        selection: edit.selection,
                        edit: Some(edit),
                    })
                })
                .map_err(Into::into)
        };

        self.complete_history(anchor, result, window, cx);
    }

    fn bytes_from_utf16(&self, range: Range<usize>) -> Range<usize> {
        let text = self.right.document.text();
        editing::from_utf16(text, range.start)..editing::from_utf16(text, range.end)
    }

    fn bytes_to_utf16(&self, range: Range<usize>) -> Range<usize> {
        let text = self.right.document.text();
        editing::to_utf16(text, range.start)..editing::to_utf16(text, range.end)
    }
}

impl EntityInputHandler for AlignedEditor {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        self.right_selection()?;

        let bytes = self.bytes_from_utf16(range);
        *actual = Some(self.bytes_to_utf16(bytes.clone()));
        self.right.document.text().get(bytes).map(str::to_owned)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let selection = self.right_selection()?;
        Some(UTF16Selection {
            range: self.bytes_to_utf16(selection.range()),
            reversed: selection.head < selection.anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.right_selection()?;
        self.marked_range().map(|range| self.bytes_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_composition();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_text(cx) {
            return;
        }
        let Some(selection) = self.right_selection() else {
            return;
        };

        let range = range
            .map(|range| self.bytes_from_utf16(range))
            .or_else(|| self.marked_range())
            .unwrap_or(selection.range());

        self.replace(range, text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_text(cx) {
            return;
        }
        let Some(selection) = self.right_selection() else {
            return;
        };

        let range = range
            .map(|range| self.bytes_from_utf16(range))
            .or_else(|| self.marked_range())
            .unwrap_or(selection.range());
        let selected = selected.map(|range| {
            editing::from_utf16(text, range.start)..editing::from_utf16(text, range.end)
        });
        let inserting = Self::vim_enabled(cx) && self.vim.mode() == yori::vim::Mode::Insert;
        let anchor = self.view_anchor();
        match self
            .edit_target()
            .replace_marked(selection, range, text, selected, inserting)
        {
            Ok(update) => self.complete_edit(anchor, update, Placement::Edit, window, cx),
            Err(error) => {
                eprintln!("composition rejected: {error}");
                window.play_system_bell();
            }
        }
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.right_selection()?;

        let range = self.bytes_from_utf16(range);
        let (row, x) = self.cursor_position(range.start, window, cx);
        let (end_row, end_x) = self.cursor_position(range.end, window, cx);
        let origin = self.content_bounds.get().origin;
        let width = if row == end_row {
            (end_x - x).max(1.0)
        } else {
            1.0
        };

        Some(Bounds::new(
            point(
                origin.x
                    + px(self.geometry().right_pane_left() + GUTTER_WIDTH + x
                        - self.horizontal_scroll),
                origin.y
                    + px(HEADER_HEIGHT + display_units(row) * LINE_HEIGHT - self.vertical_scroll),
            ),
            gpui_kit::size(px(width), px(LINE_HEIGHT)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui_kit::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.right_selection()?;
        let (side, offset) = self.source_offset_at(point, window, cx);
        (side == Side::Right).then(|| editing::to_utf16(self.right.document.text(), offset))
    }
}

pub(super) fn bind_keys(cx: &mut App) {
    let command = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };

    cx.bind_keys([
        KeyBinding::new("alt-up", PreviousChange, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-down", NextChange, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-enter", RestoreSelectedLines, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-h", FocusPreviousPane, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-l", FocusNextPane, Some(KEY_CONTEXT)),
        KeyBinding::new(&format!("{command}-c"), CopySelected, Some(KEY_CONTEXT)),
        KeyBinding::new(&format!("{command}-v"), Paste, Some(KEY_CONTEXT)),
        KeyBinding::new(&format!("{command}-x"), CutSelected, Some(KEY_CONTEXT)),
        KeyBinding::new(&format!("{command}-a"), SelectAll, Some(KEY_CONTEXT)),
        KeyBinding::new(&format!("{command}-z"), Undo, Some(KEY_CONTEXT)),
        KeyBinding::new(&format!("{command}-shift-z"), Redo, Some(KEY_CONTEXT)),
        KeyBinding::new(&format!("{command}-y"), Redo, Some(KEY_CONTEXT)),
        KeyBinding::new("backspace", Backspace, Some(KEY_CONTEXT)),
        KeyBinding::new("delete", Delete, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", Newline, Some(KEY_CONTEXT)),
        KeyBinding::new("tab", InsertTab, Some(KEY_CONTEXT)),
        KeyBinding::new("left", MoveLeft, Some(KEY_CONTEXT)),
        KeyBinding::new("right", MoveRight, Some(KEY_CONTEXT)),
        KeyBinding::new("up", MoveUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", MoveDown, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-up", SelectUp, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-down", SelectDown, Some(KEY_CONTEXT)),
        KeyBinding::new("home", MoveHome, Some(KEY_CONTEXT)),
        KeyBinding::new("end", MoveEnd, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-home", SelectHome, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-end", SelectEnd, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-home", MoveStart, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-end", MoveFinish, Some(KEY_CONTEXT)),
    ]);
}
