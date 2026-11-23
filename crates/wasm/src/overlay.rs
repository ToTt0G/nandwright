//! Unsaved editor buffers layered over the real files.
//!
//! A student runs a test without saving first. The editor hands the text of
//! every unsaved document to the engine, and reads of those paths see that
//! text instead of what is on disk.

use std::collections::HashMap;
use std::sync::Mutex;

use n2t_core::host::{DirEntry, Host, HostError, SharedHost, path};

pub struct OverlayHost {
    inner: SharedHost,
    buffers: Mutex<HashMap<String, Vec<u8>>>,
}

/// Paths are compared without regard to ASCII case and separators, the way
/// Windows compares them; a buffer for `c:/x/And.hdl` shadows `C:\x\And.hdl`.
fn key(file: &str) -> String {
    path::normalize(file).to_ascii_lowercase()
}

impl OverlayHost {
    pub fn new(inner: SharedHost) -> Self {
        Self {
            inner,
            buffers: Mutex::new(HashMap::new()),
        }
    }

    pub fn set(&self, file: &str, text: &str) {
        if let Ok(mut buffers) = self.buffers.lock() {
            buffers.insert(key(file), text.as_bytes().to_vec());
        }
    }

    pub fn clear(&self, file: &str) {
        if let Ok(mut buffers) = self.buffers.lock() {
            buffers.remove(&key(file));
        }
    }

    pub fn clear_all(&self) {
        if let Ok(mut buffers) = self.buffers.lock() {
            buffers.clear();
        }
    }
}

impl Host for OverlayHost {
    fn read(&self, file: &str) -> Result<Vec<u8>, HostError> {
        if let Ok(buffers) = self.buffers.lock()
            && let Some(data) = buffers.get(&key(file))
        {
            return Ok(data.clone());
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
