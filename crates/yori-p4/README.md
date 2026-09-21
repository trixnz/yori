# yori-p4

`yori-p4` is yori's safe asynchronous boundary around Perforce's official C++
P4API. It does not invoke or require the `p4` executable.

A dedicated worker enters the P4API thread runtime, creates and uses the
thread-affine `ClientApi`, destroys the client, and then leaves the thread
runtime. Process-wide P4API libraries are reference-counted around those worker
lifetimes. Native entry points are serialized because P4API lifecycle state is
process-global; none of this work runs on the UI thread. Initialization and
cleanup failures are returned as typed actionable errors.

Calls use tagged output and ambient configuration from the selected working
directory, including `P4CONFIG`, ticket, and trust files. The public API covers
client context, pending/default and submitted changelists, opened files,
descriptions, have revisions, inclusive and exclusion workspace mappings, and
depot content.

## Native artifacts

The build script downloads the pinned P4API archive for the target on first use,
verifies its SHA-256, and reuses the extracted files from the platform cache.
Set `P4API_CACHE_DIR` only when the cache must live somewhere other than the
platform default, such as in CI or a Nix build.

| Target | P4API 2025.1 patch 3042095 artifact | SHA-256 |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | `p4api-glibc2.12-openssl3.5.tgz` | `b3840d7e4b889e480929134703d2215409f8a86e8e206158fb6651734086805d` |
| `x86_64-pc-windows-msvc` | `p4api_vs2022_dyn_openssl3.5.zip` | `b05db557dc5dd8d3b3e316632afb457bb1c4bdf4b696ffc14e5df78acc743a57` |

P4API and OpenSSL 3.5 are statically linked on every supported platform.
`openssl-src` builds the pinned OpenSSL source during the Rust build, avoiding a
runtime OpenSSL dependency or mismatch. The Windows P4API artifact's `dyn`
marker refers to its MSVC `/MD` runtime compatibility, not OpenSSL linkage.

Binary packages must ship the repository's `THIRD_PARTY_NOTICES.md` alongside
the yori license. The Nix package installs both under `share/doc/yori`.
