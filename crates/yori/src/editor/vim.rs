//! Native GPUI routing for the bounded, headless Vim command layer.

use gpui_kit::component::{WindowExt, notification::Notification};
use gpui_kit::{App, ClipboardItem, Context, KeyDownEvent, Window};
use yori::vim::{Clipboard, EditTarget, Mode};
use yori_document::editing::{EditUpdate, TextSelection};

use super::{AlignedEditor, Selection, Side, completion::Placement};

#[cfg(test)]
mod tests;

/// Vim yanks, deletes and pastes go straight through the OS clipboard.
struct SystemClipboard<'a>(&'a mut App);

impl Clipboard for SystemClipboard<'_> {
    fn read(&mut self) -> Option<String> {
        self.0.read_from_clipboard().and_then(|item| item.text())
    }

    fn write(&mut self, text: String) {
        self.0.write_to_clipboard(ClipboardItem::new_string(text));
    }
}

impl AlignedEditor {
    pub(super) fn vim_enabled(cx: &App) -> bool {
        crate::config::editor(cx).vim_keybindings
    }

    pub(super) fn accepts_text(&self, cx: &App) -> bool {
        self.can_edit() && (!Self::vim_enabled(cx) || self.vim.mode() == Mode::Insert)
    }

    pub(super) fn cancel_vim(&mut self) {
        // History always belongs to the editable pane, even when focus moved left.
        let selection = self.right_selection().unwrap_or(TextSelection::caret(0));
        let target = if !self.right.editable {
            EditTarget::ReadOnly(&self.right.document)
        } else if let Some(merge) = &mut self.merge {
            EditTarget::Merge(&mut merge.session)
        } else {
            EditTarget::Document(&mut self.right.document, &mut self.history)
        };
        let conflict_ranges_restored = self.vim.cancel(target, selection);
        self.complete_retirement(conflict_ranges_restored);
    }

    pub(super) fn reposition_vim(&mut self) {
        let selection = self.right_selection().unwrap_or(TextSelection::caret(0));
        let target = if !self.right.editable {
            EditTarget::ReadOnly(&self.right.document)
        } else if let Some(merge) = &mut self.merge {
            EditTarget::Merge(&mut merge.session)
        } else {
            EditTarget::Document(&mut self.right.document, &mut self.history)
        };
        let conflict_ranges_restored = self.vim.reposition(target, selection);
        self.complete_retirement(conflict_ranges_restored);
    }

    pub(super) fn sync_vim_selection(&mut self, cx: &App) {
        if !Self::vim_enabled(cx) {
            return;
        }
        let Some(selection) = &self.selection else {
            return;
        };

        let document = match selection.side {
            Side::Left => &self.left.document,
            Side::Right => &self.right.document,
            Side::Incoming => {
                &self
                    .merge
                    .as_ref()
                    .expect("incoming pane")
                    .incoming
                    .document
            }
        };
        let selection = TextSelection {
            anchor: selection.anchor,
            head: selection.head,
        };
        self.vim.select(document, selection);
    }

    pub(super) fn vim_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !Self::vim_enabled(cx) || !self.focus.is_focused(window) {
            return;
        }

        let stroke = &event.keystroke;
        let redo = stroke.modifiers.control
            && !stroke.modifiers.alt
            && !stroke.modifiers.platform
            && stroke.key == "r";
        // Keep application shortcuts (tabs, change navigation, clipboard) outside modal parsing.
        if !redo && (stroke.modifiers.control || stroke.modifiers.platform || stroke.modifiers.alt)
        {
            if self.vim.mode() != Mode::Insert {
                self.cancel_vim();
            }
            return;
        }

        let key = if redo {
            "ctrl-r"
        } else {
            stroke.key_char.as_deref().unwrap_or(&stroke.key)
        };
        if self.vim.mode() == Mode::Insert && key != "escape" {
            return;
        }

        let selection = self.selection.clone().unwrap_or(Selection {
            side: Side::Right,
            anchor: 0,
            head: 0,
        });
        let old = TextSelection {
            anchor: selection.anchor,
            head: selection.head,
        };
        if matches!(key, "u" | "ctrl-r") {
            if let Some(redo) = self.vim.external_history_key(key) {
                self.travel_history(redo, window, cx);
            } else {
                cx.notify();
            }
            window.prevent_default();
            cx.stop_propagation();
            return;
        }

        if let Err(error) = self.handle_vim_command(key, selection.side, old, window, cx) {
            eprintln!("Vim command rejected: {error}");
            self.cancel_vim();
            window.play_system_bell();
            cx.notify();
        }

        window.prevent_default();
        cx.stop_propagation();
    }

    fn handle_vim_command(
        &mut self,
        key: &str,
        side: Side,
        old: TextSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), yori_diff::merge::MergeError> {
        let anchor = self.view_anchor(window, cx);
        let target = match side {
            Side::Right if !self.right.editable => EditTarget::ReadOnly(&self.right.document),
            Side::Right => {
                if let Some(merge) = &mut self.merge {
                    EditTarget::Merge(&mut merge.session)
                } else {
                    EditTarget::Document(&mut self.right.document, &mut self.history)
                }
            }
            Side::Left => EditTarget::ReadOnly(&self.left.document),
            Side::Incoming => EditTarget::ReadOnly(
                &self
                    .merge
                    .as_ref()
                    .expect("incoming pane")
                    .incoming
                    .document,
            ),
        };
        let outcome = self
            .vim
            .handle(key, target, old, &mut SystemClipboard(cx))?;

        self.complete_edit(
            anchor,
            EditUpdate {
                selection: outcome.selection,
                edit: outcome.edit,
            },
            Placement::Vim {
                side,
                conflict_ranges_restored: outcome.conflict_ranges_restored,
            },
            window,
            cx,
        );

        Ok(())
    }

    pub(super) fn toggle_vim(
        &mut self,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut config = crate::config::editor(cx);
        config.vim_keybindings = enabled;
        let diagnostic = match crate::config::update_editor(config, cx) {
            Ok(diagnostic) => diagnostic,
            Err(error) => {
                window.push_notification(Notification::error(error), cx);
                return;
            }
        };
        if let Some(diagnostic) = diagnostic {
            window.push_notification(Notification::error(diagnostic), cx);
        }

        self.cancel_vim();
        self.vim_keybindings = enabled.into();
        if enabled && self.selection.is_none() {
            self.selection = Some(Selection {
                side: Side::Right,
                anchor: 0,
                head: 0,
            });
        }

        self.focus.focus(window, cx);
        cx.notify();
    }
}
