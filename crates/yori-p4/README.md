# yori-p4

`yori-p4` is Yori's safe asynchronous boundary around Perforce's official C++
P4API. It does not invoke or require the `p4` executable.

A dedicated worker creates, uses, and destroys the thread-affine `ClientApi`.
Calls use tagged output and ambient configuration from the selected working
directory, including `P4CONFIG`, ticket, and trust files. The public API covers
client context, pending/default and submitted changelists, opened files,
descriptions, have revisions, workspace mappings, and depot content.

## Native artifacts

The build expects `P4API_ROOT` to contain extracted `include/` and `lib/`
directories. `scripts/fetch-p4api` provisions these pinned archives and verifies
the archive SHA-256 before extraction.

| Target | P4API 2025.1 patch 3042095 artifact | SHA-256 |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | `p4api-glibc2.3-openssl3.5.tgz` | `466352f49f585f514bfee13efcb927e93ea567bcc2dde200ef2c750d8555a0dc` |
| `aarch64-unknown-linux-gnu` | `p4api-openssl3.5.tgz` | `3bba25b44917341ddbc2416bdc68ab84118d2dc37ca90edc5ec829064ade2b63` |
| `x86_64-pc-windows-msvc` | `p4api_vs2022_dyn_openssl3.5.zip` | `b05db557dc5dd8d3b3e316632afb457bb1c4bdf4b696ffc14e5df78acc743a57` |

P4API and OpenSSL 3.5 are statically linked on every supported platform.
`openssl-src` builds the pinned OpenSSL source during the Rust build, avoiding a
runtime OpenSSL dependency or mismatch. The Windows P4API artifact's `dyn`
marker refers to its MSVC `/MD` runtime compatibility, not OpenSSL linkage.

Binary packages must ship the repository's `THIRD_PARTY_NOTICES.md` alongside
the Yori license. The Nix package installs both under `share/doc/yori`.
