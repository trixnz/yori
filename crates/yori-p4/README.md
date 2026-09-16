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
| `x86_64-unknown-linux-gnu` | `p4api-glibc2.3-openssl3.tgz` | `ee54ca7f0cd191b88a151e5c07a2e5cb53e4345da70335fd76bf2545efdeb821` |
| `aarch64-unknown-linux-gnu` | `p4api-openssl3.tgz` | `fd950aa1cfe4110e279508aba144a035cbc704a58d20acee08bfd11de6c60f4f` |
| `x86_64-pc-windows-msvc` | `p4api_vs2022_dyn_openssl3.zip` | `ad91d06dde00453a16dafd9601192c8e06a1c3477c12df5c54d6d0dfeba79f54` |

The Windows dynamic-runtime archive matches Rust's default MSVC `/MD` runtime.
Linux and Windows builds link compatible OpenSSL 3 libraries supplied by the
build environment. Nix pins both the P4API fetch and OpenSSL dependency.

Binary packages must ship the repository's `THIRD_PARTY_NOTICES.md` alongside
the Yori license. The Nix package installs both under `share/doc/yori`.
