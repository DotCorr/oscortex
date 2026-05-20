//! Virtual Filesystem — M11.
//!
//! Provides a minimal VFS layer:
//! * [`VfsNode`] trait — anything that can be read/written/stated.
//! * [`mount`] / [`lookup`] — global mount table.
//!
//! On top of this, `initramfs` (USTAR tar format) is the first concrete
//! filesystem.  The initramfs is embedded directly in the kernel binary via
//! `include_bytes!` so no disk driver is needed for early boot.

pub mod initramfs;
pub mod ramdisk;
pub mod ext2;  // Phase 51: ext2 read-only over virtio-blk

use spin::Mutex;

// ── VFS node trait ────────────────────────────────────────────────────────────

/// A VFS node: file or directory.
pub trait VfsNode: Send + Sync {
    fn read(&self, buf: &mut [u8], offset: u64) -> Result<usize, &'static str>;
    fn write(&self, buf: &[u8], offset: u64) -> Result<usize, &'static str>;
    fn size(&self) -> u64;
    fn is_dir(&self) -> bool;
}

// ── Mount table ───────────────────────────────────────────────────────────────

const MAX_MOUNTS: usize = 16;

struct MountEntry {
    prefix: &'static str,
    fs:     &'static dyn initramfs::RamFs,
}

static MOUNTS: Mutex<[Option<MountEntry>; MAX_MOUNTS]> =
    Mutex::new([const { None }; MAX_MOUNTS]);

/// Mount a filesystem at `prefix` (e.g. `"/"`).
pub fn mount(prefix: &'static str, fs: &'static dyn initramfs::RamFs) {
    let mut mounts = MOUNTS.lock();
    for slot in mounts.iter_mut() {
        if slot.is_none() {
            *slot = Some(MountEntry { prefix, fs });
            log::info!("[VFS] Mounted '{}' at '{}'", fs.name(), prefix);
            return;
        }
    }
    log::error!("[VFS] Mount table full — cannot mount '{}' at '{}'", fs.name(), prefix);
}

/// Look up a file by absolute path.  Returns a byte slice of the file content.
///
/// Virtual `/sys/*` paths are handled before the mount table (Phase 40).
/// `/tmp/*` paths are handled by the ramdisk (Phase 44).
pub fn lookup(path: &str) -> Option<&'static [u8]> {
    // Phase 40: sysfs virtual paths — generated on demand.
    if let Some(data) = sysfs_lookup(path) {
        return Some(data);
    }
    // Phase 44: ramdisk at /tmp/.
    if path.starts_with("/tmp/") || path == "/tmp" {
        return ramdisk::lookup_static(path);
    }

    let mounts = MOUNTS.lock();
    for entry in mounts.iter().flatten() {
        let rel = path.strip_prefix(entry.prefix).unwrap_or(path);
        if let Some(data) = entry.fs.find(rel) {
            return Some(data);
        }
    }
    None
}

/// List files whose path starts with `prefix`, writing "name\n" entries into
/// `out[..cap]`.  Returns total bytes written.
///
/// Sources searched (in order): initramfs, ramdisk.
pub fn list_prefix(prefix: &str, out: *mut u8, cap: usize) -> usize {
    if cap == 0 || out.is_null() { return 0; }
    let mut written = 0usize;
    // Initramfs (embedded static data).
    written += initramfs::list_in_embedded(prefix, unsafe { out.add(written) }, cap - written);
    // Ramdisk /tmp/.
    if written < cap {
        written += ramdisk::list_prefix(prefix, unsafe { out.add(written) }, cap - written);
    }
    written
}

/// Write `data` to path `path` in the writable ramdisk (Phase 44).
/// Only `/tmp/…` paths are accepted; returns `Err` for read-only mounts.
pub fn write(path: &str, data: &[u8]) -> Result<(), &'static str> {
    if !path.starts_with("/tmp/") {
        return Err("read-only: only /tmp/ is writable");
    }
    ramdisk::write(path, data)
}

/// Stat a VFS path: returns `Some(size_in_bytes)` if found.
pub fn stat(path: &str) -> Option<u64> {
    if path.starts_with("/tmp/") || path == "/tmp" {
        return ramdisk::stat(path);
    }
    lookup(path).map(|d| d.len() as u64)
}

/// True if `path` is a directory in any mounted filesystem.
pub fn is_dir(path: &str) -> bool {
    // Strip a mount prefix if present and probe each mount.
    let mounts = MOUNTS.lock();
    for entry in mounts.iter().flatten() {
        let rel = path.strip_prefix(entry.prefix).unwrap_or(path);
        if initramfs::is_dir_in_embedded(rel) { return true; }
    }
    initramfs::is_dir_in_embedded(path)
}

// ── Phase 40: sysfs ──────────────────────────────────────────────────────────
//
// Each virtual path is backed by a static buffer that is filled on first access.
// For Phase 40 we use simple newline-delimited text (not JSON) for debuggability.

use core::sync::atomic::{AtomicBool, Ordering as AOrdering};

macro_rules! sysfs_static {
    ($name:ident, $cap:expr) => {
        mod $name {
            pub static DATA:  spin::Mutex<[u8; $cap]> = spin::Mutex::new([0u8; $cap]);
            pub static READY: core::sync::atomic::AtomicBool =
                core::sync::atomic::AtomicBool::new(false);
            pub static LEN:   core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
        }
    };
}

sysfs_static!(sys_kernel_version, 128);
sysfs_static!(sys_input_devices,  512);
sysfs_static!(sys_process_list,   2048);
sysfs_static!(sys_app_list,       4096);
sysfs_static!(sys_usb_controllers, 512);

/// Write a decimal u32 into `buf` at `offset`.  Returns bytes written.
fn write_u32(buf: &mut [u8], offset: usize, mut v: u32) -> usize {
    if v == 0 {
        buf[offset] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 10];
    let mut n = 0usize;
    while v > 0 { tmp[n] = b'0' + (v % 10) as u8; v /= 10; n += 1; }
    for i in 0..n { buf[offset + i] = tmp[n - 1 - i]; }
    n
}

fn fill_sys_kernel_version() {
    let mut buf = sys_kernel_version::DATA.lock();
    let s = concat!("OSCortex ", env!("CARGO_PKG_VERSION"), "\n");
    let b = s.as_bytes();
    let len = b.len().min(128);
    buf[..len].copy_from_slice(&b[..len]);
    sys_kernel_version::LEN.store(len, AOrdering::Release);
    sys_kernel_version::READY.store(true, AOrdering::Release);
}

fn fill_sys_input_devices() {
    let mut buf = sys_input_devices::DATA.lock();
    let count = crate::drivers::ps2::device_count();
    let mut off = 0usize;
    let hdr = b"# PS/2 input devices\n";
    buf[..hdr.len()].copy_from_slice(hdr); off += hdr.len();
    for i in 0..count {
        let info = crate::drivers::ps2::device_info_packed(i);
        let typ  = info & 0xF;
        let irq  = (info >> 4) & 0xFF;
        let kind = if typ == 1 { b"keyboard" as &[u8] } else { b"mouse" as &[u8] };
        buf[off..off + kind.len()].copy_from_slice(kind); off += kind.len();
        buf[off] = b' '; off += 1;
        buf[off..off + 4].copy_from_slice(b"irq="); off += 4;
        off += write_u32(&mut buf[..], off, irq);
        buf[off] = b'\n'; off += 1;
    }
    sys_input_devices::LEN.store(off, AOrdering::Release);
    sys_input_devices::READY.store(true, AOrdering::Release);
}

fn fill_sys_process_list() {
    let mut buf = sys_process_list::DATA.lock();
    let mut off = 0usize;
    let hdr = b"# Running processes\n";
    buf[..hdr.len()].copy_from_slice(hdr); off += hdr.len();
    for pid in 1u32..=255u32 {
        if crate::process::get_user_context(pid).is_some() {
            buf[off..off + 4].copy_from_slice(b"pid="); off += 4;
            off += write_u32(&mut buf[..], off, pid);
            buf[off] = b'\n'; off += 1;
            if off + 32 >= 2048 { break; }
        }
    }
    sys_process_list::LEN.store(off, AOrdering::Release);
    sys_process_list::READY.store(true, AOrdering::Release);
}

fn fill_sys_app_list() {
    let mut buf = sys_app_list::DATA.lock();
    let n = crate::app_registry::count();
    let hdr = b"# Installed apps\ncount=";
    let mut off = 0usize;
    buf[..hdr.len()].copy_from_slice(hdr); off += hdr.len();
    off += write_u32(&mut buf[..], off, n);
    buf[off] = b'\n'; off += 1;
    sys_app_list::LEN.store(off, AOrdering::Release);
    sys_app_list::READY.store(true, AOrdering::Release);
}

fn fill_sys_usb_controllers() {
    let mut buf = sys_usb_controllers::DATA.lock();
    let n = crate::drivers::usb::xhci_count();
    let hdr = b"# USB XHCI controllers\ncount=";
    let mut off = 0usize;
    buf[..hdr.len()].copy_from_slice(hdr); off += hdr.len();
    off += write_u32(&mut buf[..], off, n);
    buf[off] = b'\n'; off += 1;
    sys_usb_controllers::LEN.store(off, AOrdering::Release);
    sys_usb_controllers::READY.store(true, AOrdering::Release);
}

/// Helper: return a stable &'static [u8] slice backed by a sysfs static buffer.
/// The buffer is regenerated on every call (single-writer, no caching needed
/// for Phase 40 — reads are infrequent debug/tool calls).
macro_rules! sysfs_serve {
    ($mod:ident, $fill:ident) => {{
        $fill();
        let buf   = $mod::DATA.lock();
        let len   = $mod::LEN.load(AOrdering::Acquire);
        // SAFETY: The static buffer lives for the lifetime of the kernel.
        // We return a slice of the static backing array.  The Mutex is
        // locked only during fill; callers receive a raw &'static slice.
        unsafe {
            let ptr = buf.as_ptr();
            core::mem::drop(buf);
            Some(core::slice::from_raw_parts(ptr, len))
        }
    }};
}

fn sysfs_lookup(path: &str) -> Option<&'static [u8]> {
    match path {
        "/sys/kernel/version"    => sysfs_serve!(sys_kernel_version,  fill_sys_kernel_version),
        "/sys/input/devices"     => sysfs_serve!(sys_input_devices,   fill_sys_input_devices),
        "/sys/process/list"      => sysfs_serve!(sys_process_list,    fill_sys_process_list),
        "/sys/app/list"          => sysfs_serve!(sys_app_list,        fill_sys_app_list),
        "/sys/usb/controllers"   => sysfs_serve!(sys_usb_controllers, fill_sys_usb_controllers),
        _ => None,
    }
}

pub fn init() {
    // Mount the embedded initramfs at root.
    initramfs::mount_embedded();
    log::info!("[VFS] Virtual filesystem initialised");
}
