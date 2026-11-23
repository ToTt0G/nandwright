//! The engine's only door to the outside world.
//!
//! Library code never touches `std::fs` or `std::path`: the engine is compiled
//! to `wasm32-unknown-unknown`, where neither works, and where `std::path`
//! would apply Unix rules to a student's `C:\Users\…` paths. Files are reached
//! through a [`Host`], and paths are plain strings in *engine form*:
//!
//! - separators are `/`;
//! - a Windows drive is kept as written (`C:/Users/me/nand2tetris/projects/01`);
//! - a UNC share is `//server/share/…`.
//!
//! Hosts accept engine-form paths as they are. Both Node's `fs` and Rust's
//! `std::fs` take forward slashes on Windows, so no conversion back is needed.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

/// Why a host operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// The file or directory does not exist.
    NotFound,
    /// Any other failure, described for the user.
    Other(String),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostError::NotFound => f.write_str("the file does not exist"),
            HostError::Other(message) => f.write_str(message),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// File access as the engine sees it. Implementations must not panic.
pub trait Host {
    fn read(&self, path: &str) -> Result<Vec<u8>, HostError>;
    /// The entries of a directory, in any order.
    fn list(&self, dir: &str) -> Result<Vec<DirEntry>, HostError>;
    fn write(&self, path: &str, data: &[u8]) -> Result<(), HostError>;
}

pub type SharedHost = Arc<dyn Host + Send + Sync>;

/// Text read from a host, with what is needed to write it back unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextFile {
    pub text: String,
    pub encoding: Encoding,
}

/// How a file's bytes map to text.
///
/// Files in the official course distribution are not all UTF-8: some carry
/// Windows-1252 bytes (a curly quote in a comment). They are decoded so the
/// student sees the right characters, and encoded the same way when a
/// formatter writes them back, so formatting never changes a file's bytes
/// outside the edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Windows1252,
}

/// Reads a text file: UTF-8, or Windows-1252 if the bytes are not valid UTF-8.
pub fn read_text(host: &dyn Host, path: &str) -> Result<TextFile, HostError> {
    Ok(decode_text(host.read(path)?))
}

/// Decodes file bytes: UTF-8, or Windows-1252 if they are not valid UTF-8.
pub fn decode_text(bytes: Vec<u8>) -> TextFile {
    match String::from_utf8(bytes) {
        Ok(text) => TextFile {
            text,
            encoding: Encoding::Utf8,
        },
        Err(err) => TextFile {
            text: decode_windows_1252(err.as_bytes()),
            encoding: Encoding::Windows1252,
        },
    }
}

/// Encodes text for writing. Returns `None` if the text has a character the
/// encoding cannot represent (the caller should then keep UTF-8).
pub fn encode_text(text: &str, encoding: Encoding) -> Option<Vec<u8>> {
    match encoding {
        Encoding::Utf8 => Some(text.as_bytes().to_vec()),
        Encoding::Windows1252 => {
            let mut out = Vec::with_capacity(text.len());
            for c in text.chars() {
                out.push(encode_windows_1252(c)?);
            }
            Some(out)
        }
    }
}

const CP1252_80_9F: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

/// Decodes Windows-1252. The five bytes the code page leaves undefined map to
/// the C1 control with the same value (as browsers do), so decoding followed
/// by encoding returns the original bytes for every input.
fn decode_windows_1252(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if (0x80..=0x9F).contains(&b) {
            out.push(CP1252_80_9F[(b - 0x80) as usize]);
        } else {
            out.push(char::from(b));
        }
    }
    out
}

fn encode_windows_1252(c: char) -> Option<u8> {
    let code = c as u32;
    if code < 0x80 || (0xA0..=0xFF).contains(&code) {
        return Some(code as u8);
    }
    CP1252_80_9F
        .iter()
        .position(|&m| m == c)
        .map(|i| 0x80 + i as u8)
}

/// Path manipulation on engine-form strings.
pub mod path {
    /// Converts a native path to engine form: backslashes become slashes and
    /// the Windows extended-length prefix (`\\?\C:\…`, `\\?\UNC\server\…`) is
    /// removed, so it never reaches a message the student reads.
    pub fn normalize(native: &str) -> String {
        let mut text = native.replace('\\', "/");
        if let Some(rest) = text.strip_prefix("//?/UNC/") {
            text = format!("//{rest}");
        } else if let Some(rest) = text.strip_prefix("//?/") {
            text = rest.to_string();
        }
        // Collapse repeated separators, keeping a leading `//` (UNC).
        let unc = text.starts_with("//");
        let mut out = String::with_capacity(text.len());
        let mut previous_slash = false;
        for c in text.chars() {
            if c == '/' {
                if !previous_slash {
                    out.push(c);
                }
                previous_slash = true;
            } else {
                out.push(c);
                previous_slash = false;
            }
        }
        if unc {
            out.insert(0, '/');
        }
        // Drop a trailing separator, unless the path is a root.
        if out.len() > 1 && out.ends_with('/') && !is_root(&out) {
            out.pop();
        }
        out
    }

    fn is_root(path: &str) -> bool {
        path == "/" || (path.len() == 3 && path.as_bytes()[1] == b':' && path.ends_with('/'))
    }

    /// The directory containing `path` (`""` if it has no directory part).
    pub fn dir(path: &str) -> &str {
        match path.rfind('/') {
            None => "",
            Some(0) => "/",
            Some(i) if i == 2 && path.as_bytes()[1] == b':' => &path[..3],
            Some(i) => &path[..i],
        }
    }

    /// The last component of `path`.
    pub fn file_name(path: &str) -> &str {
        match path.rfind('/') {
            None => path,
            Some(i) => &path[i + 1..],
        }
    }

    /// The file name without its last extension.
    pub fn stem(path: &str) -> &str {
        let name = file_name(path);
        match name.rfind('.') {
            Some(i) if i > 0 => &name[..i],
            _ => name,
        }
    }

    /// The last extension without the dot, as written (`""` if there is none).
    pub fn extension(path: &str) -> &str {
        let name = file_name(path);
        match name.rfind('.') {
            Some(i) if i > 0 => &name[i + 1..],
            _ => "",
        }
    }

    /// Whether the path's extension is `ext`, ignoring ASCII case.
    pub fn has_extension(path: &str, ext: &str) -> bool {
        extension(path).eq_ignore_ascii_case(ext)
    }

    /// Joins a directory and a relative name. An absolute `name` wins.
    pub fn join(dir: &str, name: &str) -> String {
        let name = name.replace('\\', "/");
        if dir.is_empty() || is_absolute(&name) {
            return name;
        }
        if dir.ends_with('/') {
            format!("{dir}{name}")
        } else {
            format!("{dir}/{name}")
        }
    }

    pub fn is_absolute(path: &str) -> bool {
        let bytes = path.as_bytes();
        path.starts_with('/')
            || (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && bytes[2] == b'/')
    }

    /// `path` relative to `base` when it lies inside it, otherwise `path`.
    /// For display only.
    pub fn relative_to<'a>(path: &'a str, base: &str) -> &'a str {
        if base.is_empty() {
            return path;
        }
        match path.strip_prefix(base) {
            Some(rest) if base.ends_with('/') => rest,
            Some(rest) => rest.strip_prefix('/').unwrap_or(path),
            None => path,
        }
    }
}

/// An in-memory file system, for tests and for running without a disk.
#[derive(Debug, Default)]
pub struct MemoryHost {
    files: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl MemoryHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(self, path: &str, contents: impl AsRef<[u8]>) -> Self {
        self.insert(path, contents);
        self
    }

    pub fn insert(&self, path: &str, contents: impl AsRef<[u8]>) {
        if let Ok(mut files) = self.files.lock() {
            files.insert(path::normalize(path), contents.as_ref().to_vec());
        }
    }

    pub fn remove(&self, path: &str) {
        if let Ok(mut files) = self.files.lock() {
            files.remove(&path::normalize(path));
        }
    }

    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        self.files.lock().ok()?.get(&path::normalize(path)).cloned()
    }

    pub fn get_text(&self, path: &str) -> Option<String> {
        self.get(path).map(|b| decode_text(b).text)
    }
}

impl Host for MemoryHost {
    fn read(&self, path: &str) -> Result<Vec<u8>, HostError> {
        self.get(path).ok_or(HostError::NotFound)
    }

    fn list(&self, dir: &str) -> Result<Vec<DirEntry>, HostError> {
        let dir = path::normalize(dir);
        let prefix = if dir.ends_with('/') || dir.is_empty() {
            dir.clone()
        } else {
            format!("{dir}/")
        };
        let files = self
            .files
            .lock()
            .map_err(|_| HostError::Other("the in-memory file system is poisoned".into()))?;
        let mut entries: Vec<DirEntry> = Vec::new();
        for key in files.keys() {
            let Some(rest) = key.strip_prefix(&prefix) else {
                continue;
            };
            let (name, is_dir) = match rest.find('/') {
                Some(i) => (&rest[..i], true),
                None => (rest, false),
            };
            if name.is_empty() {
                continue;
            }
            if !entries.iter().any(|e| e.name == name) {
                entries.push(DirEntry {
                    name: name.to_string(),
                    is_dir,
                });
            }
        }
        if entries.is_empty() {
            return Err(HostError::NotFound);
        }
        Ok(entries)
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), HostError> {
        self.insert(path, data);
        Ok(())
    }
}

/// The real file system. Not available on wasm, where the embedder supplies
/// the host.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct StdHost;

#[cfg(not(target_arch = "wasm32"))]
impl StdHost {
    /// The absolute engine-form path of a native path. The file need not
    /// exist. Symbolic links are left alone: the student's own spelling of the
    /// path is what messages should show.
    pub fn absolute(native: &std::path::Path) -> String {
        let absolute = std::path::absolute(native).unwrap_or_else(|_| native.to_path_buf());
        path::normalize(&absolute.to_string_lossy())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn io_error(e: std::io::Error) -> HostError {
    if e.kind() == std::io::ErrorKind::NotFound {
        HostError::NotFound
    } else {
        HostError::Other(e.to_string())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Host for StdHost {
    fn read(&self, path: &str) -> Result<Vec<u8>, HostError> {
        std::fs::read(path).map_err(io_error)
    }

    fn list(&self, dir: &str) -> Result<Vec<DirEntry>, HostError> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(dir).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            // Follow links: a linked project folder is still a folder.
            let is_dir = std::fs::metadata(entry.path())
                .map(|m| m.is_dir())
                .unwrap_or(false);
            entries.push(DirEntry { name, is_dir });
        }
        Ok(entries)
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), HostError> {
        std::fs::write(path, data).map_err(io_error)
    }
}

#[cfg(test)]
mod tests {
    use super::path::*;
    use super::*;

    #[test]
    fn normalises_windows_paths() {
        assert_eq!(
            normalize(r"C:\Users\me\OneDrive - Uni\nand2tetris\projects\01\And.hdl"),
            "C:/Users/me/OneDrive - Uni/nand2tetris/projects/01/And.hdl"
        );
        assert_eq!(normalize(r"\\?\C:\course\And.hdl"), "C:/course/And.hdl");
        assert_eq!(
            normalize(r"\\?\UNC\server\share\And.hdl"),
            "//server/share/And.hdl"
        );
        assert_eq!(normalize(r"\\server\share\x"), "//server/share/x");
        assert_eq!(normalize("C:\\"), "C:/");
        assert_eq!(normalize("/home/me/x/"), "/home/me/x");
        assert_eq!(normalize("/"), "/");
        assert_eq!(normalize("a//b///c"), "a/b/c");
    }

    #[test]
    fn splits_paths() {
        let p = "C:/Users/me/My Projects/01/And.hdl";
        assert_eq!(dir(p), "C:/Users/me/My Projects/01");
        assert_eq!(file_name(p), "And.hdl");
        assert_eq!(stem(p), "And");
        assert_eq!(extension(p), "hdl");
        assert_eq!(dir("C:/And.hdl"), "C:/");
        assert_eq!(dir("/And.hdl"), "/");
        assert_eq!(dir("And.hdl"), "");
        assert_eq!(stem(".hidden"), ".hidden");
        assert_eq!(extension(".hidden"), "");
        assert_eq!(stem("a.b.tst"), "a.b");
        assert!(has_extension("X.HDL", "hdl"));
    }

    #[test]
    fn joins_paths() {
        assert_eq!(join("C:/x", "And.hdl"), "C:/x/And.hdl");
        assert_eq!(join("C:/", "And.hdl"), "C:/And.hdl");
        assert_eq!(join("", "And.hdl"), "And.hdl");
        assert_eq!(join("/x", "/abs/And.hdl"), "/abs/And.hdl");
        assert_eq!(join("/x", "D:/abs/And.hdl"), "D:/abs/And.hdl");
        assert_eq!(join("/x", "sub\\And.hdl"), "/x/sub/And.hdl");
        assert_eq!(relative_to("/a/b/c.hdl", "/a"), "b/c.hdl");
        assert_eq!(relative_to("/ab/c.hdl", "/a"), "/ab/c.hdl");
    }

    #[test]
    fn windows_1252_round_trips_every_byte() {
        // Every byte string that is not valid UTF-8 must survive
        // decode + encode unchanged, or formatting would corrupt the file.
        let all: Vec<u8> = (0u8..=255).collect();
        let file = decode_text(all.clone());
        assert_eq!(file.encoding, Encoding::Windows1252);
        assert_eq!(encode_text(&file.text, file.encoding), Some(all));
        // A curly quote, as found in the course's built-in ALU.hdl.
        let quote = decode_text(b"// don\x92t".to_vec());
        assert_eq!(quote.text, "// don\u{2019}t");
        assert_eq!(
            encode_text(&quote.text, quote.encoding).as_deref(),
            Some(&b"// don\x92t"[..])
        );
        // Text that Windows-1252 cannot hold is refused, not mangled.
        assert_eq!(encode_text("λ", Encoding::Windows1252), None);
    }

    #[test]
    fn memory_host_lists_directories() {
        let host = MemoryHost::new()
            .with("/p/01/And.hdl", "x")
            .with("/p/01/And.tst", "y")
            .with("/p/02/Add16.hdl", "z");
        let mut top = host.list("/p").unwrap();
        top.sort();
        assert_eq!(top.len(), 2);
        assert!(top.iter().all(|e| e.is_dir));
        assert_eq!(host.list("/p/01").unwrap().len(), 2);
        assert_eq!(host.list("/nope"), Err(HostError::NotFound));
        assert_eq!(host.read("/p/01/And.hdl").unwrap(), b"x");
        assert_eq!(host.read("\\p\\01\\And.hdl").unwrap(), b"x");
    }
}
