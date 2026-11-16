//! A bounded Vim-style command layer over yori's existing document and history.
//!
//! There is no second editable buffer. Native typing continues through the normal
//! edit path in Insert mode; this module handles commands and source selections.

mod keys;
mod motions;
mod target;

pub use target::EditTarget;

use std::ops::Range;

use yori_diff::merge::MergeError;
use yori_document::{
    Document, InputError,
    editing::{self, EditOutcome, SourceEdit, TextSelection},
};

use keys::{Command, Insert, Keys, Motion, Operator, Target};
use motions::{first_word, inner_word, line_range, motion_range, move_to, normal_cursor};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
}

impl Mode {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Visual => "VISUAL",
            Self::VisualLine => "VISUAL LINE",
        }
    }
}

/// The system clipboard is the only register: yank, delete and change write it, and
/// paste reads it. Linewise text always ends with a line ending, so text from any
/// source that ends with one pastes as whole lines, as Vim treats `"+` contents.
pub trait Clipboard {
    fn read(&mut self) -> Option<String>;
    fn write(&mut self, text: String);
}

struct OperationRange {
    bytes: Range<usize>,
    linewise: bool,
}

pub struct Outcome {
    pub selection: TextSelection,
    pub edit: Option<EditOutcome>,
    /// Owner-derived range restoration when retiring a net-zero merge group.
    pub conflict_ranges_restored: bool,
    /// False only for native Insert-mode input.
    pub consumed: bool,
}

#[derive(Default)]
pub struct Vim {
    mode: Mode,
    keys: Keys,
    // Visual endpoints are inclusive source cursors, unlike the UI's half-open selection.
    visual: Option<(usize, usize)>,
    column: Option<usize>,
}

impl Vim {
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    #[must_use]
    pub fn cursor(&self, selection: TextSelection) -> usize {
        self.visual.map_or(selection.head, |(_, head)| head)
    }

    /// Mouse placement ends a typing transaction without taking an Insert user out of Insert.
    /// Returns whether the owner restored conflict ranges on retirement.
    pub fn reposition(&mut self, target: EditTarget<'_>, selection: TextSelection) -> bool {
        let inserting = self.mode == Mode::Insert;
        let conflict_ranges_restored = self.cancel(target, selection);

        if inserting {
            self.mode = Mode::Insert;
        }

        conflict_ranges_restored
    }

    /// Import a real mouse/keyboard selection without introducing a second selection model.
    pub fn select(&mut self, document: &Document, selection: TextSelection) {
        self.keys.clear();
        self.column = None;
        if self.mode == Mode::Insert {
            return;
        }

        let range = selection.range();
        if range.is_empty() {
            self.visual = None;
            self.mode = Mode::Normal;
            return;
        }

        let last = editing::previous_grapheme(document.text(), range.end);
        self.visual = Some(if selection.head < selection.anchor {
            (last, range.start)
        } else {
            (range.start, last)
        });
        self.mode = Mode::Visual;
    }

    /// End a modal interaction before focus loss or disabling Vim.
    /// This leaves the caller's selection intact and commits, rather than discards, typing.
    /// Returns whether the owner restored conflict ranges on retirement.
    pub fn cancel(&mut self, mut target: EditTarget<'_>, selection: TextSelection) -> bool {
        let conflict_ranges_restored = target.finish(selection);
        *self = Self::default();

        conflict_ranges_restored
    }

    /// Route history keys through a host's richer history (for example merge
    /// text plus resolution status) without changing command-prefix handling.
    pub fn external_history_key(&mut self, key: &str) -> Option<bool> {
        if self.mode == Mode::Insert || !matches!(key, "u" | "ctrl-r") {
            return None;
        }

        match self.keys.feed(key, self.visual.is_some()) {
            Some(Command::Undo) => Some(false),
            Some(Command::Redo) => Some(true),
            _ => None,
        }
    }

    pub fn handle(
        &mut self,
        key: &str,
        mut target: EditTarget<'_>,
        selection: TextSelection,
        clipboard: &mut dyn Clipboard,
    ) -> Result<Outcome, MergeError> {
        let document = target.document();
        if key == "escape" {
            let head = if self.mode == Mode::Insert {
                let content = document.line_content_range(document.line_at_offset(selection.head));
                editing::previous_grapheme(document.text(), selection.head).max(content.start)
            } else {
                self.visual.map_or(selection.head, |(_, head)| head)
            };

            let selection = TextSelection::caret(normal_cursor(document, head));
            let conflict_ranges_restored = self.cancel(target, selection);
            let mut outcome = Self::outcome(selection, None);
            outcome.conflict_ranges_restored = conflict_ranges_restored;

            return Ok(outcome);
        }
        if self.mode == Mode::Insert {
            return Ok(Outcome {
                selection,
                edit: None,
                conflict_ranges_restored: false,
                consumed: false,
            });
        }

        let visual = self.visual.is_some();
        let Some(command) = self.keys.feed(key, visual) else {
            return Ok(Self::outcome(selection, None));
        };

        let cursor = normal_cursor(
            document,
            self.visual.map_or(selection.head, |(_, head)| head),
        );
        let mutable = !matches!(
            command,
            Command::Move(..) | Command::Visual(_) | Command::Operate(Operator::Yank, ..)
        );
        if mutable && !target.writable() {
            return Ok(Self::outcome(selection, None));
        }

        if matches!(command, Command::Undo | Command::Redo) {
            let conflict_ranges_restored = target.finish(selection);
            *self = Self::default();
            let update = target.travel(selection, command == Command::Redo)?;
            let next = update.as_ref().map_or(selection, |update| update.selection);

            let mut outcome = Self::outcome(
                TextSelection::caret(normal_cursor(target.document(), next.head)),
                update.and_then(|update| update.edit),
            );
            outcome.conflict_ranges_restored = conflict_ranges_restored;

            return Ok(outcome);
        }

        let inserting = matches!(
            command,
            Command::Insert(_) | Command::Operate(Operator::Change, ..)
        );
        // Merge historically groups every command; plain text groups only an
        // Insert run. The targets defer starting a group until input validates.
        let grouped = inserting || matches!(target, EditTarget::Merge(_));
        let mut outcome = target.edit(selection, grouped, |source| {
            self.execute(command, source, selection, cursor, clipboard)
        })?;

        if self.mode == Mode::Insert {
            target.begin(selection);
        } else if grouped {
            outcome.conflict_ranges_restored = target.finish(outcome.selection);
        }

        Ok(outcome)
    }

    fn execute(
        &mut self,
        command: Command,
        source: &mut dyn SourceEdit,
        selection: TextSelection,
        cursor: usize,
        clipboard: &mut dyn Clipboard,
    ) -> Result<Outcome, InputError> {
        let document = source.document();
        match command {
            Command::Move(motion, count) => {
                let head = normal_cursor(
                    document,
                    move_to(document, cursor, motion, count, &mut self.column),
                );
                let selection = if let Some((anchor, _)) = self.visual {
                    self.visual = Some((anchor, head));
                    self.visual_selection(document, anchor, head)
                } else {
                    TextSelection::caret(head)
                };

                Ok(Self::outcome(selection, None))
            }
            Command::Visual(linewise) => {
                let mode = if linewise {
                    Mode::VisualLine
                } else {
                    Mode::Visual
                };
                if self.mode == mode {
                    self.mode = Mode::Normal;
                    self.visual = None;

                    return Ok(Self::outcome(TextSelection::caret(cursor), None));
                }

                self.mode = mode;
                let anchor = self.visual.map_or(cursor, |(anchor, _)| anchor);
                self.visual = Some((anchor, cursor));

                Ok(Self::outcome(
                    self.visual_selection(document, anchor, cursor),
                    None,
                ))
            }
            Command::Insert(insert) => self.insert(insert, source, cursor),
            Command::Operate(operator, target, count) => {
                let (range, linewise) =
                    self.target(document, selection, cursor, operator, target, count);
                let target = OperationRange {
                    bytes: range,
                    linewise,
                };

                self.operate(operator, source, selection, cursor, target, clipboard)
            }
            Command::DeleteChar(count) => {
                let (range, linewise) = if self.visual.is_some() {
                    (selection.range(), self.mode == Mode::VisualLine)
                } else {
                    (
                        motion_range(document, cursor, Motion::Right, count, false).0,
                        false,
                    )
                };

                self.operate(
                    Operator::Delete,
                    source,
                    selection,
                    cursor,
                    OperationRange {
                        bytes: range,
                        linewise,
                    },
                    clipboard,
                )
            }
            Command::Paste(after, count) => self.paste(source, selection, clipboard, after, count),
            Command::Undo | Command::Redo => {
                unreachable!("history is routed before source commands")
            }
        }
    }

    fn outcome(selection: TextSelection, edit: Option<EditOutcome>) -> Outcome {
        Outcome {
            selection,
            edit,
            conflict_ranges_restored: false,
            consumed: true,
        }
    }

    fn visual_selection(&self, document: &Document, anchor: usize, head: usize) -> TextSelection {
        if self.mode == Mode::VisualLine {
            let range = line_range(document, anchor, head);
            return if head < anchor {
                TextSelection {
                    anchor: range.end,
                    head: range.start,
                }
            } else {
                TextSelection {
                    anchor: range.start,
                    head: range.end,
                }
            };
        }

        if head < anchor {
            TextSelection {
                anchor: editing::next_grapheme(document.text(), anchor),
                head,
            }
        } else {
            TextSelection {
                anchor,
                head: editing::next_grapheme(document.text(), head),
            }
        }
    }

    fn target(
        &self,
        document: &Document,
        selection: TextSelection,
        cursor: usize,
        operator: Operator,
        target: Target,
        count: usize,
    ) -> (Range<usize>, bool) {
        match target {
            Target::Selection => (selection.range(), self.mode == Mode::VisualLine),
            Target::SelectedLines => {
                let (anchor, head) = self.visual.unwrap_or((cursor, cursor));
                (line_range(document, anchor, head), true)
            }
            Target::InnerWord => (inner_word(document, cursor, count, false), false),
            Target::Motion(motion) => motion_range(
                document,
                cursor,
                motion,
                count,
                operator == Operator::Change,
            ),
            Target::Lines => {
                let end = document
                    .line_content_range(document.line_at_offset(cursor) + count - 1)
                    .start;
                (line_range(document, cursor, end), true)
            }
        }
    }

    fn operate(
        &mut self,
        operator: Operator,
        source: &mut dyn SourceEdit,
        selection: TextSelection,
        cursor: usize,
        target: OperationRange,
        clipboard: &mut dyn Clipboard,
    ) -> Result<Outcome, InputError> {
        let document = source.document();
        let OperationRange {
            bytes: mut range,
            linewise,
        } = target;
        if range.is_empty() && operator != Operator::Change {
            return Ok(Self::outcome(selection, None));
        }

        let copied = clipboard_text(document, range.clone(), linewise);

        // Like Vim, a yank leaves the cursor at the start of what it copied, except
        // that a linewise yank from the first line (`yy`, `Y`, `yj`) keeps its column.
        let starts_on_cursor_line =
            document.line_at_offset(cursor) == document.line_at_offset(range.start);
        let yank_head = match self.visual {
            Some((anchor, head)) => anchor.min(head),
            None if linewise && starts_on_cursor_line => cursor,
            None => range.start,
        };

        self.mode = Mode::Normal;
        self.visual = None;
        self.column = None;

        if operator == Operator::Yank {
            clipboard.write(copied);

            return Ok(Self::outcome(
                TextSelection::caret(normal_cursor(document, yank_head)),
                None,
            ));
        }

        let replacement = if operator == Operator::Change
            && linewise
            && range.end > range.start
            && document.text()[range.clone()].ends_with('\n')
        {
            document.newline()
        } else {
            ""
        };

        // Removing an unterminated final line also removes its preceding separator.
        // The clipboard still contains only the selected line(s).
        if operator == Operator::Delete
            && linewise
            && range.end == document.text().len()
            && !document.text().ends_with('\n')
            && range.start > 0
        {
            range.start = editing::previous_grapheme(document.text(), range.start);
        }

        let replacement = replacement.to_owned();
        let update = source.replace(range.clone(), &replacement)?;
        let document = source.document();

        // An empty `cw` at a line end must not wipe text copied from elsewhere.
        if !copied.is_empty() {
            clipboard.write(copied);
        }

        let head = if operator == Operator::Change {
            self.mode = Mode::Insert;
            range.start
        } else {
            normal_cursor(document, range.start)
        };

        Ok(Self::outcome(TextSelection::caret(head), update.edit))
    }

    fn insert(
        &mut self,
        insert: Insert,
        source: &mut dyn SourceEdit,
        cursor: usize,
    ) -> Result<Outcome, InputError> {
        let document = source.document();
        let content = document.line_content_range(document.line_at_offset(cursor));
        let offset = match insert {
            Insert::Here => cursor,
            Insert::After => editing::next_grapheme(document.text(), cursor).min(content.end),
            Insert::First => first_word(document, cursor),
            Insert::End | Insert::Below => content.end,
            Insert::Above => content.start,
        };

        self.mode = Mode::Insert;
        self.visual = None;
        self.column = None;

        if matches!(insert, Insert::Above | Insert::Below) {
            let newline = document.newline().to_owned();
            let update = source.replace(offset..offset, &newline)?;
            let head = if insert == Insert::Above {
                offset
            } else {
                update.selection.head
            };

            return Ok(Self::outcome(TextSelection::caret(head), update.edit));
        }

        Ok(Self::outcome(TextSelection::caret(offset), None))
    }

    fn paste(
        &mut self,
        source: &mut dyn SourceEdit,
        selection: TextSelection,
        clipboard: &mut dyn Clipboard,
        after: bool,
        count: usize,
    ) -> Result<Outcome, InputError> {
        let document = source.document();
        let Some(text) = clipboard.read().filter(|text| !text.is_empty()) else {
            return Ok(Self::outcome(selection, None));
        };

        let cursor = normal_cursor(
            document,
            self.visual.map_or(selection.head, |(_, head)| head),
        );
        let linewise = text.ends_with('\n');
        let mut text = text.repeat(count);

        let content = document.line_content_range(document.line_at_offset(cursor));
        let range = if self.visual.is_some() {
            selection.range()
        } else {
            let offset = if linewise {
                if after {
                    line_range(document, cursor, cursor).end
                } else {
                    content.start
                }
            } else if after {
                editing::next_grapheme(document.text(), cursor).min(content.end)
            } else {
                cursor
            };
            offset..offset
        };

        let mut head = range.start;
        if linewise
            && range.start == document.text().len()
            && !document.text().is_empty()
            && !document.text().ends_with('\n')
        {
            let ending_len = if text.ends_with("\r\n") { 2 } else { 1 };
            text.truncate(text.len() - ending_len);
            text.insert_str(0, document.newline());
            head += document.newline().len();
        }
        if self.visual.is_some() {
            clipboard.write(clipboard_text(
                document,
                range.clone(),
                self.mode == Mode::VisualLine,
            ));
        }

        self.mode = Mode::Normal;
        self.visual = None;
        self.column = None;

        let update = source.replace(range, &text)?;
        let document = source.document();
        if !linewise {
            head = editing::previous_grapheme(document.text(), update.selection.head);
        }

        Ok(Self::outcome(
            TextSelection::caret(normal_cursor(document, head)),
            update.edit,
        ))
    }
}

/// Linewise text always carries a line ending, so its shape survives the clipboard.
fn clipboard_text(document: &Document, range: Range<usize>, linewise: bool) -> String {
    let mut text = document.copy_range(range).to_owned();
    if linewise && !text.ends_with('\n') {
        text.push_str(document.newline());
    }

    text
}

#[cfg(test)]
mod tests;
