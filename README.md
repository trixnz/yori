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
- Perforce pending and submitted changelists through the native P4API, without the
  `p4` executable
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

Perforce review links Perforce's official C++ P4API rather than shelling out to the
`p4` executable. Cargo downloads the pinned P4API for the build target on first use,
verifies its SHA-256, and reuses the extracted files from the platform cache.
That cache lives under `%LOCALAPPDATA%\yori\p4api` on Windows and
`$XDG_CACHE_HOME/yori/p4api` or `~/.cache/yori/p4api` on Linux:

```sh
cargo build --release -p yori
```

P4API and the pinned OpenSSL 3.5 source are statically linked. Building requires
`curl`, `tar`, Perl, and the platform C/C++ build toolchain, including `make` on
Unix, but no system OpenSSL development package. Windows 10 and later include
`curl` and `tar`; Nix provisions all required build tools and the P4API cache.

The resulting binary carries P4API with it. Running yori needs no Perforce
installation, though Perforce review still needs a reachable server and the client
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

Additional invocations open files in the existing application window. Automated
invocations exit once the workspace accepts the request; they do not wait for editing
or merge completion. External-tool integrations requiring that lifecycle remain
experimental.

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

Perforce review reads the ambient configuration of the launch directory, including
`P4CONFIG`, ticket, and trust files, and offers the pending and submitted changelists
of that client.

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
```

yori rewrites only the keys it owns, so comments and unrelated entries survive a
change made through the dialog.

## Project structure

- `crates/yori-document` owns source text, line endings, edits, and history.
- `crates/yori-diff` owns comparison, alignment, restoration, and merge behavior.
- `crates/yori-p4` owns asynchronous access to the native Perforce P4API.
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

yori is available under the [MIT License](LICENSE). Binary distributions also
include the applicable [third-party notices](THIRD_PARTY_NOTICES.md) for P4API,
OpenSSL, and bundled components.
