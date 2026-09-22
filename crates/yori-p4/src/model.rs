use std::{fmt, num::NonZeroU32, path::PathBuf, str::FromStr};

/// Ambient connection and workspace context reported by the Perforce server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientInfo {
    pub server_address: String,
    pub server_version: String,
    pub user_name: String,
    pub client_name: String,
    pub client_root: Option<PathBuf>,
    pub current_directory: PathBuf,
    pub case_handling: Option<String>,
    pub unicode_enabled: bool,
}

/// A numbered changelist or the current client's default changelist.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ChangelistId {
    Default,
    Number(NonZeroU32),
}

impl fmt::Display for ChangelistId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => formatter.write_str("default"),
            Self::Number(number) => number.fmt(formatter),
        }
    }
}

impl FromStr for ChangelistId {
    type Err = ParseChangelistIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "default" {
            return Ok(Self::Default);
        }

        value
            .parse::<NonZeroU32>()
            .map(Self::Number)
            .map_err(|_| ParseChangelistIdError(value.to_owned()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseChangelistIdError(String);

impl fmt::Display for ParseChangelistIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid changelist identifier: {}", self.0)
    }
}

impl std::error::Error for ParseChangelistIdError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangelistStatus {
    Pending,
    Submitted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangelistSummary {
    pub id: ChangelistId,
    pub status: ChangelistStatus,
    pub description: String,
    pub user: String,
    pub client: String,
    pub modified_unix_seconds: Option<u64>,
}

/// Pending changelists visible in the ambient client context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingChangelists {
    pub default: ChangelistSummary,
    pub numbered: Vec<ChangelistSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenedFile {
    pub depot_path: String,
    pub client_path: Option<String>,
    pub local_path: Option<PathBuf>,
    pub moved_file: Option<String>,
    pub revision: Option<u32>,
    pub have_revision: Option<u32>,
    pub action: FileAction,
    pub changelist: ChangelistId,
    pub file_type: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangelistDescription {
    pub summary: ChangelistSummary,
    pub files: Vec<ChangedFile>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangedFile {
    pub depot_path: String,
    pub moved_file: Option<String>,
    pub revision: u32,
    pub action: FileAction,
    pub file_type: Option<String>,
    pub file_size: Option<u64>,
    pub digest: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileAction {
    Add,
    Archive,
    Branch,
    Delete,
    Edit,
    Import,
    Integrate,
    MoveAdd,
    MoveDelete,
    Purge,
    Unknown(String),
}

impl From<&str> for FileAction {
    fn from(value: &str) -> Self {
        match value {
            "add" => Self::Add,
            "archive" => Self::Archive,
            "branch" => Self::Branch,
            "delete" => Self::Delete,
            "edit" => Self::Edit,
            "import" => Self::Import,
            "integrate" => Self::Integrate,
            "move/add" => Self::MoveAdd,
            "move/delete" => Self::MoveDelete,
            "purge" => Self::Purge,
            other => Self::Unknown(other.to_owned()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HaveRevision {
    pub depot_path: String,
    pub client_path: String,
    pub local_path: PathBuf,
    pub revision: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceMapping {
    pub depot_path: String,
    pub client_path: String,
    pub local_path: PathBuf,
    pub is_exclusion: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DepotRevision {
    pub depot_path: String,
    pub revision: NonZeroU32,
}

impl DepotRevision {
    pub(crate) fn new(depot_path: String, revision: u32) -> crate::Result<Self> {
        let revision = NonZeroU32::new(revision)
            .ok_or_else(|| crate::Error::invalid_response("p4 print returned revision zero"))?;

        Ok(Self {
            depot_path,
            revision,
        })
    }
}

impl fmt::Display for DepotRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}#{}", self.depot_path, self.revision)
    }
}
