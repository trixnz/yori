//! Provider-agnostic multi-file review sessions.

pub(crate) mod model;
pub(crate) mod perforce;
mod session;

pub(crate) use model::{ReviewSource, ReviewSourceIdentity};
pub(crate) use perforce::PerforceContext;
pub(crate) use session::{ReviewChanged, ReviewSession, init};
