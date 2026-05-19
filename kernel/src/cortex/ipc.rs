//! Cortex inter-core IPC — Ring-0 messages between Cortex shards on different CPUs.

pub fn handle_ipc_interrupt() {
    // TODO: drain per-core IPC mailbox and dispatch messages.
}
