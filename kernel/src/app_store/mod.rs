//! Persistent `.osx` bundle storage on the first virtio-blk data volume.
//!
//! **Not** a consumer "App Store" — this is kernel block I/O for saving installed
//! `.osx` packages across reboots. The Flutter shell provides the install UI;
//! syscalls (`app_install` / `app_list`) are the API; this module is the disk backend.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

const MAGIC: &[u8; 8] = b"OSSTORE1";
const SUPER_SECTOR: u64 = 0;
const CATALOG_SECTOR: u64 = 1;
const CATALOG_SECTORS: u64 = 12;
const CATALOG_ENTRIES: usize = 48;
const CATALOG_ENTRY_BYTES: usize = 128;
const DATA_START_SECTOR: u64 = 64;

#[repr(C, packed)]
struct Superblock {
    magic: [u8; 8],
    version: u32,
    next_app_id: u32,
    next_data_sector: u64,
    entry_count: u32,
    _pad: [u8; 492],
}

#[repr(C, packed)]
struct CatalogEntry {
    flags: u32,
    id: u32,
    name: [u8; 64],
    version: [u8; 16],
    entry_offset: u64,
    stack_size: u64,
    data_sector: u64,
    bundle_sectors: u32,
    bundle_bytes: u32,
    _pad: u32,
}

static STORE_READY: AtomicBool = AtomicBool::new(false);

struct StoreState {
    next_app_id: u32,
    next_data_sector: u64,
}

static STATE: Mutex<StoreState> = Mutex::new(StoreState {
    next_app_id: 1,
    next_data_sector: DATA_START_SECTOR,
});

fn sectors_for_bytes(len: usize) -> u32 {
    ((len + 511) / 512) as u32
}

fn read_superblock() -> Result<Superblock, &'static str> {
    let mut buf = [0u8; 512];
    crate::drivers::virtio_blk::read_sectors(SUPER_SECTOR, 1, &mut buf)?;
    let sb = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Superblock) };
    if &sb.magic != MAGIC {
        return Err("bad magic");
    }
    Ok(sb)
}

fn write_superblock(sb: &Superblock) -> Result<(), &'static str> {
    let mut buf = [0u8; 512];
    unsafe {
        core::ptr::copy_nonoverlapping(
            sb as *const Superblock as *const u8,
            buf.as_mut_ptr(),
            core::mem::size_of::<Superblock>(),
        );
    }
    crate::drivers::virtio_blk::write_sectors(SUPER_SECTOR, 1, &buf)
}

fn format_store() -> Result<(), &'static str> {
    let sb = Superblock {
        magic: *MAGIC,
        version: 1,
        next_app_id: 1,
        next_data_sector: DATA_START_SECTOR,
        entry_count: 0,
        _pad: [0; 492],
    };
    write_superblock(&sb)?;
    let zero = [0u8; 512];
    for s in CATALOG_SECTOR..CATALOG_SECTOR + CATALOG_SECTORS {
        crate::drivers::virtio_blk::write_sectors(s, 1, &zero)?;
    }
    log::info!("[app_store] formatted new data volume");
    Ok(())
}

fn read_catalog() -> Result<Vec<CatalogEntry>, &'static str> {
    let mut buf = alloc::vec::Vec::from([0u8; 512 * CATALOG_SECTORS as usize]);
    crate::drivers::virtio_blk::read_sectors(CATALOG_SECTOR, CATALOG_SECTORS, &mut buf)?;
    let mut out = Vec::new();
    for i in 0..CATALOG_ENTRIES {
        let off = i * CATALOG_ENTRY_BYTES;
        let entry = unsafe {
            core::ptr::read_unaligned(buf[off..].as_ptr() as *const CatalogEntry)
        };
        if { entry.flags } == 1 {
            out.push(entry);
        }
    }
    Ok(out)
}

fn write_catalog(entries: &[CatalogEntry]) -> Result<(), &'static str> {
    let mut buf = alloc::vec::Vec::from([0u8; 512 * CATALOG_SECTORS as usize]);
    for (i, entry) in entries.iter().enumerate().take(CATALOG_ENTRIES) {
        let off = i * CATALOG_ENTRY_BYTES;
        unsafe {
            core::ptr::copy_nonoverlapping(
                entry as *const CatalogEntry as *const u8,
                buf[off..].as_mut_ptr(),
                CATALOG_ENTRY_BYTES,
            );
        }
    }
    crate::drivers::virtio_blk::write_sectors(CATALOG_SECTOR, CATALOG_SECTORS, &buf)
}

fn bundle_from_entry(entry: &CatalogEntry) -> Result<Vec<u8>, &'static str> {
    let sectors = entry.bundle_sectors as u64;
    let mut buf = alloc::vec![0u8; (sectors * 512) as usize];
    crate::drivers::virtio_blk::read_sectors(entry.data_sector, sectors, &mut buf)?;
    buf.truncate(entry.bundle_bytes as usize);
    Ok(buf)
}

pub fn is_ready() -> bool {
    STORE_READY.load(Ordering::Acquire)
}

fn hydrate_registry() -> Result<u32, &'static str> {
    let entries = read_catalog()?;
    let mut loaded = 0u32;
    for entry in entries {
        let bundle = bundle_from_entry(&entry)?;
        if crate::app_registry::install_from_store(&bundle, entry.id).is_some() {
            loaded += 1;
        }
    }
    Ok(loaded)
}

/// Initialise the block app store and hydrate the in-memory registry.
pub fn init() -> bool {
    if !crate::drivers::virtio_blk::is_ready() {
        log::warn!("[app_store] no virtio-blk — runtime install will not persist");
        return false;
    }

    if read_superblock().is_err() {
        match format_store() {
            Ok(()) => {}
            Err(e) => {
                log::warn!("[app_store] format failed: {}", e);
                return false;
            }
        }
    }

    let sb = match read_superblock() {
        Ok(s) => s,
        Err(_) => return false,
    };

    {
        let mut st = STATE.lock();
        st.next_app_id = { sb.next_app_id }.max(1);
        st.next_data_sector = { sb.next_data_sector }.max(DATA_START_SECTOR);
    }

    STORE_READY.store(true, Ordering::Release);

    match hydrate_registry() {
        Ok(n) => log::info!("[app_store] ready — {} apps loaded from disk", n),
        Err(e) => log::warn!("[app_store] hydrate failed: {}", e),
    }
    true
}

pub fn persist_bundle(app_id: u32, bundle: &[u8]) -> Result<(), &'static str> {
    if !is_ready() {
        return Ok(());
    }
    let hdr = crate::app_registry::parse_header(bundle).ok_or("bad bundle")?;

    let mut st = STATE.lock();
    let data_sector = st.next_data_sector;
    let bundle_sectors = sectors_for_bytes(bundle.len());
    let mut padded = bundle.to_vec();
    padded.resize(bundle_sectors as usize * 512, 0);
    crate::drivers::virtio_blk::write_sectors(data_sector, bundle_sectors as u64, &padded)?;

    let mut entries = read_catalog()?;
    if entries.len() >= CATALOG_ENTRIES {
        return Err("catalog full");
    }

    entries.push(CatalogEntry {
        flags: 1,
        id: app_id,
        name: hdr.name,
        version: hdr.version,
        entry_offset: hdr.entry_offset,
        stack_size: hdr.stack_size,
        data_sector,
        bundle_sectors,
        bundle_bytes: bundle.len() as u32,
        _pad: 0,
    });
    write_catalog(&entries)?;

    st.next_data_sector = data_sector + bundle_sectors as u64;
    st.next_app_id = app_id.saturating_add(1).max(st.next_app_id);

    let sb = Superblock {
        magic: *MAGIC,
        version: 1,
        next_app_id: st.next_app_id,
        next_data_sector: st.next_data_sector,
        entry_count: entries.len() as u32,
        _pad: [0; 492],
    };
    write_superblock(&sb)?;
    log::info!("[app_store] persisted app_id={} ({} bytes)", app_id, bundle.len());
    Ok(())
}

pub fn remove_bundle(app_id: u32) -> Result<(), &'static str> {
    if !is_ready() {
        return Ok(());
    }
    let mut entries = read_catalog()?;
    let before = entries.len();
    entries.retain(|e| e.id != app_id);
    if entries.len() == before {
        return Err("not found");
    }
    write_catalog(&entries)?;

    let st = STATE.lock();
    let sb = Superblock {
        magic: *MAGIC,
        version: 1,
        next_app_id: st.next_app_id,
        next_data_sector: st.next_data_sector,
        entry_count: entries.len() as u32,
        _pad: [0; 492],
    };
    write_superblock(&sb)?;
    Ok(())
}
