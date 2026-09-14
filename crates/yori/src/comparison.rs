//! File roles shared by CLI handoff, workspace identity, and editor construction.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MergePaths {
    pub base: PathBuf,
    pub local: PathBuf,
    pub incoming: PathBuf,
    pub result: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ComparisonPaths {
    Diff { baseline: PathBuf, local: PathBuf },
    Merge(MergePaths),
}

impl ComparisonPaths {
    pub fn diff(baseline: PathBuf, local: PathBuf) -> Self {
        Self::Diff { baseline, local }
    }

    pub fn from_paths(paths: &[PathBuf]) -> Result<Self, String> {
        match paths {
            [baseline, local] => Ok(Self::diff(baseline.clone(), local.clone())),
            [base, local, incoming, result] => Ok(Self::Merge(MergePaths {
                base: base.clone(),
                local: local.clone(),
                incoming: incoming.clone(),
                result: result.clone(),
            })),
            _ => Err("expected two diff paths or four merge paths".into()),
        }
    }

    pub fn paths(&self) -> Vec<&Path> {
        match self {
            Self::Diff { baseline, local } => vec![baseline, local],
            Self::Merge(paths) => vec![&paths.base, &paths.local, &paths.incoming, &paths.result],
        }
    }

    pub fn resolve(&self) -> Result<Self, String> {
        let input = |path: &Path| {
            path.canonicalize()
                .map_err(|error| format!("cannot open {}: {error}", path.display()))
        };

        match self {
            Self::Diff { baseline, local } => Ok(Self::diff(input(baseline)?, input(local)?)),
            Self::Merge(paths) => Ok(Self::Merge(MergePaths {
                base: input(&paths.base)?,
                local: input(&paths.local)?,
                incoming: input(&paths.incoming)?,
                result: resolve_result(&paths.result)?,
            })),
        }
    }

    pub fn target(&self) -> &Path {
        match self {
            Self::Diff { local, .. } => local,
            Self::Merge(paths) => &paths.result,
        }
    }

    pub fn description(&self) -> String {
        match self {
            Self::Diff { baseline, local } => {
                format!(
                    "Baseline: {}\nLocal: {}",
                    baseline.display(),
                    local.display()
                )
            }
            Self::Merge(paths) => format!(
                "Base: {}\nLocal: {}\nIncoming: {}\nResult: {}",
                paths.base.display(),
                paths.local.display(),
                paths.incoming.display(),
                paths.result.display(),
            ),
        }
    }

    pub fn qualifier(&self) -> String {
        match self {
            Self::Diff { baseline, .. } => baseline.display().to_string(),
            Self::Merge(paths) => format!(
                "merge {} + {} (base {})",
                paths.local.display(),
                paths.incoming.display(),
                paths.base.display(),
            ),
        }
    }
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
