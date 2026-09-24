<h1>
  <img src="assets/platform/linux/hicolor/256x256/apps/io.github.trixnz.yori.png" width="32" alt="" valign="middle">
  yori
</h1>

yori is a native desktop application for diffing, merging, and reviewing source files.

It combines side-by-side comparison with direct editing, supporting two-way
comparisons, three-way merges, multi-file reviews of Git and Perforce changes, change
restoration, conflict resolution, and multiple open files in a tabbed workspace.

> [!NOTE]
> yori is under active development. Linux remains the primary development platform.
> Linux and Windows are tested in CI; macOS is not yet supported. Perforce review is
> new and needs a configured Perforce client in the directory yori is launched from.

## Features

### Compare and merge

- Side-by-side two-way file comparison
- Three-way merging with explicit conflict resolution
- Whole-change and selected-line restoration
- Synchronized scrolling and change navigation
- Word- and token-level change highlighting
- Multiple comparisons, merges, and reviews in tabs

### Review

- Multi-file review of any Git revision: a commit, branch, tag, or other revision
- Review of uncommitted working changes, including untracked files
- Perforce pending and submitted changelists through the user-installed `p4` CLI
- Keyboard-first file navigator with per-file change counts
- Local files stay editable in working-change and pending-changelist reviews;
  historical revisions open read-only
- Binary files and submodules are listed with their status rather than diffed

### Edit

- Editable local files with undo and redo
- Syntax highlighting for C, C++, Go, and Rust
- Guarded saves with external file-change detection
- Preservation of LF, CRLF, Unicode, tabs, and missing final newlines
- Optional whitespace markers, change connections, and Vim-style input

Alignment gaps, conflict controls, and other presentation elements never become
part of the underlying source files.

## Run with Nix

```sh
nix run github:trixnz/yori
```

Pass file arguments after `--`:

```sh
nix run github:trixnz/yori -- BASELINE LOCAL
```

### Optional binary cache

CI publishes builds to the `trixnz-yori` Cachix cache. Nix can build yori
without it, but configuring the cache avoids rebuilding available revisions.

Configure it automatically with Cachix:

```sh
nix run nixpkgs#cachix -- use trixnz-yori
```

Or add it to a NixOS configuration:

```nix
nix.settings = {
  extra-substituters = [
    "https://trixnz-yori.cachix.org"
  ];
  extra-trusted-public-keys = [
    "trixnz-yori.cachix.org-1:v0OV3ETheOdXTYZRhtaDUeO5dEtzzboH0fB6mRag3Z0="
  ];
};
```

For other multi-user Nix installations, add the equivalent settings to
`/etc/nix/nix.conf`:

```ini
extra-substituters = https://trixnz-yori.cachix.org
extra-trusted-public-keys = trixnz-yori.cachix.org-1:v0OV3ETheOdXTYZRhtaDUeO5dEtzzboH0fB6mRag3Z0=
```

## Building from source

Build yori with Cargo:

```sh
cargo build --release -p yori
```

Perforce support adds no native build dependencies. At runtime, Perforce review
requires the official `p4` executable on `PATH`, a reachable server, and the client
configuration described under [Usage](#reviewing-changes).

### Nix

```sh
nix develop
cargo build --release -p yori
```

To run yori during development:

```sh
cargo run -p yori
```

## Usage

Open the workspace:

```sh
yori
```

Compare a baseline with an editable local file:

```sh
yori BASELINE LOCAL
```

Open a three-way merge:

```sh
yori BASE LOCAL INCOMING RESULT
```

The merge view displays `LOCAL`, `RESULT`, and `INCOMING`. `BASE` provides the
common ancestor, while `RESULT` identifies the output destination.

Additional invocations open files in the existing application window. By default, an
invocation exits once the workspace accepts the request. It does not wait for editing
or merge completion.

Add `--wait` before the paths to keep the invocation running until you close its tab:

```sh
yori --wait BASE LOCAL INCOMING RESULT
```

The invocation exits with status 0 if you saved the tab, and with status 1 if you
closed it without saving. If no yori window is open, yori starts one in the
background. Closing the tab does not close the window.

#### Git mergetool

To use yori with `git mergetool`, add this to your Git configuration:

```ini
[merge]
    tool = yori
[mergetool "yori"]
    cmd = yori --wait "$BASE" "$LOCAL" "$REMOTE" "$MERGED"
    trustExitCode = true
```

Git starts one merge tab for each conflicted file. Save the result and close the tab 
to continue with the next file. 
If you close the tab without saving, Git keeps the file conflicted.

### Reviewing changes

Launch yori from a working tree to review a whole change rather than a file pair:

```sh
cd path/to/repository
yori
```

Home lists recent commits from the repository yori was launched in, with uncommitted
working changes above them, so the most likely review is one click away.
<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>G</kbd> opens the Git chooser, which also takes
any revision by name — a commit, branch, or tag.

A review opens as a single tab with a file navigator listing every changed file and
its change counts. Working changes and Perforce pending changelists leave the local
file editable and saveable; committed and submitted revisions open read-only on both
sides.

Git working changes mark files with an unresolved merge conflict as `U`. The review
shows the working file with its conflict markers. Select the file and choose **Start
three-way merge** to open a merge tab. The tab uses the base, local, and incoming
versions that Git recorded in the index. Saving the result writes the working file.
It does not mark the conflict as resolved: run `git add` when you are done.
Conflicts where one side deleted the file, and binary, symbolic link, and submodule
conflicts, cannot be merged as text.

Perforce review invokes `p4` from the launch directory and reads its ambient
configuration, including `P4CONFIG`, ticket, and trust files. The `p4` executable
must be available on `PATH`. yori offers the pending and submitted changelists of
that client.

## Keyboard shortcuts

| Action | Shortcut |
| --- | --- |
| Open a comparison | <kbd>Ctrl</kbd>+<kbd>O</kbd> |
| Open a merge | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>M</kbd> |
| Open a Git review | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>G</kbd> |
| Open Home | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>H</kbd> |
| Save | <kbd>Ctrl</kbd>+<kbd>S</kbd> |
| Close the active tab | <kbd>Ctrl</kbd>+<kbd>W</kbd> |
| Switch tabs | <kbd>Ctrl</kbd>+<kbd>Tab</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Tab</kbd> |
| Visit the previous or next change | <kbd>Alt</kbd>+<kbd>Up</kbd> / <kbd>Alt</kbd>+<kbd>Down</kbd> |
| Apply the selected-line action | <kbd>Alt</kbd>+<kbd>Enter</kbd> |
| Undo or redo | <kbd>Ctrl</kbd>+<kbd>Z</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> |
| Open preferences | <kbd>Ctrl</kbd>+<kbd>,</kbd> |
| Quit | <kbd>Ctrl</kbd>+<kbd>Q</kbd> |

Lists and the review navigator also accept <kbd>J</kbd> and <kbd>K</kbd> to move,
<kbd>Enter</kbd> or <kbd>Space</kbd> to open, and <kbd>Ctrl</kbd>+<kbd>H</kbd> /
<kbd>Ctrl</kbd>+<kbd>L</kbd> to move between panes. On macOS, substitute
<kbd>Cmd</kbd> for <kbd>Ctrl</kbd> in the table above.

## Configuration

Preferences live in `config.toml` under the platform configuration directory —
`~/.config/yori/config.toml` on Linux. Open the dialog with
<kbd>Ctrl</kbd>+<kbd>,</kbd>, or edit the file directly:

```toml
[editor]
vim_keybindings = false
show_whitespace = false
show_change_connections = false
word_wrap = false

[keybindings]
save = ["primary-s", "primary-shift-s"]
redo = []
next_change = ["alt-down", "primary-j"]
toggle_word_wrap = ["alt-z"]
```

Each key under `[keybindings]` is a stable action name whose array replaces all
of that action's defaults. Omit an action to retain its defaults, provide several
strings to assign multiple shortcuts, or use an empty array to disable it.
`primary` is portable: it resolves to <kbd>Cmd</kbd> on macOS and
<kbd>Ctrl</kbd> elsewhere. Key sequences are separated by spaces within one
string, for example `"primary-k primary-s"`.

| Action name | Default binding(s) |
| --- | --- |
| `open_comparison` | `primary-o` |
| `open_merge` | `primary-shift-m` |
| `open_git_review` | `primary-shift-g` |
| `save` | `primary-s` |
| `close_tab` | `primary-w` |
| `quit` | `primary-q` |
| `preferences` | `primary-,` |
| `next_tab` | `ctrl-tab` |
| `previous_tab` | `ctrl-shift-tab` |
| `show_home` | `primary-shift-h` |
| `previous_change` | `alt-up` |
| `next_change` | `alt-down` |
| `restore_selected_lines` | `alt-enter` |
| `focus_previous_pane` | `ctrl-h` |
| `focus_next_pane` | `ctrl-l` |
| `toggle_word_wrap` | None |
| `copy` | `primary-c` |
| `paste` | `primary-v` |
| `cut` | `primary-x` |
| `select_all` | `primary-a` |
| `undo` | `primary-z` |
| `redo` | `primary-shift-z`, `primary-y` |

Changes take effect when yori regains focus. The complete candidate keymap is
validated first; an unknown action, malformed keystroke, or duplicate in the
same key context rejects the entire candidate and leaves the last valid keymap
active. Text entry and movement keys, Vim bindings, and list-local navigation
remain fixed.

yori rewrites only the editor keys it owns, so `[keybindings]`, comments, and
unrelated entries survive a change made through the Preferences dialog.

## Project structure

- `crates/yori-document` owns source text, line endings, edits, and history.
- `crates/yori-diff` owns comparison, alignment, restoration, and merge behavior.
- `crates/yori-p4` owns asynchronous access to the Perforce `p4` CLI.
- `crates/yori` owns the application and native interface.

The document and diff crates remain independent of the UI framework and can be
tested without creating a window. The editor ownership decision is recorded in
[ADR-0001](docs/adr/0001-own-diff-editor-reuse-gpui-components.md).

## Development

Run the complete local validation suite before submitting a change:

```sh
./scripts/check
```

This checks formatting, Clippy, tests, and the native build.

## License

yori is available under the [MIT License](LICENSE).
