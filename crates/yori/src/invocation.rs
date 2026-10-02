use std::path::PathBuf;

use crate::comparison::Comparison;

/// One process invocation, independent of whether it owns or contacts the application instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InvocationRequest {
    pub directory: PathBuf,
    pub comparisons: Vec<Comparison>,
    /// Keep the invoking process until the opened tab closes, as external merge
    /// tools such as `git mergetool` expect.
    pub wait: bool,
}

impl InvocationRequest {
    pub fn new(directory: PathBuf, comparisons: Vec<Comparison>) -> Self {
        Self {
            directory,
            comparisons,
            wait: false,
        }
    }

    pub fn waiting(self) -> Self {
        Self { wait: true, ..self }
    }
}
