//! Typed asynchronous access to Perforce review data through the official native P4API.
//!
//! [`P4Client`] owns no native state itself. A dedicated worker thread creates, uses, and
//! destroys the thread-affine C++ client. Connection setup reads ambient Perforce settings,
//! including `P4CONFIG`, tickets, and trust files, from the supplied working directory.

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

#[expect(
    unsafe_code,
    reason = "cxx generates the unsafe FFI implementation behind this safe, audited bridge"
)]
mod bridge {
    pub(super) use super::{CancellationState, cancellation_requested};

    #[cxx::bridge(namespace = "yori::p4")]
    pub(crate) mod ffi {
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
        struct RawResult {
            records: Vec<RawRecord>,
            messages: Vec<RawMessage>,
            output: Vec<u8>,
        }

        extern "Rust" {
            type CancellationState;

            fn cancellation_requested(state: &CancellationState) -> bool;
        }

        unsafe extern "C++" {
            include!("p4_bridge.h");

            type NativeThread;

            fn start_thread(result: &mut RawResult) -> UniquePtr<NativeThread>;
            fn ready(self: &NativeThread) -> bool;
            fn shutdown(self: Pin<&mut NativeThread>, result: &mut RawResult);

            type NativeClient;

            fn connect(
                cwd: &str,
                port_override: &str,
                result: &mut RawResult,
            ) -> UniquePtr<NativeClient>;
            fn connected(self: &NativeClient) -> bool;
            fn close(self: Pin<&mut NativeClient>, result: &mut RawResult);
            fn run(
                self: Pin<&mut NativeClient>,
                command: &str,
                arguments: &[String],
                cancellation: &CancellationState,
                result: &mut RawResult,
            );

            fn capture_diagnostic(diagnostic: &[u8], result: &mut RawResult);
        }
    }
}

use bridge::ffi;
#[cfg(test)]
use ffi::RawField;
use ffi::{RawMessage, RawRecord, RawResult};

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "this wrapper exists only for the native invalid-UTF-8 regression"
    )
)]
fn capture_diagnostic_for_test(diagnostic: &[u8], result: &mut RawResult) {
    ffi::capture_diagnostic(diagnostic, result);
}
