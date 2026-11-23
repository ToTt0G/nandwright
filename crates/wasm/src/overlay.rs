//! Unsaved editor buffers layered over the real files.
//!
//! A student runs a test without saving first. The editor hands the text of
//! every unsaved document to the engine, and reads of those paths see that
//! text instead of what is on disk.

use std::sync::{Arc, Mutex};

use n2t_core::host::{DirEntry, Host, HostError, SharedHost, path};

/// One unsaved document.
#[derive(Clone)]
pub struct Buffer {
    /// The document's path, normalized.
    path: String,
    /// Whether the file system the document lives on ignores case, so that
    /// `and.hdl` in a script finds the buffer of `And.hdl`. On a file system
    /// that does not, those are two files and only the exact name matches.
    ignore_case: bool,
    data: Arc<[u8]>,
}

impl Buffer {
    /// `ignore_case` of `None` means "what this kind of path usually is":
    /// Windows paths (a drive letter or a UNC share) ignore case, others
    /// do not.
    pub fn new(file: &str, text: &str, ignore_case: Option<bool>) -> Self {
        let path = path::normalize(file);
        let ignore_case = ignore_case.unwrap_or_else(|| looks_like_windows(&path));
        Self {
            path,
            ignore_case,
            data: text.as_bytes().into(),
        }
    }

    fn is(&self, normalized: &str) -> bool {
        self.path == normalized || (self.ignore_case && self.path.eq_ignore_ascii_case(normalized))
    }
}

fn looks_like_windows(normalized: &str) -> bool {
    let bytes = normalized.as_bytes();
    normalized.starts_with("//")
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

pub struct OverlayHost {
    inner: SharedHost,
    buffers: Mutex<Vec<Buffer>>,
}

impl OverlayHost {
    pub fn new(inner: SharedHost) -> Self {
        Self {
            inner,
            buffers: Mutex::new(Vec::new()),
        }
    }

    /// The files themselves, without the unsaved buffers: what a write
    /// would replace.
    pub fn disk(&self) -> &SharedHost {
        &self.inner
    }

    pub fn set(&self, buffer: Buffer) {
        if let Ok(mut buffers) = self.buffers.lock() {
            buffers.retain(|b| b.path != buffer.path);
            buffers.push(buffer);
        }
    }

    pub fn clear(&self, file: &str) {
        let normalized = path::normalize(file);
        if let Ok(mut buffers) = self.buffers.lock() {
            buffers.retain(|b| !b.is(&normalized));
        }
    }

    /// Replaces every buffer at once, so that no read can fall between "the
    /// old buffers are gone" and "the new ones are there".
    pub fn replace(&self, new: Vec<Buffer>) {
        if let Ok(mut buffers) = self.buffers.lock() {
            *buffers = new;
        }
    }

    /// The buffers as they are now, over the same files. A test run keeps
    /// one, so that a `load` late in the script sees the same editor text
    /// as a `load` at its start, whatever is typed or run meanwhile.
    pub fn snapshot(&self) -> OverlayHost {
        OverlayHost {
            inner: self.inner.clone(),
            buffers: Mutex::new(self.buffers.lock().map(|b| b.clone()).unwrap_or_default()),
        }
    }
}

impl Host for OverlayHost {
    fn read(&self, file: &str) -> Result<Vec<u8>, HostError> {
        let normalized = path::normalize(file);
        if let Ok(buffers) = self.buffers.lock() {
            // An exact name wins over one that differs in case.
            let found = buffers
                .iter()
                .find(|b| b.path == normalized)
                .or_else(|| buffers.iter().find(|b| b.is(&normalized)));
            if let Some(buffer) = found {
                return Ok(buffer.data.to_vec());
            }
        }
        self.inner.read(file)
    }

    fn list(&self, dir: &str) -> Result<Vec<DirEntry>, HostError> {
        self.inner.list(dir)
    }

    fn write(&self, file: &str, data: &[u8]) -> Result<(), HostError> {
        self.inner.write(file, data)
    }
}
