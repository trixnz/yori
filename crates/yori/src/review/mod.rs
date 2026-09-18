//! Provider-agnostic multi-file review sessions.

mod git;
pub(crate) mod model;
pub(crate) mod perforce;
mod session;
mod source_chooser;

use gpui_kit::App;

pub(crate) use git::{GitCommitSummary, GitRepository};
pub(crate) use model::{ReviewSource, ReviewSourceIdentity};
pub(crate) use perforce::PerforceContext;
pub(crate) use session::{ReviewChanged, ReviewSession};
pub(crate) use source_chooser::{GitSourceChooser, GitSourceChooserEvent};

pub(crate) fn init(cx: &mut App) {
    session::init(cx);
}
