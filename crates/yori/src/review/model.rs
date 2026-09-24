//! Provider-neutral review source and manifest vocabulary.

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use yori_diff::{Alignment, DiffKind};

use crate::comparison::{Comparison, ComparisonDocument, MergeComparison};
use crate::workspace::files::{Files, Role};

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
    /// The full title, used where a review needs to be named on its own, such
    /// as a workspace tab.
    pub label: Arc<str>,
    /// What sort of change this is — "Commit", "Working changes", "Pending".
    /// Shown as context beside the review's summary, never on its own.
    pub kind: &'static str,
    /// The part of the title that distinguishes this review from its siblings,
    /// with any `kind` stripped off. This is what the navigator header leads
    /// with, so it must survive truncation.
    pub headline: Arc<str>,
    pub provider: Arc<dyn ReviewProvider>,
}

impl ReviewSource {
    pub(crate) fn new(
        identity: ReviewSourceIdentity,
        label: impl Into<Arc<str>>,
        kind: &'static str,
        headline: impl Into<Arc<str>>,
        provider: Arc<dyn ReviewProvider>,
    ) -> Self {
        Self {
            identity,
            label: label.into(),
            kind,
            headline: headline.into(),
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
    Renamed {
        from: PathBuf,
    },
    /// The source-control state records an unresolved merge conflict.
    Conflicted,
}

impl ReviewFileStatus {
    pub(crate) fn badge(&self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed { .. } => "R",
            Self::Conflicted => "U",
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Added => "Added",
            Self::Modified => "Modified",
            Self::Deleted => "Deleted",
            Self::Renamed { .. } => "Renamed",
            Self::Conflicted => "Conflicted",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReviewConflict {
    Mergeable(Box<MergeComparison>),
    Unmergeable { reason: Arc<str> },
}

impl ReviewConflict {
    pub(crate) fn unmergeable(reason: impl Into<Arc<str>>) -> Self {
        Self::Unmergeable {
            reason: reason.into(),
        }
    }

    pub(crate) fn merge(&self) -> Option<Box<MergeComparison>> {
        match self {
            Self::Mergeable(merge) => Some(merge.clone()),
            Self::Unmergeable { .. } => None,
        }
    }
}

/// Added and removed line counts for one reviewed file. Measuring is best
/// effort, so an unmeasured or unmeasurable file simply carries no stat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DiffStat {
    pub added: usize,
    pub removed: usize,
}

impl DiffStat {
    pub(crate) fn total(self) -> usize {
        self.added + self.removed
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
    /// Filled in after the manifest loads; see `ReviewManifest::measure`.
    pub stat: Option<DiffStat>,
    /// Present only while source control records this file as conflicted.
    pub conflict: Option<ReviewConflict>,
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
            stat: None,
            conflict: None,
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
            stat: None,
            conflict: None,
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
            stat: None,
            conflict: None,
        }
    }

    pub(crate) fn with_conflict(self, conflict: ReviewConflict) -> Self {
        Self {
            status: ReviewFileStatus::Conflicted,
            conflict: Some(conflict),
            ..self
        }
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
    /// Measures added and removed line counts for every text file. This resolves
    /// and diffs each file's content, so callers run it off the main thread. A
    /// file that cannot be measured keeps its empty stat rather than failing the
    /// manifest — the counts are decoration, not correctness.
    /// Orders files by path so that the navigator can group them by directory.
    /// Providers are free to build a manifest in whatever order suits them.
    pub(crate) fn sort(&mut self) {
        self.files
            .sort_by(|left, right| left.logical_path.cmp(&right.logical_path));
    }

    pub(crate) fn measure(&mut self) {
        for file in &mut self.files {
            if let ReviewFileKind::Text(comparison) = &file.kind {
                file.stat = measure_text(comparison);
            }
        }
    }

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

/// Counts the changed lines on each side of a two-way text comparison. Every
/// changed row contributes to the side it actually occupies, so a modified row
/// counts once as removed and once as added, matching `git diff --numstat`.
fn measure_text(comparison: &TextComparison) -> Option<DiffStat> {
    let resolved = comparison.comparison().resolve().ok()?;
    let files = Files::load(&resolved).ok()?;
    let alignment = Alignment::between(files.document(Role::Baseline), files.document(Role::Local));

    let mut stat = DiffStat::default();
    for row in alignment.rows() {
        if row.kind == DiffKind::Equal {
            continue;
        }

        if row.left.is_some() {
            stat.removed += 1;
        }
        if row.right.is_some() {
            stat.added += 1;
        }
    }

    Some(stat)
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
    fn measuring_counts_changed_lines_on_the_side_they_occupy() {
        let path = PathBuf::from("src/lib.rs");
        let comparison = TextComparison::new(
            ComparisonDocument::read_only_memory(path.clone(), b"a\nb\nc\n".to_vec()),
            ComparisonDocument::read_only_memory(path.clone(), b"a\nB\nc\nd\n".to_vec()),
        )
        .unwrap();
        let binary = ReviewFile::binary(
            ReviewFileIdentity::new("logo"),
            "logo.png".into(),
            ReviewFileStatus::Modified,
            "Binary content cannot be displayed.",
        );
        let mut manifest = ReviewManifest::new(vec![
            ReviewFile::text(
                ReviewFileIdentity::new("lib"),
                path,
                ReviewFileStatus::Modified,
                comparison,
            ),
            binary,
        ])
        .unwrap();

        assert!(manifest.files.iter().all(|file| file.stat.is_none()));
        manifest.measure();

        // The rewritten line counts on both sides; the appended line only adds.
        assert_eq!(
            manifest.files[0].stat,
            Some(DiffStat {
                added: 2,
                removed: 1
            })
        );
        assert_eq!(manifest.files[0].stat.unwrap().total(), 3);
        // Binary files have no lines to count, so they stay unmeasured.
        assert_eq!(manifest.files[1].stat, None);
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
}
