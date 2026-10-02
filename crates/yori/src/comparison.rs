//! Document identity, content sources, capabilities, and comparison construction.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DocumentContent {
    File(PathBuf),
    Memory(Arc<[u8]>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ComparisonDocument {
    logical_path: PathBuf,
    content: DocumentContent,
    editable: bool,
    save_destination: Option<PathBuf>,
}

impl ComparisonDocument {
    pub fn read_only_file(path: PathBuf) -> Self {
        Self {
            logical_path: path.clone(),
            content: DocumentContent::File(path),
            editable: false,
            save_destination: None,
        }
    }

    pub fn editable_file(path: PathBuf) -> Self {
        Self {
            logical_path: path.clone(),
            content: DocumentContent::File(path.clone()),
            editable: true,
            save_destination: Some(path),
        }
    }

    pub fn read_only_memory(logical_path: PathBuf, content: Vec<u8>) -> Self {
        Self {
            logical_path,
            content: DocumentContent::Memory(content.into()),
            editable: false,
            save_destination: None,
        }
    }

    pub fn editable_memory(
        logical_path: PathBuf,
        content: Vec<u8>,
        save_destination: Option<PathBuf>,
    ) -> Self {
        Self {
            logical_path,
            content: DocumentContent::Memory(content.into()),
            editable: true,
            save_destination,
        }
    }

    pub fn logical_path(&self) -> &Path {
        &self.logical_path
    }

    pub fn content(&self) -> &DocumentContent {
        &self.content
    }

    pub fn editable(&self) -> bool {
        self.editable
    }

    pub fn save_destination(&self) -> Option<&Path> {
        self.save_destination.as_deref()
    }

    fn resolve(&self) -> Result<Self, String> {
        let (content, resolved_source) = match &self.content {
            DocumentContent::File(path) => {
                let resolved = resolve_input(path)?;
                (
                    DocumentContent::File(resolved.clone()),
                    Some((path, resolved)),
                )
            }
            DocumentContent::Memory(bytes) => (DocumentContent::Memory(bytes.clone()), None),
        };

        let logical_path = resolved_source
            .as_ref()
            .filter(|(original, _)| self.logical_path == **original)
            .map_or_else(
                || self.logical_path.clone(),
                |(_, resolved)| resolved.clone(),
            );
        let save_destination = self
            .save_destination
            .as_ref()
            .map(|destination| {
                resolved_source
                    .as_ref()
                    .filter(|(original, _)| destination == *original)
                    .map_or_else(
                        || resolve_result(destination),
                        |(_, resolved)| Ok(resolved.clone()),
                    )
            })
            .transpose()?;

        Ok(Self {
            logical_path,
            content,
            editable: self.editable,
            save_destination,
        })
    }

    fn file_path(&self) -> Option<&Path> {
        match &self.content {
            DocumentContent::File(path) => Some(path),
            DocumentContent::Memory(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffComparison {
    pub baseline: ComparisonDocument,
    pub local: ComparisonDocument,
}

/// File-backed merge inputs, as named on the command line or in a file chooser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MergePaths {
    pub base: PathBuf,
    pub local: PathBuf,
    pub incoming: PathBuf,
    pub result: PathBuf,
}

/// Read-only merge inputs and the result destination. Inputs may be files or
/// memory, such as the stages of a Git index conflict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MergeComparison {
    pub base: ComparisonDocument,
    pub local: ComparisonDocument,
    pub incoming: ComparisonDocument,
    pub result: PathBuf,
}

impl MergeComparison {
    pub fn inputs(&self) -> [&ComparisonDocument; 3] {
        [&self.base, &self.local, &self.incoming]
    }
}

impl From<MergePaths> for MergeComparison {
    fn from(paths: MergePaths) -> Self {
        Self {
            base: ComparisonDocument::read_only_file(paths.base),
            local: ComparisonDocument::read_only_file(paths.local),
            incoming: ComparisonDocument::read_only_file(paths.incoming),
            result: paths.result,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Comparison {
    Diff(DiffComparison),
    Merge(Box<MergeComparison>),
}

impl From<MergePaths> for Comparison {
    fn from(paths: MergePaths) -> Self {
        Self::Merge(Box::new(paths.into()))
    }
}

impl Comparison {
    pub fn diff(baseline: PathBuf, local: PathBuf) -> Self {
        Self::two_way(
            ComparisonDocument::read_only_file(baseline),
            ComparisonDocument::editable_file(local),
        )
    }

    pub fn two_way(baseline: ComparisonDocument, local: ComparisonDocument) -> Self {
        Self::Diff(DiffComparison { baseline, local })
    }

    pub fn from_paths(paths: &[PathBuf]) -> Result<Self, String> {
        match paths {
            [baseline, local] => Ok(Self::diff(baseline.clone(), local.clone())),
            [base, local, incoming, result] => Ok(MergePaths {
                base: base.clone(),
                local: local.clone(),
                incoming: incoming.clone(),
                result: result.clone(),
            }
            .into()),
            _ => Err("expected two diff paths or four merge paths".into()),
        }
    }

    pub fn wire_paths(&self) -> Result<Vec<&Path>, String> {
        match self {
            Self::Diff(diff) => {
                let baseline = diff
                    .baseline
                    .file_path()
                    .ok_or("in-memory comparisons cannot be forwarded to another yori instance")?;
                let local = diff
                    .local
                    .file_path()
                    .ok_or("in-memory comparisons cannot be forwarded to another yori instance")?;

                let representable = !diff.baseline.editable()
                    && diff.baseline.save_destination().is_none()
                    && diff.local.editable()
                    && diff.local.save_destination() == Some(local);
                if !representable {
                    return Err(
                        "comparison document capabilities cannot be represented by the current instance protocol"
                            .into(),
                    );
                }

                Ok(vec![baseline, local])
            }
            Self::Merge(merge) => {
                let mut paths = Vec::with_capacity(4);
                for input in merge.inputs() {
                    let path = input
                        .file_path()
                        .ok_or("in-memory merges cannot be forwarded to another yori instance")?;
                    if input.editable() || input.save_destination().is_some() {
                        return Err(
                            "merge input capabilities cannot be represented by the current instance protocol"
                                .into(),
                        );
                    }

                    paths.push(path);
                }
                paths.push(&merge.result);

                Ok(paths)
            }
        }
    }

    pub fn resolve(&self) -> Result<Self, String> {
        match self {
            Self::Diff(diff) => {
                if diff.baseline.editable() {
                    return Err("baseline documents must be read-only".into());
                }

                Ok(Self::two_way(
                    diff.baseline.resolve()?,
                    diff.local.resolve()?,
                ))
            }
            Self::Merge(merge) => {
                if merge.inputs().iter().any(|input| input.editable()) {
                    return Err("merge inputs must be read-only".into());
                }

                Ok(Self::Merge(Box::new(MergeComparison {
                    base: merge.base.resolve()?,
                    local: merge.local.resolve()?,
                    incoming: merge.incoming.resolve()?,
                    result: resolve_result(&merge.result)?,
                })))
            }
        }
    }

    pub fn target(&self) -> &Path {
        match self {
            Self::Diff(diff) => diff.local.logical_path(),
            Self::Merge(merge) => &merge.result,
        }
    }

    pub fn description(&self) -> String {
        match self {
            Self::Diff(diff) => {
                format!(
                    "Baseline: {}\nLocal: {}",
                    diff.baseline.logical_path().display(),
                    diff.local.logical_path().display()
                )
            }
            Self::Merge(merge) => format!(
                "Base: {}\nLocal: {}\nIncoming: {}\nResult: {}",
                merge.base.logical_path().display(),
                merge.local.logical_path().display(),
                merge.incoming.logical_path().display(),
                merge.result.display(),
            ),
        }
    }

    pub fn qualifier(&self) -> String {
        match self {
            Self::Diff(diff) => diff.baseline.logical_path().display().to_string(),
            Self::Merge(merge) => format!(
                "merge {} + {} (base {})",
                merge.local.logical_path().display(),
                merge.incoming.logical_path().display(),
                merge.base.logical_path().display(),
            ),
        }
    }
}

fn resolve_input(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|error| format!("cannot open {}: {error}", path.display()))
}

fn resolve_result(path: &Path) -> Result<PathBuf, String> {
    // A result is a destination, not a fourth input. It may not exist yet and
    // may alias an input (common in mergetool integrations). Never create it here.
    match path.canonicalize() {
        Ok(path) if path.is_file() => Ok(path),
        Ok(path) => Err(format!("result is not a file: {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
            // Don't silently retarget a dangling symlink to a different file.
            if absolute.symlink_metadata().is_ok() {
                return Err(format!(
                    "result path has a dangling symlink: {}",
                    path.display()
                ));
            }

            let parent = absolute.parent().ok_or("result needs a parent directory")?;
            let parent = parent.canonicalize().map_err(|error| {
                format!("cannot open result directory {}: {error}", parent.display())
            })?;
            if !parent.is_dir() {
                return Err("result parent is not a directory".into());
            }

            Ok(parent.join(absolute.file_name().ok_or("result needs a filename")?))
        }
        Err(error) => Err(format!("cannot resolve result {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_documents_resolve_without_touching_their_logical_paths() {
        let directory = tempfile::tempdir().unwrap();
        let comparison = Comparison::two_way(
            ComparisonDocument::read_only_memory(
                directory.path().join("src/original.rs"),
                b"old\n".to_vec(),
            ),
            ComparisonDocument::read_only_memory(
                directory.path().join("src/current.rs"),
                b"new\n".to_vec(),
            ),
        );

        let resolved = comparison.resolve().unwrap();

        assert_eq!(resolved, comparison);
        assert!(directory.path().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn result_identity_supports_new_paths_and_input_aliases_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("local.rs");
        std::fs::write(&input, "keep me").unwrap();
        let output = directory.path().join("result.rs");
        let resolved_output = directory.path().canonicalize().unwrap().join("result.rs");

        assert_eq!(resolve_result(&output).unwrap(), resolved_output);
        assert!(!output.exists());
        assert_eq!(
            resolve_result(&input).unwrap(),
            input.canonicalize().unwrap()
        );
        assert_eq!(std::fs::read_to_string(&input).unwrap(), "keep me");
        assert!(resolve_result(directory.path()).is_err());

        #[cfg(unix)]
        {
            let alias = directory.path().join("alias.rs");
            std::os::unix::fs::symlink(&input, &alias).unwrap();
            assert_eq!(
                resolve_result(&alias).unwrap(),
                input.canonicalize().unwrap()
            );
            std::fs::remove_file(&input).unwrap();
            assert!(resolve_result(&alias).is_err());
        }
    }
}
