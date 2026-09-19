use std::{collections::BTreeMap, path::PathBuf};

use crate::{
    ChangedFile, ChangelistDescription, ChangelistId, ChangelistStatus, ChangelistSummary,
    ClientInfo, Error, FileAction, HaveRevision, OpenedFile, PendingChangelists, RawRecord,
    RawResult, Result, WorkspaceMapping,
};

struct Record<'a> {
    fields: BTreeMap<&'a str, &'a [u8]>,
}

impl<'a> Record<'a> {
    fn new(record: &'a RawRecord) -> Self {
        Self {
            fields: record
                .fields
                .iter()
                .map(|field| (field.name.as_str(), field.value.as_slice()))
                .collect(),
        }
    }

    fn text(&self, name: &str) -> Option<String> {
        self.fields
            .get(name)
            .map(|value| String::from_utf8_lossy(value).into_owned())
    }

    fn required_text(&self, name: &str) -> Result<String> {
        self.text(name)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::invalid_response(format!("Perforce response omitted {name}")))
    }

    fn number<T>(&self, name: &str) -> Result<Option<T>>
    where
        T: std::str::FromStr,
    {
        self.text(name)
            .map(|value| {
                value.parse().map_err(|_| {
                    Error::invalid_response(format!(
                        "Perforce returned an invalid {name} value: {value}"
                    ))
                })
            })
            .transpose()
    }

    fn required_number<T>(&self, name: &str) -> Result<T>
    where
        T: std::str::FromStr,
    {
        self.number(name)?
            .ok_or_else(|| Error::invalid_response(format!("Perforce response omitted {name}")))
    }

    fn indexed_text(&self, name: &str, index: usize) -> Option<String> {
        self.text(&format!("{name}{index}"))
    }

    fn indexed_number<T>(&self, name: &str, index: usize) -> Result<Option<T>>
    where
        T: std::str::FromStr,
    {
        self.number(&format!("{name}{index}"))
    }

    fn required_indexed_number<T>(&self, name: &str, index: usize) -> Result<T>
    where
        T: std::str::FromStr,
    {
        self.required_number(&format!("{name}{index}"))
    }
}

pub(crate) fn check_result(result: &RawResult) -> Result<()> {
    Error::from_messages(&result.messages).map_or(Ok(()), Err)
}

pub(crate) fn lifecycle_result(result: &RawResult, phase: &str) -> Result<()> {
    if result.messages.iter().any(|message| message.severity >= 3) {
        Err(Error::lifecycle(phase, &result.messages))
    } else {
        Ok(())
    }
}

pub(crate) fn client_info(result: &RawResult) -> Result<ClientInfo> {
    check_result(result)?;
    let record = result
        .records
        .first()
        .map(Record::new)
        .ok_or_else(|| Error::invalid_response("Perforce info returned no records"))?;

    Ok(ClientInfo {
        server_address: record.required_text("serverAddress")?,
        server_version: record.required_text("serverVersion")?,
        user_name: record.required_text("userName")?,
        client_name: record.required_text("clientName")?,
        client_root: record.text("clientRoot").map(PathBuf::from),
        current_directory: PathBuf::from(record.required_text("clientCwd")?),
        case_handling: record.text("caseHandling"),
        unicode_enabled: record.text("unicode") == Some("enabled".to_owned()),
    })
}

pub(crate) fn pending_changelists(
    result: &RawResult,
    info: &ClientInfo,
) -> Result<PendingChangelists> {
    let numbered = changelist_summaries(result, ChangelistStatus::Pending)?;

    Ok(PendingChangelists {
        default: ChangelistSummary {
            id: ChangelistId::Default,
            status: ChangelistStatus::Pending,
            description: "Default changelist".to_owned(),
            user: info.user_name.clone(),
            client: info.client_name.clone(),
            modified_unix_seconds: None,
        },
        numbered,
    })
}

pub(crate) fn submitted_changelists(result: &RawResult) -> Result<Vec<ChangelistSummary>> {
    changelist_summaries(result, ChangelistStatus::Submitted)
}

fn changelist_summaries(
    result: &RawResult,
    expected_status: ChangelistStatus,
) -> Result<Vec<ChangelistSummary>> {
    check_result(result)?;
    result
        .records
        .iter()
        .map(|raw| {
            let record = Record::new(raw);
            let id = record.required_text("change")?.parse().map_err(|_| {
                Error::invalid_response("Perforce returned an invalid changelist identifier")
            })?;
            let status = match record.text("status") {
                None => expected_status,
                Some(value) => parse_status(Some(&value)).ok_or_else(|| {
                    Error::invalid_response("Perforce returned an invalid changelist status")
                })?,
            };

            Ok(ChangelistSummary {
                id,
                status,
                description: record.required_text("desc")?,
                user: record.required_text("user")?,
                client: record.required_text("client")?,
                modified_unix_seconds: record.number("time")?,
            })
        })
        .collect()
}

pub(crate) fn opened_files(result: &RawResult) -> Result<Vec<OpenedFile>> {
    check_result(result)?;
    result
        .records
        .iter()
        .map(|raw| {
            let record = Record::new(raw);
            let changelist = record
                .text("change")
                .unwrap_or_else(|| "default".to_owned())
                .parse()
                .map_err(|_| Error::invalid_response("opened file has an invalid changelist"))?;

            Ok(OpenedFile {
                depot_path: record.required_text("depotFile")?,
                client_path: record.text("clientFile"),
                local_path: record.text("path").map(PathBuf::from),
                moved_file: record.text("movedFile"),
                revision: record.number("rev")?,
                have_revision: record.number("haveRev")?,
                action: FileAction::from(record.required_text("action")?.as_str()),
                changelist,
                file_type: record.text("type"),
            })
        })
        .collect()
}

pub(crate) fn changelist_description(result: &RawResult) -> Result<ChangelistDescription> {
    check_result(result)?;
    let record = result
        .records
        .first()
        .map(Record::new)
        .ok_or_else(|| Error::invalid_response("Perforce describe returned no records"))?;
    let status = parse_status(record.text("status").as_deref()).ok_or_else(|| {
        Error::invalid_response("Perforce describe returned an invalid changelist status")
    })?;
    let summary = ChangelistSummary {
        id: record.required_text("change")?.parse().map_err(|_| {
            Error::invalid_response("Perforce describe returned an invalid changelist identifier")
        })?,
        status,
        description: record.required_text("desc")?,
        user: record.required_text("user")?,
        client: record.required_text("client")?,
        modified_unix_seconds: record.number("time")?,
    };
    let mut files = Vec::new();

    for index in 0.. {
        let Some(depot_path) = record.indexed_text("depotFile", index) else {
            break;
        };

        let revision = record.required_indexed_number("rev", index)?;
        let action = record
            .indexed_text("action", index)
            .ok_or_else(|| Error::invalid_response("described file omitted its action"))?;

        files.push(ChangedFile {
            depot_path,
            moved_file: record.indexed_text("movedFile", index),
            revision,
            action: FileAction::from(action.as_str()),
            file_type: record.indexed_text("type", index),
            file_size: record.indexed_number("fileSize", index)?,
            digest: record.indexed_text("digest", index),
        });
    }

    Ok(ChangelistDescription { summary, files })
}

pub(crate) fn have_revisions(result: &RawResult) -> Result<Vec<HaveRevision>> {
    check_result(result)?;
    result
        .records
        .iter()
        .map(|raw| {
            let record = Record::new(raw);

            Ok(HaveRevision {
                depot_path: record.required_text("depotFile")?,
                client_path: record.required_text("clientFile")?,
                local_path: PathBuf::from(record.required_text("path")?),
                revision: record.required_number("haveRev")?,
            })
        })
        .collect()
}

pub(crate) fn workspace_mappings(result: &RawResult) -> Result<Vec<WorkspaceMapping>> {
    check_result(result)?;
    let mappings = result
        .records
        .iter()
        .map(|raw| {
            let record = Record::new(raw);

            Ok(WorkspaceMapping {
                depot_path: record.required_text("depotFile")?,
                client_path: record.required_text("clientFile")?,
                local_path: PathBuf::from(record.required_text("path")?),
                is_exclusion: record.fields.contains_key("unmap"),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    if mappings.iter().all(|mapping| mapping.is_exclusion) {
        return Err(Error::no_effective_mapping());
    }

    Ok(mappings)
}

pub(crate) fn depot_content(result: &RawResult) -> Result<Vec<u8>> {
    check_result(result)?;
    Ok(result.output.clone())
}

fn parse_status(value: Option<&str>) -> Option<ChangelistStatus> {
    match value {
        Some("pending") => Some(ChangelistStatus::Pending),
        Some("submitted") => Some(ChangelistStatus::Submitted),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RawField, RawMessage};

    fn record(fields: &[(&str, &str)]) -> RawRecord {
        RawRecord {
            fields: fields
                .iter()
                .map(|(name, value)| RawField {
                    name: (*name).to_owned(),
                    value: value.as_bytes().to_vec(),
                })
                .collect(),
        }
    }

    fn result(records: Vec<RawRecord>) -> RawResult {
        RawResult {
            records,
            messages: Vec::new(),
            output: Vec::new(),
        }
    }

    #[test]
    fn parses_client_info() {
        let result = result(vec![record(&[
            ("serverAddress", "ssl:perforce.example:1666"),
            ("serverVersion", "P4D/LINUX26X86_64/2025.1/123456"),
            ("userName", "robin"),
            ("clientName", "robin-yori"),
            ("clientRoot", "/work/robin-yori"),
            ("clientCwd", "/work/robin-yori/src"),
            ("caseHandling", "sensitive"),
            ("unicode", "enabled"),
        ])]);

        let parsed = client_info(&result).unwrap();

        assert_eq!(parsed.user_name, "robin");
        assert_eq!(parsed.client_name, "robin-yori");
        assert_eq!(parsed.client_root, Some(PathBuf::from("/work/robin-yori")));
        assert!(parsed.unicode_enabled);
    }

    #[test]
    fn parses_pending_and_submitted_changelists() {
        let info = ClientInfo {
            server_address: "perforce:1666".to_owned(),
            server_version: "P4D/test".to_owned(),
            user_name: "robin".to_owned(),
            client_name: "robin-yori".to_owned(),
            client_root: None,
            current_directory: PathBuf::from("/work"),
            case_handling: None,
            unicode_enabled: false,
        };
        let pending = result(vec![record(&[
            ("change", "42"),
            ("status", "pending"),
            ("desc", "Review API"),
            ("user", "robin"),
            ("client", "robin-yori"),
            ("time", "1735689600"),
        ])]);

        let pending = pending_changelists(&pending, &info).unwrap();

        assert_eq!(pending.default.id, ChangelistId::Default);
        assert_eq!(pending.numbered[0].id.to_string(), "42");
        assert_eq!(pending.numbered[0].status, ChangelistStatus::Pending);

        let submitted = result(vec![record(&[
            ("change", "41"),
            ("status", "submitted"),
            ("desc", "Previous API"),
            ("user", "robin"),
            ("client", "robin-yori"),
        ])]);
        assert_eq!(
            submitted_changelists(&submitted).unwrap()[0].status,
            ChangelistStatus::Submitted
        );
    }

    #[test]
    fn parses_opened_files_and_unknown_actions() {
        let result = result(vec![record(&[
            ("depotFile", "//depot/src/lib.rs"),
            ("clientFile", "//robin-yori/src/lib.rs"),
            ("path", "/work/src/lib.rs"),
            ("movedFile", "//depot/src/old-lib.rs"),
            ("rev", "8"),
            ("haveRev", "7"),
            ("action", "custom-action"),
            ("change", "default"),
            ("type", "text"),
        ])]);

        let file = opened_files(&result).unwrap().remove(0);

        assert_eq!(file.changelist, ChangelistId::Default);
        assert_eq!(file.moved_file.as_deref(), Some("//depot/src/old-lib.rs"));
        assert_eq!(file.have_revision, Some(7));
        assert_eq!(file.action, FileAction::Unknown("custom-action".to_owned()));
    }

    #[test]
    fn parses_indexed_describe_records() {
        let result = result(vec![record(&[
            ("change", "41"),
            ("status", "submitted"),
            ("desc", "Ship it"),
            ("user", "robin"),
            ("client", "robin-yori"),
            ("time", "1735689600"),
            ("depotFile0", "//depot/a.txt"),
            ("rev0", "3"),
            ("action0", "edit"),
            ("type0", "text"),
            ("fileSize0", "12"),
            ("digest0", "ABCDEF"),
            ("depotFile1", "//depot/b.bin"),
            ("movedFile1", "//depot/old-b.bin"),
            ("rev1", "1"),
            ("action1", "add"),
            ("type1", "binary"),
        ])]);

        let description = changelist_description(&result).unwrap();

        assert_eq!(description.files.len(), 2);
        assert_eq!(description.files[0].revision, 3);
        assert_eq!(description.files[0].file_size, Some(12));
        assert_eq!(description.files[1].action, FileAction::Add);
        assert_eq!(
            description.files[1].moved_file.as_deref(),
            Some("//depot/old-b.bin")
        );
    }

    #[test]
    fn parses_have_and_preserves_inclusive_and_exclusion_mappings() {
        let have = result(vec![record(&[
            ("depotFile", "//depot/a.txt"),
            ("clientFile", "//client/a.txt"),
            ("path", "/work/a.txt"),
            ("haveRev", "3"),
        ])]);
        let where_result = result(vec![
            record(&[
                ("depotFile", "//depot/a.txt"),
                ("clientFile", "//client/a.txt"),
                ("path", "/work/a.txt"),
            ]),
            record(&[
                ("depotFile", "//depot/excluded/..."),
                ("clientFile", "//client/excluded/..."),
                ("path", "/work/excluded/..."),
                ("unmap", ""),
            ]),
        ]);

        assert_eq!(have_revisions(&have).unwrap()[0].revision, 3);

        let mappings = workspace_mappings(&where_result).unwrap();
        assert_eq!(mappings.len(), 2);
        assert!(!mappings[0].is_exclusion);
        assert!(mappings[1].is_exclusion);
    }

    #[test]
    fn exclusion_only_mapping_is_actionable() {
        let excluded = result(vec![record(&[
            ("depotFile", "//depot/excluded/file.txt"),
            ("clientFile", "//client/excluded/file.txt"),
            ("path", "/work/excluded/file.txt"),
            ("unmap", ""),
        ])]);

        let error = workspace_mappings(&excluded).unwrap_err();

        assert_eq!(error.kind(), crate::ErrorKind::Mapping);
        assert!(error.to_string().contains("exclusion entries"));
    }

    #[test]
    fn depot_content_preserves_arbitrary_bytes() {
        let mut result = result(Vec::new());
        result.output = vec![0x00, 0x7f, 0x80, 0xff];

        assert_eq!(depot_content(&result).unwrap(), result.output);
    }

    #[test]
    fn rejects_missing_required_fields_and_command_errors() {
        let missing = result(vec![record(&[("userName", "robin")])]);
        assert_eq!(
            client_info(&missing).unwrap_err().kind(),
            crate::ErrorKind::InvalidResponse
        );

        let failed = RawResult {
            records: Vec::new(),
            messages: vec![RawMessage {
                severity: 3,
                generic: 0x26,
                text: b"Connect to server failed".to_vec(),
            }],
            output: Vec::new(),
        };
        assert_eq!(
            opened_files(&failed).unwrap_err().kind(),
            crate::ErrorKind::Connectivity
        );
    }
}
