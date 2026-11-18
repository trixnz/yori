//! Source-faithful documents, editing history, and source-coordinate navigation.

use std::{fmt, ops::Range, path::Path};

pub mod editing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLine {
    pub content: Range<usize>,
    pub full: Range<usize>,
    pub ending: LineEnding,
}

#[derive(Debug, Clone)]
pub struct Document {
    text: String,
    lines: Vec<SourceLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputError {
    InvalidUtf8,
    InvalidRange,
    ContainsNul,
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8 => write!(f, "input is not valid UTF-8"),
            Self::InvalidRange => write!(f, "edit range is not on valid source boundaries"),
            Self::ContainsNul => write!(f, "input contains a NUL byte"),
        }
    }
}

impl std::error::Error for InputError {}

impl Document {
    /// Decode source without normalization; reject invalid UTF-8 and NUL.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, InputError> {
        if bytes.contains(&0) {
            return Err(InputError::ContainsNul);
        }

        let text = String::from_utf8(bytes).map_err(|_| InputError::InvalidUtf8)?;
        let lines = split_lines(&text);
        Ok(Self { text, lines })
    }

    /// Read and validate a source file, including its path in any error.
    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        Self::from_bytes(bytes).map_err(|error| format!("cannot open {}: {error}", path.display()))
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn lines(&self) -> &[SourceLine] {
        &self.lines
    }

    #[must_use]
    pub fn content(&self, line: usize) -> &str {
        &self.text[self.lines[line].content.clone()]
    }

    #[must_use]
    pub fn full_line(&self, line: usize) -> &str {
        &self.text[self.lines[line].full.clone()]
    }

    #[must_use]
    pub fn copy_range(&self, range: Range<usize>) -> &str {
        assert!(range.start <= range.end && range.end <= self.text.len());
        assert!(self.text.is_char_boundary(range.start));
        assert!(self.text.is_char_boundary(range.end));

        &self.text[range]
    }

    /// Validate before replacing so unsupported input cannot partially change a document.
    pub fn replace(&mut self, range: Range<usize>, replacement: &str) -> Result<(), InputError> {
        if self.text.get(range.clone()).is_none() {
            return Err(InputError::InvalidRange);
        }
        if replacement.contains('\0') {
            return Err(InputError::ContainsNul);
        }

        let mut text = self.text.clone();
        text.replace_range(range, replacement);
        let lines = split_lines(&text);

        self.text = text;
        self.lines = lines;

        Ok(())
    }

    /// The editing position after a final newline is an empty logical line, not source bytes.
    #[must_use]
    pub fn line_at_offset(&self, offset: usize) -> usize {
        let line = self.lines.partition_point(|line| line.full.end <= offset);
        if line == self.lines.len()
            && self
                .lines
                .last()
                .is_some_and(|line| line.ending == LineEnding::None)
        {
            line.saturating_sub(1)
        } else {
            line
        }
    }

    #[must_use]
    pub fn line_content_range(&self, line: usize) -> Range<usize> {
        self.lines
            .get(line)
            .map_or(self.text.len()..self.text.len(), |line| {
                line.content.clone()
            })
    }

    #[must_use]
    pub fn newline(&self) -> &'static str {
        match self
            .lines
            .iter()
            .find(|line| line.ending != LineEnding::None)
        {
            Some(line) if line.ending == LineEnding::CrLf => "\r\n",
            Some(line) if line.ending == LineEnding::Cr => "\r",
            _ => "\n",
        }
    }
}

fn split_lines(text: &str) -> Vec<SourceLine> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                let (content_end, ending) = if index > start && bytes[index - 1] == b'\r' {
                    (index - 1, LineEnding::CrLf)
                } else {
                    (index, LineEnding::Lf)
                };

                lines.push(SourceLine {
                    content: start..content_end,
                    full: start..index + 1,
                    ending,
                });
                start = index + 1;
            }
            b'\r' if bytes.get(index + 1) != Some(&b'\n') => {
                lines.push(SourceLine {
                    content: start..index,
                    full: start..index + 1,
                    ending: LineEnding::Cr,
                });
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }

    if start < bytes.len() {
        lines.push(SourceLine {
            content: start..bytes.len(),
            full: start..bytes.len(),
            ending: LineEnding::None,
        });
    }

    lines
}
