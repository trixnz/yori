use std::{borrow::Cow, fmt, process::ExitStatus};

use crate::RawMessage;

const EV_PROTECT: i32 = 0x06;
const EV_CONFIG: i32 = 0x24;
const EV_COMM: i32 = 0x26;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Authentication,
    Trust,
    Connectivity,
    Mapping,
    Configuration,
    Permission,
    Command,
    Cancelled,
    WorkerStopped,
    InvalidResponse,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    kind: ErrorKind,
    message: String,
    remedy: Option<&'static str>,
}

impl Error {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    #[must_use]
    pub fn remedy(&self) -> Option<&'static str> {
        self.remedy
    }

    pub(crate) fn cancelled() -> Self {
        Self {
            kind: ErrorKind::Cancelled,
            message: "the Perforce request was cancelled".to_owned(),
            remedy: None,
        }
    }

    pub(crate) fn worker_stopped() -> Self {
        Self {
            kind: ErrorKind::WorkerStopped,
            message: "the Perforce worker is no longer available".to_owned(),
            remedy: Some("create a new Perforce client and retry"),
        }
    }

    pub(crate) fn worker_start_failed(error: &std::io::Error) -> Self {
        Self {
            kind: ErrorKind::WorkerStopped,
            message: format!("cannot start the Perforce worker: {error}"),
            remedy: Some("check system thread limits and retry"),
        }
    }

    pub(crate) fn invalid_response(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::InvalidResponse,
            message: message.into(),
            remedy: Some("check that the server and yori use compatible Perforce versions"),
        }
    }

    pub(crate) fn process_start_failed(executable: &str, error: &std::io::Error) -> Self {
        Self {
            kind: ErrorKind::Configuration,
            message: format!("cannot run Perforce command-line client {executable}: {error}"),
            remedy: Some("install P4 CLI and ensure p4 is on PATH"),
        }
    }

    pub(crate) fn process_io_failed(operation: &'static str) -> Self {
        Self {
            kind: ErrorKind::Command,
            message: format!("cannot {operation}"),
            remedy: Some("check the Perforce command details and retry"),
        }
    }

    pub(crate) fn process_failed(command: &str, status: ExitStatus) -> Self {
        Self {
            kind: ErrorKind::Command,
            message: format!("p4 {command} failed with {status}"),
            remedy: Some("check the Perforce command details and retry"),
        }
    }

    pub(crate) fn from_command_output(output: &[u8]) -> Self {
        Self::from_message(&RawMessage {
            severity: 3,
            generic: 0,
            text: output.to_vec(),
        })
    }

    pub(crate) fn no_effective_mapping() -> Self {
        Self {
            kind: ErrorKind::Mapping,
            message:
                "the requested path has no effective mapping in the active Perforce client view"
                    .to_owned(),
            remedy: Some("check inclusion and exclusion entries in the active P4CLIENT view"),
        }
    }

    pub(crate) fn from_messages(messages: &[RawMessage]) -> Option<Self> {
        messages
            .iter()
            .find(|message| message.severity >= 3)
            .map(Self::from_message)
    }

    pub(crate) fn from_command_messages(messages: &[RawMessage]) -> Option<Self> {
        messages
            .iter()
            .find(|message| message.severity >= 2)
            .map(Self::from_message)
    }

    fn from_message(message: &RawMessage) -> Self {
        let text = message_text(message);
        let normalized = text.to_ascii_lowercase();
        let (kind, remedy) = if normalized.contains("ssl")
            && (normalized.contains("trust") || normalized.contains("fingerprint"))
        {
            (
                ErrorKind::Trust,
                Some("establish trust with an existing Perforce client, then retry"),
            )
        } else if normalized.contains("not logged in")
            || normalized.contains("password")
            || normalized.contains("ticket expired")
            || normalized.contains("login required")
        {
            (
                ErrorKind::Authentication,
                Some("log in with an existing Perforce client, then retry"),
            )
        } else if message.generic == EV_COMM
            || normalized.contains("connect to server failed")
            || normalized.contains("tcp connect")
            || normalized.contains("connection refused")
        {
            (
                ErrorKind::Connectivity,
                Some("check P4PORT, network access, and the Perforce server status"),
            )
        } else if normalized.contains("client view")
            || normalized.contains("not in client")
            || normalized.contains("not under client's root")
        {
            (
                ErrorKind::Mapping,
                Some("check the active P4CLIENT view and workspace root"),
            )
        } else if message.generic == EV_CONFIG {
            (
                ErrorKind::Configuration,
                Some("check ambient P4CONFIG, P4PORT, P4USER, and P4CLIENT settings"),
            )
        } else if message.generic == EV_PROTECT {
            (
                ErrorKind::Permission,
                Some("check the active Perforce user's protections"),
            )
        } else {
            (
                ErrorKind::Command,
                Some("check the Perforce command details and retry"),
            )
        };

        Self {
            kind,
            message: text.trim().to_owned(),
            remedy,
        }
    }
}

fn message_text(message: &RawMessage) -> Cow<'_, str> {
    String::from_utf8_lossy(&message.text)
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)?;

        if let Some(remedy) = self.remedy {
            write!(formatter, "; {remedy}")?;
        }

        Ok(())
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    fn message(generic: i32, text: &str) -> RawMessage {
        RawMessage {
            severity: 3,
            generic,
            text: text.as_bytes().to_vec(),
        }
    }

    #[test]
    fn classifies_authentication_and_gives_external_login_guidance() {
        let error = Error::from_messages(&[message(
            EV_PROTECT,
            "Perforce password (P4PASSWD) invalid or unset.",
        )])
        .unwrap();

        assert_eq!(error.kind(), ErrorKind::Authentication);
        assert!(
            error
                .to_string()
                .contains("log in with an existing Perforce client")
        );
    }

    #[test]
    fn classifies_ssl_trust_before_connectivity() {
        let error = Error::from_messages(&[message(
            EV_COMM,
            "The authenticity of SSL connection has not been established; use p4 trust.",
        )])
        .unwrap();

        assert_eq!(error.kind(), ErrorKind::Trust);
        assert!(error.to_string().contains("establish trust"));
    }

    #[test]
    fn classifies_connectivity_configuration_mapping_and_permissions() {
        let cases = [
            (EV_COMM, "Connect to server failed", ErrorKind::Connectivity),
            (
                EV_CONFIG,
                "No client name configured",
                ErrorKind::Configuration,
            ),
            (0, "file(s) not in client view", ErrorKind::Mapping),
            (EV_PROTECT, "Protections deny access", ErrorKind::Permission),
        ];

        for (generic, text, expected) in cases {
            let error = Error::from_messages(&[message(generic, text)]).unwrap();
            assert_eq!(error.kind(), expected, "{text}");
            assert!(error.remedy().is_some());
        }
    }

    #[test]
    fn ignores_informational_messages() {
        let message = RawMessage {
            severity: 1,
            generic: 0,
            text: b"no files opened".to_vec(),
        };

        assert!(Error::from_messages(&[message]).is_none());
    }
}
