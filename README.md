# yori

yori is a native desktop application for comparing and merging source files.

It combines side-by-side review with direct editing, supporting two-way comparisons,
three-way merges, change restoration, conflict resolution, and multiple open files in
a tabbed workspace.

> [!NOTE]
> yori is under active development. Linux is currently the primary development and
> testing platform. Windows and macOS support are planned but not yet validated.

## Features

- Side-by-side two-way file comparison
- Editable local files with undo and redo
- Whole-change and selected-line restoration
- Three-way merging with explicit conflict resolution
- Multiple comparisons and merges in tabs
- Synchronized scrolling and change navigation
- Syntax highlighting for C, C++, Go, and Rust
- Word- and token-level change highlighting
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

```sh
cargo build --release -p yori
```

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

Additional invocations open files in the existing application window. Automated
external-tool integrations should be considered experimental until process waiting
and completion signaling are implemented.

## Keyboard shortcuts

| Action | Shortcut |
| --- | --- |
| Open a comparison | <kbd>Ctrl</kbd>+<kbd>O</kbd> |
| Open a merge | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>M</kbd> |
| Save | <kbd>Ctrl</kbd>+<kbd>S</kbd> |
| Close the active tab | <kbd>Ctrl</kbd>+<kbd>W</kbd> |
| Switch tabs | <kbd>Ctrl</kbd>+<kbd>Tab</kbd> |
| Visit the previous or next change | <kbd>Alt</kbd>+<kbd>Up</kbd> / <kbd>Alt</kbd>+<kbd>Down</kbd> |
| Apply the selected-line action | <kbd>Alt</kbd>+<kbd>Enter</kbd> |
| Undo or redo | <kbd>Ctrl</kbd>+<kbd>Z</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> |

## Project structure

- `crates/yori-document` owns source text, line endings, edits, and history.
- `crates/yori-diff` owns comparison, alignment, restoration, and merge behavior.
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
