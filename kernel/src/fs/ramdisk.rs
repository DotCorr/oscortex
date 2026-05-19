//! Phase 44 — In-memory writable ramdisk, mounted at `/tmp/`.
//!
//! Provides a fixed-capacity flat key-value store (path → bytes).
//! Up to 32 files, 512 KiB total.  Paths must start with `/tmp/`.

use alloc::vec::Vec;
use spin::Mutex;

// ── Storage ───────────────────────────────────────────────────────────────────

const MAX_FILES: usize = 32;
const MAX_TOTAL_BYTES: usize = 512 * 1024;
const MAX_PATH_LEN: usize = 128;

struct RamdiskEntry {
    /// NUL-padded path (absolute, `/tmp/…`).
    path: [u8; MAX_PATH_LEN],
    path_len: usize,
    data: Option<Vec<u8>>,
}

impl RamdiskEntry {
    const fn empty() -> Self {
        Self { path: [0; MAX_PATH_LEN], path_len: 0, data: None }
    }

    fn path_str(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len]).unwrap_or("")
    }
}

struct RamdiskState {
    entries: [RamdiskEntry; MAX_FILES],
    total_bytes: usize,
}

impl RamdiskState {
    fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

static RAMDISK: Mutex<RamdiskState> = Mutex::new(RamdiskState {
    entries: [const { RamdiskEntry::empty() }; MAX_FILES],
    total_bytes: 0,
});

// ── Public API ────────────────────────────────────────────────────────────────

/// Write (or overwrite) `data` to `path`.  Path must start with `/tmp/`.
pub fn write(path: &str, data: &[u8]) -> Result<(), &'static str> {
    if !path.starts_with("/tmp/") { return Err("not under /tmp/"); }
    if path.len() > MAX_PATH_LEN  { return Err("path too long"); }

    let mut rd = RAMDISK.lock();

    // Check if already exists — remove old byte count.
    let mut existing_idx: Option<usize> = None;
    for (i, e) in rd.entries.iter().enumerate() {
        if e.data.is_some() && e.path_str() == path {
            existing_idx = Some(i);
            break;
        }
    }

    let new_total = if let Some(idx) = existing_idx {
        let old_len = rd.entries[idx].data.as_ref().map(|v| v.len()).unwrap_or(0);
        rd.total_bytes - old_len + data.len()
    } else {
        rd.total_bytes + data.len()
    };

    if new_total > MAX_TOTAL_BYTES { return Err("ramdisk full"); }

    if let Some(idx) = existing_idx {
        let old_len = rd.entries[idx].data.as_ref().map(|v| v.len()).unwrap_or(0);
        rd.total_bytes -= old_len;
        let new_data = data.to_vec();
        rd.total_bytes += new_data.len();
        rd.entries[idx].data = Some(new_data);
        return Ok(());
    }

    // Find empty slot.
    for entry in rd.entries.iter_mut() {
        if entry.data.is_none() && entry.path_len == 0 {
            let pb = path.as_bytes();
            entry.path[..pb.len()].copy_from_slice(pb);
            entry.path_len = pb.len();
            entry.data = Some(data.to_vec());
            rd.total_bytes += data.len();
            return Ok(());
        }
    }
    Err("ramdisk slot table full")
}

/// Look up a file.  Returns a static-lifetime reference by leaking the data.
///
/// **Safety note**: We leak the Vec so the returned slice lives as long as the
/// entry (until overwritten).  Since entries are in a static Mutex, this is safe
/// as long as callers do not hold the reference across a `write()` to the same
/// path.  In practice, syscall callers copy the data out before returning.
pub fn lookup_static(path: &str) -> Option<&'static [u8]> {
    let rd = RAMDISK.lock();
    for entry in rd.entries.iter() {
        if entry.data.is_some() && entry.path_str() == path {
            let ptr = entry.data.as_ref().unwrap().as_ptr();
            let len = entry.data.as_ref().unwrap().len();
            // SAFETY: The Vec lives inside the static RAMDISK Mutex.
            // The returned slice is valid as long as the entry is not freed.
            return Some(unsafe { core::slice::from_raw_parts(ptr, len) });
        }
    }
    None
}

/// Stat a path: returns `Some(size)` if it exists.
pub fn stat(path: &str) -> Option<u64> {
    let rd = RAMDISK.lock();
    for entry in rd.entries.iter() {
        if entry.data.is_some() && entry.path_str() == path {
            return Some(entry.data.as_ref().unwrap().len() as u64);
        }
    }
    None
}

/// List files whose path starts with `prefix`, writing "rel_name\n" to `out`.
pub fn list_prefix(prefix: &str, out: *mut u8, cap: usize) -> usize {
    if cap == 0 || out.is_null() { return 0; }
    let prefix_norm = prefix.trim_end_matches('/');
    let rd = RAMDISK.lock();
    let mut written = 0usize;
    for entry in rd.entries.iter() {
        if entry.data.is_none() { continue; }
        let p = entry.path_str();
        if p.starts_with(prefix_norm) {
            let rest = p[prefix_norm.len()..].trim_start_matches('/');
            if rest.is_empty() { continue; }
            let needed = rest.len() + 1;
            if written + needed > cap { break; }
            unsafe {
                core::ptr::copy_nonoverlapping(rest.as_ptr(), out.add(written), rest.len());
                out.add(written + rest.len()).write(b'\n');
            }
            written += needed;
        }
    }
    written
}
