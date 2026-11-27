//! In-memory three-way merge sessions. Immutable inputs and stable conflict IDs
//! are separate from editable result ranges and explicit resolution decisions.

mod alignment;
mod editing;
mod history;
mod selection;

pub use alignment::{MergeAlignment, MergeRow};
pub use editing::MergeResultEdit;
pub use selection::MergeInput;
pub use yori_document::editing::EditUpdate as MergeUpdate;
#[cfg(test)]
mod tests;

use std::{fmt, ops::Range};

use similar::{MergeResolution, TextMerge};
use yori_document::{
    Document, InputError,
    editing::{EditHistory, EditOutcome, SourceEdit, TextSelection},
};

use history::{MergeHistory, State};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConflictId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Take {
    Local,
    Incoming,
    LocalThenIncoming,
    IncomingThenLocal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub id: ConflictId,
    pub base: Range<usize>,
    pub local: Range<usize>,
    pub incoming: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictState {
    pub result: Range<usize>,
    pub resolved: bool,
}

#[derive(Debug)]
pub enum MergeError {
    Document(InputError),
    UnknownConflict(ConflictId),
    UnsupportedResolution,
    HistoryInvariant,
}

impl fmt::Display for MergeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Document(error) => error.fmt(f),
            Self::UnknownConflict(id) => write!(f, "unknown merge conflict {}", id.0),
            Self::UnsupportedResolution => f.write_str("unsupported merge-engine resolution"),
            Self::HistoryInvariant => f.write_str("merge and document history are inconsistent"),
        }
    }
}

impl std::error::Error for MergeError {}

impl From<InputError> for MergeError {
    fn from(error: InputError) -> Self {
        Self::Document(error)
    }
}

pub struct MergeSession {
    base: Document,
    local: Document,
    incoming: Document,
    result: Document,
    conflicts: Vec<Conflict>,
    states: Vec<ConflictState>,
    edits: EditHistory,
    history: MergeHistory,
}

impl MergeSession {
    pub fn new(base: Document, local: Document, incoming: Document) -> Result<Self, MergeError> {
        let merge = TextMerge::from_lines(base.text(), local.text(), incoming.text());
        let mut result = String::new();
        let mut conflicts = Vec::new();
        let mut states = Vec::new();

        for region in merge.regions() {
            let base_range = byte_range(&base, region.base_range());
            let local_range = byte_range(&local, region.ours_range());
            let incoming_range = byte_range(&incoming, region.theirs_range());
            let start = result.len();
            let text = match region.resolution() {
                MergeResolution::Unchanged => base.copy_range(base_range.clone()),
                MergeResolution::Ours | MergeResolution::Both | MergeResolution::Conflict => {
                    local.copy_range(local_range.clone())
                }
                MergeResolution::Theirs => incoming.copy_range(incoming_range.clone()),
                _ => return Err(MergeError::UnsupportedResolution),
            };
            result.push_str(text);

            if region.is_conflict() {
                conflicts.push(Conflict {
                    id: ConflictId(conflicts.len()),
                    base: base_range,
                    local: local_range,
                    incoming: incoming_range,
                });
                states.push(ConflictState {
                    result: start..result.len(),
                    resolved: false,
                });
            }
        }

        Ok(Self {
            base,
            local,
            incoming,
            result: Document::from_bytes(result.into_bytes())?,
            conflicts,
            states,
            edits: EditHistory::default(),
            history: MergeHistory::default(),
        })
    }

    #[must_use]
    pub fn base(&self) -> &Document {
        &self.base
    }

    #[must_use]
    pub fn local(&self) -> &Document {
        &self.local
    }

    #[must_use]
    pub fn incoming(&self) -> &Document {
        &self.incoming
    }

    #[must_use]
    pub fn result(&self) -> &Document {
        &self.result
    }

    #[must_use]
    pub fn conflicts(&self) -> &[Conflict] {
        &self.conflicts
    }

    #[must_use]
    pub fn state(&self, id: ConflictId) -> Option<&ConflictState> {
        self.states.get(id.0)
    }

    pub fn unresolved(&self) -> impl Iterator<Item = ConflictId> + '_ {
        self.states
            .iter()
            .enumerate()
            .filter_map(|(index, state)| (!state.resolved).then_some(ConflictId(index)))
    }

    /// Group a native composition or a sequence of typing into one undo step.
    pub fn begin_transaction(&mut self, selection: TextSelection) {
        self.history.begin(&self.result, &self.states, selection);
        self.edits.begin_transaction(&self.result, selection);
    }

    /// Returns whether retiring a net-zero group restored conflict ranges.
    /// This is a presentation change, not a text edit or a new history step.
    pub fn finish_transaction(&mut self, selection: TextSelection) -> bool {
        self.edits.finish_transaction(&self.result, selection);
        self.history
            .finish(&self.result, &mut self.states, selection)
    }

    #[must_use]
    pub fn marked_range(&self) -> Option<Range<usize>> {
        self.edits.marked_range()
    }

    /// Ordinary edits never implicitly resolve or reopen conflicts. A replacement
    /// spanning several conflicts belongs to the first touched conflict; later
    /// covered conflicts remain addressable at empty insertion ranges.
    pub fn replace(
        &mut self,
        selection: TextSelection,
        range: Range<usize>,
        text: &str,
    ) -> Result<MergeUpdate, MergeError> {
        Ok(self.editing(selection, false).replace(range, text)?)
    }

    pub fn take(
        &mut self,
        id: ConflictId,
        choice: Take,
        selection: TextSelection,
    ) -> Result<MergeUpdate, MergeError> {
        self.apply_choice(id, choice, true, selection)
    }

    /// Restore this conflict's starting local text and unresolved status as one
    /// undo step, without rewinding decisions or edits in other conflicts.
    pub fn reset(
        &mut self,
        id: ConflictId,
        selection: TextSelection,
    ) -> Result<MergeUpdate, MergeError> {
        self.apply_choice(id, Take::Local, false, selection)
    }

    fn apply_choice(
        &mut self,
        id: ConflictId,
        choice: Take,
        resolved: bool,
        selection: TextSelection,
    ) -> Result<MergeUpdate, MergeError> {
        let conflict = self
            .conflicts
            .get(id.0)
            .ok_or(MergeError::UnknownConflict(id))?;
        let local = self.local.copy_range(conflict.local.clone());
        let incoming = self.incoming.copy_range(conflict.incoming.clone());
        let newline = self.result.newline();
        let mut text = match choice {
            Take::Local => local.to_owned(),
            Take::Incoming => incoming.to_owned(),
            Take::LocalThenIncoming => concatenate(local, incoming, newline),
            Take::IncomingThenLocal => concatenate(incoming, local, newline),
        };
        let range = self.states[id.0].result.clone();

        // Missing final newlines in an input must not glue accepted code to a
        // retained neighboring line. Preserve its bytes otherwise, including CRLF.
        if !text.is_empty()
            && range.start > 0
            && !self.result.text()[..range.start].ends_with(['\r', '\n'])
        {
            text.insert_str(0, newline);
        }
        if !text.is_empty() && range.end < self.result.text().len() && !text.ends_with(['\r', '\n'])
        {
            text.push_str(newline);
        }

        self.finish_transaction(selection);
        self.replace_inner(selection, range, &text, Some((id, resolved)))
    }

    pub fn set_resolved(
        &mut self,
        id: ConflictId,
        resolved: bool,
        selection: TextSelection,
    ) -> Result<MergeUpdate, MergeError> {
        if self.states.get(id.0).is_none() {
            return Err(MergeError::UnknownConflict(id));
        }

        self.finish_transaction(selection);
        let before = State::new(&self.states, selection);
        self.states[id.0].resolved = resolved;
        let after = State::new(&self.states, selection);
        self.history.record(before, after, false);

        Ok(MergeUpdate {
            selection,
            edit: None,
        })
    }

    pub fn undo(&mut self, selection: TextSelection) -> Result<Option<MergeUpdate>, MergeError> {
        self.finish_transaction(selection);
        self.history.undo(
            &mut self.result,
            &mut self.edits,
            &mut self.states,
            selection,
        )
    }

    pub fn redo(&mut self, selection: TextSelection) -> Result<Option<MergeUpdate>, MergeError> {
        self.finish_transaction(selection);
        self.history.redo(
            &mut self.result,
            &mut self.edits,
            &mut self.states,
            selection,
        )
    }

    fn replace_inner(
        &mut self,
        selection: TextSelection,
        range: Range<usize>,
        text: &str,
        decision: Option<(ConflictId, bool)>,
    ) -> Result<MergeUpdate, MergeError> {
        let old = self
            .result
            .text()
            .get(range.clone())
            .ok_or(InputError::InvalidRange)?;
        let changed = old != text;
        let before = State::new(&self.states, selection);
        let owner = decision
            .map(|(id, _)| id)
            .or_else(|| self.edit_owner(&range));
        let edit = self
            .edits
            .replace(&mut self.result, selection, range, text)?;

        if changed {
            remap(&mut self.states, &edit, owner);
        }
        if let Some((id, resolved)) = decision {
            self.states[id.0].resolved = resolved;
        }

        let after = State::new(&self.states, edit.selection);
        self.history.record(before, after, changed);

        Ok(MergeUpdate {
            selection: edit.selection,
            edit: changed.then_some(edit),
        })
    }

    fn edit_owner(&self, range: &Range<usize>) -> Option<ConflictId> {
        let touched = self.states.iter().position(|state| {
            if range.is_empty() {
                state.result.contains(&range.start)
                    || (state.result.is_empty() && state.result.start == range.start)
            } else {
                state.result.start < range.end && state.result.end > range.start
                    || (state.result.is_empty() && range.contains(&state.result.start))
            }
        });
        let ending = || {
            range
                .is_empty()
                .then(|| {
                    self.states
                        .iter()
                        .rposition(|state| state.result.end == range.start)
                })
                .flatten()
        };

        touched.or_else(ending).map(ConflictId)
    }
}

fn byte_range(document: &Document, lines: Range<usize>) -> Range<usize> {
    let boundary = |index: usize| {
        document
            .lines()
            .get(index)
            .map_or(document.text().len(), |line| line.full.start)
    };
    boundary(lines.start)..boundary(lines.end)
}

fn concatenate(first: &str, second: &str, newline: &str) -> String {
    let mut text = first.to_owned();
    if !first.is_empty() && !second.is_empty() && !first.ends_with(['\r', '\n']) {
        text.push_str(newline);
    }
    text.push_str(second);

    text
}

fn remap(states: &mut [ConflictState], edit: &EditOutcome, owner: Option<ConflictId>) {
    let start = edit.replaced.start;
    let end = edit.replaced.end;
    let new_end = start + edit.inserted_len;
    let after = |offset| new_end + offset - end;

    for (index, state) in states.iter_mut().enumerate() {
        let range = &mut state.result;
        if owner == Some(ConflictId(index)) {
            range.start = range.start.min(start);
            range.end = if range.end >= end {
                after(range.end)
            } else {
                new_end
            };
        } else if edit.replaced.is_empty() && range.start == start {
            // Multiple deleted conflicts can share an anchor. Explicit take
            // actions reinsert them in conflict order without consuming a neighbor.
            if owner.is_none_or(|id| index > id.0) {
                range.start += edit.inserted_len;
                range.end += edit.inserted_len;
            }
        } else if range.end > start {
            range.start = if range.start >= end {
                after(range.start)
            } else {
                new_end
            };
            range.end = if range.end > end {
                after(range.end)
            } else {
                new_end
            };
        }
    }
}
