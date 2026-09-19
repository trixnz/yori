---
status: accepted
---

# Use native source-control libraries for reviews

yori will use `gix` for Git and project-owned Rust bindings to Perforce's official
P4API for Perforce review sources, rather than invoking source-control command-line
programs. Native libraries provide structured data and direct document contents without
requiring users to install or automate separate CLI processes.

## Consequences

The Perforce module must isolate the blocking, thread-affine C++ client behind a small
Rust interface. yori's build and distribution must link the target-specific P4API and
compatible OpenSSL libraries, pin their artifacts, and ship the P4/P4API and applicable
third-party notices required for binary redistribution.
