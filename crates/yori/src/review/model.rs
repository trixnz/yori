//! Provider-neutral review source and manifest vocabulary.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "provider-facing constructors are ready before Git or Perforce providers"
    )
)]

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::comparison::{Comparison, ComparisonDocument};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ReviewSourceIdentity {
    provider: Arc<str>,
    key: Arc<str>,
}

impl ReviewSourceIdentity {
    pub(crate) fn new(provider: impl Into<Arc<str>>, key: impl Into<Arc<str>>) -> Self {
        Self {
            provider: provider.into(),
            key: key.into(),
        }
    }

    pub(crate) fn description(&self) -> String {
        format!("{}: {}", self.provider, self.key)
    }
}

#[derive(Clone)]
pub(crate) struct ReviewSource {
    pub identity: ReviewSourceIdentity,
    pub label: Arc<str>,
    pub provider: Arc<dyn ReviewProvider>,
}

impl ReviewSource {
    pub(crate) fn new(
        identity: ReviewSourceIdentity,
        label: impl Into<Arc<str>>,
        provider: Arc<dyn ReviewProvider>,
    ) -> Self {
        Self {
            identity,
            label: label.into(),
            provider,
        }
    }
}

pub(crate) trait ReviewProvider: Send + Sync + 'static {
    fn load_manifest(&self, source: &ReviewSourceIdentity) -> Result<ReviewManifest, String>;
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ReviewFileIdentity(Arc<str>);

impl ReviewFileIdentity {
    pub(crate) fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for ReviewFileIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReviewFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed { from: PathBuf },
}

impl ReviewFileStatus {
    pub(crate) fn badge(&self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed { .. } => "R",
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Added => "Added",
            Self::Modified => "Modified",
            Self::Deleted => "Deleted",
            Self::Renamed { .. } => "Renamed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReviewFileCapabilities {
    pub editable: bool,
    pub saveable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextComparison {
    comparison: Comparison,
    capabilities: ReviewFileCapabilities,
}

impl TextComparison {
    pub(crate) fn new(
        baseline: ComparisonDocument,
        local: ComparisonDocument,
    ) -> Result<Self, String> {
        if baseline.editable() {
            return Err("review baselines must be read-only".into());
        }

        let capabilities = ReviewFileCapabilities {
            editable: local.editable(),
            saveable: local.save_destination().is_some(),
        };

        Ok(Self {
            comparison: Comparison::two_way(baseline, local),
            capabilities,
        })
    }

    pub(crate) fn added(path: PathBuf, local: ComparisonDocument) -> Result<Self, String> {
        Self::new(
            ComparisonDocument::read_only_memory(path, Vec::new()),
            local,
        )
    }

    pub(crate) fn deleted(path: PathBuf, baseline: ComparisonDocument) -> Result<Self, String> {
        Self::new(
            baseline,
            ComparisonDocument::read_only_memory(path, Vec::new()),
        )
    }

    pub(crate) fn comparison(&self) -> &Comparison {
        &self.comparison
    }

    pub(crate) fn capabilities(&self) -> ReviewFileCapabilities {
        self.capabilities
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReviewFileKind {
    Text(TextComparison),
    Binary {
        explanation: Arc<str>,
    },
    Submodule {
        old_identifier: Arc<str>,
        new_identifier: Arc<str>,
    },
}

impl ReviewFileKind {
    pub(crate) fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReviewFile {
    pub identity: ReviewFileIdentity,
    pub logical_path: PathBuf,
    pub status: ReviewFileStatus,
    pub kind: ReviewFileKind,
}

impl ReviewFile {
    pub(crate) fn text(
        identity: ReviewFileIdentity,
        logical_path: PathBuf,
        status: ReviewFileStatus,
        comparison: TextComparison,
    ) -> Self {
        Self {
            identity,
            logical_path,
            status,
            kind: ReviewFileKind::Text(comparison),
        }
    }

    pub(crate) fn binary(
        identity: ReviewFileIdentity,
        logical_path: PathBuf,
        status: ReviewFileStatus,
        explanation: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            identity,
            logical_path,
            status,
            kind: ReviewFileKind::Binary {
                explanation: explanation.into(),
            },
        }
    }

    pub(crate) fn submodule(
        identity: ReviewFileIdentity,
        logical_path: PathBuf,
        status: ReviewFileStatus,
        old_identifier: impl Into<Arc<str>>,
        new_identifier: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            identity,
            logical_path,
            status,
            kind: ReviewFileKind::Submodule {
                old_identifier: old_identifier.into(),
                new_identifier: new_identifier.into(),
            },
        }
    }

    pub(crate) fn matches_query(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }

        self.logical_path
            .to_string_lossy()
            .to_lowercase()
            .contains(&query)
            || self.status.label().to_lowercase().contains(&query)
            || matches!(
                &self.status,
                ReviewFileStatus::Renamed { from }
                    if from.to_string_lossy().to_lowercase().contains(&query)
            )
    }

    pub(crate) fn path(&self) -> &Path {
        &self.logical_path
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ReviewManifest {
    pub files: Vec<ReviewFile>,
}

impl ReviewManifest {
    pub(crate) fn new(files: Vec<ReviewFile>) -> Result<Self, String> {
        let mut identities = std::collections::HashSet::new();
        for file in &files {
            if !identities.insert(file.identity.clone()) {
                return Err(format!(
                    "review manifest contains duplicate file identity {}",
                    file.identity
                ));
            }
        }

        Ok(Self { files })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comparison::DocumentContent;

    #[test]
    fn added_and_deleted_text_entries_compare_against_empty_content() {
        let added_path = PathBuf::from("src/added.rs");
        let added = TextComparison::added(
            added_path.clone(),
            ComparisonDocument::editable_memory(added_path, b"new\n".to_vec(), None),
        )
        .unwrap();
        assert_eq!(
            added.capabilities(),
            ReviewFileCapabilities {
                editable: true,
                saveable: false,
            }
        );
        let Comparison::Diff(added) = added.comparison() else {
            unreachable!();
        };
        assert!(matches!(
            added.baseline.content(),
            DocumentContent::Memory(bytes) if bytes.is_empty()
        ));

        let deleted_path = PathBuf::from("src/deleted.rs");
        let deleted = TextComparison::deleted(
            deleted_path.clone(),
            ComparisonDocument::read_only_memory(deleted_path.clone(), b"old\n".to_vec()),
        )
        .unwrap();
        let deleted_entry = ReviewFile::text(
            ReviewFileIdentity::new("deleted"),
            deleted_path,
            ReviewFileStatus::Deleted,
            deleted.clone(),
        );
        assert_eq!(deleted_entry.status.badge(), "D");
        let Comparison::Diff(deleted) = deleted.comparison() else {
            unreachable!();
        };
        assert!(matches!(
            deleted.local.content(),
            DocumentContent::Memory(bytes) if bytes.is_empty()
        ));
    }

    #[test]
    fn manifest_rejects_duplicate_provider_file_identity() {
        let identity = ReviewFileIdentity::new("same");
        let first = ReviewFile::binary(
            identity.clone(),
            "a.bin".into(),
            ReviewFileStatus::Modified,
            "Binary content cannot be displayed.",
        );
        let second = ReviewFile::submodule(
            identity,
            "vendor/lib".into(),
            ReviewFileStatus::Modified,
            "old",
            "new",
        );

        assert!(ReviewManifest::new(vec![first, second]).is_err());
    }

    #[test]
    fn renamed_entry_searches_both_paths() {
        let file = ReviewFile::binary(
            ReviewFileIdentity::new("rename"),
            "src/new-name.bin".into(),
            ReviewFileStatus::Renamed {
                from: "src/old-name.bin".into(),
            },
            "Binary content cannot be displayed.",
        );

        assert!(file.matches_query("new-name"));
        assert!(file.matches_query("old-name"));
        assert!(file.matches_query("renamed"));
    }
}
