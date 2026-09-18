//! Provider-agnostic multi-file review sessions.

pub(crate) mod model;
mod session;

pub(crate) use model::{ReviewSource, ReviewSourceIdentity};
pub(crate) use session::{ReviewChanged, ReviewSession, init};
