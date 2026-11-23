//! The WebAssembly boundary.
//!
//! Exports
//! - `nw_alloc(len) -> ptr` and `nw_free(ptr, len)`: memory for the embedder;
//! - `nw_call(ptr, len) -> ptr`: one request (UTF-8 JSON). The result points
//!   at a 4-byte little-endian length followed by that many bytes of UTF-8
//!   JSON, which the embedder frees with `nw_free(ptr, 4 + length)`.
//!
//! Imports (module `host`)
//! - `read(path_ptr, path_len, out) -> status`
//! - `list(path_ptr, path_len, out) -> status`
//! - `write(path_ptr, path_len, data_ptr, data_len, out) -> status`
//! - `log(ptr, len)`
//!
//! `status` is 0 for success, 1 for "not found", 2 for any other error. On
//! success or error the embedder allocates a buffer with `nw_alloc`, fills
//! it (file bytes; for `list` one entry per line, directories ending in `/`;
//! for an error the message) and stores its pointer and length as two
//! little-endian `u32`s at `out`. The engine frees it.
//!
//! WebAssembly here is single-threaded and a panic is a trap, which the
//! embedder handles by discarding the instance. Nothing is shared between
//! instances.

use std::cell::RefCell;
use std::sync::Arc;

use n2t_core::host::{DirEntry, Host, HostError};

use crate::Engine;

#[link(wasm_import_module = "host")]
unsafe extern "C" {
    fn read(path: *const u8, path_len: usize, out: *mut [u32; 2]) -> u32;
    fn list(path: *const u8, path_len: usize, out: *mut [u32; 2]) -> u32;
    fn write(
        path: *const u8,
        path_len: usize,
        data: *const u8,
        data_len: usize,
        out: *mut [u32; 2],
    ) -> u32;
    fn log(text: *const u8, len: usize);
}

struct WasmHost;

/// Takes ownership of a buffer the embedder allocated with `nw_alloc`.
fn take(out: [u32; 2]) -> Vec<u8> {
    let (ptr, len) = (out[0] as usize as *mut u8, out[1] as usize);
    if ptr.is_null() {
        return Vec::new();
    }
    // SAFETY: the embedder obtained `ptr` from `nw_alloc(len)` and filled it.
    unsafe { owned(ptr, len) }
}

fn result(status: u32, out: [u32; 2]) -> Result<Vec<u8>, HostError> {
    let data = take(out);
    match status {
        0 => Ok(data),
        1 => Err(HostError::NotFound),
        _ => Err(HostError::Other(
            String::from_utf8_lossy(&data).into_owned(),
        )),
    }
}

impl Host for WasmHost {
    fn read(&self, path: &str) -> Result<Vec<u8>, HostError> {
        let mut out = [0u32; 2];
        // SAFETY: `path` is valid for `path.len()` bytes and `out` for two u32s.
        let status = unsafe { read(path.as_ptr(), path.len(), &mut out) };
        result(status, out)
    }

    fn list(&self, dir: &str) -> Result<Vec<DirEntry>, HostError> {
        let mut out = [0u32; 2];
        // SAFETY: as in `read`.
        let status = unsafe { list(dir.as_ptr(), dir.len(), &mut out) };
        let data = result(status, out)?;
        Ok(String::from_utf8_lossy(&data)
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| match line.strip_suffix('/') {
                Some(name) => DirEntry {
                    name: name.to_string(),
                    is_dir: true,
                },
                None => DirEntry {
                    name: line.to_string(),
                    is_dir: false,
                },
            })
            .collect())
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), HostError> {
        let mut out = [0u32; 2];
        // SAFETY: both slices are valid for their lengths, `out` for two u32s.
        let status = unsafe {
            write(
                path.as_ptr(),
                path.len(),
                data.as_ptr(),
                data.len(),
                &mut out,
            )
        };
        result(status, out).map(|_| ())
    }
}

thread_local! {
    static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let message = format!("nandwright engine panic: {info}");
        // SAFETY: `message` is valid for its length.
        unsafe { log(message.as_ptr(), message.len()) };
    }));
}

/// Hands a buffer to the embedder. Buffers cross the boundary as boxed
/// slices, whose allocation is exactly their length.
fn leak(buffer: Vec<u8>) -> *mut u8 {
    Box::into_raw(buffer.into_boxed_slice()) as *mut u8
}

/// Takes back a buffer of exactly `len` bytes made by [`leak`].
///
/// # Safety
/// `ptr` must come from [`leak`] of a buffer of `len` bytes, and must not be
/// used afterwards.
unsafe fn owned(ptr: *mut u8, len: usize) -> Vec<u8> {
    // SAFETY: guaranteed by the caller.
    unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)) }.into_vec()
}

#[unsafe(no_mangle)]
pub extern "C" fn nw_alloc(len: usize) -> *mut u8 {
    leak(vec![0u8; len])
}

/// # Safety
/// `ptr` must come from `nw_alloc(len)` or be the result of `nw_call` with
/// `len` equal to four plus the length stored at `ptr`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nw_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        // SAFETY: guaranteed by the caller.
        drop(unsafe { owned(ptr, len) });
    }
}

/// # Safety
/// `ptr` must point at `len` bytes of UTF-8 allocated with `nw_alloc(len)`.
/// The engine takes ownership of that buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nw_call(ptr: *mut u8, len: usize) -> *mut u8 {
    // SAFETY: guaranteed by the caller.
    let request = unsafe { owned(ptr, len) };
    let request = String::from_utf8_lossy(&request);
    let reply = ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        let engine = engine.get_or_insert_with(|| {
            install_panic_hook();
            Engine::new(Arc::new(WasmHost))
        });
        engine.call(&request)
    });
    let mut buffer = Vec::with_capacity(4 + reply.len());
    buffer.extend_from_slice(&(reply.len() as u32).to_le_bytes());
    buffer.extend_from_slice(reply.as_bytes());
    leak(buffer)
}
