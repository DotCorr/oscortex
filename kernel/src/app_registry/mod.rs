//! `.osx` application registry.
//!
//! ## Bundle format (`*.osx`)
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

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ── Bundle parsing ────────────────────────────────────────────────────────────

const MAGIC: &[u8; 4] = b"OSCP";
const HEADER_SIZE: usize = 112;

/// Host bootstrap: shell mode (rdi) — see `tools/flutter-embedder`.
pub const HOST_MODE_SHELL: u64 = 1;
/// Host bootstrap: user app mode (rdi) with app_id in rsi.
pub const HOST_MODE_APP: u64 = 2;
const HOST_ELF_PATH: &str = "/bin/oscortex-host";

pub struct BundleHeader {
    pub name: [u8; 64],
    pub version: [u8; 16],
    pub entry_offset: u64,
    pub stack_size: u64,
    pub aot_len: u32,
}

/// Parse the fixed-size bundle header.  Returns `None` if the bundle is too
/// short, the magic is wrong, or the declared `aot_len` exceeds the buffer.
pub fn parse_header(bundle: &[u8]) -> Option<BundleHeader> {
    if bundle.len() < HEADER_SIZE {
        return None;
    }
    if &bundle[0..4] != MAGIC {
        return None;
    }
    // format_version at [4..8] — accept 1 only.
    let ver = u32::from_le_bytes(bundle[4..8].try_into().ok()?);
    if ver != 1 {
        return None;
    }

    let mut name = [0u8; 64];
    let mut version = [0u8; 16];
    name.copy_from_slice(&bundle[8..72]);
    version.copy_from_slice(&bundle[72..88]);
    let entry_offset = u64::from_le_bytes(bundle[88..96].try_into().ok()?);
    let stack_size = u64::from_le_bytes(bundle[96..104].try_into().ok()?);
    let aot_len = u32::from_le_bytes(bundle[104..108].try_into().ok()?);

    if bundle.len() < HEADER_SIZE + aot_len as usize {
        return None;
    }
    Some(BundleHeader {
        name,
        version,
        entry_offset,
        stack_size,
        aot_len,
    })
}

// ── App record ────────────────────────────────────────────────────────────────

pub struct AppRecord {
    pub id: u32,
    pub name: [u8; 64],
    pub version: [u8; 16],
    pub entry_offset: u64,
    pub stack_size: u64,
    pub system: bool,
    /// Copy of the AOT snapshot, kept in kernel heap.
    pub aot_data: Vec<u8>,
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
        Self {
            slots: Vec::new(),
            next_id: 1,
        }
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
        if self.slots.iter().flatten().count() >= MAX_APPS {
            return false;
        }
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

    fn allocate_id(&mut self) -> u32 {
        loop {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1).max(1);
            if self.find(id).is_none() {
                return id;
            }
        }
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

fn install_record(bundle: &[u8], id: u32, system: bool) -> Option<u32> {
    let hdr = parse_header(bundle)?;
    let aot_data: Vec<u8> = bundle[HEADER_SIZE..HEADER_SIZE + hdr.aot_len as usize].to_vec();
    let mut table = APP_TABLE.lock();
    if table.find(id).is_some() {
        return None;
    }

    let name_end = hdr.name.iter().position(|&b| b == 0).unwrap_or(64);
    if table.find_by_name(&hdr.name[..name_end]).is_some() {
        return None;
    }

    let record = AppRecord {
        id,
        name: hdr.name,
        version: hdr.version,
        entry_offset: hdr.entry_offset,
        stack_size: if hdr.stack_size == 0 {
            512 * 1024
        } else {
            hdr.stack_size
        },
        system,
        aot_data,
    };
    if !table.insert(record) {
        return None;
    }
    if table.next_id <= id {
        table.next_id = id.saturating_add(1).max(1);
    }
    Some(id)
}

fn install_record_auto_id(bundle: &[u8], system: bool) -> Option<u32> {
    let hdr = parse_header(bundle)?;
    let aot_data: Vec<u8> = bundle[HEADER_SIZE..HEADER_SIZE + hdr.aot_len as usize].to_vec();
    let mut table = APP_TABLE.lock();

    let name_end = hdr.name.iter().position(|&b| b == 0).unwrap_or(64);
    if table.find_by_name(&hdr.name[..name_end]).is_some() {
        return None;
    }

    let id = table.allocate_id();
    let record = AppRecord {
        id,
        name: hdr.name,
        version: hdr.version,
        entry_offset: hdr.entry_offset,
        stack_size: if hdr.stack_size == 0 {
            512 * 1024
        } else {
            hdr.stack_size
        },
        system,
        aot_data,
    };
    if !table.insert(record) {
        return None;
    }
    Some(id)
}

/// Install with a fixed id (used when hydrating from block store).
pub fn install_from_store(bundle: &[u8], id: u32) -> Option<u32> {
    install_record(bundle, id, false)
}

/// Install a read-only system app from the embedded filesystem.
pub fn install_system_from_path(path: &str, id: u32) -> Option<u32> {
    let bundle = crate::fs::lookup(path)?;
    let installed = install_record(bundle, id, true)?;
    log::info!("[APP] Seeded system app '{}' id={}", path, installed);
    Some(installed)
}

/// Install a read-only system app from the embedded filesystem using the next
/// registry id. This is used for additional `.osx` bundles found under
/// `/Applications` after the stable core app IDs are reserved.
pub fn install_system_from_path_auto(path: &str) -> Option<u32> {
    let bundle = crate::fs::lookup(path)?;
    let installed = install_record_auto_id(bundle, true)?;
    log::info!("[APP] Discovered system app '{}' id={}", path, installed);
    Some(installed)
}

/// Seed core system apps that live in the OS Applications directory.
pub fn install_system_apps() {
    const SYSTEM_APPS: &[(&str, u32)] = &[
        ("/Applications/Canvas.app/Canvas.osx", 1),
        ("/Applications/Files.app/Files.osx", 2),
        ("/Applications/Web Link.app/Web Link.osx", 3),
    ];

    for (path, id) in SYSTEM_APPS {
        if install_system_from_path(path, *id).is_none() {
            log::warn!("[APP] System app seed skipped: {}", path);
        }
    }

    discover_applications_dir();
}

fn discover_applications_dir() {
    const PREFIX: &str = "/Applications";
    const BUF_LEN: usize = 32 * 1024;
    let mut buf = alloc::vec![0u8; BUF_LEN];
    let written = crate::fs::list_prefix(PREFIX, buf.as_mut_ptr(), buf.len()).min(buf.len());
    if written == 0 {
        log::info!("[APP] No additional /Applications bundles discovered");
        return;
    }

    let mut discovered = 0u32;
    for line in buf[..written].split(|&b| b == b'\n') {
        if !line.ends_with(b".osx") {
            continue;
        }
        let Ok(rel) = core::str::from_utf8(line) else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        let mut path = String::from(PREFIX);
        path.push('/');
        path.push_str(rel);
        if install_system_from_path_auto(&path).is_some() {
            discovered += 1;
        }
    }

    log::info!(
        "[APP] /Applications discovery complete: {} additional app(s)",
        discovered
    );
}

/// Install a `.osx` bundle from a byte slice.
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
        name: hdr.name,
        version: hdr.version,
        entry_offset: hdr.entry_offset,
        stack_size: if hdr.stack_size == 0 {
            512 * 1024
        } else {
            hdr.stack_size
        },
        system: false,
        aot_data,
    };
    if !table.insert(record) {
        return None;
    }

    let _ = crate::app_store::persist_bundle(id, bundle);

    log::info!(
        "[APP] Installed '{}' id={}",
        {
            let end = hdr.name.iter().position(|&b| b == 0).unwrap_or(64);
            core::str::from_utf8(&hdr.name[..end]).unwrap_or("?")
        },
        id
    );
    Some(id)
}

/// Uninstall an app by `app_id`.  Returns `true` if found and removed.
pub fn uninstall(app_id: u32) -> bool {
    let mut table = APP_TABLE.lock();
    if table.find(app_id).map(|r| r.system).unwrap_or(false) {
        log::warn!("[APP] Refusing to uninstall system app_id={}", app_id);
        return false;
    }
    let found = table.remove(app_id);
    if found {
        let _ = crate::app_store::remove_bundle(app_id);
        log::info!("[APP] Uninstalled app_id={}", app_id);
    }
    found
}

/// Serialise the app list into `buf` as packed 88-byte records:
/// `[id: u32le][name: u8×64][version: u8×16][flags: u32le]`
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
            let flags = if record.system { 1u32 } else { 0u32 };
            buf[offset + 84..offset + 88].copy_from_slice(&flags.to_le_bytes());
            offset += RECORD_SZ;
        }
    }
    count
}

/// Map an installed app's AOT payload into `pid`'s address space.
/// Returns `(va, byte_len)` on success.
pub fn map_aot_into_process(app_id: u32, pid: u32) -> Result<(u64, u64), i64> {
    let aot_data = {
        let table = APP_TABLE.lock();
        let record = table.find(app_id).ok_or(-2i64)?;
        record.aot_data.clone()
    };

    let pml4_phys = match crate::process::get_user_context(pid) {
        Some(ctx) => ctx.pml4_phys,
        None => return Err(-3),
    };

    // Load the AOT ELF and register it globally in the dynamic linker's LIBS table.
    let handle = match crate::process::dl::dlopen(pid, pml4_phys, &aot_data, b"/system/flutter/libapp.so") {
        Ok(h) => h,
        Err(e) => {
            log::warn!("[APP] dlopen app AOT failed: {}", e);
            return Err(-12);
        }
    };

    let va = crate::process::dl::get_load_base(handle, pid).ok_or(-12i64)?;

    Ok((va, aot_data.len() as u64))
}

/// Launch an installed app in a **new** Flutter host process.
///
/// Spawns `/bin/oscortex-host` with bootstrap `(HOST_MODE_APP, app_id)`, maps
/// the stored AOT ELF into that process, and returns the new PID.
pub fn launch(app_id: u32, _flags: u32) -> i64 {
    let name = {
        let table = APP_TABLE.lock();
        let record = match table.find(app_id) {
            Some(r) => r,
            None => return -2,
        };
        record.name
    };

    let elf = match crate::fs::lookup(HOST_ELF_PATH) {
        Some(data) => data,
        None => {
            log::error!("[APP] host binary missing: {}", HOST_ELF_PATH);
            return -2;
        }
    };

    let bootstrap = crate::process::SpawnBootstrap {
        rdi: HOST_MODE_APP,
        rsi: app_id as u64,
        rdx: 0,
        parent_pid: 1,
    };

    let child_pid = match crate::process::spawn_with_bootstrap(elf, "oscortex-host", bootstrap) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("[APP] host spawn failed: {}", e);
            return -12;
        }
    };

    let (aot_va, _aot_size) = match map_aot_into_process(app_id, child_pid) {
        Ok(v) => v,
        Err(e) => {
            let _ = crate::process::kill(child_pid);
            return e;
        }
    };

    crate::process::set_bootstrap_regs(child_pid, HOST_MODE_APP, app_id as u64, aot_va);

    crate::wm::push_app_event(child_pid, crate::embedder::abi::APP_LAUNCH, 0);

    // Switch focus to the freshly launched app. This does double duty:
    //  (1) the compositor mirrors the focused pid's surface, so the app becomes
    //      visible once it produces its first frame, and
    //  (2) the scheduler / APIC-timer preemption prioritises the focused pid,
    //      giving the new host the CPU it needs to JIT-warm up — without this the
    //      shell (still frame-pumping) starves the launched app on a single core
    //      (observed: shell 2644 syscalls vs app 37, app never even starts JIT).
    crate::wm::set_focus_pid(child_pid);

    let name_end = name.iter().position(|&b| b == 0).unwrap_or(64);
    let name_str = core::str::from_utf8(&name[..name_end]).unwrap_or("?");
    log::info!(
        "[APP] Launched '{}' app_id={} host_pid={}",
        name_str,
        app_id,
        child_pid
    );
    child_pid as i64
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
