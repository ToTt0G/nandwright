//! Locating project-local chip sources.
//!
//! Chip lookup is source-first: `Xxx.hdl` in the project directory is used if
//! present (even if it is a stub or broken), and only a genuine *not found*
//! falls back to the built-in library. Built-in fallback is handled by the
//! elaborator; providers only report what exists in the project.

use std::collections::HashMap;
use std::sync::Arc;

use crate::host::{HostError, SharedHost, path, read_text};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// A stable identity for the source, e.g. a canonical path or editor URI.
    pub id: String,
    pub text: Arc<str>,
}

impl SourceFile {
    pub fn new(id: impl Into<String>, text: impl Into<Arc<str>>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceLookup {
    Found(SourceFile),
    /// No source exists; `searched` describes the locations examined.
    NotFound {
        searched: Vec<String>,
    },
    /// A source exists but could not be read. This is *not* a reason to fall
    /// back to a built-in.
    Error {
        id: String,
        message: String,
    },
}

/// Provides project-local chip sources by chip name.
pub trait SourceProvider {
    fn load_chip(&self, chip_name: &str) -> SourceLookup;
}

impl<T: SourceProvider + ?Sized> SourceProvider for &T {
    fn load_chip(&self, chip_name: &str) -> SourceLookup {
        (**self).load_chip(chip_name)
    }
}

impl<T: SourceProvider + ?Sized> SourceProvider for Box<T> {
    fn load_chip(&self, chip_name: &str) -> SourceLookup {
        (**self).load_chip(chip_name)
    }
}

impl<T: SourceProvider + ?Sized> SourceProvider for Arc<T> {
    fn load_chip(&self, chip_name: &str) -> SourceLookup {
        (**self).load_chip(chip_name)
    }
}

/// Whether `name` can be a chip name (and therefore a safe file stem).
pub fn is_valid_chip_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Reads `<project_dir>/<Name>.hdl` through a [`Host`].
///
/// The lookup is whatever the host's file system does with that name, which
/// is what the official simulator does too: on Windows `and.hdl` satisfies a
/// part named `And`, on Linux it does not and the built-in chip is used.
#[derive(Clone)]
pub struct HostSourceProvider {
    host: SharedHost,
    project_dir: String,
}

impl std::fmt::Debug for HostSourceProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostSourceProvider")
            .field("project_dir", &self.project_dir)
            .finish()
    }
}

impl HostSourceProvider {
    /// `project_dir` is an engine-form path (see [`crate::host`]).
    pub fn new(host: SharedHost, project_dir: impl Into<String>) -> Self {
        Self {
            host,
            project_dir: project_dir.into(),
        }
    }

    pub fn host(&self) -> &SharedHost {
        &self.host
    }

    pub fn project_dir(&self) -> &str {
        &self.project_dir
    }

    /// The path a chip's source would have.
    pub fn chip_path(&self, chip_name: &str) -> String {
        path::join(&self.project_dir, &format!("{chip_name}.hdl"))
    }
}

impl SourceProvider for HostSourceProvider {
    fn load_chip(&self, chip_name: &str) -> SourceLookup {
        let path = self.chip_path(chip_name);
        if !is_valid_chip_name(chip_name) {
            return SourceLookup::NotFound {
                searched: vec![path],
            };
        }
        match read_text(self.host.as_ref(), &path) {
            Ok(file) => SourceLookup::Found(SourceFile::new(path, file.text)),
            Err(HostError::NotFound) => SourceLookup::NotFound {
                searched: vec![path],
            },
            Err(e) => SourceLookup::Error {
                id: path,
                message: e.to_string(),
            },
        }
    }
}

/// An in-memory set of sources keyed by chip name.
#[derive(Debug, Clone, Default)]
pub struct MemorySourceProvider {
    files: HashMap<String, SourceFile>,
}

impl MemorySourceProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a source using `<chip_name>.hdl` as its identity.
    pub fn with(mut self, chip_name: &str, text: &str) -> Self {
        self.insert(chip_name, SourceFile::new(format!("{chip_name}.hdl"), text));
        self
    }

    pub fn insert(&mut self, chip_name: impl Into<String>, file: SourceFile) {
        self.files.insert(chip_name.into(), file);
    }

    pub fn remove(&mut self, chip_name: &str) -> Option<SourceFile> {
        self.files.remove(chip_name)
    }
}

impl SourceProvider for MemorySourceProvider {
    fn load_chip(&self, chip_name: &str) -> SourceLookup {
        match self.files.get(chip_name) {
            Some(file) => SourceLookup::Found(file.clone()),
            None => SourceLookup::NotFound {
                searched: vec![format!("{chip_name}.hdl (in memory)")],
            },
        }
    }
}

/// Layers unsaved editor buffers over another provider.
#[derive(Debug, Clone)]
pub struct OverlaySourceProvider<P> {
    overlays: HashMap<String, SourceFile>,
    inner: P,
}

impl<P> OverlaySourceProvider<P> {
    pub fn new(inner: P) -> Self {
        Self {
            overlays: HashMap::new(),
            inner,
        }
    }

    pub fn set_overlay(&mut self, chip_name: impl Into<String>, file: SourceFile) {
        self.overlays.insert(chip_name.into(), file);
    }

    pub fn remove_overlay(&mut self, chip_name: &str) -> Option<SourceFile> {
        self.overlays.remove(chip_name)
    }

    pub fn inner(&self) -> &P {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut P {
        &mut self.inner
    }
}

impl<P: SourceProvider> SourceProvider for OverlaySourceProvider<P> {
    fn load_chip(&self, chip_name: &str) -> SourceLookup {
        match self.overlays.get(chip_name) {
            Some(file) => SourceLookup::Found(file.clone()),
            None => self.inner.load_chip(chip_name),
        }
    }
}
