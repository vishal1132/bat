use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};

use content_inspector::{self, ContentType};

use crate::error::*;

const THEME_PREVIEW_FILE: &[u8] = include_bytes!("../assets/theme_preview.rs");

/// A description of an Input source.
/// This tells bat how to refer to the input.
#[derive(Clone)]
pub struct InputDescription {
    name: String,
    kind: Option<String>,
    summary: Option<String>,
}

impl InputDescription {
    /// Creates a description for an input.
    ///
    /// The name should uniquely describes where the input came from (e.g. "README.md")
    pub fn new(name: impl Into<String>) -> Self {
        InputDescription {
            name: name.into(),
            kind: None,
            summary: None,
        }
    }

    /// A description for the type of input (e.g. "File")
    pub fn with_kind(mut self, kind: Option<impl Into<String>>) -> Self {
        self.kind = kind.map(|kind| kind.into());
        self
    }

    /// A summary description of the input.
    ///
    /// Defaults to "{kind} '{name}'"
    pub fn with_summary(mut self, summary: Option<impl Into<String>>) -> Self {
        self.summary = summary.map(|summary| summary.into());
        self
    }

    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn kind(&self) -> Option<&String> {
        self.kind.as_ref()
    }

    pub fn summary(&self) -> String {
        self.summary.clone().unwrap_or_else(|| match &self.kind {
            None => self.name.clone(),
            Some(kind) => format!("{} '{}'", kind.to_lowercase(), self.name),
        })
    }
}

pub(crate) enum InputKind<'a> {
    OrdinaryFile(OsString),
    StdIn,
    ThemePreviewFile,
    CustomReader(Box<dyn Read + 'a>),
}

impl<'a> InputKind<'a> {
    pub fn description(&self) -> InputDescription {
        match self {
            InputKind::OrdinaryFile(ref path) => {
                InputDescription::new(path.to_string_lossy()).with_kind(Some("File"))
            }
            InputKind::StdIn => InputDescription::new("STDIN"),
            InputKind::ThemePreviewFile => InputDescription::new(""),
            InputKind::CustomReader(_) => InputDescription::new("READER"),
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct InputMetadata {
    pub(crate) user_provided_name: Option<OsString>,
}

pub struct Input<'a> {
    pub(crate) kind: InputKind<'a>,
    pub(crate) metadata: InputMetadata,
    pub(crate) description: Option<InputDescription>,
}

pub(crate) enum OpenedInputKind {
    OrdinaryFile(OsString),
    StdIn,
    ThemePreviewFile,
    CustomReader,
}

impl OpenedInputKind {
    pub(crate) fn is_theme_preview_file(&self) -> bool {
        match self {
            OpenedInputKind::ThemePreviewFile => true,
            _ => false,
        }
    }
}

pub(crate) struct OpenedInput<'a> {
    pub(crate) kind: OpenedInputKind,
    pub(crate) metadata: InputMetadata,
    pub(crate) reader: InputReader<'a>,
    pub(crate) description: InputDescription,
}

impl<'a> Input<'a> {
    pub fn ordinary_file(path: &OsStr) -> Self {
        Input {
            kind: InputKind::OrdinaryFile(path.to_os_string()),
            metadata: InputMetadata::default(),
            description: None,
        }
    }

    pub fn stdin() -> Self {
        Input {
            kind: InputKind::StdIn,
            metadata: InputMetadata::default(),
            description: None,
        }
    }

    pub fn theme_preview_file() -> Self {
        Input {
            kind: InputKind::ThemePreviewFile,
            metadata: InputMetadata::default(),
            description: None,
        }
    }

    pub fn from_reader(reader: Box<dyn Read + 'a>) -> Self {
        Input {
            kind: InputKind::CustomReader(reader),
            metadata: InputMetadata::default(),
            description: None,
        }
    }

    pub fn is_stdin(&self) -> bool {
        if let InputKind::StdIn = self.kind {
            true
        } else {
            false
        }
    }

    pub fn with_name(mut self, provided_name: Option<&OsStr>) -> Self {
        self.metadata.user_provided_name = provided_name.map(|n| n.to_owned());
        self
    }

    pub fn with_description(mut self, description: Option<InputDescription>) -> Self {
        self.description = description;
        self
    }

    pub fn description(&self) -> InputDescription {
        if let Some(ref description) = self.description {
            description.clone()
        } else if let Some(ref name) = self.metadata.user_provided_name {
            InputDescription::new(name.to_string_lossy()).with_kind(Some("File"))
        } else {
            self.kind.description()
        }
    }

    pub(crate) fn open<R: BufRead + 'a>(self, stdin: R) -> Result<OpenedInput<'a>> {
        let description = self.description().clone();
        match self.kind {
            InputKind::StdIn => Ok(OpenedInput {
                kind: OpenedInputKind::StdIn,
                description,
                metadata: self.metadata,
                reader: InputReader::new(stdin),
            }),
            InputKind::OrdinaryFile(path) => Ok(OpenedInput {
                kind: OpenedInputKind::OrdinaryFile(path.clone()),
                description,
                metadata: self.metadata,
                reader: {
                    let file = File::open(&path)
                        .map_err(|e| format!("'{}': {}", path.to_string_lossy(), e))?;
                    if file.metadata()?.is_dir() {
                        return Err(format!("'{}' is a directory.", path.to_string_lossy()).into());
                    }
                    InputReader::new(BufReader::new(file))
                },
            }),
            InputKind::ThemePreviewFile => Ok(OpenedInput {
                kind: OpenedInputKind::ThemePreviewFile,
                description,
                metadata: self.metadata,
                reader: InputReader::new(THEME_PREVIEW_FILE),
            }),
            InputKind::CustomReader(reader) => Ok(OpenedInput {
                description,
                kind: OpenedInputKind::CustomReader,
                metadata: self.metadata,
                reader: InputReader::new(BufReader::new(reader)),
            }),
        }
    }
}

pub(crate) struct InputReader<'a> {
    inner: Box<dyn BufRead + 'a>,
    pub(crate) first_line: Vec<u8>,
    pub(crate) content_type: Option<ContentType>,
}

impl<'a> InputReader<'a> {
    fn new<R: BufRead + 'a>(mut reader: R) -> InputReader<'a> {
        let mut first_line = vec![];
        reader.read_until(b'\n', &mut first_line).ok();

        let content_type = if first_line.is_empty() {
            None
        } else {
            Some(content_inspector::inspect(&first_line[..]))
        };

        if content_type == Some(ContentType::UTF_16LE) {
            reader.read_until(0x00, &mut first_line).ok();
        }

        InputReader {
            inner: Box::new(reader),
            first_line,
            content_type,
        }
    }

    pub(crate) fn read_line(&mut self, buf: &mut Vec<u8>) -> io::Result<bool> {
        if self.first_line.is_empty() {
            let res = self.inner.read_until(b'\n', buf).map(|size| size > 0)?;

            if self.content_type == Some(ContentType::UTF_16LE) {
                self.inner.read_until(0x00, buf).ok();
            }

            Ok(res)
        } else {
            buf.append(&mut self.first_line);
            Ok(true)
        }
    }
}

#[test]
fn basic() {
    let content = b"#!/bin/bash\necho hello";
    let mut reader = InputReader::new(&content[..]);

    assert_eq!(b"#!/bin/bash\n", &reader.first_line[..]);

    let mut buffer = vec![];

    let res = reader.read_line(&mut buffer);
    assert!(res.is_ok());
    assert_eq!(true, res.unwrap());
    assert_eq!(b"#!/bin/bash\n", &buffer[..]);

    buffer.clear();

    let res = reader.read_line(&mut buffer);
    assert!(res.is_ok());
    assert_eq!(true, res.unwrap());
    assert_eq!(b"echo hello", &buffer[..]);

    buffer.clear();

    let res = reader.read_line(&mut buffer);
    assert!(res.is_ok());
    assert_eq!(false, res.unwrap());
    assert!(buffer.is_empty());
}

#[test]
fn utf16le() {
    let content = b"\xFF\xFE\x73\x00\x0A\x00\x64\x00";
    let mut reader = InputReader::new(&content[..]);

    assert_eq!(b"\xFF\xFE\x73\x00\x0A\x00", &reader.first_line[..]);

    let mut buffer = vec![];

    let res = reader.read_line(&mut buffer);
    assert!(res.is_ok());
    assert_eq!(true, res.unwrap());
    assert_eq!(b"\xFF\xFE\x73\x00\x0A\x00", &buffer[..]);

    buffer.clear();

    let res = reader.read_line(&mut buffer);
    assert!(res.is_ok());
    assert_eq!(true, res.unwrap());
    assert_eq!(b"\x64\x00", &buffer[..]);

    buffer.clear();

    let res = reader.read_line(&mut buffer);
    assert!(res.is_ok());
    assert_eq!(false, res.unwrap());
    assert!(buffer.is_empty());
}
