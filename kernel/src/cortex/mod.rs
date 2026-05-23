//! AI Cortex — the brain of OSCortex.
//!
//! The Cortex is an AI runtime that lives in kernel Ring 0. It is responsible
//! for:
//!
//!   1. **Interrupt observation** — every hardware/software interrupt passes
//!      through `interrupt_hook()` before the kernel handles it. The Cortex
//!      builds a statistical model of system behaviour.
//!
//!   2. **Runtime driver generation** — when a new device is detected (or an
//!      existing driver fails), the Cortex generates/loads a WASM driver module
//!      without restarting the kernel. The `driver_gen` submodule manages this.
//!
//!   3. **Self-healing** — the `healing` submodule watches kernel subsystems.
//!      On anomaly detection it can hot-patch code, replace a driver, or
//!      quarantine a faulty module — all at runtime.
//!
//!   4. **Context memory** — the `context` submodule maintains a growing graph
//!      of system state (processes, devices, I/O patterns, error history).
//!      This is the "memory" that makes the OS smarter over time.
//!
//!   5. **PID-0 API** — `syscall::pid0` exposes a privileged channel through
//!      which privileged processes can query/influence the Cortex.
//!
//!   6. **Inter-core Cortex messages** — `ipc` handles Ring-0 IPIs between
//!      Cortex shards on different CPU cores.
//!
//! ## Safety contract
//!
//! The Cortex may NOT:
//!   - Write outside the Cortex Arena (enforced by the MMU guard pages).
//!   - Execute arbitrary machine code it generates (all generated code goes
//!     through the WASM sandbox → verified JIT path).
//!   - Block indefinitely (all Cortex routines have a deadline enforced by
//!     the APIC watchdog timer).

pub mod context;
pub mod driver_gen;
pub mod healing;
pub mod inference;
pub mod ipc;
pub mod observer;
pub mod pid0;

use crate::arch::InterruptFrame;
use spin::Mutex;

// ── Cortex global state ───────────────────────────────────────────────────────

/// Master Cortex state, protected by a spinlock.
///
/// All submodules borrow from this; they must never hold the lock across a
/// potential interrupt (grab → do minimal work → release).
static CORTEX: Mutex<CortexState> = Mutex::new(CortexState::new());

pub struct CortexState {
    pub initialised:       bool,
    pub healing_enabled:   bool,
    pub driver_gen_enabled: bool,
    pub interrupt_count:   u64,
    pub anomaly_count:     u64,
    pub context:           context::ContextGraph,
    pub observer:          observer::InterruptObserver,
}

impl CortexState {
    pub const fn new() -> Self {
        Self {
            initialised:        false,
            healing_enabled:    true,
            driver_gen_enabled: true,
            interrupt_count:    0,
            anomaly_count:      0,
            context:            context::ContextGraph::new(),
            observer:           observer::InterruptObserver::new(),
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the AI Cortex. Called once by `kernel_main` after all other
/// subsystems are up.
pub fn init() {
    log::info!("[Cortex] Initialising AI runtime...");
    
    // Boot the inference engine (loads quantised model into Cortex Arena).
    log::info!("[Cortex] Starting inference engine...");
    inference::init();
    log::info!("[Cortex] Inference OK");
    
    // Start the context graph.
    log::info!("[Cortex] Starting context graph...");
    context::init();
    log::info!("[Cortex] Context OK");
    
    // Arm the healing watchdog.
    log::info!("[Cortex] Starting healing...");
    healing::init();
    log::info!("[Cortex] Healing OK");
    
    // Wire up the driver generation subsystem.
    log::info!("[Cortex] Starting driver generation...");
    driver_gen::init();
    log::info!("[Cortex] Driver gen OK");
    
    // Register the PID-0 syscall channel.
    log::info!("[Cortex] Starting PID-0 channel...");
    pid0::init();
    log::info!("[Cortex] PID-0 OK");

    let mut c = CORTEX.lock();
    c.initialised = true;

    log::info!("[Cortex] AI runtime online — self-healing ACTIVE, driver-gen ACTIVE");
}

/// Enter the Cortex-managed idle loop. This is the last thing `kernel_main`
/// calls. The Cortex drives the scheduler from here.
pub fn run() -> ! {
    // Register the current BSP execution context as the idle/Cortex task so
    // context_switch has a valid slot to save its RSP into on first yield.
    crate::sched::register_current_as_task0("cortex-idle");

    // Spawn a background Cortex worker that runs deferred inference/healing.
    crate::sched::spawn_kernel_task("cortex-bg", cortex_background);

    // Enable interrupts — the APIC timer will start firing and preempting.
    crate::arch::enable_interrupts();
    loop {
        // Cooperative yield: hand CPU to the next ready task from the idle loop.
        // Disable interrupts during the context switch to prevent preemption races.
        crate::arch::disable_interrupts();
        crate::sched::schedule();
        crate::arch::enable_interrupts();
        // Run a bounded Cortex maintenance tick (context graph, anomaly scan).
        tick();
        // Pause until the next interrupt.
        crate::arch::halt();
    }
}

/// Background Cortex worker task — deferred inference and healing.
///
/// Runs as a kernel task scheduled alongside the idle loop. Keeps a heartbeat
/// log so we can verify the scheduler is producing real context switches.
fn cortex_background() {
    log::info!("[Cortex::BG] background worker started — scheduler ALIVE");
    let mut n: u64 = 0;
    loop {
        n = n.wrapping_add(1);
        // Log a heartbeat every ~100 K iterations to confirm liveness.
        if n % 100_000 == 0 {
            log::warn!("[Cortex::BG] heartbeat n={}k", n / 1_000);
        }
        core::hint::spin_loop();
    }
}

/// Called on every timer tick. Runs a bounded Cortex maintenance cycle.
pub fn tick() {
    let deadline = crate::arch::rdtsc() + 100_000; // ~100 µs budget
    let mut c = CORTEX.lock();
    if !c.initialised {
        return;
    }
    observer::analyze(&mut c, deadline);
    healing::check(&mut c, deadline);
    context::flush_pending(&mut c);
    drop(c);

    // Phase 33-A: compositor and WM tick are now driven by the APIC timer ISR
    // (hardware-cadenced at 60/120 Hz via vsync_due()).  Do NOT call them here
    // any more — that would race with the ISR and produce duplicate frames from
    // the idle spin loop.
}

/// Called by every interrupt handler before the kernel acts on the interrupt.
///
/// The Cortex can:
///   - record the event in its context graph
///   - detect anomalies
///   - veto an interrupt (return `false` to suppress default handling)
///   - schedule async remediation
#[inline]
pub fn interrupt_hook(vector: u8, frame: &InterruptFrame, error: Option<u64>) {
    // Fast path: try to acquire without blocking. If the Cortex lock is
    // contended we skip the hook (never block in an interrupt handler).
    if let Some(mut c) = CORTEX.try_lock() {
        c.interrupt_count += 1;
        c.observer.record(vector, frame.ip, error);
        // Anomaly detection is cheap (threshold check on ring-buffer stats).
        if observer::is_anomalous(&c, vector) {
            c.anomaly_count += 1;
            healing::schedule_async_check(vector);
        }
    }
}

/// Called by the page-fault handler. Returns `true` if the Cortex resolved
/// the fault (demand paging, CoW, guard page expansion, etc.).
pub fn handle_page_fault(cr2: u64, error: u64) -> bool {
    crate::mm::paging::demand_page(cr2, error)
}

