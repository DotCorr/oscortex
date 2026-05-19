//! .oscapp application registry — Phase 38.
//!
//! ## Bundle format (`*.oscapp`)
//!
//! A simple binary layout recognised by the magic `"OSCP"`:
//!
//! ```text
//! Offset  Size  Field
//! ------  ----  -----
//!  0       4    magic: 0x4F 0x53 0x43 0x50  ("OSCP")
//!  4       4    format_version: u32le = 1
//!  8      64    name:         [u8; 64]  (null-padded UTF-8)
//! 72      16    version:      [u8; 16]  (null-padded UTF-8)
//! 88       8    entry_offset: u64le     (byte offset of Dart entry in AOT)
//! 96       8    stack_size:   u64le     (0 → default 512 KiB)
//!104       4    aot_len:      u32le     (byte count of AOT snapshot)
//!108       4    _reserved:    u32le = 0
//!112+   aot_len AOT snapshot bytes
//! ```
//!
//! Everything after `112 + aot_len` is asset/metadata and is ignored by the
//! kernel in Phase 38.
//!
//! ## Syscall ABI
//!
//! | Number | Name            | Args                                  | Returns     |
//! |--------|-----------------|---------------------------------------|-------------|
//! | 0x36F  | app_install     | (bundle_ptr, bundle_len, id_out_ptr)  | 0 / -ERRNO  |
//! | 0x370  | app_list        | (buf_ptr, buf_len)                    | count       |
//! | 0x371  | app_launch      | (app_id, flags)                       | pid / -ERR  |
//! | 0x372  | app_uninstall   | (app_id)                              | 0 / -ERRNO  |

use spin::Mutex;
use alloc::vec::Vec;

// ── Bundle parsing ────────────────────────────────────────────────────────────

const MAGIC: &[u8; 4] = b"OSCP";
const HEADER_SIZE: usize = 112;

pub struct BundleHeader {
    pub name:         [u8; 64],
    pub version:      [u8; 16],
    pub entry_offset: u64,
    pub stack_size:   u64,
    pub aot_len:      u32,
}

/// Parse the fixed-size bundle header.  Returns `None` if the bundle is too
/// short, the magic is wrong, or the declared `aot_len` exceeds the buffer.
pub fn parse_header(bundle: &[u8]) -> Option<BundleHeader> {
    if bundle.len() < HEADER_SIZE { return None; }
    if &bundle[0..4] != MAGIC { return None; }
    // format_version at [4..8] — accept 1 only.
    let ver = u32::from_le_bytes(bundle[4..8].try_into().ok()?);
    if ver != 1 { return None; }

    let mut name    = [0u8; 64];
    let mut version = [0u8; 16];
    name.copy_from_slice(&bundle[8..72]);
    version.copy_from_slice(&bundle[72..88]);
    let entry_offset = u64::from_le_bytes(bundle[88..96].try_into().ok()?);
    let stack_size   = u64::from_le_bytes(bundle[96..104].try_into().ok()?);
    let aot_len      = u32::from_le_bytes(bundle[104..108].try_into().ok()?);

    if bundle.len() < HEADER_SIZE + aot_len as usize { return None; }
    Some(BundleHeader { name, version, entry_offset, stack_size, aot_len })
}

// ── App record ────────────────────────────────────────────────────────────────

pub struct AppRecord {
    pub id:           u32,
    pub name:         [u8; 64],
    pub version:      [u8; 16],
    pub entry_offset: u64,
    pub stack_size:   u64,
    /// Copy of the AOT snapshot, kept in kernel heap.
    pub aot_data:     Vec<u8>,
}

impl AppRecord {
    /// Null-terminated name as a str slice (best-effort).
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(64);
        core::str::from_utf8(&self.name[..end]).unwrap_or("<invalid>")
    }
    /// Null-terminated version as a str slice (best-effort).
    pub fn version_str(&self) -> &str {
        let end = self.version.iter().position(|&b| b == 0).unwrap_or(16);
        core::str::from_utf8(&self.version[..end]).unwrap_or("<invalid>")
    }
}

// ── Registry table ────────────────────────────────────────────────────────────

const MAX_APPS: usize = 64;

struct AppTable {
    slots: Vec<Option<AppRecord>>,
    next_id: u32,
}

impl AppTable {
    const fn empty() -> Self {
        Self { slots: Vec::new(), next_id: 1 }
    }

    fn find(&self, id: u32) -> Option<&AppRecord> {
        self.slots.iter().flatten().find(|r| r.id == id)
    }

    fn find_by_name(&self, name: &[u8]) -> Option<&AppRecord> {
        self.slots.iter().flatten().find(|r| {
            let end = r.name.iter().position(|&b| b == 0).unwrap_or(64);
            &r.name[..end] == name
        })
    }

    fn insert(&mut self, record: AppRecord) -> bool {
        if self.slots.iter().flatten().count() >= MAX_APPS { return false; }
        // Reuse a None slot first.
        for slot in self.slots.iter_mut() {
            if slot.is_none() {
                *slot = Some(record);
                return true;
            }
        }
        self.slots.push(Some(record));
        true
    }

    fn remove(&mut self, id: u32) -> bool {
        for slot in self.slots.iter_mut() {
            if slot.as_ref().map(|r| r.id == id).unwrap_or(false) {
                *slot = None;
                return true;
            }
        }
        false
    }
}

static APP_TABLE: Mutex<AppTable> = Mutex::new(AppTable::empty());

// ── Public API ────────────────────────────────────────────────────────────────

/// Install an `.oscapp` bundle from a byte slice.
///
/// Returns the new `app_id` on success, or `None` on parse error / table full.
pub fn install(bundle: &[u8]) -> Option<u32> {
    let hdr = parse_header(bundle)?;
    let aot_data: Vec<u8> = bundle[HEADER_SIZE..HEADER_SIZE + hdr.aot_len as usize].to_vec();

    let mut table = APP_TABLE.lock();

    // Reject duplicates by name.
    let name_end = hdr.name.iter().position(|&b| b == 0).unwrap_or(64);
    if table.find_by_name(&hdr.name[..name_end]).is_some() {
        return None; // already installed — caller should uninstall first
    }

    let id = table.next_id;
    table.next_id = table.next_id.wrapping_add(1).max(1);

    let record = AppRecord {
        id,
        name:         hdr.name,
        version:      hdr.version,
        entry_offset: hdr.entry_offset,
        stack_size:   if hdr.stack_size == 0 { 512 * 1024 } else { hdr.stack_size },
        aot_data,
    };
    if !table.insert(record) { return None; }

    log::info!("[APP] Installed '{}' id={}", {
        let end = hdr.name.iter().position(|&b| b == 0).unwrap_or(64);
        core::str::from_utf8(&hdr.name[..end]).unwrap_or("?")
    }, id);
    Some(id)
}

/// Uninstall an app by `app_id`.  Returns `true` if found and removed.
pub fn uninstall(app_id: u32) -> bool {
    let mut table = APP_TABLE.lock();
    let found = table.remove(app_id);
    if found {
        log::info!("[APP] Uninstalled app_id={}", app_id);
    }
    found
}

/// Serialise the app list into `buf` as packed 88-byte records:
/// `[id: u32le][name: u8×64][version: u8×16][_: u32le padding]`
///
/// Returns the number of installed apps (even if buf was too small to hold all).
pub fn list(buf: &mut [u8]) -> u32 {
    const RECORD_SZ: usize = 88; // 4 + 64 + 16 + 4 pad
    let table = APP_TABLE.lock();
    let mut count = 0u32;
    let mut offset = 0usize;
    for record in table.slots.iter().flatten() {
        count += 1;
        if offset + RECORD_SZ <= buf.len() {
            buf[offset..offset + 4].copy_from_slice(&record.id.to_le_bytes());
            buf[offset + 4..offset + 68].copy_from_slice(&record.name);
            buf[offset + 68..offset + 84].copy_from_slice(&record.version);
            buf[offset + 84..offset + 88].copy_from_slice(&[0u8; 4]);
            offset += RECORD_SZ;
        }
    }
    count
}

/// Launch an installed app.
///
/// Internally:
///   1. Copies the stored AOT snapshot into the current process's address space
///      (via `process::dl::mmap_anon`) so the Dart runtime can find it.
///   2. Spawns a new isolate with `crate::isolate::spawn`.
///   3. Registers the app as an active process.
///
/// Returns the new isolate ID, or -ERRNO on failure.
pub fn launch(app_id: u32, _flags: u32) -> i64 {
    // Lock only to clone the needed data, then release before calling into
    // process/isolate subsystems (which take their own locks).
    let (aot_data, entry_offset, stack_size, name) = {
        let table = APP_TABLE.lock();
        let record = match table.find(app_id) {
            Some(r) => r,
            None    => return -2, // ENOENT
        };
        (
            record.aot_data.clone(),
            record.entry_offset,
            record.stack_size,
            record.name,
        )
    };

    let pid = crate::process::current_pid();
    if pid == 0 { return -1; } // EPERM — must be called from userspace

    let pml4_phys = match crate::process::get_user_context(pid) {
        Some(ctx) => ctx.pml4_phys,
        None      => return -9, // EBADF
    };

    // Map AOT snapshot into the calling process's address space
    // (PROT_READ | PROT_EXEC = flags 5).
    let pages = (aot_data.len() + 4095) / 4096;
    let va = crate::process::dl::mmap_anon(pid, pml4_phys, 0, pages, 5);
    if va == u64::MAX { return -12; } // ENOMEM

    unsafe {
        let hhdm = crate::mm::frame_allocator::hhdm_offset();
        let dst = (va + hhdm) as *mut u8;
        core::ptr::copy_nonoverlapping(aot_data.as_ptr(), dst, aot_data.len());
    }

    // Spawn a Dart isolate pointing at the mapped AOT snapshot.
    let aot_size = aot_data.len() as u64;
    let stack_sz = stack_size as usize;
    match crate::isolate::spawn(pid, va, aot_size, entry_offset, stack_sz) {
        Ok(iso_id) => {
            let name_end = name.iter().position(|&b| b == 0).unwrap_or(64);
            let name_str = core::str::from_utf8(&name[..name_end]).unwrap_or("?");
            log::info!("[APP] Launched '{}' app_id={} isolate={}", name_str, app_id, iso_id);
            iso_id as i64
        }
        Err(e) => {
            log::warn!("[APP] launch failed: {}", e);
            -12 // ENOMEM / table full
        }
    }
}

/// Return the number of currently installed apps.
pub fn count() -> u32 {
    APP_TABLE.lock().slots.iter().flatten().count() as u32
}

/// Serialise a single app record into the provided 88-byte buffer.
/// Returns `true` on success.
pub fn get_info(app_id: u32, buf: &mut [u8; 88]) -> bool {
    let table = APP_TABLE.lock();
    let record = match table.find(app_id) {
        Some(r) => r,
        None => return false,
    };
    buf[0..4].copy_from_slice(&record.id.to_le_bytes());
    buf[4..68].copy_from_slice(&record.name);
    buf[68..84].copy_from_slice(&record.version);
    buf[84..88].copy_from_slice(&(record.aot_data.len() as u32).to_le_bytes());
    true
}
