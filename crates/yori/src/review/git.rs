//! Git repository discovery and native `gix` review sources.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use gix::{
    bstr::{BStr, ByteSlice},
    diff::tree_with_rewrites::Change,
    objs::tree::EntryMode,
    status::{Item as StatusItem, UntrackedFiles, index_worktree, tree_index},
};

use crate::comparison::ComparisonDocument;

use super::model::{
    ReviewFile, ReviewFileIdentity, ReviewFileStatus, ReviewManifest, ReviewProvider, ReviewSource,
    ReviewSourceIdentity, TextComparison,
};

const RECENT_COMMIT_LIMIT: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitCommitSummary {
    pub revision: String,
    pub short_id: String,
    pub title: String,
    pub is_merge: bool,
    /// Commit time in seconds since the epoch, for rendering a relative age.
    pub time_seconds: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct GitRepository {
    work_dir: PathBuf,
    identity: PathBuf,
}

impl GitRepository {
    pub(crate) fn discover(context: &Path) -> Result<Self, String> {
        let repository = gix::discover(context).map_err(|error| {
            format!(
                "cannot find a Git repository from {}: {error}",
                context.display()
            )
        })?;
        let work_dir = repository
            .workdir()
            .ok_or("Git reviews require a repository with a working tree")?
            .canonicalize()
            .map_err(|error| format!("cannot resolve the Git working tree: {error}"))?;
        let identity = repository
            .common_dir()
            .canonicalize()
            .map_err(|error| format!("cannot resolve the Git repository: {error}"))?;

        Ok(Self { work_dir, identity })
    }

    pub(crate) fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    pub(crate) fn working_source(&self) -> ReviewSource {
        let key = format!("{}:working", self.work_dir.display());
        let repository = repository_name(&self.work_dir);
        ReviewSource::new(
            ReviewSourceIdentity::new("git", key),
            format!("Working changes — {repository}"),
            "Working changes",
            repository,
            Arc::new(GitProvider {
                work_dir: self.work_dir.clone(),
                source: GitSource::Working,
            }),
        )
    }

    pub(crate) fn commit_source(&self, revision: &str) -> Result<ReviewSource, String> {
        let repository = self.open()?;
        let commit = resolve_commit(&repository, revision)?;
        let id = commit.id().detach();
        let title = commit_title(&commit);
        let short_id = commit
            .short_id()
            .map_or_else(|_| id.to_string()[..7].to_owned(), |id| id.to_string());
        let key = format!("{}:commit:{id}", self.identity.display());

        let headline = format!("{short_id} {title}");
        Ok(ReviewSource::new(
            ReviewSourceIdentity::new("git", key),
            headline.clone(),
            "Commit",
            headline,
            Arc::new(GitProvider {
                work_dir: self.work_dir.clone(),
                source: GitSource::Commit(id),
            }),
        ))
    }

    pub(crate) fn recent_commits(&self) -> Result<Vec<GitCommitSummary>, String> {
        let repository = self.open()?;
        let mut head = repository
            .head()
            .map_err(|error| format!("cannot read Git HEAD: {error}"))?;
        if head.is_unborn() {
            return Ok(Vec::new());
        }
        let head = head
            .peel_to_commit()
            .map_err(|error| format!("cannot resolve Git HEAD to a commit: {error}"))?;
        let walk = head
            .ancestors()
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::default(),
            ))
            .all()
            .map_err(|error| format!("cannot walk commits reachable from HEAD: {error}"))?;

        walk.take(RECENT_COMMIT_LIMIT)
            .map(|info| {
                let info = info.map_err(|error| format!("cannot read recent commit: {error}"))?;
                let commit = info
                    .object()
                    .map_err(|error| format!("cannot read commit {}: {error}", info.id))?;
                let id = commit.id().detach();
                let short_id = commit
                    .short_id()
                    .map_or_else(|_| id.to_string()[..7].to_owned(), |id| id.to_string());
                let is_merge = commit.parent_ids().nth(1).is_some();

                let time_seconds = commit.time().map_or(0, |time| time.seconds);

                Ok(GitCommitSummary {
                    revision: id.to_string(),
                    short_id,
                    title: commit_title(&commit),
                    is_merge,
                    time_seconds,
                })
            })
            .collect()
    }

    fn open(&self) -> Result<gix::Repository, String> {
        gix::open(&self.work_dir).map_err(|error| {
            format!(
                "cannot open Git repository at {}: {error}",
                self.work_dir.display()
            )
        })
    }
}

fn repository_name(work_dir: &Path) -> String {
    work_dir
        .file_name()
        .unwrap_or(work_dir.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn commit_title(commit: &gix::Commit<'_>) -> String {
    commit.message_raw_sloppy().lines().next().map_or_else(
        || "(no commit message)".to_owned(),
        |line| String::from_utf8_lossy(line).into_owned(),
    )
}

fn resolve_commit<'repo>(
    repository: &'repo gix::Repository,
    revision: &str,
) -> Result<gix::Commit<'repo>, String> {
    let revision = revision.trim();
    if revision.is_empty() {
        return Err("enter a Git revision".into());
    }

    repository
        .rev_parse_single(revision)
        .map_err(|error| format!("cannot resolve Git revision {revision:?}: {error}"))?
        .object()
        .map_err(|error| format!("cannot read Git revision {revision:?}: {error}"))?
        .peel_to_commit()
        .map_err(|error| format!("Git revision {revision:?} is not a commit: {error}"))
}

#[derive(Clone, Copy, Debug)]
enum GitSource {
    Working,
    Commit(gix::ObjectId),
}

struct GitProvider {
    work_dir: PathBuf,
    source: GitSource,
}

impl ReviewProvider for GitProvider {
    fn load_manifest(&self, _: &ReviewSourceIdentity) -> Result<ReviewManifest, String> {
        let repository = gix::open(&self.work_dir).map_err(|error| {
            format!(
                "cannot open Git repository at {}: {error}",
                self.work_dir.display()
            )
        })?;

        match self.source {
            GitSource::Working => working_manifest(&repository),
            GitSource::Commit(id) => commit_manifest(&repository, id),
        }
    }
}

#[derive(Clone, Debug)]
enum Snapshot {
    Blob {
        bytes: Vec<u8>,
        mode: EntryMode,
        physical_symlink: bool,
    },
    Submodule {
        id: String,
    },
}

impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Blob {
                    bytes: left_bytes,
                    mode: left_mode,
                    ..
                },
                Self::Blob {
                    bytes: right_bytes,
                    mode: right_mode,
                    ..
                },
            ) => left_bytes == right_bytes && left_mode == right_mode,
            (Self::Submodule { id: left }, Self::Submodule { id: right }) => left == right,
            (Self::Blob { .. }, Self::Submodule { .. })
            | (Self::Submodule { .. }, Self::Blob { .. }) => false,
        }
    }
}

impl Eq for Snapshot {}

impl Snapshot {
    fn is_binary(&self) -> bool {
        matches!(self, Self::Blob { bytes, .. } if bytes.iter().take(8_000).any(|byte| *byte == 0))
    }

    fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Blob { bytes, .. } => Some(bytes),
            Self::Submodule { .. } => None,
        }
    }

    fn is_physical_symlink(&self) -> bool {
        matches!(
            self,
            Self::Blob {
                physical_symlink: true,
                ..
            }
        )
    }

    fn submodule_id(&self) -> Option<&str> {
        match self {
            Self::Submodule { id } => Some(id),
            Self::Blob { .. } => None,
        }
    }

    fn submodule_display(snapshot: Option<&Self>) -> &str {
        match snapshot {
            Some(Self::Submodule { id }) => id,
            Some(Self::Blob { .. }) => "(file)",
            None => "(none)",
        }
    }
}

#[derive(Debug)]
struct WorkingChange {
    path: PathBuf,
    old: Option<Snapshot>,
    new: Option<Snapshot>,
}

fn working_candidates(
    repository: &gix::Repository,
) -> Result<(BTreeSet<PathBuf>, HashMap<PathBuf, PathBuf>), String> {
    let mut candidates = BTreeSet::new();
    let mut rename_hints = HashMap::new();
    let status = repository
        .status(gix::progress::Discard)
        .map_err(|error| format!("cannot prepare Git status: {error}"))?
        .untracked_files(UntrackedFiles::Files)
        .tree_index_track_renames(tree_index::TrackRenames::Given(
            gix::diff::Rewrites::default(),
        ))
        .index_worktree_rewrites(Some(gix::diff::Rewrites::default()))
        .into_iter(Vec::<gix::bstr::BString>::new())
        .map_err(|error| format!("cannot inspect Git working changes: {error}"))?;

    for item in status {
        let item = item.map_err(|error| format!("cannot inspect Git working changes: {error}"))?;
        let path = git_path(item.location())?;
        candidates.insert(path);

        let rename = match item {
            StatusItem::TreeIndex(gix::diff::index::Change::Rewrite {
                source_location,
                location,
                copy: false,
                ..
            }) => Some((
                git_path(source_location.as_ref())?,
                git_path(location.as_ref())?,
            )),
            StatusItem::IndexWorktree(index_worktree::Item::Rewrite {
                source,
                dirwalk_entry,
                copy: false,
                ..
            }) => Some((
                git_path(source.rela_path())?,
                git_path(dirwalk_entry.rela_path.as_ref())?,
            )),
            StatusItem::IndexWorktree(_) | StatusItem::TreeIndex(_) => None,
        };
        if let Some((source, destination)) = rename {
            candidates.insert(source.clone());
            candidates.insert(destination.clone());
            rename_hints.insert(destination, source);
        }
    }

    Ok((candidates, rename_hints))
}

fn working_manifest(repository: &gix::Repository) -> Result<ReviewManifest, String> {
    let work_dir = repository
        .workdir()
        .ok_or("Git reviews require a repository with a working tree")?;
    let baseline_tree_id = repository
        .head_tree_id_or_empty()
        .map_err(|error| format!("cannot resolve the HEAD tree: {error}"))?;
    let baseline_tree = baseline_tree_id
        .object()
        .map(gix::Object::into_tree)
        .map_err(|error| format!("cannot read the HEAD tree: {error}"))?;
    let index = repository
        .index_or_empty()
        .map_err(|error| format!("cannot read the Git index: {error}"))?;

    let (candidates, rename_hints) = working_candidates(repository)?;
    let mut changes = BTreeMap::new();
    for path in candidates {
        let old = tree_snapshot(repository, &baseline_tree, &path)?;
        let new = worktree_snapshot(repository, work_dir, &index, &path, old.as_ref())?;

        if old == new && !matches!(old, Some(Snapshot::Submodule { .. })) {
            continue;
        }

        changes.insert(path.clone(), WorkingChange { path, old, new });
    }

    let rename_pairs = reconcile_renames(&changes, rename_hints);
    let mut consumed = BTreeSet::new();
    let mut files = Vec::new();
    for (destination, source) in rename_pairs {
        let Some(old) = changes.get(&source).and_then(|change| change.old.clone()) else {
            continue;
        };
        let Some(new) = changes
            .get(&destination)
            .and_then(|change| change.new.clone())
        else {
            continue;
        };

        consumed.insert(source.clone());
        consumed.insert(destination.clone());
        files.push(working_file(
            work_dir,
            destination,
            ReviewFileStatus::Renamed { from: source },
            Some(&old),
            Some(&new),
        )?);
    }

    for (path, change) in changes {
        if consumed.contains(&path) {
            continue;
        }

        let status = match (&change.old, &change.new) {
            (None, Some(_)) => ReviewFileStatus::Added,
            (Some(_), None) => ReviewFileStatus::Deleted,
            (Some(_), Some(_)) => ReviewFileStatus::Modified,
            (None, None) => continue,
        };
        files.push(working_file(
            work_dir,
            change.path,
            status,
            change.old.as_ref(),
            change.new.as_ref(),
        )?);
    }

    files.sort_by(|left, right| left.logical_path.cmp(&right.logical_path));
    ReviewManifest::new(files)
}

fn reconcile_renames(
    changes: &BTreeMap<PathBuf, WorkingChange>,
    mut hints: HashMap<PathBuf, PathBuf>,
) -> BTreeMap<PathBuf, PathBuf> {
    let deletions = changes
        .iter()
        .filter(|(_, change)| change.old.is_some() && change.new.is_none())
        .map(|(path, change)| (path, change.old.as_ref().expect("filtered")))
        .collect::<Vec<_>>();
    let additions = changes
        .iter()
        .filter(|(_, change)| change.old.is_none() && change.new.is_some())
        .map(|(path, change)| (path, change.new.as_ref().expect("filtered")))
        .collect::<Vec<_>>();

    for (destination, added) in additions {
        if hints.contains_key(destination) {
            continue;
        }

        if let Some((source, _)) = deletions.iter().find(|(source, removed)| {
            !hints.values().any(|hint| hint == *source) && *removed == added
        }) {
            hints.insert(destination.clone(), (*source).clone());
        }
    }

    hints
        .into_iter()
        .filter(|(destination, source)| {
            changes
                .get(destination)
                .is_some_and(|change| change.new.is_some())
                && changes
                    .get(source)
                    .is_some_and(|change| change.old.is_some() && change.new.is_none())
        })
        .collect()
}

fn working_file(
    work_dir: &Path,
    path: PathBuf,
    status: ReviewFileStatus,
    old: Option<&Snapshot>,
    new: Option<&Snapshot>,
) -> Result<ReviewFile, String> {
    let identity = ReviewFileIdentity::new(format!("working:{}", path.display()));
    let destination = work_dir.join(&path);

    if old.and_then(Snapshot::submodule_id).is_some()
        || new.and_then(Snapshot::submodule_id).is_some()
    {
        return Ok(ReviewFile::submodule(
            identity,
            path,
            status,
            Snapshot::submodule_display(old),
            Snapshot::submodule_display(new),
        ));
    }

    if old.is_some_and(Snapshot::is_binary) || new.is_some_and(Snapshot::is_binary) {
        return Ok(ReviewFile::binary(
            identity,
            path,
            status,
            "Binary content cannot be displayed. The working file remains unchanged.",
        ));
    }

    let baseline = old.and_then(Snapshot::bytes).unwrap_or_default().to_vec();
    let local = new.and_then(Snapshot::bytes).unwrap_or_default().to_vec();
    let local = if new.is_some_and(Snapshot::is_physical_symlink) {
        ComparisonDocument::read_only_memory(path.clone(), local)
    } else {
        ComparisonDocument::editable_memory(path.clone(), local, Some(destination))
    };
    let comparison = TextComparison::new(
        ComparisonDocument::read_only_memory(path.clone(), baseline),
        local,
    )?;

    Ok(ReviewFile::text(identity, path, status, comparison))
}

fn worktree_snapshot(
    repository: &gix::Repository,
    work_dir: &Path,
    index: &gix::worktree::Index,
    path: &Path,
    baseline: Option<&Snapshot>,
) -> Result<Option<Snapshot>, String> {
    let destination = work_dir.join(path);
    let metadata = match gix::index::fs::Metadata::from_path_no_follow(&destination) {
        Ok(metadata) => metadata,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(error) => {
            return Err(format!("cannot inspect {}: {error}", destination.display()));
        }
    };
    let git_path = gix::path::into_bstr(path);
    let index_entry = index.entry_by_path(git_path.as_ref());
    let is_submodule = baseline
        .is_some_and(|snapshot| matches!(snapshot, Snapshot::Submodule { .. }))
        || index_entry.is_some_and(|entry| entry.mode.is_submodule());

    if is_submodule && metadata.is_dir() {
        let id = gix::open(&destination)
            .ok()
            .and_then(|nested| nested.head_id().ok().map(gix::Id::detach))
            .map_or_else(
                || index_entry.map_or_else(|| "(unborn)".to_owned(), |entry| entry.id.to_string()),
                |id| id.to_string(),
            );

        return Ok(Some(Snapshot::Submodule { id }));
    }

    let mode = worktree_mode(repository, index_entry, &metadata)?;
    if metadata.is_symlink() {
        let target = fs::read_link(&destination).map_err(|error| {
            format!(
                "cannot read symbolic link {}: {error}",
                destination.display()
            )
        })?;

        return Ok(Some(Snapshot::Blob {
            bytes: target.as_os_str().as_encoded_bytes().to_vec(),
            mode,
            physical_symlink: true,
        }));
    }
    if !metadata.is_file() {
        return Ok(None);
    }

    let bytes = fs::read(&destination)
        .map_err(|error| format!("cannot read {}: {error}", destination.display()))?;

    Ok(Some(Snapshot::Blob {
        bytes,
        mode,
        physical_symlink: false,
    }))
}

fn worktree_mode(
    repository: &gix::Repository,
    index_entry: Option<&gix::index::Entry>,
    metadata: &gix::index::fs::Metadata,
) -> Result<EntryMode, String> {
    let config = repository.config_snapshot();
    let executable_bit = config
        .try_boolean("core.fileMode")
        .map_err(|error| format!("cannot read core.fileMode: {error}"))?
        .unwrap_or(true);
    let symlink = config
        .try_boolean("core.symlinks")
        .map_err(|error| format!("cannot read core.symlinks: {error}"))?
        .unwrap_or(true);
    let index_mode = index_entry.map_or(gix::index::entry::Mode::FILE, |entry| entry.mode);
    let mode = index_mode
        .change_to_match_fs(metadata, symlink, executable_bit)
        .map_or(index_mode, |change| change.apply(index_mode));

    mode.to_tree_entry_mode()
        .ok_or_else(|| format!("unsupported Git file mode {mode:?}"))
}

fn tree_snapshot(
    repository: &gix::Repository,
    tree: &gix::Tree<'_>,
    path: &Path,
) -> Result<Option<Snapshot>, String> {
    let Some(entry) = tree
        .lookup_entry_by_path(path)
        .map_err(|error| format!("cannot read HEAD path {}: {error}", path.display()))?
    else {
        return Ok(None);
    };
    if entry.mode().is_tree() {
        return Ok(None);
    }

    snapshot_from_tree_entry(repository, entry.mode(), entry.object_id()).map(Some)
}

fn snapshot_from_tree_entry(
    repository: &gix::Repository,
    mode: EntryMode,
    id: gix::ObjectId,
) -> Result<Snapshot, String> {
    if mode.is_commit() {
        return Ok(Snapshot::Submodule { id: id.to_string() });
    }
    if mode.is_tree() {
        return Err("directories cannot be reviewed as files".into());
    }

    let object = repository
        .find_object(id)
        .map_err(|error| format!("cannot read Git object {id}: {error}"))?;
    let blob = object
        .try_into_blob()
        .map_err(|error| format!("Git object {id} is not a blob: {error}"))?;
    Ok(Snapshot::Blob {
        bytes: blob.data.clone(),
        mode,
        physical_symlink: false,
    })
}

fn commit_manifest(
    repository: &gix::Repository,
    commit_id: gix::ObjectId,
) -> Result<ReviewManifest, String> {
    let commit = repository
        .find_commit(commit_id)
        .map_err(|error| format!("cannot read Git commit {commit_id}: {error}"))?;
    let parents = commit.parent_ids().map(gix::Id::detach).collect::<Vec<_>>();
    if parents.len() > 1 {
        return Err(format!(
            "Merge commit {commit_id} has {} parents. Parent selection is not supported yet.",
            parents.len()
        ));
    }

    let tree = commit
        .tree()
        .map_err(|error| format!("cannot read tree for commit {commit_id}: {error}"))?;
    let parent_tree =
        if let Some(parent) = parents.first() {
            let parent = repository
                .find_commit(*parent)
                .map_err(|error| format!("cannot read parent commit {parent}: {error}"))?;
            Some(parent.tree().map_err(|error| {
                format!("cannot read parent tree for commit {commit_id}: {error}")
            })?)
        } else {
            None
        };
    let options = gix::diff::Options::default().with_rewrites(Some(gix::diff::Rewrites::default()));
    let changes = repository
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(options))
        .map_err(|error| format!("cannot diff Git commit {commit_id}: {error}"))?;

    let mut files = Vec::new();
    for change in changes {
        if let Some(file) = historical_file(repository, change)? {
            files.push(file);
        }
    }

    files.sort_by(|left, right| left.logical_path.cmp(&right.logical_path));
    ReviewManifest::new(files)
}

fn historical_file(
    repository: &gix::Repository,
    change: Change,
) -> Result<Option<ReviewFile>, String> {
    let (path, status, old, new) = match change {
        Change::Addition {
            location,
            entry_mode,
            id,
            ..
        } => {
            if entry_mode.is_tree() {
                return Ok(None);
            }

            (
                git_path(location.as_ref())?,
                ReviewFileStatus::Added,
                None,
                Some(snapshot_from_tree_entry(repository, entry_mode, id)?),
            )
        }
        Change::Deletion {
            location,
            entry_mode,
            id,
            ..
        } => {
            if entry_mode.is_tree() {
                return Ok(None);
            }

            (
                git_path(location.as_ref())?,
                ReviewFileStatus::Deleted,
                Some(snapshot_from_tree_entry(repository, entry_mode, id)?),
                None,
            )
        }
        Change::Modification {
            location,
            previous_entry_mode,
            previous_id,
            entry_mode,
            id,
        } => {
            if previous_entry_mode.is_tree() || entry_mode.is_tree() {
                return Ok(None);
            }

            (
                git_path(location.as_ref())?,
                ReviewFileStatus::Modified,
                Some(snapshot_from_tree_entry(
                    repository,
                    previous_entry_mode,
                    previous_id,
                )?),
                Some(snapshot_from_tree_entry(repository, entry_mode, id)?),
            )
        }
        Change::Rewrite {
            source_location,
            source_entry_mode,
            source_id,
            entry_mode,
            id,
            location,
            copy,
            ..
        } => {
            if source_entry_mode.is_tree() || entry_mode.is_tree() {
                return Ok(None);
            }

            let path = git_path(location.as_ref())?;
            let status = if copy {
                ReviewFileStatus::Added
            } else {
                ReviewFileStatus::Renamed {
                    from: git_path(source_location.as_ref())?,
                }
            };
            (
                path,
                status,
                Some(snapshot_from_tree_entry(
                    repository,
                    source_entry_mode,
                    source_id,
                )?),
                Some(snapshot_from_tree_entry(repository, entry_mode, id)?),
            )
        }
    };

    Ok(Some(snapshot_file(
        path,
        status,
        old.as_ref(),
        new.as_ref(),
    )?))
}

fn snapshot_file(
    path: PathBuf,
    status: ReviewFileStatus,
    old: Option<&Snapshot>,
    new: Option<&Snapshot>,
) -> Result<ReviewFile, String> {
    let identity = ReviewFileIdentity::new(format!("commit:{}", path.display()));

    if old.and_then(Snapshot::submodule_id).is_some()
        || new.and_then(Snapshot::submodule_id).is_some()
    {
        return Ok(ReviewFile::submodule(
            identity,
            path,
            status,
            Snapshot::submodule_display(old),
            Snapshot::submodule_display(new),
        ));
    }

    if old.is_some_and(Snapshot::is_binary) || new.is_some_and(Snapshot::is_binary) {
        return Ok(ReviewFile::binary(
            identity,
            path,
            status,
            "Binary content cannot be displayed.",
        ));
    }

    let baseline = old.and_then(Snapshot::bytes).unwrap_or_default().to_vec();
    let local = new.and_then(Snapshot::bytes).unwrap_or_default().to_vec();
    let comparison = TextComparison::new(
        ComparisonDocument::read_only_memory(path.clone(), baseline),
        ComparisonDocument::read_only_memory(path.clone(), local),
    )?;

    Ok(ReviewFile::text(identity, path, status, comparison))
}

fn git_path(path: &BStr) -> Result<PathBuf, String> {
    gix::path::try_from_bstr(path)
        .map(std::borrow::Cow::into_owned)
        .map_err(|error| format!("Git path is not valid on this platform: {error}"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use gix::{
        bstr::ByteSlice,
        index::entry::{Flags, Mode, Stat},
        objs::{Tree, tree::Entry},
    };
    use tempfile::TempDir;

    use super::*;
    use crate::{
        comparison::{Comparison, DocumentContent},
        review::model::ReviewFileKind,
    };

    struct RepositoryFixture {
        _directory: TempDir,
        root: PathBuf,
        repository: gix::Repository,
        head: Option<gix::ObjectId>,
        tree: gix::ObjectId,
        timestamp: i64,
    }

    impl RepositoryFixture {
        fn unborn() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().join("repository");
            let repository = gix::init(&root).unwrap();
            let tree = repository.empty_tree().id().detach();

            Self {
                _directory: directory,
                root,
                repository,
                head: None,
                tree,
                timestamp: 1,
            }
        }

        fn committed(files: &[(&str, &[u8])]) -> Self {
            let mut fixture = Self::unborn();
            let entries = files
                .iter()
                .map(|(path, bytes)| (*path, TestEntry::Blob(bytes.to_vec())))
                .collect::<Vec<_>>();
            fixture.commit("initial", &entries, &[]);
            fixture.write_worktree(files);
            fixture.write_index_from_head();
            fixture
        }

        fn commit(
            &mut self,
            message: &str,
            entries: &[(&str, TestEntry)],
            extra_parents: &[gix::ObjectId],
        ) -> gix::ObjectId {
            let tree = write_tree(&self.repository, entries);

            self.commit_tree(message, tree, extra_parents)
        }

        fn commit_tree(
            &mut self,
            message: &str,
            tree: gix::ObjectId,
            extra_parents: &[gix::ObjectId],
        ) -> gix::ObjectId {
            let mut parents = self.head.into_iter().collect::<Vec<_>>();
            parents.extend_from_slice(extra_parents);
            let time = format!("{} +0000", self.timestamp);
            let signature = gix::actor::SignatureRef {
                name: "yori fixture".into(),
                email: "fixture@example.com".into(),
                time: &time,
            };
            self.timestamp += 1;
            let id = self
                .repository
                .commit_as(signature, signature, "HEAD", message, tree, parents)
                .unwrap()
                .detach();
            self.head = Some(id);
            self.tree = tree;

            id
        }

        fn detached_commit(
            &mut self,
            message: &str,
            entries: &[(&str, TestEntry)],
            parents: &[gix::ObjectId],
        ) -> gix::ObjectId {
            let tree = write_tree(&self.repository, entries);
            let time = format!("{} +0000", self.timestamp);
            let signature = gix::actor::SignatureRef {
                name: "yori fixture".into(),
                email: "fixture@example.com".into(),
                time: &time,
            };
            self.timestamp += 1;
            self.repository
                .new_commit_as(signature, signature, message, tree, parents.iter().copied())
                .unwrap()
                .id()
                .detach()
        }

        fn write_worktree(&self, files: &[(&str, &[u8])]) {
            for (path, bytes) in files {
                let path = self.root.join(path);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::write(path, bytes).unwrap();
            }
        }

        fn write_index_from_head(&self) {
            self.repository
                .index_from_tree(&self.tree)
                .unwrap()
                .write(gix::index::write::Options::default())
                .unwrap();
        }

        fn discovered(&self) -> GitRepository {
            GitRepository::discover(&self.root).unwrap()
        }
    }

    #[derive(Clone)]
    enum TestEntry {
        Blob(Vec<u8>),
        Binary(Vec<u8>),
        Symlink(Vec<u8>),
        Submodule(gix::ObjectId),
    }

    fn write_tree(repository: &gix::Repository, entries: &[(&str, TestEntry)]) -> gix::ObjectId {
        let mut entries = entries
            .iter()
            .map(|(name, entry)| {
                let (mode, oid) = match entry {
                    TestEntry::Blob(bytes) | TestEntry::Binary(bytes) => (
                        EntryMode::try_from(0o100_644).unwrap(),
                        repository.write_blob(bytes).unwrap().detach(),
                    ),
                    TestEntry::Symlink(target) => (
                        EntryMode::try_from(0o120_000).unwrap(),
                        repository.write_blob(target).unwrap().detach(),
                    ),
                    TestEntry::Submodule(id) => (EntryMode::try_from(0o160_000).unwrap(), *id),
                };

                Entry {
                    mode,
                    filename: (*name).into(),
                    oid,
                }
            })
            .collect::<Vec<_>>();
        entries.sort();

        repository.write_object(Tree { entries }).unwrap().detach()
    }

    fn load(source: &ReviewSource) -> ReviewManifest {
        source.provider.load_manifest(&source.identity).unwrap()
    }

    fn by_path(manifest: &ReviewManifest) -> BTreeMap<&str, &ReviewFile> {
        manifest
            .files
            .iter()
            .map(|file| (file.logical_path.to_str().unwrap(), file))
            .collect()
    }

    fn text_bytes(file: &ReviewFile) -> (&[u8], &[u8], bool, bool) {
        let ReviewFileKind::Text(text) = &file.kind else {
            panic!("expected text file")
        };
        let Comparison::Diff(diff) = text.comparison() else {
            panic!("expected diff")
        };
        let DocumentContent::Memory(baseline) = diff.baseline.content() else {
            panic!("expected memory baseline")
        };
        let DocumentContent::Memory(local) = diff.local.content() else {
            panic!("expected memory local")
        };

        (
            baseline,
            local,
            diff.local.editable(),
            diff.local.save_destination().is_some(),
        )
    }

    #[test]
    fn discovers_from_nested_context_and_keeps_repository_identity_stable() {
        let fixture = RepositoryFixture::committed(&[("tracked.txt", b"base\n")]);
        let nested = fixture.root.join("src/deep");
        fs::create_dir_all(&nested).unwrap();

        let root = GitRepository::discover(&fixture.root).unwrap();
        let nested = GitRepository::discover(&nested).unwrap();

        assert_eq!(root.work_dir(), fixture.root.canonicalize().unwrap());
        assert_eq!(nested.work_dir(), root.work_dir());
        assert_eq!(
            root.working_source().identity,
            nested.working_source().identity
        );
    }

    #[test]
    fn working_manifest_combines_index_and_worktree_and_excludes_ignored_files() {
        let fixture = RepositoryFixture::committed(&[
            (".gitignore", b"ignored.txt\n"),
            ("staged.txt", b"base staged\n"),
            ("mixed.txt", b"base mixed\n"),
            ("deleted.txt", b"delete me\n"),
            ("old-name.txt", b"rename me\n"),
        ]);
        fixture.write_worktree(&[
            ("staged.txt", b"staged value\n"),
            ("mixed.txt", b"staged intermediate\n"),
            ("new-name.txt", b"rename me\n"),
            ("untracked.txt", b"new\n"),
            ("ignored.txt", b"ignored\n"),
            ("binary.dat", b"text\0binary"),
        ]);
        fs::remove_file(fixture.root.join("deleted.txt")).unwrap();
        fs::remove_file(fixture.root.join("old-name.txt")).unwrap();

        let mut index = fixture.repository.index_from_tree(&fixture.tree).unwrap();
        for (path, bytes) in [
            ("staged.txt", b"staged value\n".as_slice()),
            ("mixed.txt", b"staged intermediate\n".as_slice()),
        ] {
            let id = fixture.repository.write_blob(bytes).unwrap().detach();
            index
                .entry_mut_by_path_and_stage(
                    path.as_bytes().as_bstr(),
                    gix::index::entry::Stage::Unconflicted,
                )
                .unwrap()
                .id = id;
        }
        for path in ["deleted.txt", "old-name.txt"] {
            let entry = index
                .entry_index_by_path(path.as_bytes().as_bstr())
                .unwrap();
            index.remove_entry_at_index(entry);
        }
        let renamed = fixture
            .repository
            .write_blob(b"rename me\n")
            .unwrap()
            .detach();
        index.dangerously_push_entry(
            Stat::default(),
            renamed,
            Flags::empty(),
            Mode::FILE,
            b"new-name.txt".as_bstr(),
        );
        index.sort_entries();
        index.write(gix::index::write::Options::default()).unwrap();
        fs::write(fixture.root.join("mixed.txt"), "final unstaged\n").unwrap();

        let manifest = load(&fixture.discovered().working_source());
        let files = by_path(&manifest);

        assert_eq!(files.len(), 6);
        assert!(!files.contains_key("ignored.txt"));
        assert_eq!(files["staged.txt"].status, ReviewFileStatus::Modified);
        assert_eq!(files["deleted.txt"].status, ReviewFileStatus::Deleted);
        assert_eq!(files["untracked.txt"].status, ReviewFileStatus::Added);
        assert_eq!(
            files["new-name.txt"].status,
            ReviewFileStatus::Renamed {
                from: "old-name.txt".into()
            }
        );
        assert!(matches!(
            files["binary.dat"].kind,
            ReviewFileKind::Binary { .. }
        ));

        let (baseline, local, editable, saveable) = text_bytes(files["mixed.txt"]);
        assert_eq!(baseline, b"base mixed\n");
        assert_eq!(local, b"final unstaged\n");
        assert!(editable);
        assert!(saveable);
        let ReviewFileKind::Text(comparison) = &files["mixed.txt"].kind else {
            unreachable!()
        };
        let Comparison::Diff(diff) = comparison.comparison() else {
            unreachable!()
        };
        let expected_destination = fixture.root.join("mixed.txt").canonicalize().unwrap();
        assert_eq!(
            diff.local.save_destination(),
            Some(expected_destination.as_path())
        );
    }

    #[cfg(unix)]
    #[test]
    fn working_symlink_snapshot_is_read_only_and_cannot_save_through_the_target() {
        use std::os::unix::fs::symlink;

        let fixture = RepositoryFixture::unborn();
        let target = fixture.root.join("target.txt");
        let link = fixture.root.join("link.txt");
        fs::write(&target, "target contents\n").unwrap();
        symlink("target.txt", &link).unwrap();

        let manifest = load(&fixture.discovered().working_source());
        let files = by_path(&manifest);
        let (_, local, editable, saveable) = text_bytes(files["link.txt"]);

        assert_eq!(local, b"target.txt");
        assert!(!editable);
        assert!(!saveable);
        assert_eq!(fs::read_to_string(target).unwrap(), "target contents\n");
    }

    #[test]
    fn emulated_symlink_staged_then_restored_to_head_is_net_unchanged() {
        use std::io::Write;

        let mut fixture = RepositoryFixture::unborn();
        fixture.commit(
            "symlink",
            &[("link", TestEntry::Symlink(b"head-target".to_vec()))],
            &[],
        );
        fixture.write_index_from_head();
        fs::write(fixture.root.join("link"), b"head-target").unwrap();

        let mut config = fs::OpenOptions::new()
            .append(true)
            .open(fixture.repository.path().join("config"))
            .unwrap();
        writeln!(config, "[core]\n\tsymlinks = false").unwrap();

        let mut index = fixture.repository.index_from_tree(&fixture.tree).unwrap();
        index
            .entry_mut_by_path_and_stage(b"link".as_bstr(), gix::index::entry::Stage::Unconflicted)
            .unwrap()
            .id = fixture
            .repository
            .write_blob(b"staged-target")
            .unwrap()
            .detach();
        index.write(gix::index::write::Options::default()).unwrap();

        let manifest = load(&fixture.discovered().working_source());

        assert!(manifest.files.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn working_manifest_reports_executable_bit_changes_when_filemode_is_enabled() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = RepositoryFixture::committed(&[("script.sh", b"echo test\n")]);
        let path = fixture.root.join("script.sh");
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();

        let manifest = load(&fixture.discovered().working_source());
        let files = by_path(&manifest);

        assert_eq!(files["script.sh"].status, ReviewFileStatus::Modified);
    }

    #[cfg(unix)]
    #[test]
    fn working_manifest_respects_disabled_filemode() {
        use std::{io::Write, os::unix::fs::PermissionsExt};

        let fixture = RepositoryFixture::committed(&[("script.sh", b"echo test\n")]);
        let mut config = fs::OpenOptions::new()
            .append(true)
            .open(fixture.repository.path().join("config"))
            .unwrap();
        writeln!(config, "[core]\n\tfilemode = false").unwrap();
        let path = fixture.root.join("script.sh");
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();

        let manifest = load(&fixture.discovered().working_source());

        assert!(manifest.files.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn working_manifest_reports_symlink_replaced_by_regular_file() {
        let mut fixture = RepositoryFixture::unborn();
        fixture.commit(
            "symlink",
            &[("link", TestEntry::Symlink(b"target".to_vec()))],
            &[],
        );
        fixture.write_index_from_head();
        fs::write(fixture.root.join("link"), b"target").unwrap();

        let manifest = load(&fixture.discovered().working_source());
        let files = by_path(&manifest);

        assert_eq!(files["link"].status, ReviewFileStatus::Modified);
        let (_, local, editable, saveable) = text_bytes(files["link"]);
        assert_eq!(local, b"target");
        assert!(editable);
        assert!(saveable);
    }

    #[test]
    fn directory_replaced_by_file_yields_addition_and_descendant_deletion() {
        let mut fixture = RepositoryFixture::unborn();
        let child = fixture.repository.write_blob(b"child\n").unwrap().detach();
        let subtree = fixture
            .repository
            .write_object(Tree {
                entries: vec![Entry {
                    mode: EntryMode::try_from(0o100_644).unwrap(),
                    filename: "child.txt".into(),
                    oid: child,
                }],
            })
            .unwrap()
            .detach();
        let root = fixture
            .repository
            .write_object(Tree {
                entries: vec![Entry {
                    mode: EntryMode::try_from(0o040_000).unwrap(),
                    filename: "node".into(),
                    oid: subtree,
                }],
            })
            .unwrap()
            .detach();
        fixture.commit_tree("directory", root, &[]);
        fixture.write_index_from_head();
        fs::write(fixture.root.join("node"), b"replacement\n").unwrap();

        let manifest = load(&fixture.discovered().working_source());
        let files = by_path(&manifest);

        assert_eq!(files["node"].status, ReviewFileStatus::Added);
        assert_eq!(files["node/child.txt"].status, ReviewFileStatus::Deleted);
    }

    #[test]
    fn uninitialized_submodule_uses_the_index_gitlink_without_upward_discovery() {
        let mut fixture = RepositoryFixture::unborn();
        let baseline_id = fixture.detached_commit(
            "baseline submodule",
            &[("file", TestEntry::Blob(b"baseline\n".to_vec()))],
            &[],
        );
        let index_id = fixture.detached_commit(
            "indexed submodule",
            &[("file", TestEntry::Blob(b"indexed\n".to_vec()))],
            &[],
        );
        fixture.commit(
            "superproject",
            &[("vendor", TestEntry::Submodule(baseline_id))],
            &[],
        );
        fixture.write_index_from_head();
        let mut index = fixture.repository.index_from_tree(&fixture.tree).unwrap();
        index
            .entry_mut_by_path_and_stage(
                b"vendor".as_bstr(),
                gix::index::entry::Stage::Unconflicted,
            )
            .unwrap()
            .id = index_id;
        index.write(gix::index::write::Options::default()).unwrap();
        fs::create_dir(fixture.root.join("vendor")).unwrap();

        let manifest = load(&fixture.discovered().working_source());
        let files = by_path(&manifest);
        let ReviewFileKind::Submodule {
            old_identifier,
            new_identifier,
        } = &files["vendor"].kind
        else {
            panic!("expected submodule entry")
        };

        assert_eq!(old_identifier.as_ref(), baseline_id.to_string());
        assert_eq!(new_identifier.as_ref(), index_id.to_string());
        assert_ne!(new_identifier.as_ref(), fixture.head.unwrap().to_string());
    }

    #[test]
    fn unborn_repository_uses_an_empty_editable_baseline() {
        let fixture = RepositoryFixture::unborn();
        fs::write(fixture.root.join("first.txt"), "first\n").unwrap();

        let manifest = load(&fixture.discovered().working_source());

        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].status, ReviewFileStatus::Added);
        let (baseline, local, editable, saveable) = text_bytes(&manifest.files[0]);
        assert!(baseline.is_empty());
        assert_eq!(local, b"first\n");
        assert!(editable);
        assert!(saveable);
    }

    #[test]
    fn historical_revisions_are_read_only_and_represent_all_file_kinds() {
        let mut fixture = RepositoryFixture::committed(&[
            ("deleted.txt", b"old\n"),
            ("renamed.txt", b"same\n"),
            ("modified.txt", b"before\n"),
        ]);
        let parent = fixture.head.unwrap();
        let commit = fixture.commit(
            "all kinds",
            &[
                ("added.txt", TestEntry::Blob(b"new\n".to_vec())),
                ("renamed-new.txt", TestEntry::Blob(b"same\n".to_vec())),
                ("modified.txt", TestEntry::Blob(b"after\n".to_vec())),
                ("binary.dat", TestEntry::Binary(b"new\0binary".to_vec())),
                ("vendor", TestEntry::Submodule(parent)),
            ],
            &[],
        );
        fixture.write_index_from_head();

        let repository = fixture.discovered();
        let recent = repository.recent_commits().unwrap();
        assert_eq!(recent[0].revision, commit.to_string());
        assert_eq!(recent[0].title, "all kinds");
        let manifest = load(&repository.commit_source(&commit.to_string()).unwrap());
        let files = by_path(&manifest);

        assert_eq!(files["added.txt"].status, ReviewFileStatus::Added);
        assert_eq!(files["deleted.txt"].status, ReviewFileStatus::Deleted);
        assert_eq!(files["modified.txt"].status, ReviewFileStatus::Modified);
        assert_eq!(
            files["renamed-new.txt"].status,
            ReviewFileStatus::Renamed {
                from: "renamed.txt".into()
            }
        );
        assert!(matches!(
            files["binary.dat"].kind,
            ReviewFileKind::Binary { .. }
        ));
        assert!(matches!(
            files["vendor"].kind,
            ReviewFileKind::Submodule { .. }
        ));
        let (_, local, editable, saveable) = text_bytes(files["modified.txt"]);
        assert_eq!(local, b"after\n");
        assert!(!editable);
        assert!(!saveable);
    }

    #[test]
    fn direct_revision_accepts_names_and_merge_commits_remain_visible_but_unsupported() {
        let mut fixture = RepositoryFixture::committed(&[("file.txt", b"base\n")]);
        let first = fixture.head.unwrap();
        let side = fixture.detached_commit(
            "side",
            &[("file.txt", TestEntry::Blob(b"side\n".to_vec()))],
            &[first],
        );
        let merge = fixture.commit(
            "merge topic",
            &[("file.txt", TestEntry::Blob(b"merged\n".to_vec()))],
            &[side],
        );
        fixture.write_index_from_head();
        let repository = fixture.discovered();

        let recent = repository.recent_commits().unwrap();
        assert_eq!(recent[0].revision, merge.to_string());
        assert!(recent[0].is_merge);
        let source = repository.commit_source("HEAD").unwrap();
        let error = source.provider.load_manifest(&source.identity).unwrap_err();

        assert!(error.contains("Merge commit"));
        assert!(error.contains("Parent selection is not supported"));
        assert!(repository.commit_source("does-not-exist").is_err());
    }
}
