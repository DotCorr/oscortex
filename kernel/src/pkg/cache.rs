//! LRU cache manager for on-demand packages.
//!
//! Tracks which apps are ephemeral (fetched on demand) vs system (pinned).
//! When the cache is full, the least-recently-used ephemeral entry is evicted
//! to make room for a new fetch.

use spin::Mutex;
use super::sha256::Sha256Hash;

/// Maximum number of simultaneously cached on-demand apps.
const MAX_CACHED: usize = 16;

/// A single cache entry.
#[derive(Clone, Copy)]
struct CacheEntry {
    /// Whether this slot is occupied.
    active: bool,
    /// The app_id in the app_registry.
    app_id: u32,
    /// SHA-256 hash (for cache-hit lookup by content).
    hash: Sha256Hash,
    /// Frame counter at last use (for LRU).
    last_used: u64,
    /// If true, never evict (system app).
    pinned: bool,
}

impl CacheEntry {
    const fn empty() -> Self {
        Self {
            active: false,
            app_id: 0,
            hash: [0u8; 32],
            last_used: 0,
            pinned: false,
        }
    }
}

struct CacheState {
    entries: [CacheEntry; MAX_CACHED],
}

impl CacheState {
    const fn new() -> Self {
        Self {
            entries: [CacheEntry::empty(); MAX_CACHED],
        }
    }
}

static CACHE: Mutex<CacheState> = Mutex::new(CacheState::new());

/// Check if an app with the given hash is already cached.
/// Returns `Some(app_id)` if found (and updates last_used).
pub fn lookup(hash: &Sha256Hash) -> Option<u32> {
    let mut cache = CACHE.lock();
    let now = crate::compositor::frame_counter();
    for entry in cache.entries.iter_mut() {
        if entry.active && super::sha256::hash_eq(&entry.hash, hash) {
            entry.last_used = now;
            return Some(entry.app_id);
        }
    }
    None
}

/// Check if an app with the given app_id is cached.
pub fn lookup_by_id(app_id: u32) -> bool {
    let cache = CACHE.lock();
    cache.entries.iter().any(|e| e.active && e.app_id == app_id)
}

/// Insert a newly fetched app into the cache.
///
/// If the cache is full, evicts the LRU ephemeral entry first.
/// Returns `true` on success, `false` if all slots are pinned (cannot evict).
pub fn insert(app_id: u32, hash: Sha256Hash, pinned: bool) -> bool {
    let mut cache = CACHE.lock();
    let now = crate::compositor::frame_counter();

    // Try to find an empty slot.
    for entry in cache.entries.iter_mut() {
        if !entry.active {
            *entry = CacheEntry {
                active: true,
                app_id,
                hash,
                last_used: now,
                pinned,
            };
            log::info!("[pkg/cache] inserted app_id={} (slot free)", app_id);
            return true;
        }
    }

    // Cache full — evict LRU non-pinned entry.
    let victim = cache
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.active && !e.pinned)
        .min_by_key(|(_, e)| e.last_used);

    match victim {
        Some((idx, _)) => {
            let old_id = cache.entries[idx].app_id;
            // Uninstall the old app from the registry.
            crate::app_registry::uninstall(old_id);
            log::info!("[pkg/cache] evicted app_id={} for app_id={}", old_id, app_id);

            cache.entries[idx] = CacheEntry {
                active: true,
                app_id,
                hash,
                last_used: now,
                pinned,
            };
            true
        }
        None => {
            log::warn!("[pkg/cache] all {} slots pinned — cannot evict", MAX_CACHED);
            false
        }
    }
}

/// Remove an entry by app_id (e.g. manual eviction via syscall).
pub fn remove(app_id: u32) -> bool {
    let mut cache = CACHE.lock();
    for entry in cache.entries.iter_mut() {
        if entry.active && entry.app_id == app_id {
            if entry.pinned {
                log::warn!("[pkg/cache] refusing to evict pinned app_id={}", app_id);
                return false;
            }
            entry.active = false;
            crate::app_registry::uninstall(app_id);
            log::info!("[pkg/cache] removed app_id={}", app_id);
            return true;
        }
    }
    false
}

/// Touch an entry (update last_used timestamp) on launch.
pub fn touch(app_id: u32) {
    let mut cache = CACHE.lock();
    let now = crate::compositor::frame_counter();
    for entry in cache.entries.iter_mut() {
        if entry.active && entry.app_id == app_id {
            entry.last_used = now;
            return;
        }
    }
}

/// Current cache occupancy.
pub fn count() -> (u32, u32) {
    let cache = CACHE.lock();
    let total = cache.entries.iter().filter(|e| e.active).count() as u32;
    let pinned = cache.entries.iter().filter(|e| e.active && e.pinned).count() as u32;
    (total, pinned)
}

/// Serialise cache status into a buffer for the sysfs.
/// Format: "slots=X/16 pinned=Y\n" per line for each active entry.
pub fn status_text(buf: &mut [u8]) -> usize {
    let cache = CACHE.lock();
    let (total, pinned) = {
        let t = cache.entries.iter().filter(|e| e.active).count();
        let p = cache.entries.iter().filter(|e| e.active && e.pinned).count();
        (t, p)
    };

    let s = alloc::format!(
        "slots={}/{}\npinned={}\nephemeral={}\n",
        total, MAX_CACHED, pinned, total - pinned
    );
    let bytes = s.as_bytes();
    let copy = bytes.len().min(buf.len());
    buf[..copy].copy_from_slice(&bytes[..copy]);
    copy
}
