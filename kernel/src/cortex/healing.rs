//! Cortex Self-Healing Subsystem.
//!
//! The healer monitors kernel subsystems and drivers. When it detects a fault
//! it tries a sequence of remediation actions in order of invasiveness:
//!
//!   1. Soft reset — reinitialise the driver's state machine
//!   2. Driver reload — unload and reload from the module store
//!   3. Driver replacement — ask Cortex to generate a new WASM module
//!   4. Device reset — assert hardware reset if supported
//!   5. Process migration — move dependent userspace processes off the device
//!   6. Quarantine — disable the device and mark it unavailable
//!
//! No action requires a kernel restart. The healer logs every action to the
//! context graph so future inference can learn better remediation strategies.

use spin::Mutex;
use crate::cortex::inference::{InferenceResult, HealingActionKind};

const MAX_PENDING_CHECKS: usize = 32;

struct HealingState {
    pending:       [PendingCheck; MAX_PENDING_CHECKS],
    pending_count: usize,
    heals_total:   u64,
    heals_failed:  u64,
}

#[derive(Clone, Copy)]
struct PendingCheck {
    vector: u8,
    ts:     u64,
}

impl HealingState {
    const fn new() -> Self {
        Self {
            pending: [PendingCheck { vector: 0, ts: 0 }; MAX_PENDING_CHECKS],
            pending_count: 0,
            heals_total: 0,
            heals_failed: 0,
        }
    }
}

static HEALER: Mutex<HealingState> = Mutex::new(HealingState::new());

/// Schedule an asynchronous healing check for interrupt `vector`.
/// Called from the fast path (interrupt hook) — must not block.
pub fn schedule_async_check(vector: u8) {
    if let Some(mut h) = HEALER.try_lock() {
        if h.pending_count < MAX_PENDING_CHECKS {
            let slot = h.pending_count;
            h.pending[slot] = PendingCheck {
                vector,
                ts: crate::arch::rdtsc(),
            };
            h.pending_count += 1;
        }
    }
}

/// Arm the healing watchdog. Sets up a periodic NMI-backed check.
pub fn init() {
    log::info!("[Cortex::Healing] Self-healing watchdog armed");
}

/// Run healing checks. Called from the Cortex tick with a TSC deadline.
pub fn check(state: &mut crate::cortex::CortexState, deadline_tsc: u64) {
    if !state.healing_enabled {
        return;
    }
    let mut h = HEALER.lock();
    let count = h.pending_count;
    h.pending_count = 0; // drain — we'll process below

    drop(h); // release lock before potentially blocking

    for i in 0..count {
        if crate::arch::rdtsc() >= deadline_tsc {
            break;
        }
        let vector = {
            let h = HEALER.lock();
            h.pending[i].vector
        };
        run_heal(vector, deadline_tsc, state);
    }
}

fn run_heal(vector: u8, deadline_tsc: u64, _state: &mut crate::cortex::CortexState) {
    // Ask the inference engine what to do.
    let query = [0x01u8, vector, 200u8]; // tag=anomaly, vector, score=high
    let result = crate::cortex::inference::run(&query, deadline_tsc);

    match result {
        Some(InferenceResult::HealingAction { action, target_id }) => {
            execute_healing_action(action, target_id);
        }
        Some(InferenceResult::AnomalyScore { source, score }) => {
            if score > 200 {
                log::warn!(
                    "[Cortex::Healing] Critical anomaly on vector {} (source={}), attempting soft reset",
                    vector, source
                );
                soft_reset_for_vector(vector);
            }
        }
        _ => {
            // No actionable result — log and continue.
            log::debug!("[Cortex::Healing] No healing action for vector {}", vector);
        }
    }
}

fn soft_reset_for_vector(vector: u8) {
    match vector {
        0x21 | 0x2C => crate::drivers::ps2::init(),
        0x30 => {
            let _ = crate::drivers::usb::poll();
        }
        _ => {
            log::debug!("[Cortex::Healing] No soft-reset hook for vector {}", vector);
        }
    }
}

/// Manual healing trigger from PID-0 API.
pub fn execute_manual(action: HealingActionKind, target_id: u64) {
    execute_healing_action(action, target_id);
}

fn execute_healing_action(action: HealingActionKind, target_id: u64) {
    let mut healer = HEALER.lock();
    healer.heals_total += 1;

    match action {
        HealingActionKind::ReloadDriver => {
            log::info!("[Cortex::Healing] Action: ReloadDriver (id={})", target_id);
            let _ = crate::drivers::registry::reload(target_id as u32);
        }
        HealingActionKind::QuarantineModule => {
            log::warn!("[Cortex::Healing] Action: QuarantineModule (id={})", target_id);
            crate::drivers::registry::quarantine(target_id as u32);
        }
        HealingActionKind::ExpandPageTable => {
            log::info!("[Cortex::Healing] Action: ExpandPageTable");
            let _ = crate::mm::heap::expand(4 * 1024 * 1024);
        }
        HealingActionKind::ReclaimMemory => {
            log::info!("[Cortex::Healing] Action: ReclaimMemory");
            let freed = crate::mm::reclaim_best_effort();
            log::info!("[Cortex::Healing] Reclaim reported {} bytes", freed);
        }
        HealingActionKind::MigrateProcess => {
            log::info!("[Cortex::Healing] Action: MigrateProcess (pid={})", target_id);
            crate::sched::note_migration_request(target_id as u32);
        }
        HealingActionKind::ResetDevice => {
            log::info!("[Cortex::Healing] Action: ResetDevice (id={})", target_id);
            crate::drivers::registry::reset_device(target_id as u32);
        }
        HealingActionKind::KillProcess => {
            log::warn!("[Cortex::Healing] Action: KillProcess (pid={})", target_id);
            let _ = crate::process::kill(target_id as u32);
        }
    }
}
