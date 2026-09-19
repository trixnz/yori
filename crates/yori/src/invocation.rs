use std::path::PathBuf;

use crate::comparison::Comparison;

/// One process invocation, independent of whether it owns or contacts the application instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InvocationRequest {
    pub directory: PathBuf,
    pub comparisons: Vec<Comparison>,
}

impl InvocationRequest {
    pub fn new(directory: PathBuf, comparisons: Vec<Comparison>) -> Self {
        Self {
            directory,
            comparisons,
        }
    }
}
