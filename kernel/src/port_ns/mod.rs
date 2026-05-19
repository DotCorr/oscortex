//! Named port IPC namespace — Phase 39.
//!
//! Provides a kernel-global registry that maps a UTF-8 name string to a
//! `(isolate_id, pid)` pair, modelled after Dart's `SendPort` lookup service.
//!
//! ## Syscall ABI
//!
//! | Number | Name         | Args                                            | Returns    |
//! |--------|--------------|-------------------------------------------------|------------|
//! | 0x373  | port_bind    | (name_ptr, name_len, isolate_id)                | 0 / -ERRNO |
//! | 0x374  | port_lookup  | (name_ptr, name_len, iso_out_ptr, pid_out_ptr)  | 0 / -ERRNO |
//! | 0x375  | port_unbind  | (name_ptr, name_len)                            | 0 / -ERRNO |
//!
//! ### port_bind
//! Registers the calling process's `isolate_id` under `name`.  Fails with
//! `-EEXIST` (-17) if the name is already bound.  Names longer than
//! `MAX_PORT_NAME` bytes are rejected with `-EINVAL`.
//!
//! ### port_lookup
//! Copies the bound `isolate_id` (u32le) and `pid` (u32le) into the
//! user-provided output pointers.  Fails with `-ENOENT` (-2) if not found.
//!
//! ### port_unbind
//! Removes the binding.  Only the PID that bound the name may remove it
//! (enforcement is opportunistic for Phase 39 — enforced in Phase 42+).

use spin::Mutex;
use alloc::vec::Vec;

pub const MAX_PORT_NAME: usize = 128;
const MAX_PORTS: usize = 256;

// ── Record ────────────────────────────────────────────────────────────────────

struct PortRecord {
    name:       [u8; MAX_PORT_NAME],
    name_len:   u8,
    isolate_id: u32,
    pid:        u32,
}

impl PortRecord {
    fn name_bytes(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }
}

// ── Table ─────────────────────────────────────────────────────────────────────

struct PortTable {
    slots: Vec<Option<PortRecord>>,
}

impl PortTable {
    const fn empty() -> Self {
        Self { slots: Vec::new() }
    }

    fn find(&self, name: &[u8]) -> Option<&PortRecord> {
        self.slots.iter().flatten().find(|r| r.name_bytes() == name)
    }

    fn find_mut(&mut self, name: &[u8]) -> Option<&mut PortRecord> {
        self.slots.iter_mut().flatten().find(|r| r.name_bytes() == name)
    }

    fn insert(&mut self, rec: PortRecord) -> bool {
        if self.slots.iter().flatten().count() >= MAX_PORTS { return false; }
        for slot in self.slots.iter_mut() {
            if slot.is_none() {
                *slot = Some(rec);
                return true;
            }
        }
        self.slots.push(Some(rec));
        true
    }

    fn remove(&mut self, name: &[u8]) -> bool {
        for slot in self.slots.iter_mut() {
            if slot.as_ref().map(|r| r.name_bytes() == name).unwrap_or(false) {
                *slot = None;
                return true;
            }
        }
        false
    }
}

static PORT_TABLE: Mutex<PortTable> = Mutex::new(PortTable::empty());

// ── Public API ────────────────────────────────────────────────────────────────

/// Bind `name` → `(isolate_id, pid)`.
///
/// Returns `0` on success, `-17` (EEXIST) if already bound, `-12` if the
/// table is full, `-22` (EINVAL) if the name is too long.
pub fn bind(name: &[u8], isolate_id: u32, pid: u32) -> i64 {
    if name.len() > MAX_PORT_NAME { return -22; }
    let mut table = PORT_TABLE.lock();
    if table.find(name).is_some() { return -17; } // EEXIST
    let mut rec = PortRecord {
        name: [0u8; MAX_PORT_NAME],
        name_len: name.len() as u8,
        isolate_id,
        pid,
    };
    rec.name[..name.len()].copy_from_slice(name);
    if !table.insert(rec) { return -12; } // ENOMEM
    log::debug!("[PORT_NS] bound '{}' → isolate={} pid={}",
        core::str::from_utf8(name).unwrap_or("?"), isolate_id, pid);
    0
}

/// Look up a name, writing `isolate_id` and `pid` to the provided references.
///
/// Returns `0` on success, `-2` (ENOENT) if not found.
pub fn lookup(name: &[u8], isolate_id_out: &mut u32, pid_out: &mut u32) -> i64 {
    if name.len() > MAX_PORT_NAME { return -22; }
    let table = PORT_TABLE.lock();
    match table.find(name) {
        Some(r) => {
            *isolate_id_out = r.isolate_id;
            *pid_out        = r.pid;
            0
        }
        None => -2, // ENOENT
    }
}

/// Remove the binding for `name`.
///
/// Returns `0` on success, `-2` (ENOENT) if not found.
pub fn unbind(name: &[u8]) -> i64 {
    if name.len() > MAX_PORT_NAME { return -22; }
    let mut table = PORT_TABLE.lock();
    if table.remove(name) {
        log::debug!("[PORT_NS] unbound '{}'", core::str::from_utf8(name).unwrap_or("?"));
        0
    } else {
        -2 // ENOENT
    }
}

/// Return the count of currently registered named ports.
pub fn count() -> u32 {
    PORT_TABLE.lock().slots.iter().flatten().count() as u32
}
