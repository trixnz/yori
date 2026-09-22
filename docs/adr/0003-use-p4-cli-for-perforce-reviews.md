---
status: accepted
---

# Use the P4 CLI for Perforce reviews

yori will use `gix` for Git and invoke the user's official `p4` executable for
Perforce review sources. This supersedes ADR-0002's decision to link P4API.

The Perforce boundary runs tagged JSON commands in the selected working directory,
where `p4` applies the user's ambient client, server, ticket, trust, and `P4CONFIG`
configuration. Review-wide path and content queries are batched so process startup
cost does not grow linearly with the number of files. Binary content uses raw
`p4 print` output because tagged JSON cannot preserve arbitrary bytes reliably.

## Consequences

Perforce review requires `p4` on `PATH`; Git and file comparison do not. yori no
longer builds, links, caches, distributes, or notices P4API and OpenSSL. The process
boundary adds command startup and protocol parsing costs, but removes the dominant
native build cost and delegates Perforce compatibility and credential handling to
the supported client installation.
