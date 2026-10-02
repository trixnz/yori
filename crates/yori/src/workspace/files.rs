//! Per-tab disk versions: only real content sources and save destinations are tracked.

use crate::{
    comparison::{Comparison, ComparisonDocument, DocumentContent},
    storage::Snapshot,
};
use std::path::{Path, PathBuf};
use yori_document::Document;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Baseline,
    Local,
    Base,
    Incoming,
    Result,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Self::Baseline => "Baseline",
            Self::Local => "Local",
            Self::Base => "Base",
            Self::Incoming => "Incoming",
            Self::Result => "Result",
        }
    }
}

pub(crate) struct TrackedFile {
    pub role: Role,
    pub path: PathBuf,
    pub accepted: Snapshot,
    pub current: Result<Snapshot, String>,
    source: bool,
    destination: bool,
    dismissed: Option<Result<Snapshot, String>>,
}

impl TrackedFile {
    pub fn reloadable(&self) -> bool {
        self.source
    }
}

pub(crate) struct Files {
    pub entries: Vec<TrackedFile>,
    documents: Vec<(Role, Document)>,
}

impl Files {
    pub(crate) fn load(comparison: &Comparison) -> Result<Self, String> {
        let mut files = Self {
            entries: Vec::new(),
            documents: Vec::new(),
        };

        match comparison {
            Comparison::Diff(diff) => {
                files.load_document(Role::Baseline, &diff.baseline)?;
                files.load_document(Role::Local, &diff.local)?;
            }
            Comparison::Merge(merge) => {
                files.load_document(Role::Base, &merge.base)?;
                files.load_document(Role::Local, &merge.local)?;
                files.load_document(Role::Incoming, &merge.incoming)?;
                files.track_destination(Role::Result, &merge.result)?;
            }
        }

        Ok(files)
    }

    fn load_document(&mut self, role: Role, descriptor: &ComparisonDocument) -> Result<(), String> {
        let document = match descriptor.content() {
            DocumentContent::File(path) => {
                self.load_file_document(role, path, descriptor.save_destination() == Some(path))?;
                None
            }
            DocumentContent::Memory(bytes) => {
                Some(Document::from_bytes(bytes.to_vec()).map_err(|error| {
                    format!(
                        "cannot open {}: {error}",
                        descriptor.logical_path().display()
                    )
                })?)
            }
        };

        if let Some(document) = document {
            self.documents.push((role, document));
        }

        if let Some(destination) = descriptor.save_destination()
            && !self
                .entries
                .iter()
                .any(|file| file.role == role && file.path == destination)
        {
            self.track_destination(role, destination)?;
        }

        Ok(())
    }

    fn load_file_document(
        &mut self,
        role: Role,
        path: &Path,
        destination: bool,
    ) -> Result<(), String> {
        let accepted = Snapshot::read(path)?;
        let document = accepted.document(path)?;

        self.documents.push((role, document));
        self.entries.push(TrackedFile {
            role,
            path: path.to_owned(),
            current: Ok(accepted.clone()),
            accepted,
            source: true,
            destination,
            dismissed: None,
        });

        Ok(())
    }

    fn track_destination(&mut self, role: Role, path: &Path) -> Result<(), String> {
        if let Some(file) = self
            .entries
            .iter_mut()
            .find(|file| file.role == role && file.path == path)
        {
            file.destination = true;
            return Ok(());
        }

        let accepted = Snapshot::read(path)?;
        self.entries.push(TrackedFile {
            role,
            path: path.to_owned(),
            current: Ok(accepted.clone()),
            accepted,
            source: false,
            destination: true,
            dismissed: None,
        });

        Ok(())
    }

    pub(crate) fn document(&self, role: Role) -> &Document {
        &self
            .documents
            .iter()
            .find(|(document_role, _)| *document_role == role)
            .expect("role has loaded document content")
            .1
    }

    pub fn file(&self, role: Role) -> &TrackedFile {
        self.entries
            .iter()
            .find(|file| file.role == role && file.source)
            .expect("role has a file-backed content source")
    }

    pub fn tracked(&self, role: Role, path: &Path) -> &TrackedFile {
        self.entries
            .iter()
            .find(|file| file.role == role && file.path == path)
            .expect("role and path identify a tracked file")
    }

    pub fn notice(&self) -> Option<&TrackedFile> {
        self.entries.iter().find(|file| {
            let missing_input = file.source
                && !file.destination
                && file.current.as_ref().is_ok_and(Snapshot::is_missing);
            !missing_input
                && file.current != Ok(file.accepted.clone())
                && file.dismissed.as_ref() != Some(&file.current)
        })
    }

    pub(crate) fn destination(&self) -> Option<&TrackedFile> {
        self.entries.iter().find(|file| file.destination)
    }

    pub fn dismiss(&mut self, role: Role, path: &Path, observed: Result<Snapshot, String>) {
        if let Some(file) = self
            .entries
            .iter_mut()
            .find(|file| file.role == role && file.path == path)
        {
            // A newer change while the dialog was open still needs its own decision.
            file.dismissed = Some(observed);
        }
    }

    pub fn accept(&mut self, role: Role, snapshot: Snapshot) {
        if let Some(file) = self
            .entries
            .iter_mut()
            .find(|file| file.role == role && file.source)
        {
            file.accepted = snapshot.clone();
            file.current = Ok(snapshot);
            file.dismissed = None;
        }
    }

    pub(crate) fn saved(&mut self, snapshot: &Snapshot) {
        let Some(path) = self.destination().map(|file| file.path.clone()) else {
            return;
        };

        // If a destination aliases an input, keep that input's immutable editor
        // snapshot but don't report our own save as somebody else's incoming change.
        for file in &mut self.entries {
            if file.path == path {
                file.accepted = snapshot.clone();
                file.current = Ok(snapshot.clone());
                file.dismissed = None;
            }
        }
    }
}
