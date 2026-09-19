//! Presentation metadata for a source document, independent of editor state.

use std::path::Path;

use yori_document::{Document, LineEnding};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    PlainText,
    Go,
    C,
    Cpp,
    Rust,
}

impl Language {
    pub const ALL: [Self; 5] = [Self::PlainText, Self::Go, Self::C, Self::Cpp, Self::Rust];

    #[must_use]
    pub fn detect(path: &Path) -> Self {
        let extension = path.extension().and_then(|extension| extension.to_str());
        if extension == Some("C") {
            return Self::Cpp;
        }

        match extension.unwrap_or_default().to_ascii_lowercase().as_str() {
            "go" => Self::Go,
            "c" | "h" => Self::C,
            "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" => Self::Cpp,
            "rs" => Self::Rust,
            _ => Self::PlainText,
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::PlainText => "Plain text",
            Self::Go => "Go",
            Self::C => "C",
            Self::Cpp => "C++",
            Self::Rust => "Rust",
        }
    }

    #[must_use]
    pub fn grammar(self) -> Option<&'static str> {
        match self {
            Self::PlainText => None,
            Self::Go => Some("go"),
            Self::C => Some("c"),
            Self::Cpp => Some("cpp"),
            Self::Rust => Some("rust"),
        }
    }
}

/// A summary of actual terminators, not a request to normalize source bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEndings {
    None,
    Lf,
    CrLf,
    Mixed,
}

impl LineEndings {
    #[must_use]
    pub fn from_document(document: &Document) -> Self {
        let mut lf = false;
        let mut crlf = false;

        for line in document.lines() {
            match line.ending {
                LineEnding::Lf => lf = true,
                LineEnding::CrLf => crlf = true,
                LineEnding::None => {}
            }
            if lf && crlf {
                return Self::Mixed;
            }
        }

        match (lf, crlf) {
            (false, false) => Self::None,
            (true, false) => Self::Lf,
            (false, true) => Self::CrLf,
            (true, true) => Self::Mixed,
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "—",
            Self::Lf => "LF",
            Self::CrLf => "CRLF",
            Self::Mixed => "Mixed",
        }
    }

    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::None => "No line endings",
            Self::Lf => "LF line endings",
            Self::CrLf => "CRLF line endings",
            Self::Mixed => "Mixed LF and CRLF line endings",
        }
    }
}

/// Splits a path into the file name that identifies it and the directory that
/// contains it. A path with no meaningful parent reports `None` so callers can
/// choose between omitting the directory and naming the current directory.
#[must_use]
pub fn path_labels(path: &Path) -> (String, Option<String>) {
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned();
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.display().to_string());

    (name, directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_detection_covers_sources_headers_and_temporary_files() {
        let cases = [
            ("src/server.go", Language::Go),
            ("src/core.c", Language::C),
            ("include/core.h", Language::C),
            ("src/core.C", Language::Cpp),
            ("src/core.cpp", Language::Cpp),
            ("src/core.cc", Language::Cpp),
            ("src/core.cxx", Language::Cpp),
            ("include/core.hpp", Language::Cpp),
            ("include/core.hh", Language::Cpp),
            ("include/core.hxx", Language::Cpp),
            ("src/main.rs", Language::Rust),
            ("src/MAIN.RS", Language::Rust),
            ("/tmp/p4-12345", Language::PlainText),
            ("notes.txt", Language::PlainText),
            ("README", Language::PlainText),
        ];

        for (path, expected) in cases {
            assert_eq!(Language::detect(Path::new(path)), expected, "{path}");
        }
    }

    #[test]
    fn line_ending_metadata_describes_bytes_without_changing_them() {
        let cases = [
            ("", LineEndings::None),
            ("unterminated", LineEndings::None),
            ("one\n", LineEndings::Lf),
            ("one\r\n", LineEndings::CrLf),
            ("one\r\ntwo\n", LineEndings::Mixed),
            ("one\r\ntwo", LineEndings::CrLf),
        ];

        for (text, expected) in cases {
            let document = Document::from_bytes(text.as_bytes().to_vec()).unwrap();
            assert_eq!(LineEndings::from_document(&document), expected);
            assert_eq!(document.text(), text);
        }
    }
}
