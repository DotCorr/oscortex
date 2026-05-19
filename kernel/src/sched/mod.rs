//! Scheduler — preemptive round-robin with Cortex-guided priority hints.
//!
//! ## Design
//!
//! Each kernel task owns a heap-allocated 32 KiB stack. On the first context
//! switch to a new task, `context_switch` pops six zero registers and `ret`s
//! into `arch::task_entry`, which enables interrupts then calls the task fn.
//!
//! `schedule()` is called both:
//!   • from the APIC timer ISR every `SCHED_SLICE` ticks (preemptive), and
//!   • from `cortex::run()`'s idle loop (cooperative yield on halt).
//!
//! The SCHED spinlock is released **before** `context_switch` so the new task
//! can lock it freely. Raw pointers into the static task array remain valid
//! because the array never moves.

use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Maximum concurrent kernel tasks (stack budget: 64 × 32 KiB = 2 MiB).
const MAX_TASKS: usize = 64;
/// 32 KiB kernel stack per task.
const STACK_SIZE: usize = 32 * 1024;
/// Context-switch every N APIC ticks (~50 ms at a 10 ms APIC period).
const SCHED_SLICE: u64 = 5;

// ── Task ──────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskState { Ready, Running, Blocked, Zombie }

/// Kernel task descriptor.
pub struct Task {
    pub pid:       u64,
    pub state:     TaskState,
    pub priority:  u8,
    pub name:      &'static str,
    /// Saved kernel RSP — written/read by `context_switch`.
    pub kernel_sp: u64,
    /// Base of the heap-allocated stack (for future deallocation).
    stack_base:    *mut u8,
}

// SAFETY: `stack_base` is exclusively owned by this Task; never aliased.
unsafe impl Send for Task {}

// ── Scheduler ─────────────────────────────────────────────────────────────────

struct Scheduler {
    tasks:   [Option<Task>; MAX_TASKS],
    count:   usize,
    current: usize,
    ticks:   u64,
}

impl Scheduler {
    const fn new() -> Self {
        const NONE: Option<Task> = None;
        Self { tasks: [NONE; MAX_TASKS], count: 0, current: 0, ticks: 0 }
    }
}

static SCHED: Mutex<Scheduler> = Mutex::new(Scheduler::new());
static NEXT_PID: AtomicU64 = AtomicU64::new(0);

// ── Stack helpers ─────────────────────────────────────────────────────────────

fn alloc_stack() -> *mut u8 {
    use alloc::alloc::{alloc, Layout};
    let layout = Layout::from_size_align(STACK_SIZE, 16)
        .expect("[Sched] invalid stack layout");
    let ptr = unsafe { alloc(layout) };
    if ptr.is_null() { panic!("[Sched] stack allocation failed — out of heap memory"); }
    ptr
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    log::info!("[Sched] Scheduler initialised (max_tasks={}, stack={}KiB, slice={} ticks)",
        MAX_TASKS, STACK_SIZE / 1024, SCHED_SLICE);
}

/// AP parking stub.
///
/// The current scheduler is a single global queue/state machine and is not yet
/// safe for concurrent scheduling decisions from multiple CPUs. Park APs with
/// interrupts disabled so only the BSP drives preemption.
pub fn ap_start(cpu_idx: u32) -> ! {
    log::info!("[Sched] AP cpu={} ready, parked (BSP-only scheduler)", cpu_idx);
    crate::arch::halt_forever();
}

/// Register the **current** execution context (BSP idle loop) as the idle task.
///
/// Appends to the existing task list rather than resetting it, so that tasks
/// already spawned (e.g. user-init) are preserved.  The current execution
/// context becomes the "running" task at whatever slot it occupies.
pub fn register_current_as_task0(name: &'static str) {
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
    let task = Task {
        pid,
        state:      TaskState::Running,
        priority:   128,
        name,
        kernel_sp:  0,
        stack_base: core::ptr::null_mut(),
    };
    let mut s = SCHED.lock();
    let slot = s.count;
    assert!(slot < MAX_TASKS, "[Sched] task table full in register_current_as_task0");
    s.tasks[slot] = Some(task);
    s.current = slot;
    s.count += 1;
    log::info!("[Sched] registered '{}' as task-{} (pid={})", name, slot, pid);
}

/// Spawn a new kernel task that calls `f()`. Returns the new PID.
///
/// The task starts with interrupts enabled once first scheduled.
pub fn spawn_kernel_task(name: &'static str, f: fn()) -> u64 {
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
    let stack_base = alloc_stack();

    // Build the initial kernel stack frame so that context_switch can
    // transparently switch into this task for the first time.
    //
    // context_switch (running on the old task) will:
    //   1. mov rsp, kernel_sp          ← load this task's stack
    //   2. pop r15..rbx  (6 × zero)
    //   3. ret                         → jumps to arch::task_entry
    //
    // arch::task_entry then:
    //   4. pop rdi                     ← task fn pointer we pushed below
    //   5. sti                         ← enable interrupts
    //   6. call rdi                    ← run the task
    //
    // Stack layout (high→low address, i.e. push order):
    //   [f as u64]           ← popped by task_entry as "argument"
    //   [task_entry addr]    ← ret address seen by context_switch
    //   [rbx = 0]
    //   [rbp = 0]
    //   [r12 = 0]
    //   [r13 = 0]
    //   [r14 = 0]
    //   [r15 = 0]            ← kernel_sp points here
    let kernel_sp = unsafe {
        let stack_top = stack_base.add(STACK_SIZE) as *mut u64;
        let mut sp = stack_top;
        sp = sp.sub(1); sp.write(f as u64);
        sp = sp.sub(1); sp.write(crate::arch::task_entry as unsafe extern "C" fn() as u64);
        sp = sp.sub(1); sp.write(0); // r15
        sp = sp.sub(1); sp.write(0); // r14
        sp = sp.sub(1); sp.write(0); // r13
        sp = sp.sub(1); sp.write(0); // r12
        sp = sp.sub(1); sp.write(0); // rbp
        sp = sp.sub(1); sp.write(0); // rbx
        sp as u64
    };

    let task = Task { pid, state: TaskState::Ready, priority: 128, name, kernel_sp, stack_base };

    let mut s = SCHED.lock();
    assert!(s.count < MAX_TASKS, "[Sched] task table full");
    let slot = s.count;
    s.tasks[slot] = Some(task);
    s.count += 1;

    log::info!("[Sched] spawned '{}' pid={} stack={:#x}", name, pid, kernel_sp);
    pid
}

/// Mark the currently-running scheduler task as a Zombie.
///
/// Call this from a kernel task that is about to SYSRET into userspace and
/// will never return to kernel mode.  The scheduler will skip saving its
/// kernel_sp on subsequent preemptions, so the zombie slot can never corrupt
/// the interrupt stack or accidentally resume as a kernel task.
pub fn mark_current_zombie() {
    let mut s = SCHED.lock();
    let cur = s.current;
    if let Some(t) = s.tasks[cur].as_mut() {
        t.state = TaskState::Zombie;
    }
}

/// Called on every APIC timer tick from the ISR.
pub fn tick() {
    let should_schedule = {
        let mut s = SCHED.lock();
        s.ticks += 1;
        s.count > 1 && s.ticks % SCHED_SLICE == 0
    };
    if should_schedule {
        schedule();
    }
}

/// Pick the next ready task and switch to it (round-robin).
///
/// Safe to call from both the idle loop and the timer ISR (both run with IF=0
/// or hold no conflicting locks).
pub fn schedule() {
    // ── Determine next task under lock ────────────────────────────────────────
    let (old_sp_ptr, new_sp_val) = {
        let mut s = SCHED.lock();
        if s.count <= 1 { return; }

        let current = s.current;

        // Find next ready task (round-robin).
        let mut next = (current + 1) % s.count;
        let start = next;
        loop {
            let ok = s.tasks[next].as_ref().map_or(false, |t| {
                t.state == TaskState::Ready
            });
            if ok { break; }
            next = (next + 1) % s.count;
            if next == start { return; } // no other ready task
        }

        // Determine whether the outgoing task is a Zombie (e.g. a kernel
        // launcher task that already executed sysretq and transferred to
        // userspace — it must never be resumed as a kernel task again).
        let current_is_zombie = s.tasks[current]
            .as_ref()
            .map_or(false, |t| t.state == TaskState::Zombie);

        // Update states: zombies stay zombie; running tasks become ready.
        if let Some(t) = s.tasks[current].as_mut() {
            if t.state == TaskState::Running { t.state = TaskState::Ready; }
        }
        if let Some(t) = s.tasks[next].as_mut() {
            t.state = TaskState::Running;
        }

        // Capture pointers/values before releasing the lock.
        // SAFETY: tasks live in a static array — pointers remain valid.
        //
        // For Zombie tasks we pass a throwaway scratch slot — we must still
        // call context_switch to actually swap the stack pointer, but we
        // don't want the interrupt-stack RSP to be written into the zombie
        // task's kernel_sp (which would corrupt the slot and crash on any
        // future spurious wake-up).  A static scratch word is safe because
        // the zombie is never resumed, so nobody reads the value back.
        static mut ZOMBIE_SP_SCRATCH: u64 = 0;
        let old_sp_ptr: *mut u64 = if current_is_zombie {
            unsafe { core::ptr::addr_of_mut!(ZOMBIE_SP_SCRATCH) }
        } else {
            s.tasks[current]
                .as_mut()
                .map(|t| &mut t.kernel_sp as *mut u64)
                .unwrap()
        };
        let new_sp_val = s.tasks[next]
            .as_ref()
            .map(|t| t.kernel_sp)
            .unwrap();

        s.current = next;
        (old_sp_ptr, new_sp_val)
    }; // ── lock released here ─────────────────────────────────────────────────

    // SAFETY: old_sp_ptr points into the static task array (never freed/moved).
    // new_sp_val is a valid kernel stack pointer set up by spawn_kernel_task or
    // by a prior context_switch.  Called with IF=0 (inside ISR or idle halt).
    unsafe { crate::arch::context_switch(old_sp_ptr, new_sp_val); }
}
