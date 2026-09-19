//! Perforce context discovery and provider-to-review-manifest mapping.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    future::Future,
    num::NonZeroU32,
    path::{Path, PathBuf},
    pin::pin,
    sync::Arc,
    task::{Context as TaskContext, Poll, Wake, Waker},
    thread,
};

use yori_p4::{
    ChangedFile, ChangelistDescription, ChangelistId, ChangelistStatus, ChangelistSummary,
    ClientInfo, DepotRevision, ErrorKind, FileAction, HaveRevision, OpenedFile, P4Client,
    PendingChangelists, WorkspaceMapping,
};

use crate::comparison::ComparisonDocument;

use super::model::{
    ReviewFile, ReviewFileIdentity, ReviewFileStatus, ReviewManifest, ReviewProvider, ReviewSource,
    ReviewSourceIdentity, TextComparison,
};

const RECENT_CHANGELIST_LIMIT: u32 = 25;
const PROVIDER_NAME: &str = "Perforce";

type ProviderResult<T> = Result<T, String>;
type MappingResult<T> = Result<T, ConnectionFailure>;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConnectionFailure {
    kind: ErrorKind,
    message: String,
}

impl From<yori_p4::Error> for ConnectionFailure {
    fn from(error: yori_p4::Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

impl fmt::Display for ConnectionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

trait PerforceConnection: Send + Sync {
    fn client_info(&self) -> ProviderResult<ClientInfo>;
    fn pending_changelists(&self) -> ProviderResult<PendingChangelists>;
    fn submitted_changelists(
        &self,
        maximum: NonZeroU32,
        file_specification: Option<&str>,
    ) -> ProviderResult<Vec<ChangelistSummary>>;
    fn opened_files(&self, changelist: ChangelistId) -> ProviderResult<Vec<OpenedFile>>;
    fn changelist_description(
        &self,
        changelist: NonZeroU32,
    ) -> ProviderResult<ChangelistDescription>;
    fn have_revisions(&self, file_specifications: &[String]) -> ProviderResult<Vec<HaveRevision>>;
    fn workspace_mappings(
        &self,
        file_specifications: &[String],
    ) -> MappingResult<Vec<WorkspaceMapping>>;
    fn depot_content(&self, revision: &DepotRevision) -> ProviderResult<Vec<u8>>;
}

struct NativePerforceConnection {
    client: P4Client,
}

impl NativePerforceConnection {
    fn connect(directory: &Path) -> ProviderResult<Arc<dyn PerforceConnection>> {
        let client = block_on(P4Client::connect(directory)).map_err(|error| error.to_string())?;

        Ok(Arc::new(Self { client }))
    }
}

impl PerforceConnection for NativePerforceConnection {
    fn client_info(&self) -> ProviderResult<ClientInfo> {
        block_on(self.client.client_info(&yori_p4::CancellationToken::new()))
            .map_err(|error| error.to_string())
    }

    fn pending_changelists(&self) -> ProviderResult<PendingChangelists> {
        block_on(
            self.client
                .pending_changelists(&yori_p4::CancellationToken::new()),
        )
        .map_err(|error| error.to_string())
    }

    fn submitted_changelists(
        &self,
        maximum: NonZeroU32,
        file_specification: Option<&str>,
    ) -> ProviderResult<Vec<ChangelistSummary>> {
        block_on(self.client.submitted_changelists(
            maximum,
            file_specification,
            &yori_p4::CancellationToken::new(),
        ))
        .map_err(|error| error.to_string())
    }

    fn opened_files(&self, changelist: ChangelistId) -> ProviderResult<Vec<OpenedFile>> {
        block_on(
            self.client
                .opened_files(changelist, &yori_p4::CancellationToken::new()),
        )
        .map_err(|error| error.to_string())
    }

    fn changelist_description(
        &self,
        changelist: NonZeroU32,
    ) -> ProviderResult<ChangelistDescription> {
        block_on(
            self.client
                .changelist_description(changelist, &yori_p4::CancellationToken::new()),
        )
        .map_err(|error| error.to_string())
    }

    fn have_revisions(&self, file_specifications: &[String]) -> ProviderResult<Vec<HaveRevision>> {
        block_on(
            self.client
                .have_revisions(file_specifications, &yori_p4::CancellationToken::new()),
        )
        .map_err(|error| error.to_string())
    }

    fn workspace_mappings(
        &self,
        file_specifications: &[String],
    ) -> MappingResult<Vec<WorkspaceMapping>> {
        block_on(
            self.client
                .workspace_mappings(file_specifications, &yori_p4::CancellationToken::new()),
        )
        .map_err(ConnectionFailure::from)
    }

    fn depot_content(&self, revision: &DepotRevision) -> ProviderResult<Vec<u8>> {
        block_on(
            self.client
                .depot_content(revision, &yori_p4::CancellationToken::new()),
        )
        .map_err(|error| error.to_string())
    }
}

struct ThreadWake(thread::Thread);

impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWake(thread::current())));
    let mut context = TaskContext::from_waker(&waker);
    let mut future = pin!(future);

    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => thread::park(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PerforceReviewKind {
    Pending(ChangelistId),
    Submitted(NonZeroU32),
}

pub(crate) struct PerforceContext {
    connection: Arc<dyn PerforceConnection>,
    info: ClientInfo,
    pending: PendingChangelists,
    recent: Vec<ChangelistSummary>,
}

impl PerforceContext {
    pub(crate) fn discover(directory: &Path) -> ProviderResult<Self> {
        let connection = NativePerforceConnection::connect(directory)?;
        Self::discover_with(connection)
    }

    fn discover_with(connection: Arc<dyn PerforceConnection>) -> ProviderResult<Self> {
        let info = connection.client_info()?;
        let pending = connection.pending_changelists()?;
        let client_files = format!("//{}/...", info.client_name);
        let recent = connection.submitted_changelists(
            NonZeroU32::new(RECENT_CHANGELIST_LIMIT).expect("recent limit is nonzero"),
            Some(&client_files),
        )?;

        Ok(Self {
            connection,
            info,
            pending,
            recent,
        })
    }

    pub(crate) fn client_label(&self) -> String {
        format!("{} on {}", self.info.client_name, self.info.server_address)
    }

    pub(crate) fn pending(&self) -> impl Iterator<Item = &ChangelistSummary> {
        std::iter::once(&self.pending.default).chain(self.pending.numbered.iter())
    }

    pub(crate) fn recent(&self) -> &[ChangelistSummary] {
        &self.recent
    }

    pub(crate) fn pending_source(&self, summary: &ChangelistSummary) -> ReviewSource {
        self.source(PerforceReviewKind::Pending(summary.id), "Pending", summary)
    }

    pub(crate) fn submitted_source(
        &self,
        summary: &ChangelistSummary,
    ) -> ProviderResult<ReviewSource> {
        let ChangelistId::Number(number) = summary.id else {
            return Err("submitted changelists must have a number".into());
        };

        Ok(self.source(PerforceReviewKind::Submitted(number), "Submitted", summary))
    }

    pub(crate) fn source_for_number(&self, number: NonZeroU32) -> ProviderResult<ReviewSource> {
        let description = self.connection.changelist_description(number)?;
        let summary = &description.summary;

        match summary.status {
            ChangelistStatus::Pending => {
                if summary.client != self.info.client_name || summary.user != self.info.user_name {
                    return Err(format!(
                        "pending changelist {number} belongs to {} on client {}; select a changelist owned by {} on client {}",
                        summary.user, summary.client, self.info.user_name, self.info.client_name
                    ));
                }

                Ok(self.source(
                    PerforceReviewKind::Pending(ChangelistId::Number(number)),
                    "Pending",
                    summary,
                ))
            }
            ChangelistStatus::Submitted => {
                Ok(self.source(PerforceReviewKind::Submitted(number), "Submitted", summary))
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        info: ClientInfo,
        pending: PendingChangelists,
        recent: Vec<ChangelistSummary>,
        descriptions: HashMap<NonZeroU32, ChangelistDescription>,
    ) -> Self {
        let connection = Arc::new(StaticTestConnection {
            info: info.clone(),
            descriptions,
        });

        Self {
            connection,
            info,
            pending,
            recent,
        }
    }

    fn source(
        &self,
        kind: PerforceReviewKind,
        descriptor: &'static str,
        summary: &ChangelistSummary,
    ) -> ReviewSource {
        let key = match kind {
            PerforceReviewKind::Pending(changelist) => format!(
                "{} | {} | pending {changelist}",
                self.info.server_address, self.info.client_name
            ),
            PerforceReviewKind::Submitted(changelist) => format!(
                "{} | {} | submitted {changelist}",
                self.info.server_address, self.info.client_name
            ),
        };

        let provider = PerforceReviewProvider {
            connection: Arc::clone(&self.connection),
            info: self.info.clone(),
            kind,
        };

        let headline = source_headline(summary);
        ReviewSource::new(
            ReviewSourceIdentity::new(PROVIDER_NAME, key),
            format!("{descriptor} {headline}"),
            descriptor,
            headline,
            Arc::new(provider),
        )
    }
}

#[cfg(test)]
struct StaticTestConnection {
    info: ClientInfo,
    descriptions: HashMap<NonZeroU32, ChangelistDescription>,
}

#[cfg(test)]
impl PerforceConnection for StaticTestConnection {
    fn client_info(&self) -> ProviderResult<ClientInfo> {
        Ok(self.info.clone())
    }

    fn pending_changelists(&self) -> ProviderResult<PendingChangelists> {
        Err("test context has no discovery response".into())
    }

    fn submitted_changelists(
        &self,
        _: NonZeroU32,
        _: Option<&str>,
    ) -> ProviderResult<Vec<ChangelistSummary>> {
        Err("test context has no discovery response".into())
    }

    fn opened_files(&self, _: ChangelistId) -> ProviderResult<Vec<OpenedFile>> {
        Ok(Vec::new())
    }

    fn changelist_description(
        &self,
        changelist: NonZeroU32,
    ) -> ProviderResult<ChangelistDescription> {
        self.descriptions
            .get(&changelist)
            .cloned()
            .ok_or_else(|| format!("missing test changelist {changelist}"))
    }

    fn have_revisions(&self, _: &[String]) -> ProviderResult<Vec<HaveRevision>> {
        Ok(Vec::new())
    }

    fn workspace_mappings(&self, _: &[String]) -> MappingResult<Vec<WorkspaceMapping>> {
        Ok(Vec::new())
    }

    fn depot_content(&self, revision: &DepotRevision) -> ProviderResult<Vec<u8>> {
        Err(format!("missing test content for {revision}"))
    }
}

/// Names a changelist by number and first description line, without the
/// pending/submitted prefix that `ReviewSource::kind` already carries.
fn source_headline(summary: &ChangelistSummary) -> String {
    let description = summary
        .description
        .lines()
        .next()
        .unwrap_or_default()
        .trim();

    if description.is_empty() {
        summary.id.to_string()
    } else {
        format!("{}: {description}", summary.id)
    }
}

struct PerforceReviewProvider {
    connection: Arc<dyn PerforceConnection>,
    info: ClientInfo,
    kind: PerforceReviewKind,
}

impl ReviewProvider for PerforceReviewProvider {
    fn load_manifest(&self, _: &ReviewSourceIdentity) -> ProviderResult<ReviewManifest> {
        match self.kind {
            PerforceReviewKind::Pending(changelist) => self.load_pending(changelist),
            PerforceReviewKind::Submitted(changelist) => self.load_submitted(changelist),
        }
    }
}

impl PerforceReviewProvider {
    fn load_pending(&self, changelist: ChangelistId) -> ProviderResult<ReviewManifest> {
        let opened = self.connection.opened_files(changelist)?;
        let opened = opened
            .into_iter()
            .filter(|file| file.changelist == changelist)
            .collect::<Vec<_>>();
        let have_specs = opened
            .iter()
            .filter(|file| !matches!(file.action, FileAction::Add | FileAction::MoveAdd))
            .map(|file| file.depot_path.clone())
            .collect::<Vec<_>>();
        let have = self
            .connection
            .have_revisions(&have_specs)?
            .into_iter()
            .map(|revision| (revision.depot_path.clone(), revision))
            .collect::<HashMap<_, _>>();
        let paired_move_deletes = paired_move_deletes_opened(&opened);
        let mut files = Vec::with_capacity(opened.len());

        for file in &opened {
            if matches!(file.action, FileAction::MoveDelete)
                && paired_move_deletes.contains(&file.depot_path)
            {
                continue;
            }

            files.push(self.pending_file(file, &opened, &have)?);
        }

        ReviewManifest::new(files)
    }

    fn pending_file(
        &self,
        file: &OpenedFile,
        opened: &[OpenedFile],
        have: &HashMap<String, HaveRevision>,
    ) -> ProviderResult<ReviewFile> {
        let local_path = self.pending_local_path(file)?;
        let logical_path = self.logical_path(&local_path);
        let identity = ReviewFileIdentity::new(file.depot_path.clone());
        let rename_source = move_source_opened(file, opened);
        let status = pending_status(
            file,
            rename_source.map(|source| {
                self.logical_path_for_depot_or_local(
                    &source.depot_path,
                    source.local_path.as_deref(),
                )
            }),
        );

        if is_binary(file.file_type.as_deref()) {
            return Ok(ReviewFile::binary(
                identity,
                logical_path,
                status,
                binary_explanation(&file.action, file.file_type.as_deref()),
            ));
        }

        let comparison = match file.action {
            FileAction::Add => TextComparison::added(
                logical_path.clone(),
                ComparisonDocument::editable_file(local_path),
            )?,
            FileAction::MoveAdd => {
                let Some(source) = rename_source else {
                    return TextComparison::added(
                        logical_path.clone(),
                        ComparisonDocument::editable_file(local_path),
                    )
                    .map(|comparison| {
                        ReviewFile::text(identity, logical_path, status, comparison)
                    });
                };

                let baseline = self.pending_baseline(source, have)?;

                TextComparison::new(
                    ComparisonDocument::read_only_memory(
                        self.logical_path_for_depot_or_local(
                            &source.depot_path,
                            source.local_path.as_deref(),
                        ),
                        baseline,
                    ),
                    ComparisonDocument::editable_file(local_path),
                )?
            }
            FileAction::Delete
            | FileAction::MoveDelete
            | FileAction::Archive
            | FileAction::Purge => TextComparison::deleted(
                logical_path.clone(),
                ComparisonDocument::read_only_memory(
                    logical_path.clone(),
                    self.pending_baseline(file, have)?,
                ),
            )?,
            FileAction::Branch
            | FileAction::Edit
            | FileAction::Import
            | FileAction::Integrate
            | FileAction::Unknown(_) => TextComparison::new(
                ComparisonDocument::read_only_memory(
                    logical_path.clone(),
                    self.pending_baseline(file, have)?,
                ),
                ComparisonDocument::editable_file(local_path),
            )?,
        };

        Ok(ReviewFile::text(identity, logical_path, status, comparison))
    }

    fn pending_baseline(
        &self,
        file: &OpenedFile,
        have: &HashMap<String, HaveRevision>,
    ) -> ProviderResult<Vec<u8>> {
        let revision = have
            .get(&file.depot_path)
            .map(|revision| revision.revision)
            .or(file.have_revision)
            .and_then(NonZeroU32::new)
            .ok_or_else(|| {
                format!(
                    "{} has no HAVE revision; sync the workspace file or reopen it for add",
                    file.depot_path
                )
            })?;

        self.connection.depot_content(&DepotRevision {
            depot_path: file.depot_path.clone(),
            revision,
        })
    }

    fn pending_local_path(&self, file: &OpenedFile) -> ProviderResult<PathBuf> {
        if let Some(path) = &file.local_path {
            return Ok(path.clone());
        }

        self.effective_mapping(&file.depot_path)
            .map_err(|error| error.to_string())?
            .map(|mapping| mapping.local_path)
            .ok_or_else(|| {
                format!(
                    "{} is not mapped by the active Perforce client {}; check P4CLIENT and its view",
                    file.depot_path, self.info.client_name
                )
            })
    }

    fn load_submitted(&self, changelist: NonZeroU32) -> ProviderResult<ReviewManifest> {
        let description = self.connection.changelist_description(changelist)?;
        if description.summary.status != ChangelistStatus::Submitted {
            return Err(format!("changelist {changelist} is not submitted"));
        }

        let mut mappings = HashMap::new();
        for file in &description.files {
            if let Some(mapping) = self
                .effective_mapping(&file.depot_path)
                .map_err(|error| error.to_string())?
            {
                mappings.insert(file.depot_path.clone(), mapping);
            }
        }

        let represented_move_deletes = represented_move_deletes(&description.files, &mappings);
        let mut files = Vec::with_capacity(mappings.len());

        for file in &description.files {
            let Some(mapping) = mappings.get(&file.depot_path) else {
                continue;
            };

            if matches!(file.action, FileAction::MoveDelete)
                && represented_move_deletes.contains(&file.depot_path)
            {
                continue;
            }

            let rename_source = move_source_changed(file, &description.files);
            files.push(self.submitted_file(file, rename_source, mapping)?);
        }

        if files.is_empty() && !description.files.is_empty() {
            return Err(format!(
                "submitted changelist {changelist} has no files in the active client view {}; check P4CLIENT and its view",
                self.info.client_name
            ));
        }

        ReviewManifest::new(files)
    }

    fn submitted_file(
        &self,
        file: &ChangedFile,
        rename_source: Option<&ChangedFile>,
        mapping: &WorkspaceMapping,
    ) -> ProviderResult<ReviewFile> {
        let logical_path = self.logical_path(&mapping.local_path);
        let identity = ReviewFileIdentity::new(file.depot_path.clone());
        let status = submitted_status(
            file,
            rename_source
                .map(|source| self.logical_path_for_depot_or_local(&source.depot_path, None)),
        );

        if is_binary(file.file_type.as_deref()) {
            return Ok(ReviewFile::binary(
                identity,
                logical_path,
                status,
                binary_explanation(&file.action, file.file_type.as_deref()),
            ));
        }

        let comparison = match file.action {
            FileAction::Add => TextComparison::added(
                logical_path.clone(),
                ComparisonDocument::read_only_memory(
                    logical_path.clone(),
                    self.submitted_content(file)?,
                ),
            )?,
            FileAction::MoveAdd => {
                let baseline = rename_source.map_or_else(
                    || Ok(Vec::new()),
                    |source| self.submitted_previous_content(source),
                )?;
                let baseline_path = rename_source.map_or_else(
                    || logical_path.clone(),
                    |source| self.logical_path_for_depot_or_local(&source.depot_path, None),
                );

                TextComparison::new(
                    ComparisonDocument::read_only_memory(baseline_path, baseline),
                    ComparisonDocument::read_only_memory(
                        logical_path.clone(),
                        self.submitted_content(file)?,
                    ),
                )?
            }
            FileAction::Delete
            | FileAction::MoveDelete
            | FileAction::Archive
            | FileAction::Purge => TextComparison::deleted(
                logical_path.clone(),
                ComparisonDocument::read_only_memory(
                    logical_path.clone(),
                    self.submitted_previous_content(file)?,
                ),
            )?,
            FileAction::Branch
            | FileAction::Edit
            | FileAction::Import
            | FileAction::Integrate
            | FileAction::Unknown(_) => TextComparison::new(
                ComparisonDocument::read_only_memory(
                    logical_path.clone(),
                    self.submitted_previous_content(file)?,
                ),
                ComparisonDocument::read_only_memory(
                    logical_path.clone(),
                    self.submitted_content(file)?,
                ),
            )?,
        };

        Ok(ReviewFile::text(identity, logical_path, status, comparison))
    }

    fn submitted_content(&self, file: &ChangedFile) -> ProviderResult<Vec<u8>> {
        let revision = NonZeroU32::new(file.revision).ok_or_else(|| {
            format!(
                "Perforce returned revision zero for submitted file {}",
                file.depot_path
            )
        })?;

        self.connection.depot_content(&DepotRevision {
            depot_path: file.depot_path.clone(),
            revision,
        })
    }

    fn submitted_previous_content(&self, file: &ChangedFile) -> ProviderResult<Vec<u8>> {
        let Some(revision) = file.revision.checked_sub(1).and_then(NonZeroU32::new) else {
            return Ok(Vec::new());
        };

        self.connection.depot_content(&DepotRevision {
            depot_path: file.depot_path.clone(),
            revision,
        })
    }

    fn effective_mapping(&self, depot_path: &str) -> MappingResult<Option<WorkspaceMapping>> {
        let mappings = match self.connection.workspace_mappings(&[depot_path.to_owned()]) {
            Ok(mappings) => mappings,
            Err(error) if error.kind == ErrorKind::Mapping => return Ok(None),
            Err(error) => return Err(error),
        };

        Ok(mappings
            .into_iter()
            .rev()
            .find(|mapping| !mapping.is_exclusion))
    }

    fn logical_path(&self, local_path: &Path) -> PathBuf {
        self.info
            .client_root
            .as_deref()
            .and_then(|root| local_path.strip_prefix(root).ok())
            .map_or_else(|| local_path.to_owned(), Path::to_path_buf)
    }

    fn logical_path_for_depot_or_local(
        &self,
        depot_path: &str,
        local_path: Option<&Path>,
    ) -> PathBuf {
        local_path.map_or_else(
            || PathBuf::from(depot_path.trim_start_matches("//")),
            |path| self.logical_path(path),
        )
    }
}

fn paired_move_deletes_opened(files: &[OpenedFile]) -> HashSet<String> {
    files
        .iter()
        .filter(|file| matches!(file.action, FileAction::MoveAdd))
        .filter_map(|file| file.moved_file.clone())
        .collect()
}

fn move_source_opened<'a>(file: &OpenedFile, files: &'a [OpenedFile]) -> Option<&'a OpenedFile> {
    if !matches!(file.action, FileAction::MoveAdd) {
        return None;
    }

    file.moved_file.as_deref().and_then(|source| {
        files
            .iter()
            .find(|candidate| candidate.depot_path == source)
    })
}

fn represented_move_deletes(
    files: &[ChangedFile],
    mappings: &HashMap<String, WorkspaceMapping>,
) -> HashSet<String> {
    files
        .iter()
        .filter(|file| {
            matches!(file.action, FileAction::MoveAdd) && mappings.contains_key(&file.depot_path)
        })
        .filter_map(|file| file.moved_file.clone())
        .collect()
}

fn move_source_changed<'a>(
    file: &ChangedFile,
    files: &'a [ChangedFile],
) -> Option<&'a ChangedFile> {
    if !matches!(file.action, FileAction::MoveAdd) {
        return None;
    }

    file.moved_file.as_deref().and_then(|source| {
        files
            .iter()
            .find(|candidate| candidate.depot_path == source)
    })
}

fn pending_status(file: &OpenedFile, rename_from: Option<PathBuf>) -> ReviewFileStatus {
    match (&file.action, rename_from) {
        (FileAction::MoveAdd, Some(from)) => ReviewFileStatus::Renamed { from },
        (FileAction::Add | FileAction::MoveAdd, None) => ReviewFileStatus::Added,
        (
            FileAction::Delete | FileAction::MoveDelete | FileAction::Archive | FileAction::Purge,
            _,
        ) => ReviewFileStatus::Deleted,
        _ => ReviewFileStatus::Modified,
    }
}

fn submitted_status(file: &ChangedFile, rename_from: Option<PathBuf>) -> ReviewFileStatus {
    match (&file.action, rename_from) {
        (FileAction::MoveAdd, Some(from)) => ReviewFileStatus::Renamed { from },
        (FileAction::Add | FileAction::MoveAdd, None) => ReviewFileStatus::Added,
        (
            FileAction::Delete | FileAction::MoveDelete | FileAction::Archive | FileAction::Purge,
            _,
        ) => ReviewFileStatus::Deleted,
        _ if file.revision == 1 => ReviewFileStatus::Added,
        _ => ReviewFileStatus::Modified,
    }
}

fn is_binary(file_type: Option<&str>) -> bool {
    let Some(base) = file_type.and_then(|file_type| file_type.split('+').next()) else {
        return false;
    };

    !matches!(base, "text" | "unicode" | "utf8" | "utf16" | "symlink")
}

fn binary_explanation(action: &FileAction, file_type: Option<&str>) -> String {
    format!(
        "Perforce {} action on {} content. Text comparison is unavailable.",
        action_label(action),
        file_type.unwrap_or("binary")
    )
}

fn action_label(action: &FileAction) -> &str {
    match action {
        FileAction::Add => "add",
        FileAction::Archive => "archive",
        FileAction::Branch => "branch",
        FileAction::Delete => "delete",
        FileAction::Edit => "edit",
        FileAction::Import => "import",
        FileAction::Integrate => "integrate",
        FileAction::MoveAdd => "move/add",
        FileAction::MoveDelete => "move/delete",
        FileAction::Purge => "purge",
        FileAction::Unknown(action) => action,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::comparison::{Comparison, DocumentContent};

    use super::*;

    #[derive(Default)]
    struct FakeConnection {
        info: Option<ClientInfo>,
        pending: Option<PendingChangelists>,
        recent: Vec<ChangelistSummary>,
        opened: HashMap<ChangelistId, Vec<OpenedFile>>,
        descriptions: HashMap<NonZeroU32, ChangelistDescription>,
        have: Vec<HaveRevision>,
        mappings: HashMap<String, MappingResult<Vec<WorkspaceMapping>>>,
        contents: HashMap<String, Vec<u8>>,
        calls: Mutex<Vec<String>>,
    }

    impl PerforceConnection for FakeConnection {
        fn client_info(&self) -> ProviderResult<ClientInfo> {
            self.calls.lock().unwrap().push("info".into());
            self.info.clone().ok_or_else(|| "missing info".into())
        }

        fn pending_changelists(&self) -> ProviderResult<PendingChangelists> {
            self.calls.lock().unwrap().push("pending".into());
            self.pending.clone().ok_or_else(|| "missing pending".into())
        }

        fn submitted_changelists(
            &self,
            _: NonZeroU32,
            file_specification: Option<&str>,
        ) -> ProviderResult<Vec<ChangelistSummary>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("recent:{}", file_specification.unwrap_or_default()));
            Ok(self.recent.clone())
        }

        fn opened_files(&self, changelist: ChangelistId) -> ProviderResult<Vec<OpenedFile>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("opened:{changelist}"));
            Ok(self.opened.get(&changelist).cloned().unwrap_or_default())
        }

        fn changelist_description(
            &self,
            changelist: NonZeroU32,
        ) -> ProviderResult<ChangelistDescription> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("describe:{changelist}"));
            self.descriptions
                .get(&changelist)
                .cloned()
                .ok_or_else(|| "missing description".into())
        }

        fn have_revisions(
            &self,
            file_specifications: &[String],
        ) -> ProviderResult<Vec<HaveRevision>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("have:{}", file_specifications.join(",")));
            Ok(self.have.clone())
        }

        fn workspace_mappings(
            &self,
            file_specifications: &[String],
        ) -> MappingResult<Vec<WorkspaceMapping>> {
            let Some(path) = file_specifications.first() else {
                return Err(ConnectionFailure {
                    kind: ErrorKind::InvalidResponse,
                    message: "missing file specification".into(),
                });
            };

            self.calls.lock().unwrap().push(format!("where:{path}"));
            self.mappings
                .get(path)
                .cloned()
                .unwrap_or_else(|| Ok(Vec::new()))
        }

        fn depot_content(&self, revision: &DepotRevision) -> ProviderResult<Vec<u8>> {
            self.calls.lock().unwrap().push(format!("print:{revision}"));
            self.contents
                .get(&revision.to_string())
                .cloned()
                .ok_or_else(|| format!("missing content for {revision}"))
        }
    }

    fn info(root: &Path) -> ClientInfo {
        ClientInfo {
            server_address: "ssl:perforce.example:1666".into(),
            server_version: "P4D/test".into(),
            user_name: "robin".into(),
            client_name: "robin-yori".into(),
            client_root: Some(root.to_owned()),
            current_directory: root.to_owned(),
            case_handling: Some("sensitive".into()),
            unicode_enabled: true,
        }
    }

    fn summary(id: ChangelistId, status: ChangelistStatus, description: &str) -> ChangelistSummary {
        ChangelistSummary {
            id,
            status,
            description: description.into(),
            user: "robin".into(),
            client: "robin-yori".into(),
            modified_unix_seconds: None,
        }
    }

    fn pending_lists() -> PendingChangelists {
        PendingChangelists {
            default: summary(
                ChangelistId::Default,
                ChangelistStatus::Pending,
                "Default changelist",
            ),
            numbered: vec![summary(
                ChangelistId::Number(NonZeroU32::new(42).unwrap()),
                ChangelistStatus::Pending,
                "Numbered work",
            )],
        }
    }

    fn opened(
        depot_path: &str,
        local_path: &Path,
        action: FileAction,
        changelist: ChangelistId,
    ) -> OpenedFile {
        OpenedFile {
            depot_path: depot_path.into(),
            client_path: None,
            local_path: Some(local_path.to_owned()),
            moved_file: None,
            revision: None,
            have_revision: None,
            action,
            changelist,
            file_type: Some("text".into()),
        }
    }

    fn changed(depot_path: &str, revision: u32, action: FileAction) -> ChangedFile {
        ChangedFile {
            depot_path: depot_path.into(),
            moved_file: None,
            revision,
            action,
            file_type: Some("text".into()),
            file_size: None,
            digest: None,
        }
    }

    fn mapping(depot_path: &str, local_path: &Path) -> WorkspaceMapping {
        WorkspaceMapping {
            depot_path: depot_path.into(),
            client_path: depot_path.replacen("//depot", "//robin-yori", 1),
            local_path: local_path.to_owned(),
            is_exclusion: false,
        }
    }

    struct TextDetails {
        baseline: Vec<u8>,
        local: Vec<u8>,
        baseline_path: PathBuf,
        local_path: PathBuf,
        editable: bool,
        save_destination: Option<PathBuf>,
    }

    fn text_details(file: &ReviewFile) -> TextDetails {
        let super::super::model::ReviewFileKind::Text(comparison) = &file.kind else {
            panic!("expected text comparison");
        };
        let Comparison::Diff(comparison) = comparison.comparison() else {
            panic!("expected two-way comparison");
        };
        let read = |document: &ComparisonDocument| match document.content() {
            DocumentContent::Memory(content) => content.to_vec(),
            DocumentContent::File(path) => std::fs::read(path).unwrap(),
        };

        TextDetails {
            baseline: read(&comparison.baseline),
            local: read(&comparison.local),
            baseline_path: comparison.baseline.logical_path().to_owned(),
            local_path: comparison.local.logical_path().to_owned(),
            editable: comparison.local.editable(),
            save_destination: comparison.local.save_destination().map(Path::to_owned),
        }
    }

    fn assert_pending_action_manifest(manifest: &ReviewManifest, paths: &[PathBuf; 5]) {
        assert_eq!(manifest.files.len(), 4);

        let added = &manifest.files[0];
        let added_text = text_details(added);
        assert_eq!(added.identity.to_string(), "//depot/added.txt");
        assert_eq!(added.logical_path, PathBuf::from("added.txt"));
        assert_eq!(added.status, ReviewFileStatus::Added);
        assert!(added_text.baseline.is_empty());
        assert_eq!(added_text.local, b"added\n");
        assert_eq!(added_text.save_destination, Some(paths[0].clone()));

        let deleted = &manifest.files[1];
        let deleted_text = text_details(deleted);
        assert_eq!(deleted.identity.to_string(), "//depot/deleted.txt");
        assert_eq!(deleted.logical_path, PathBuf::from("deleted.txt"));
        assert_eq!(deleted.status, ReviewFileStatus::Deleted);
        assert_eq!(deleted_text.baseline, b"deleted\n");
        assert!(deleted_text.local.is_empty());
        assert_eq!(deleted_text.save_destination, None);

        let renamed = &manifest.files[2];
        let renamed_text = text_details(renamed);
        assert_eq!(renamed.identity.to_string(), "//depot/new.txt");
        assert_eq!(renamed.logical_path, PathBuf::from("new.txt"));
        assert_eq!(
            renamed.status,
            ReviewFileStatus::Renamed {
                from: PathBuf::from("old.txt")
            }
        );
        assert_eq!(renamed_text.baseline, b"before move\n");
        assert_eq!(renamed_text.local, b"moved\n");
        assert_eq!(renamed_text.baseline_path, PathBuf::from("old.txt"));
        assert_eq!(renamed_text.local_path, paths[3]);
        assert_eq!(renamed_text.save_destination, Some(paths[3].clone()));

        let binary = &manifest.files[3];
        assert_eq!(binary.identity.to_string(), "//depot/image.bin");
        assert_eq!(binary.logical_path, PathBuf::from("image.bin"));
        assert_eq!(binary.status, ReviewFileStatus::Modified);
        assert!(matches!(
            binary.kind,
            super::super::model::ReviewFileKind::Binary { .. }
        ));
    }

    #[test]
    fn discovery_scopes_recent_changes_to_the_invocation_client() {
        let root = PathBuf::from("/work/client");
        let fake = Arc::new(FakeConnection {
            info: Some(info(&root)),
            pending: Some(pending_lists()),
            recent: vec![summary(
                ChangelistId::Number(NonZeroU32::new(41).unwrap()),
                ChangelistStatus::Submitted,
                "Submitted work",
            )],
            ..FakeConnection::default()
        });

        let context = PerforceContext::discover_with(fake.clone()).unwrap();

        assert_eq!(context.pending().count(), 2);
        assert_eq!(context.recent().len(), 1);
        assert_eq!(
            fake.calls.lock().unwrap().as_slice(),
            ["info", "pending", "recent://robin-yori/..."]
        );
    }

    #[test]
    fn default_and_numbered_pending_use_only_assigned_files_and_have_baselines() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let default_path = root.join("src/default.rs");
        let numbered_path = root.join("src/numbered.rs");
        std::fs::create_dir_all(default_path.parent().unwrap()).unwrap();
        std::fs::write(&default_path, "default local\n").unwrap();
        std::fs::write(&numbered_path, "numbered local\n").unwrap();
        let numbered = ChangelistId::Number(NonZeroU32::new(42).unwrap());
        let mut fake = FakeConnection {
            info: Some(info(root)),
            pending: Some(pending_lists()),
            have: vec![
                HaveRevision {
                    depot_path: "//depot/src/default.rs".into(),
                    client_path: "//robin-yori/src/default.rs".into(),
                    local_path: default_path.clone(),
                    revision: 3,
                },
                HaveRevision {
                    depot_path: "//depot/src/numbered.rs".into(),
                    client_path: "//robin-yori/src/numbered.rs".into(),
                    local_path: numbered_path.clone(),
                    revision: 7,
                },
            ],
            ..FakeConnection::default()
        };
        fake.opened.insert(
            ChangelistId::Default,
            vec![
                opened(
                    "//depot/src/default.rs",
                    &default_path,
                    FileAction::Edit,
                    ChangelistId::Default,
                ),
                opened(
                    "//depot/src/numbered.rs",
                    &numbered_path,
                    FileAction::Edit,
                    numbered,
                ),
            ],
        );
        fake.opened.insert(
            numbered,
            vec![opened(
                "//depot/src/numbered.rs",
                &numbered_path,
                FileAction::Edit,
                numbered,
            )],
        );
        fake.contents.insert(
            "//depot/src/default.rs#3".into(),
            b"default have\n".to_vec(),
        );
        fake.contents.insert(
            "//depot/src/numbered.rs#7".into(),
            b"numbered have\n".to_vec(),
        );
        let fake = Arc::new(fake);
        let context = PerforceContext::discover_with(fake).unwrap();

        let default_source = context.pending_source(&context.pending.default);
        let default_manifest = default_source
            .provider
            .load_manifest(&default_source.identity)
            .unwrap();
        assert_eq!(default_manifest.files.len(), 1);
        let default_file = &default_manifest.files[0];
        let default = text_details(default_file);
        assert_eq!(default_file.identity.to_string(), "//depot/src/default.rs");
        assert_eq!(default_file.logical_path, PathBuf::from("src/default.rs"));
        assert_eq!(default_file.status, ReviewFileStatus::Modified);
        assert_eq!(default.baseline, b"default have\n");
        assert_eq!(default.local, b"default local\n");
        assert_eq!(default.baseline_path, PathBuf::from("src/default.rs"));
        assert_eq!(default.local_path, default_path);
        assert!(default.editable);
        assert_eq!(default.save_destination, Some(default_path.clone()));

        let numbered_source = context.pending_source(&context.pending.numbered[0]);
        let numbered_manifest = numbered_source
            .provider
            .load_manifest(&numbered_source.identity)
            .unwrap();
        assert_eq!(numbered_manifest.files.len(), 1);
        let numbered_file = &numbered_manifest.files[0];
        let numbered = text_details(numbered_file);
        assert_eq!(
            numbered_file.identity.to_string(),
            "//depot/src/numbered.rs"
        );
        assert_eq!(numbered_file.logical_path, PathBuf::from("src/numbered.rs"));
        assert_eq!(numbered_file.status, ReviewFileStatus::Modified);
        assert_eq!(numbered.baseline, b"numbered have\n");
        assert_eq!(numbered.local, b"numbered local\n");
        assert_eq!(numbered.baseline_path, PathBuf::from("src/numbered.rs"));
        assert_eq!(numbered.local_path, numbered_path);
        assert!(numbered.editable);
        assert_eq!(numbered.save_destination, Some(numbered_path.clone()));
    }

    #[test]
    fn pending_actions_map_add_delete_move_and_binary_without_temp_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let paths = [
            "added.txt",
            "deleted.txt",
            "old.txt",
            "new.txt",
            "image.bin",
        ]
        .map(|name| root.join(name));
        std::fs::write(&paths[0], "added\n").unwrap();
        std::fs::write(&paths[3], "moved\n").unwrap();
        std::fs::write(&paths[4], [0, 1, 2]).unwrap();
        let mut old = opened(
            "//depot/old.txt",
            &paths[2],
            FileAction::MoveDelete,
            ChangelistId::Default,
        );
        old.moved_file = Some("//depot/new.txt".into());
        let mut new = opened(
            "//depot/new.txt",
            &paths[3],
            FileAction::MoveAdd,
            ChangelistId::Default,
        );
        new.moved_file = Some("//depot/old.txt".into());
        let mut binary = opened(
            "//depot/image.bin",
            &paths[4],
            FileAction::Edit,
            ChangelistId::Default,
        );
        binary.file_type = Some("binary+l".into());
        let opened_files = vec![
            opened(
                "//depot/added.txt",
                &paths[0],
                FileAction::Add,
                ChangelistId::Default,
            ),
            opened(
                "//depot/deleted.txt",
                &paths[1],
                FileAction::Delete,
                ChangelistId::Default,
            ),
            old,
            new,
            binary,
        ];
        let mut fake = FakeConnection {
            info: Some(info(root)),
            pending: Some(pending_lists()),
            opened: HashMap::from([(ChangelistId::Default, opened_files)]),
            have: vec![
                HaveRevision {
                    depot_path: "//depot/deleted.txt".into(),
                    client_path: "//robin-yori/deleted.txt".into(),
                    local_path: paths[1].clone(),
                    revision: 2,
                },
                HaveRevision {
                    depot_path: "//depot/old.txt".into(),
                    client_path: "//robin-yori/old.txt".into(),
                    local_path: paths[2].clone(),
                    revision: 5,
                },
                HaveRevision {
                    depot_path: "//depot/image.bin".into(),
                    client_path: "//robin-yori/image.bin".into(),
                    local_path: paths[4].clone(),
                    revision: 1,
                },
            ],
            ..FakeConnection::default()
        };
        fake.contents
            .insert("//depot/deleted.txt#2".into(), b"deleted\n".to_vec());
        fake.contents
            .insert("//depot/old.txt#5".into(), b"before move\n".to_vec());
        let fake = Arc::new(fake);
        let context = PerforceContext::discover_with(fake).unwrap();
        let source = context.pending_source(&context.pending.default);

        let manifest = source.provider.load_manifest(&source.identity).unwrap();

        assert_pending_action_manifest(&manifest, &paths);
    }

    #[test]
    fn submitted_documents_are_read_only_in_memory_and_limited_to_client_view() {
        let root = PathBuf::from("/work/client");
        let number = NonZeroU32::new(41).unwrap();
        let included = "//depot/src/lib.rs";
        let excluded = "//other/private.txt";
        let description = ChangelistDescription {
            summary: summary(
                ChangelistId::Number(number),
                ChangelistStatus::Submitted,
                "Submitted work",
            ),
            files: vec![
                changed(included, 4, FileAction::Edit),
                changed(excluded, 1, FileAction::Add),
            ],
        };
        let mut fake = FakeConnection {
            info: Some(info(&root)),
            pending: Some(pending_lists()),
            recent: vec![description.summary.clone()],
            descriptions: HashMap::from([(number, description)]),
            ..FakeConnection::default()
        };
        fake.mappings.insert(
            included.into(),
            Ok(vec![mapping(included, &root.join("src/lib.rs"))]),
        );
        fake.mappings.insert(excluded.into(), Ok(Vec::new()));
        fake.contents
            .insert(format!("{included}#3"), b"before\n".to_vec());
        fake.contents
            .insert(format!("{included}#4"), b"after\n".to_vec());
        let fake = Arc::new(fake);
        let context = PerforceContext::discover_with(fake).unwrap();
        let source = context.submitted_source(&context.recent[0]).unwrap();

        let manifest = source.provider.load_manifest(&source.identity).unwrap();

        assert_eq!(manifest.files.len(), 1);
        let details = text_details(&manifest.files[0]);
        assert_eq!(details.baseline, b"before\n");
        assert_eq!(details.local, b"after\n");
        assert!(!details.editable);
        assert_eq!(details.save_destination, None);
    }

    #[test]
    fn submitted_actions_preserve_move_delete_and_binary_presentation() {
        let root = PathBuf::from("/work/client");
        let number = NonZeroU32::new(43).unwrap();
        let mut move_delete = changed("//depot/old.txt", 6, FileAction::MoveDelete);
        move_delete.moved_file = Some("//depot/new.txt".into());
        let mut move_add = changed("//depot/new.txt", 1, FileAction::MoveAdd);
        move_add.moved_file = Some("//depot/old.txt".into());
        let deleted = changed("//depot/deleted.txt", 3, FileAction::Delete);
        let mut binary = changed("//depot/image.bin", 2, FileAction::Edit);
        binary.file_type = Some("binary+l".into());
        let description = ChangelistDescription {
            summary: summary(
                ChangelistId::Number(number),
                ChangelistStatus::Submitted,
                "Action coverage",
            ),
            files: vec![move_delete, move_add, deleted, binary],
        };
        let mut fake = FakeConnection {
            info: Some(info(&root)),
            pending: Some(pending_lists()),
            recent: vec![description.summary.clone()],
            descriptions: HashMap::from([(number, description)]),
            ..FakeConnection::default()
        };
        for path in ["new.txt", "deleted.txt", "image.bin"] {
            let depot_path = format!("//depot/{path}");
            fake.mappings.insert(
                depot_path.clone(),
                Ok(vec![mapping(&depot_path, &root.join(path))]),
            );
        }
        fake.contents
            .insert("//depot/old.txt#5".into(), b"before move\n".to_vec());
        fake.contents
            .insert("//depot/new.txt#1".into(), b"after move\n".to_vec());
        fake.contents
            .insert("//depot/deleted.txt#2".into(), b"before delete\n".to_vec());
        let fake = Arc::new(fake);
        let context = PerforceContext::discover_with(fake).unwrap();
        let source = context.submitted_source(&context.recent[0]).unwrap();

        let manifest = source.provider.load_manifest(&source.identity).unwrap();

        assert_eq!(manifest.files.len(), 3);

        let renamed = &manifest.files[0];
        let renamed_text = text_details(renamed);
        assert_eq!(renamed.identity.to_string(), "//depot/new.txt");
        assert_eq!(renamed.logical_path, PathBuf::from("new.txt"));
        assert_eq!(
            renamed.status,
            ReviewFileStatus::Renamed {
                from: PathBuf::from("depot/old.txt")
            }
        );
        assert_eq!(renamed_text.baseline, b"before move\n");
        assert_eq!(renamed_text.local, b"after move\n");
        assert_eq!(renamed_text.baseline_path, PathBuf::from("depot/old.txt"));
        assert_eq!(renamed_text.local_path, PathBuf::from("new.txt"));
        assert!(!renamed_text.editable);
        assert_eq!(renamed_text.save_destination, None);

        let deleted = &manifest.files[1];
        let deleted_text = text_details(deleted);
        assert_eq!(deleted.identity.to_string(), "//depot/deleted.txt");
        assert_eq!(deleted.logical_path, PathBuf::from("deleted.txt"));
        assert_eq!(deleted.status, ReviewFileStatus::Deleted);
        assert_eq!(deleted_text.baseline, b"before delete\n");
        assert!(deleted_text.local.is_empty());
        assert_eq!(deleted_text.save_destination, None);

        let binary = &manifest.files[2];
        assert_eq!(binary.identity.to_string(), "//depot/image.bin");
        assert_eq!(binary.logical_path, PathBuf::from("image.bin"));
        assert_eq!(binary.status, ReviewFileStatus::Modified);
        assert!(matches!(
            binary.kind,
            super::super::model::ReviewFileKind::Binary { .. }
        ));
    }

    #[test]
    fn submitted_mapping_operational_failure_rejects_the_whole_manifest() {
        let root = PathBuf::from("/work/client");
        let number = NonZeroU32::new(44).unwrap();
        let included = "//depot/included.txt";
        let unavailable = "//depot/unavailable.txt";
        let description = ChangelistDescription {
            summary: summary(
                ChangelistId::Number(number),
                ChangelistStatus::Submitted,
                "Mixed mapping result",
            ),
            files: vec![
                changed(included, 1, FileAction::Add),
                changed(unavailable, 1, FileAction::Add),
            ],
        };
        let fake = Arc::new(FakeConnection {
            info: Some(info(&root)),
            pending: Some(pending_lists()),
            recent: vec![description.summary.clone()],
            descriptions: HashMap::from([(number, description)]),
            mappings: HashMap::from([
                (
                    included.into(),
                    Ok(vec![mapping(included, &root.join("included.txt"))]),
                ),
                (
                    unavailable.into(),
                    Err(ConnectionFailure {
                        kind: ErrorKind::Authentication,
                        message: "Perforce ticket expired; log in with an existing Perforce client, then retry"
                            .into(),
                    }),
                ),
            ]),
            contents: HashMap::from([(format!("{included}#1"), b"included\n".to_vec())]),
            ..FakeConnection::default()
        });
        let context = PerforceContext::discover_with(fake.clone()).unwrap();
        let source = context.submitted_source(&context.recent[0]).unwrap();

        let error = source.provider.load_manifest(&source.identity).unwrap_err();

        assert!(error.contains("ticket expired"));
        assert!(error.contains("log in with an existing Perforce client"));
        assert!(
            fake.calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| !call.starts_with("print:")),
            "mapping must complete before any partial manifest content is loaded"
        );
    }

    #[test]
    fn submitted_move_source_remains_a_deletion_when_destination_is_outside_view() {
        let root = PathBuf::from("/work/client");
        let number = NonZeroU32::new(45).unwrap();
        let old_path = "//depot/visible/old.txt";
        let new_path = "//depot/hidden/new.txt";
        let mut move_delete = changed(old_path, 6, FileAction::MoveDelete);
        move_delete.moved_file = Some(new_path.into());
        let mut move_add = changed(new_path, 1, FileAction::MoveAdd);
        move_add.moved_file = Some(old_path.into());
        let description = ChangelistDescription {
            summary: summary(
                ChangelistId::Number(number),
                ChangelistStatus::Submitted,
                "Move across client view",
            ),
            files: vec![move_delete, move_add],
        };
        let fake = Arc::new(FakeConnection {
            info: Some(info(&root)),
            pending: Some(pending_lists()),
            recent: vec![description.summary.clone()],
            descriptions: HashMap::from([(number, description)]),
            mappings: HashMap::from([
                (
                    old_path.into(),
                    Ok(vec![mapping(old_path, &root.join("visible/old.txt"))]),
                ),
                (
                    new_path.into(),
                    Err(ConnectionFailure {
                        kind: ErrorKind::Mapping,
                        message: "file(s) not in client view".into(),
                    }),
                ),
            ]),
            contents: HashMap::from([(format!("{old_path}#5"), b"before move\n".to_vec())]),
            ..FakeConnection::default()
        });
        let context = PerforceContext::discover_with(fake).unwrap();
        let source = context.submitted_source(&context.recent[0]).unwrap();

        let manifest = source.provider.load_manifest(&source.identity).unwrap();

        assert_eq!(manifest.files.len(), 1);
        assert_eq!(
            manifest.files[0].logical_path,
            PathBuf::from("visible/old.txt")
        );
        assert!(matches!(
            manifest.files[0].status,
            ReviewFileStatus::Deleted
        ));
        let details = text_details(&manifest.files[0]);
        assert_eq!(details.baseline, b"before move\n");
        assert!(details.local.is_empty());
        assert!(!details.editable);
        assert_eq!(details.save_destination, None);
    }

    #[test]
    fn direct_numbers_canonicalize_source_identity_and_reject_foreign_pending_changes() {
        let root = PathBuf::from("/work/client");
        let number = NonZeroU32::new(41).unwrap();
        let submitted = summary(
            ChangelistId::Number(number),
            ChangelistStatus::Submitted,
            "Submitted work",
        );
        let description = ChangelistDescription {
            summary: submitted.clone(),
            files: Vec::new(),
        };
        let fake = Arc::new(FakeConnection {
            info: Some(info(&root)),
            pending: Some(pending_lists()),
            recent: vec![submitted.clone()],
            descriptions: HashMap::from([(number, description)]),
            ..FakeConnection::default()
        });
        let context = PerforceContext::discover_with(fake).unwrap();

        let recent = context.submitted_source(&submitted).unwrap();
        let direct = context.source_for_number(number).unwrap();
        assert_eq!(recent.identity, direct.identity);

        let foreign_number = NonZeroU32::new(42).unwrap();
        let mut foreign = summary(
            ChangelistId::Number(foreign_number),
            ChangelistStatus::Pending,
            "Someone else's work",
        );
        foreign.client = "other-client".into();
        let foreign_context = PerforceContext::discover_with(Arc::new(FakeConnection {
            info: Some(info(&root)),
            pending: Some(pending_lists()),
            descriptions: HashMap::from([(
                foreign_number,
                ChangelistDescription {
                    summary: foreign,
                    files: Vec::new(),
                },
            )]),
            ..FakeConnection::default()
        }))
        .unwrap();

        let error = foreign_context
            .source_for_number(foreign_number)
            .err()
            .unwrap();
        assert!(error.contains("other-client"));
        assert!(error.contains("robin-yori"));
    }

    #[test]
    fn mapping_and_connection_errors_remain_actionable() {
        let root = PathBuf::from("/work/client");
        let number = NonZeroU32::new(41).unwrap();
        let depot_path = "//depot/outside.txt";
        let description = ChangelistDescription {
            summary: summary(
                ChangelistId::Number(number),
                ChangelistStatus::Submitted,
                "Outside view",
            ),
            files: vec![changed(depot_path, 1, FileAction::Add)],
        };
        let fake = Arc::new(FakeConnection {
            info: Some(info(&root)),
            pending: Some(pending_lists()),
            recent: vec![description.summary.clone()],
            descriptions: HashMap::from([(number, description)]),
            mappings: HashMap::from([(
                depot_path.into(),
                Err(ConnectionFailure {
                    kind: ErrorKind::Mapping,
                    message: "file(s) not in client view; check the active P4CLIENT view".into(),
                }),
            )]),
            ..FakeConnection::default()
        });
        let context = PerforceContext::discover_with(fake).unwrap();
        let source = context.submitted_source(&context.recent[0]).unwrap();

        let error = source.provider.load_manifest(&source.identity).unwrap_err();

        assert!(error.contains("client view"));
        assert!(!error.to_lowercase().contains("password prompt"));
    }
}
