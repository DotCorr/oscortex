//! System-wide clipboard.
//!
//! A single kernel-owned UTF-8 text buffer shared by every process. Unlike a
//! per-embedder buffer, copying text in one app and pasting it in another (or in
//! the shell) works, because the bytes live in the kernel, not in any one host's
//! address space. Backs the Flutter `flutter/platform` `Clipboard.setData` /
//! `Clipboard.getData` / `Clipboard.hasStrings` methods.

use spin::Mutex;
use alloc::vec::Vec;

use crate::embedder::abi::CLIPBOARD_MAX;

static CLIPBOARD: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// Replace the clipboard contents with `data` (truncated to `CLIPBOARD_MAX`).
pub fn set(data: &[u8]) {
    let n = data.len().min(CLIPBOARD_MAX);
    let mut c = CLIPBOARD.lock();
    c.clear();
    c.extend_from_slice(&data[..n]);
}

/// Copy the clipboard contents into `out`, returning the FULL stored length
/// (which may exceed `out.len()` — the caller can detect truncation and retry).
pub fn get(out: &mut [u8]) -> usize {
    let c = CLIPBOARD.lock();
    let n = c.len().min(out.len());
    out[..n].copy_from_slice(&c[..n]);
    c.len()
}

/// Number of bytes currently stored.
pub fn len() -> usize {
    CLIPBOARD.lock().len()
}

/// True if the clipboard holds any text.
pub fn has_text() -> bool {
    !CLIPBOARD.lock().is_empty()
}
