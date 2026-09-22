//! GPUI input integration for the same aligned surface used by the read-only checkpoint.

use super::completion::Placement;
use super::{
    ActiveTheme, AlignedEditor, App, Backspace, Bounds, Context, CopySelected, CutSelected, Delete,
    DisplayLine, EntityInputHandler, Font, GUTTER_WIDTH, HEADER_HEIGHT, InsertTab, KEY_CONTEXT,
    KeyBinding, LINE_HEIGHT, Motion, MoveDown, MoveEnd, MoveFinish, MoveHome, MoveLeft, MoveRight,
    MoveStart, MoveUp, Newline, Paste, Pixels, Range, Redo, RestoreSelectedLines, SelectAll,
    SelectDown, SelectEnd, SelectHome, SelectLeft, SelectRight, SelectUp, Selection, Side,
    SourceLocation, TAB_WIDTH, TextRun, UTF16Selection, Undo, VisualAffinity, Window, point, px,
    wrapping::WrapProjection,
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
        let anchor = self.view_anchor(window, cx);

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
        let anchor = self.view_anchor(window, cx);

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
        let anchor = self.view_anchor(window, cx);
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

    pub(super) fn source_position(
        &self,
        side: Side,
        offset: usize,
        window: &mut Window,
        cx: &App,
    ) -> (usize, f32) {
        let projection = self.wrap_projection(window, cx);

        self.source_position_in(side, offset, &projection, window, cx)
    }

    pub(super) fn source_position_in(
        &self,
        side: Side,
        offset: usize,
        projection: &WrapProjection,
        window: &mut Window,
        cx: &App,
    ) -> (usize, f32) {
        let document = &self.document(side).document;
        let row = self.row_for_source(side, offset);
        let range = document.line_content_range(document.line_at_offset(offset));
        let display =
            DisplayLine::from_source(&document.text()[range.clone()], range.start, TAB_WIDTH);
        let Some(projected) = projection.row(self, row) else {
            return (projection.visual_rows(), 0.0);
        };
        let display_offset = display.display_offset(offset);
        let segments = projected.segments(side);
        let affinity = self.visual_affinity.filter(|affinity| {
            affinity.projection == projection.identity()
                && affinity.side == side
                && affinity.offset == offset
                && affinity.logical_row == row
        });
        let continuation = affinity
            .filter(|affinity| segments.get(affinity.continuation).is_some())
            .map_or_else(
                || {
                    segments
                        .iter()
                        .position(|segment| display_offset < segment.end)
                        .unwrap_or_else(|| segments.len().saturating_sub(1))
                },
                |affinity| affinity.continuation,
            );
        let Some(segment) = segments.get(continuation) else {
            return (projected.visual_start, 0.0);
        };
        if display.text.is_empty() {
            return (projected.visual_start + continuation, 0.0);
        }

        let text = &display.text[segment.clone()];
        let segment_offset = display_offset.clamp(segment.start, segment.end) - segment.start;
        let run = TextRun {
            len: text.len(),
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
            text.to_owned().into(),
            cx.theme().mono_font_size,
            &[run],
            None,
        );

        (
            projected.visual_start + continuation,
            f32::from(line.x_for_index(segment_offset)),
        )
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
        let projection = self.wrap_projection(window, cx);
        let (row, x) = self.source_position_in(side, offset, &projection, window, cx);
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
            .min(geometry.vertical_scroll_limit(projection.visual_rows()));

        if self.word_wrap.enabled() {
            self.horizontal_scroll = 0.0;
        } else {
            let width = geometry.text_viewport_width();
            if x < self.horizontal_scroll {
                self.horizontal_scroll = x;
            } else if x + 2.0 > self.horizontal_scroll + width {
                self.horizontal_scroll = (x + 2.0 - width).max(0.0);
            }
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
        let (next, visual_affinity) = if self.word_wrap.enabled()
            && matches!(
                motion,
                Motion::Up | Motion::Down | Motion::Home | Motion::End
            ) {
            self.wrapped_navigation(side, old, motion, extend, window, cx)
        } else {
            self.preferred_visual_x = None;
            let document = &self.document(side).document;
            let mut column = self.preferred_column;
            let next = editing::navigate(document, old, motion, extend, &mut column);
            self.preferred_column = column;

            let affinity = self.word_wrap.enabled().then(|| {
                let projection = self.wrap_projection(window, cx);
                let prefer_previous = matches!(motion, Motion::Left | Motion::Finish);
                self.visual_affinity_for_offset(side, next.head, prefer_previous, &projection)
            });
            (next, affinity.flatten())
        };

        self.visual_affinity = visual_affinity;
        self.selection = Some(Selection {
            side,
            anchor: next.anchor,
            head: next.head,
        });
        self.locate_caret_change();
        self.reveal_cursor(window, cx);

        cx.notify();
    }

    fn wrapped_navigation(
        &mut self,
        side: Side,
        selection: TextSelection,
        motion: Motion,
        extend: bool,
        window: &mut Window,
        cx: &App,
    ) -> (TextSelection, Option<VisualAffinity>) {
        let projection = self.wrap_projection(window, cx);
        let (visual_row, current_x) =
            self.source_position_in(side, selection.head, &projection, window, cx);
        let location = match motion {
            Motion::Up | Motion::Down => {
                let preferred_x = *self.preferred_visual_x.get_or_insert(current_x);
                let backwards = matches!(motion, Motion::Up);
                let mut target = visual_row;

                loop {
                    let next = if backwards {
                        target.saturating_sub(1)
                    } else {
                        (target + 1).min(projection.visual_rows().saturating_sub(1))
                    };
                    if next == target {
                        break SourceLocation {
                            projection: projection.identity(),
                            side,
                            offset: selection.head,
                            logical_row: self.row_for_source(side, selection.head),
                            continuation: projection.visual_location(visual_row).1,
                        };
                    }
                    target = next;

                    let (row, continuation) = projection.visual_location(target);
                    if projection
                        .row(self, row)
                        .is_some_and(|row| row.segments(side).get(continuation).is_some())
                    {
                        break self.source_location_for_x_in(
                            side,
                            target,
                            preferred_x,
                            &projection,
                            window,
                            cx,
                        );
                    }
                }
            }
            Motion::Home | Motion::End => {
                self.preferred_visual_x = None;
                let (row, continuation) = projection.visual_location(visual_row);
                let display_byte = projection
                    .row(self, row)
                    .and_then(|row| row.segments(side).get(continuation).cloned())
                    .map_or(0, |segment| {
                        if matches!(motion, Motion::Home) {
                            segment.start
                        } else {
                            segment.end
                        }
                    });

                SourceLocation {
                    projection: projection.identity(),
                    side,
                    offset: self.source_offset_for(side, row, display_byte),
                    logical_row: row,
                    continuation,
                }
            }
            Motion::Left | Motion::Right | Motion::Start | Motion::Finish => unreachable!(),
        };
        self.preferred_column = None;

        (
            TextSelection {
                anchor: if extend {
                    selection.anchor
                } else {
                    location.offset
                },
                head: location.offset,
            },
            Some(location.affinity()),
        )
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
        self.visual_affinity = None;
        self.sync_vim_selection(cx);
        self.locate_caret_change();
        self.preferred_column = None;
        self.preferred_visual_x = None;

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
        let anchor = self.view_anchor(window, cx);
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
        let anchor = self.view_anchor(window, cx);
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
        let projection = self.wrap_projection(window, cx);
        let (row, x) = self.source_position_in(Side::Right, range.start, &projection, window, cx);
        let (end_row, end_x) =
            self.source_position_in(Side::Right, range.end, &projection, window, cx);
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
        let location = self.source_location_at(point, window, cx);
        if location.side != Side::Right {
            return None;
        }

        self.visual_affinity = Some(location.affinity());
        Some(editing::to_utf16(
            self.right.document.text(),
            location.offset,
        ))
    }
}

pub(super) fn fixed_key_bindings() -> Vec<KeyBinding> {
    vec![
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
    ]
}
