# yori-p4

`yori-p4` is yori's asynchronous boundary around the official Perforce `p4`
command-line client. Perforce review requires `p4` on `PATH`; the crate does not
link P4API or OpenSSL and has no native build script.

A dedicated worker serializes commands away from the UI thread. Commands run in
the selected working directory so `p4` resolves the user's ambient `P4CONFIG`,
ticket, trust, environment, and platform configuration normally. Process launch,
connection, authentication, mapping, and command failures are returned as typed,
actionable errors.

Structured commands use `p4 -ztag -Mj`. Queries over multiple depot paths or
revisions use `-x -` so one review does not start one process per file. Text depot
contents are decoded from the batched JSON stream; binary contents use raw
`p4 print -q` output so arbitrary bytes are preserved.

The public API covers client context, pending/default and submitted changelists,
opened files, descriptions, have revisions, inclusive and exclusion workspace
mappings, and depot content. Tests exercise captured `p4` protocol output and the
higher-level review behavior without requiring a live Perforce server.
