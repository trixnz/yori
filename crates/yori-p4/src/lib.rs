//! Typed asynchronous access to Perforce review data through the `p4` command-line client.
//!
//! [`P4Client`] runs commands from the supplied working directory so `p4` discovers the same
//! ambient `P4CONFIG`, ticket, trust, and Windows registry settings as an interactive invocation.

mod client;
mod error;
mod model;
mod parse;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub use client::{CancellationToken, P4Client};
pub use error::{Error, ErrorKind, Result};
pub use model::{
    ChangedFile, ChangelistDescription, ChangelistId, ChangelistStatus, ChangelistSummary,
    ClientInfo, DepotRevision, FileAction, HaveRevision, OpenedFile, ParseChangelistIdError,
    PendingChangelists, WorkspaceMapping,
};

#[derive(Debug, Default)]
struct CancellationState {
    cancelled: AtomicBool,
    parent: Option<Arc<Self>>,
}

impl CancellationState {
    fn following(parent: Arc<Self>) -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            parent: Some(parent),
        }
    }
}

fn cancellation_requested(state: &CancellationState) -> bool {
    state.cancelled.load(Ordering::Acquire)
        || state.parent.as_deref().is_some_and(cancellation_requested)
}

#[derive(Clone, Debug, Default)]
struct RawField {
    name: String,
    value: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
struct RawRecord {
    fields: Vec<RawField>,
}

#[derive(Clone, Debug, Default)]
struct RawMessage {
    severity: i32,
    generic: i32,
    text: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
struct RawPrintedFile {
    depot_path: String,
    revision: u32,
    file_type: Option<String>,
    contents: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
struct RawResult {
    records: Vec<RawRecord>,
    messages: Vec<RawMessage>,
    output: Vec<u8>,
    printed_files: Vec<RawPrintedFile>,
}
