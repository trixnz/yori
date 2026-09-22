# yori

yori compares and reconciles individual source files. Its primary surface
combines reviewing differences with editing the participating documents.

## Language

**Document**:
The source contents of one participating file, distinct from their presentation
in a comparison.

**Two-way comparison**:
A comparison between two documents, either of which may be read-only or editable
according to its role.
_Avoid_: Three-way merge when describing a baseline/local comparison

**Baseline**:
The reference document against which local changes are reviewed; in the primary
Perforce workflow, it is the read-only server revision.

**Local document**:
The editable working-file contents being reviewed against a baseline in the
primary workflow.

**Diff editor**:
The unified surface for reviewing differences, directly editing permitted
documents and transferring changes between them.
_Avoid_: Separate edit mode when describing a different viewing surface

**Display row**:
A horizontal position in the diff editor that can contain source lines, alignment
gaps, conflict controls or base context; it is not itself a source line.

**Alignment row**:
A blank display position on one side of a comparison that lines up corresponding
source content without being a source line itself.
_Avoid_: Empty source line, padding newline

**Continuation row**:
A presentation-only visual line created when source content wraps within a display row.
Shorter panes contribute blank continuation space so corresponding content stays aligned.
_Avoid_: Wrapped source line, inserted newline

**Change transfer**:
An edit that applies a change from one participating document to the other;
restoring a baseline block into a local document is one example.
_Avoid_: Three-way merge, conflict resolution

**Three-way merge**:
Reconciliation of two versions relative to a common base into a result document,
including resolution of conflicting changes.

**Merge display**:
The aligned presentation of local, result and incoming documents in a three-way
merge, including conflict controls and optional base excerpts. Presentation-only
content is distinct from the participating documents.

**Base**:
The common-ancestor document used to distinguish independent changes from conflicts
in a three-way merge.

**Incoming document**:
The other changed version being reconciled with the local document relative to
base. The role does not imply a particular version-control system.

**Result document**:
The editable reconciliation of local and incoming changes. Its destination path
is distinct from its current contents and does not make it another merge input.

**Conflict**:
Overlapping changes that need an explicit reconciliation decision. Editing or
taking selected lines does not by itself declare the conflict resolved.

**Saved checkpoint**:
The document contents last loaded or successfully saved, used to distinguish
unsaved edits from differences against another file. A new merge result has no
saved checkpoint until it is explicitly saved.

**Home**:
The always-accessible workspace surface for starting workflows and opening application
preferences. It is not an open comparison, merge or review session.
_Avoid_: Landing page, standalone mode

**Review source**:
The source-control state selected for review, such as working changes, a Git commit or a
submitted Perforce changelist.
_Avoid_: Change set

**Working changes**:
Editable local differences used as a review source. Git working changes combine staged,
unstaged and untracked files; Perforce working changes belong to one pending changelist.

**Review session**:
A collection of file comparisons reviewed together because they belong to one review
source. It does not stage, commit, submit, revert or reorganize source-control state.
_Avoid_: Review tab, change tab, source-control client

**Workspace**:
The application's collection of open diff, merge and review-session tabs, not a source
project or directory-comparison scope.
