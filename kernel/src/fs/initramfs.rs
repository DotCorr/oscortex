//! Embedded initramfs — USTAR (POSIX tar) reader.
//!
//! The kernel embeds a small tar archive at compile time.  At runtime the
//! USTAR reader walks the 512-byte headers to find files by name.
//!
//! ## Building the initramfs
//!
//! A helper script (`scripts/make-initramfs.sh`) creates the tar from the
//! `initramfs/` directory in the repo root:
//! ```sh
//! cd initramfs && tar cf ../initramfs.tar *
//! ```
//! If the file does not exist at build time, a minimal placeholder is used.

/// Trait implemented by the initramfs singleton so `fs::mount` can store it.
pub trait RamFs: Send + Sync {
    fn name(&self) -> &str;
    fn find(&self, path: &str) -> Option<&'static [u8]>;
}

// ── Embedded tar data ─────────────────────────────────────────────────────────

/// The initramfs is embedded at compile time.  The `build.rs` generates
/// `initramfs.tar` in `OUT_DIR` if the real one is missing.
static INITRAMFS_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/initramfs.tar"));

// ── USTAR header (512 bytes) ──────────────────────────────────────────────────

const BLOCK: usize = 512;

/// Parse an octal ASCII field (e.g. the file-size field in a tar header).
fn parse_octal(s: &[u8]) -> u64 {
    let mut val = 0u64;
    for &b in s {
        if b == 0 || b == b' ' { break; }
        if b >= b'0' && b <= b'7' {
            val = val * 8 + (b - b'0') as u64;
        }
    }
    val
}

// ── USTAR parser ─────────────────────────────────────────────────────────────

/// Walk the embedded tar and find `path`.  Returns the raw file bytes.
pub fn find_in_tar(data: &'static [u8], path: &str) -> Option<&'static [u8]> {
    let mut pos = 0usize;
    while pos + BLOCK <= data.len() {
        let header = &data[pos..pos + BLOCK];
        // First byte of name == 0 → end of archive.
        if header[0] == 0 { break; }

        // File name: bytes 0..100, NUL-terminated.
        let name_bytes = &header[0..100];
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(100);
        let name = core::str::from_utf8(&name_bytes[..name_len]).unwrap_or("");

        // File size: bytes 124..135 (octal ASCII).
        let size = parse_octal(&header[124..135]) as usize;

        // Type flag: byte 156. '0' or '\0' = regular file.
        let typeflag = header[156];
        let is_file = typeflag == b'0' || typeflag == 0;

        let data_start = pos + BLOCK;
        let data_end   = data_start + size;

        // Match: strip leading "./" if present.
        let cmp = name.trim_start_matches("./");
        let want = path.trim_start_matches('/').trim_start_matches("./");
        if is_file && cmp == want && data_end <= data.len() {
            return Some(&data[data_start..data_end]);
        }

        // Skip to next header (round up to 512-byte boundary).
        let blocks_used = size.div_ceil(BLOCK);
        pos += BLOCK + blocks_used * BLOCK;
    }
    None
}

/// Walk the embedded tar and list every regular file whose path starts with
/// `prefix`.  Each matching name (relative to `prefix`, without leading `/`)
/// is written to `out` as `"name\n"`.  Returns total bytes written.
pub fn list_in_tar(data: &'static [u8], prefix: &str, out: *mut u8, cap: usize) -> usize {
    let prefix = prefix.trim_start_matches('/');
    let mut written = 0usize;
    let mut pos = 0usize;
    while pos + BLOCK <= data.len() {
        let header = &data[pos..pos + BLOCK];
        if header[0] == 0 { break; }

        let name_bytes = &header[0..100];
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(100);
        let name = core::str::from_utf8(&name_bytes[..name_len]).unwrap_or("");
        let size = parse_octal(&header[124..135]) as usize;
        let typeflag = header[156];
        let is_file = typeflag == b'0' || typeflag == 0;

        let cmp = name.trim_start_matches("./");

        if is_file {
            let rel = if prefix.is_empty() {
                Some(cmp)
            } else if cmp.starts_with(prefix) {
                let rest = cmp[prefix.len()..].trim_start_matches('/');
                if !rest.is_empty() { Some(rest) } else { None }
            } else {
                None
            };
            if let Some(rel_name) = rel {
                let line = rel_name.as_bytes();
                let needed = line.len() + 1; // +1 for '\n'
                if written + needed <= cap {
                    unsafe {
                        core::ptr::copy_nonoverlapping(line.as_ptr(), out.add(written), line.len());
                        out.add(written + line.len()).write(b'\n');
                    }
                    written += needed;
                }
            }
        }

        let blocks_used = size.div_ceil(BLOCK);
        pos += BLOCK + blocks_used * BLOCK;
    }
    written
}



/// Convenience wrapper: list files in the embedded initramfs tar.
pub fn list_in_embedded(prefix: &str, out: *mut u8, cap: usize) -> usize {
    list_in_tar(INITRAMFS_DATA, prefix, out, cap)
}

struct EmbeddedRamFs;

impl RamFs for EmbeddedRamFs {
    fn name(&self) -> &str { "initramfs" }
    fn find(&self, path: &str) -> Option<&'static [u8]> {
        find_in_tar(INITRAMFS_DATA, path)
    }
}

static EMBEDDED_RAMFS: EmbeddedRamFs = EmbeddedRamFs;

/// Called from `fs::init()` — mounts the embedded tar at `/`.
pub fn mount_embedded() {
    crate::fs::mount("/", &EMBEDDED_RAMFS);
}
