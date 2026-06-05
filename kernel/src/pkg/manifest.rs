//! Package manifest and catalog format.
//!
//! The catalog is a simple binary blob served by the package server:
//!
//! ```text
//! [4 bytes: entry_count LE]
//! [N × PkgManifest (128 bytes each)]
//! ```

use alloc::vec::Vec;

/// Size of a single manifest entry in the binary catalog.
pub const MANIFEST_SIZE: usize = 128;

/// Package flags.
pub const FLAG_SYSTEM: u32 = 0x01; // system app — never evicted

/// A single package manifest entry.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct PkgManifest {
    /// Fully-qualified package name (e.g. "com.dotcorr.files"), null-padded.
    pub name: [u8; 64],
    /// Semantic version string, null-padded.
    pub version: [u8; 16],
    /// SHA-256 hash of the `.osx` bundle bytes.
    pub hash: [u8; 32],
    /// Bundle size in bytes (for progress tracking).
    pub size_bytes: u32,
    /// Package flags (see `FLAG_*` constants).
    pub flags: u32,
    /// Reserved for future use.
    pub _reserved: [u8; 8],
}

impl PkgManifest {
    /// Package name as a `&str`.
    pub fn name_str(&self) -> &str {
        let name = unsafe { core::ptr::addr_of!(self.name).read_unaligned() };
        let end = name.iter().position(|&b| b == 0).unwrap_or(64);
        // SAFETY: we just copied the bytes, so the slice is aligned.
        unsafe {
            let ptr = core::ptr::addr_of!(self.name) as *const u8;
            core::str::from_utf8(core::slice::from_raw_parts(ptr, end)).unwrap_or("<invalid>")
        }
    }

    /// Version as a `&str`.
    pub fn version_str(&self) -> &str {
        let version = unsafe { core::ptr::addr_of!(self.version).read_unaligned() };
        let end = version.iter().position(|&b| b == 0).unwrap_or(16);
        unsafe {
            let ptr = core::ptr::addr_of!(self.version) as *const u8;
            core::str::from_utf8(core::slice::from_raw_parts(ptr, end)).unwrap_or("?")
        }
    }

    /// Whether this is a system (pinned) package.
    pub fn is_system(&self) -> bool {
        let flags = unsafe { core::ptr::addr_of!(self.flags).read_unaligned() };
        flags & FLAG_SYSTEM != 0
    }
}

/// Parse a binary catalog blob into a list of manifests.
///
/// Format: `[u32le count][count × PkgManifest]`
pub fn parse_catalog(data: &[u8]) -> Option<Vec<PkgManifest>> {
    if data.len() < 4 {
        return None;
    }
    let count = u32::from_le_bytes(data[0..4].try_into().ok()?) as usize;
    let expected_len = 4 + count * MANIFEST_SIZE;
    if data.len() < expected_len {
        log::warn!("[pkg] catalog truncated: expected {} bytes, got {}", expected_len, data.len());
        return None;
    }

    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let offset = 4 + i * MANIFEST_SIZE;
        let entry = unsafe {
            core::ptr::read_unaligned(data[offset..].as_ptr() as *const PkgManifest)
        };
        entries.push(entry);
    }

    log::info!("[pkg] parsed catalog: {} packages", count);
    Some(entries)
}

/// Serialise a list of manifests into the binary catalog format.
/// Used by the host-side pkg-server.
pub fn serialize_catalog(entries: &[PkgManifest]) -> Vec<u8> {
    let count = entries.len() as u32;
    let mut buf = Vec::with_capacity(4 + entries.len() * MANIFEST_SIZE);
    buf.extend_from_slice(&count.to_le_bytes());
    for entry in entries {
        let bytes = unsafe {
            core::slice::from_raw_parts(
                entry as *const PkgManifest as *const u8,
                MANIFEST_SIZE,
            )
        };
        buf.extend_from_slice(bytes);
    }
    buf
}
