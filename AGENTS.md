# Agent Instructions

## Screenshots and privacy

Never capture screenshots of the user's system, desktop, windows, or applications,
including through tools, scripts, or subagents. When visual evidence is needed,
ask the user to take and provide a screenshot instead.

## Native UI

For GPUI styling, layout, icon work, or screenshot feedback, read `docs/agents/native-ui.md`.

## Rust readability

Write for human scanning, not minimum vertical space. Use blank lines as logical
punctuation within functions and multi-step closures:

- Separate an opening guard section from the main path with a blank line.
  Related guards may stay together; a guard inside a loop follows the same rule.
- Separate distinct phases: setup, computation, state updates, external/UI effects,
  and final notification or result. Keep a coherent state update together, then
  leave breathing room before focus changes, I/O, or notification when those are
  a separate step. Short, single-purpose functions can remain compact.
- Keep statements that express one idea together: related local derivations,
  paired updates, and assertion clusters. Do not insert a blank line after every
  declaration, statement, or fixed number of lines.
- Keep fluent GPUI builder chains contiguous. Separate preparatory locals from
  the builder, and give multi-step closure bodies the same logical grouping as
  functions. Use meaningful locals to clarify dense nested expressions; do not
  mechanically expand trivial closures or split every modifier with blank lines.
- In tests, separate substantial setup, action, and assertion phases, and put a
  blank line between independent scenarios. Whitespace should show this structure
  without mandatory arrange/act/assert comments.

Apply readability conventions in the first draft. Do not write deliberately
compressed code with the intention of formatting it later. This includes test setup,
actions, and assertions.

## Local validation

After the final edit, run `./scripts/check`. A task is complete only when the
format, Clippy, test, and build checks all pass with zero warnings. Fix failures
and rerun the gate; if blocked, report the failing command and leave the work
explicitly unfinished. Report native GUI behavior separately: automated checks
do not establish that interactions work.

Keep the lint baseline intact. Do not weaken lint settings, remove checks, or add
blanket suppressions to get a passing result. When a lint genuinely does not fit,
use the narrowest `#[expect(..., reason = "...")]` with a concrete justification
and mention the exception in the handoff. Changes to workspace-wide lint policy
require owner approval.

## Commits and pull requests

Use [Conventional Commits](https://www.conventionalcommits.org/) for every commit message and pull-request title:

```text
<type>[optional scope]: <description>
```

Use the type that best describes the change, such as `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`, or `chore`.

Write descriptions in lowercase imperative form without a trailing period. Mark breaking changes with `!` or a `BREAKING CHANGE:` footer as defined by the convention.

## Agent skills

### Issue tracker

Issues are tracked in GitHub Issues. See `docs/agents/issue-tracker.md`.

### Triage labels

Triage uses the five default canonical labels. See `docs/agents/triage-labels.md`.

### Domain docs

Domain documentation uses a single-context layout. See `docs/agents/domain.md`.
