//! PID-0 API — privileged kernel introspection & Cortex control channel.
//!
//! PID-0 is a virtual process that represents the kernel itself. It exposes a
//! set of special syscalls that no other process can invoke:
//!
//!   0x1000 — CORTEX_QUERY    Query the context graph
//!   0x1001 — CORTEX_LOAD_MODEL  Upload a new inference model capsule
//!   0x1002 — CORTEX_LOAD_DRIVER Load a WASM driver module
//!   0x1003 — CORTEX_HEAL     Manually trigger a healing action
//!   0x1004 — CORTEX_STATUS   Get Cortex runtime statistics
//!   0x1005 — CORTEX_KILL_DRIVER Unload a driver by id
//!   0x1006 — CORTEX_CONTEXT_DUMP Dump the context graph (for userspace AI)
//!
//! Access is gated by the `CAP_CORTEX` capability (see security module).

use crate::cortex::context::NodeKind;
use crate::cortex::healing;
use crate::cortex::inference::HealingActionKind;
use crate::syscall::{read_user_bytes, write_user_bytes};

#[repr(C)]
struct CortexStatusOut {
    initialised: u8,
    healing_enabled: u8,
    driver_gen_enabled: u8,
    _pad: u8,
    interrupt_count: u64,
    anomaly_count: u64,
    node_count: u32,
    driver_count: u32,
}

pub fn init() {
    log::info!("[Cortex::PID0] PID-0 Cortex API registered on syscall 0x1000-0x100F");
}

fn require_cortex_caller() -> bool {
    // The kernel idle task (pid 0) is implicitly trusted; every other caller
    // must hold CAP_CORTEX (the system shell does, launched apps do not). A
    // real per-PCB capability check now, not a PID whitelist.
    crate::process::current_pid() == 0
        || crate::process::current_has_caps(crate::security::Capabilities::CORTEX)
}

fn node_kind_from_u64(kind: u64) -> Option<NodeKind> {
    match kind {
        0 => Some(NodeKind::Device),
        1 => Some(NodeKind::Process),
        2 => Some(NodeKind::Driver),
        3 => Some(NodeKind::Interrupt),
        4 => Some(NodeKind::AnomalyEvent),
        5 => Some(NodeKind::HealingAction),
        _ => None,
    }
}

fn healing_action_from_u64(action: u64) -> Option<HealingActionKind> {
    match action {
        0 => Some(HealingActionKind::ReloadDriver),
        1 => Some(HealingActionKind::QuarantineModule),
        2 => Some(HealingActionKind::ExpandPageTable),
        3 => Some(HealingActionKind::ReclaimMemory),
        4 => Some(HealingActionKind::MigrateProcess),
        5 => Some(HealingActionKind::ResetDevice),
        6 => Some(HealingActionKind::KillProcess),
        _ => None,
    }
}

/// Dispatch a PID-0 syscall. Called from the syscall handler when the
/// calling process holds `CAP_CORTEX` and the syscall number is in range.
pub fn dispatch(number: u64, arg0: u64, arg1: u64, arg2: u64) -> i64 {
    if !require_cortex_caller() {
        return -1; // EPERM
    }
    match number {
        0x1000 => cortex_query(arg0, arg1, arg2),
        0x1001 => cortex_load_model(arg0, arg1),
        0x1002 => cortex_load_driver(arg0, arg1, arg2),
        0x1003 => cortex_heal(arg0, arg1),
        0x1004 => cortex_status(arg0),
        0x1005 => cortex_kill_driver(arg0),
        0x1006 => cortex_context_dump(arg0, arg1),
        _ => -38, // ENOSYS
    }
}

fn cortex_query(kind: u64, id: u64, buf_ptr: u64) -> i64 {
    let Some(node_kind) = node_kind_from_u64(kind) else {
        return -22;
    };
    if buf_ptr == 0 {
        return -22;
    }
    let mut buf = [0u8; 80];
    let n = crate::cortex::with_state(|s| s.context.write_node(node_kind, id, &mut buf));
    if n == 0 {
        return -2; // ENOENT
    }
    unsafe {
        if write_user_bytes(buf_ptr, &buf[..n]) {
            n as i64
        } else {
            -14 // EFAULT
        }
    }
}

fn cortex_load_model(buf_ptr: u64, len: u64) -> i64 {
    if buf_ptr == 0 || len == 0 || len > 512 {
        return -22;
    }
    let bytes = match unsafe { read_user_bytes(buf_ptr, len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    match crate::cortex::inference::load_runtime_capsule(bytes) {
        Ok(()) => 0,
        Err(e) => {
            log::warn!("[Cortex::PID0] Model capsule rejected: {}", e);
            -22
        }
    }
}

fn cortex_load_driver(buf_ptr: u64, len: u64, name_ptr: u64) -> i64 {
    if buf_ptr == 0 || len == 0 || name_ptr == 0 {
        return -22;
    }
    let wasm = match unsafe { read_user_bytes(buf_ptr, len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let name = match unsafe { read_user_bytes(name_ptr, 63) } {
        Some(b) => b,
        None => return -14,
    };
    match crate::drivers::registry::load(name, wasm) {
        Ok(id) => id as i64,
        Err(e) => {
            log::warn!("[Cortex::PID0] Driver load failed: {:?}", e);
            -22
        }
    }
}

fn cortex_heal(action: u64, target_id: u64) -> i64 {
    let Some(kind) = healing_action_from_u64(action) else {
        return -22;
    };
    healing::execute_manual(kind, target_id);
    0
}

fn cortex_status(buf_ptr: u64) -> i64 {
    if buf_ptr == 0 {
        return -22;
    }
    let out = crate::cortex::with_state(|s| CortexStatusOut {
        initialised: s.initialised as u8,
        healing_enabled: s.healing_enabled as u8,
        driver_gen_enabled: s.driver_gen_enabled as u8,
        _pad: 0,
        interrupt_count: s.interrupt_count,
        anomaly_count: s.anomaly_count,
        node_count: s.context.node_count() as u32,
        driver_count: crate::drivers::registry::count() as u32,
    });
    let bytes = unsafe {
        core::slice::from_raw_parts(
            &out as *const CortexStatusOut as *const u8,
            core::mem::size_of::<CortexStatusOut>(),
        )
    };
    unsafe {
        if write_user_bytes(buf_ptr, bytes) {
            bytes.len() as i64
        } else {
            -14
        }
    }
}

fn cortex_kill_driver(id: u64) -> i64 {
    crate::drivers::registry::quarantine(id as u32);
    0
}

fn cortex_context_dump(buf_ptr: u64, max_len: u64) -> i64 {
    if buf_ptr == 0 || max_len < 8 {
        return -22;
    }
    let cap = max_len as usize;
    let mut scratch = [0u8; 4096];
    let slice = if cap <= scratch.len() {
        &mut scratch[..cap]
    } else {
        return -22;
    };
    let n = crate::cortex::with_state(|s| {
        let max_nodes = ((cap.saturating_sub(8)) / 81).min(32);
        s.context.dump_flat(slice, max_nodes)
    });
    if n == 0 {
        return -2;
    }
    unsafe {
        if write_user_bytes(buf_ptr, &slice[..n]) {
            n as i64
        } else {
            -14
        }
    }
}
