//! Content-faithful file snapshots and guarded atomic replacement, independent of GPUI.

mod watch;
pub(crate) use watch::FileWatch;

#[cfg(test)]
mod tests;

use atomicwrites::{AllowOverwrite, AtomicFile, DisallowOverwrite, Error as AtomicWriteError};
use std::{
    fs::{self, File, Metadata, Permissions},
    io::{Read, Write},
    path::Path,
    sync::Arc,
    time::SystemTime,
};
use yori_document::Document;

#[cfg(test)]
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

#[cfg(test)]
static SAVE_DELAYS: OnceLock<Mutex<HashMap<PathBuf, async_channel::Receiver<()>>>> =
    OnceLock::new();

#[cfg(test)]
pub(crate) fn delay_next_save(path: &Path) -> async_channel::Sender<()> {
    let (release, delay) = async_channel::bounded(1);
    let previous = SAVE_DELAYS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(path.to_path_buf(), delay);
    assert!(previous.is_none(), "save delay already installed for path");

    release
}

#[cfg(test)]
fn wait_for_save_delay(path: &Path) {
    let delay = SAVE_DELAYS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(path);
    if let Some(delay) = delay {
        delay
            .recv_blocking()
            .expect("delayed save was dropped without release");
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FileVersion {
    bytes: Arc<[u8]>,
    permissions: Permissions,
}

impl PartialEq for FileVersion {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Eq for FileVersion {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Snapshot {
    Missing,
    File(FileVersion),
}

impl Snapshot {
    pub fn read(path: &Path) -> Result<Self, String> {
        read_snapshot(path).map_err(|error| format!("cannot inspect {}: {error}", path.display()))
    }

    pub fn document(&self, path: &Path) -> Result<Document, String> {
        let Self::File(file) = self else {
            return Err(format!("file no longer exists: {}", path.display()));
        };

        Document::from_bytes(file.bytes.to_vec())
            .map_err(|error| format!("cannot open {}: {error}", path.display()))
    }

    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    readonly: bool,
}

impl From<&Metadata> for FileStamp {
    fn from(metadata: &Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            readonly: metadata.permissions().readonly(),
        }
    }
}

fn regular_file_metadata(path: &Path) -> Result<Option<Metadata>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err("refusing to follow a symbolic link".into())
        }
        Ok(metadata) if metadata.is_file() => Ok(Some(metadata)),
        Ok(_) => Err("not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn read_snapshot(path: &Path) -> Result<Snapshot, String> {
    let Some(path_before) = regular_file_metadata(path)? else {
        return Ok(Snapshot::Missing);
    };

    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let before = file.metadata().map_err(|error| error.to_string())?;
    if !before.is_file() {
        return Err("not a regular file".into());
    }

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let after = file.metadata().map_err(|error| error.to_string())?;
    let Some(path_after) = regular_file_metadata(path)? else {
        return Err("file changed while it was being read; retry".into());
    };

    if FileStamp::from(&path_before) != FileStamp::from(&before)
        || FileStamp::from(&before) != FileStamp::from(&after)
        || FileStamp::from(&after) != FileStamp::from(&path_after)
        || after.len() != bytes.len() as u64
    {
        return Err("file changed while it was being read; retry".into());
    }

    Ok(Snapshot::File(FileVersion {
        bytes: bytes.into(),
        permissions: after.permissions(),
    }))
}

#[derive(Debug)]
pub(crate) enum SaveError {
    Changed(Snapshot),
    Failed(String),
}

impl From<String> for SaveError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

#[derive(Debug)]
enum StageError {
    Changed(Snapshot),
    Failed(String),
}

/// Compare against the contents the user loaded or explicitly approved, then
/// publish the complete replacement in one atomic operation.
pub(crate) fn save(path: &Path, expected: &Snapshot, bytes: &[u8]) -> Result<Snapshot, SaveError> {
    #[cfg(test)]
    wait_for_save_delay(path);

    save_with_before_replace(path, expected, bytes, || {})
}

fn save_with_before_replace(
    path: &Path,
    expected: &Snapshot,
    bytes: &[u8],
    before_replace: impl FnOnce(),
) -> Result<Snapshot, SaveError> {
    let current = Snapshot::read(path)?;
    if &current != expected {
        return Err(SaveError::Changed(current));
    }
    if let Snapshot::File(file) = &current
        && file.bytes.as_ref() == bytes
    {
        return Ok(current);
    }
    ensure_writable(&current)?;

    let permissions = match &current {
        Snapshot::Missing => None,
        Snapshot::File(file) => Some(file.permissions.clone()),
    };
    let overwrite = match expected {
        Snapshot::Missing => DisallowOverwrite,
        Snapshot::File(_) => AllowOverwrite,
    };
    let atomic = AtomicFile::new(path, overwrite);
    let result = atomic.write(|temporary| {
        temporary
            .write_all(bytes)
            .map_err(|error| StageError::Failed(error.to_string()))?;
        if let Some(permissions) = permissions {
            temporary
                .set_permissions(permissions)
                .map_err(|error| StageError::Failed(error.to_string()))?;
        }

        before_replace();
        let current = Snapshot::read(path).map_err(StageError::Failed)?;
        if &current != expected {
            return Err(StageError::Changed(current));
        }
        ensure_writable(&current).map_err(StageError::Failed)?;

        Ok(())
    });

    match result {
        Ok(()) => {}
        Err(AtomicWriteError::Internal(error)) => {
            return Err(SaveError::Failed(error.to_string()));
        }
        Err(AtomicWriteError::User(StageError::Changed(snapshot))) => {
            return Err(SaveError::Changed(snapshot));
        }
        Err(AtomicWriteError::User(StageError::Failed(error))) => {
            return Err(SaveError::Failed(error));
        }
    }

    let actual = Snapshot::read(path)?;
    if let Snapshot::File(file) = &actual
        && file.bytes.as_ref() == bytes
    {
        return Ok(actual);
    }

    Err(SaveError::Failed(
        "destination changed immediately after saving; your editor contents are still retained"
            .into(),
    ))
}

fn ensure_writable(snapshot: &Snapshot) -> Result<(), String> {
    if let Snapshot::File(file) = snapshot
        && file.permissions.readonly()
    {
        return Err("file is read-only; check it out or change its permissions first".into());
    }

    Ok(())
}
