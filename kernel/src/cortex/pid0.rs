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

pub fn init() {
    log::info!("[Cortex::PID0] PID-0 Cortex API registered on syscall 0x1000-0x100F");
}

/// Dispatch a PID-0 syscall. Called from the syscall handler when the
/// calling process holds `CAP_CORTEX` and the syscall number is in range.
pub fn dispatch(number: u64, arg0: u64, arg1: u64, arg2: u64) -> i64 {
    match number {
        0x1000 => cortex_query(arg0, arg1, arg2),
        0x1001 => cortex_load_model(arg0, arg1),
        0x1002 => cortex_load_driver(arg0, arg1, arg2),
        0x1003 => cortex_heal(arg0, arg1),
        0x1004 => cortex_status(arg0),
        0x1005 => cortex_kill_driver(arg0),
        0x1006 => cortex_context_dump(arg0, arg1),
        _ => -1, // ENOSYS
    }
}

fn cortex_query(_kind: u64, _id: u64, _buf_ptr: u64) -> i64 {
    // TODO: copy context graph node data to userspace buffer.
    0
}

fn cortex_load_model(buf_ptr: u64, len: u64) -> i64 {
    // Validate and load a signed model capsule from userspace.
    // Security: capsule must be signed with the Dotcorr kernel signing key.
    // TODO: implement capsule signature verification and model swap.
    log::info!("[Cortex::PID0] Model capsule load requested: ptr={:#x} len={}", buf_ptr, len);
    -38 // ENOSYS for now
}

fn cortex_load_driver(buf_ptr: u64, len: u64, name_ptr: u64) -> i64 {
    log::info!("[Cortex::PID0] Driver load via PID-0: ptr={:#x} len={} name={:#x}",
        buf_ptr, len, name_ptr);
    // TODO: copy WASM bytes from userspace, pass to driver_gen::registry::load().
    -38
}

fn cortex_heal(action: u64, target_id: u64) -> i64 {
    log::info!("[Cortex::PID0] Manual heal: action={} target={}", action, target_id);
    -38
}

fn cortex_status(buf_ptr: u64) -> i64 {
    // TODO: serialise CortexState stats into userspace buffer.
    let _ = buf_ptr;
    0
}

fn cortex_kill_driver(id: u64) -> i64 {
    crate::drivers::registry::quarantine(id as u32);
    0
}

fn cortex_context_dump(_buf_ptr: u64, _max_len: u64) -> i64 {
    // TODO: serialise context graph to userspace buffer (JSON or flatbuffers).
    -38
}
